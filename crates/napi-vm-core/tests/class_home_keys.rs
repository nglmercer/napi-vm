use napi_vm_core::{Interpreter, Value};

fn truth(source: &str) {
    let mut vm = Interpreter::with_builtins();
    let result = vm.eval_source(source);
    assert!(
        matches!(result, Ok(Value::Bool(true))),
        "{source}: {result:?}"
    );
}

#[test]
fn symbols_keep_identity_in_class_methods_accessors_and_fields() {
    truth(
        "var key=Symbol('member');class A{[key](){return 42;}static [key](){return 7;}}var a=new A();a[key]()===42&&A[key]()===7&&Object.getOwnPropertySymbols(A.prototype)[0]===key&&Object.getOwnPropertySymbols(A)[0]===key&&a[key].name==='[member]';",
    );
    truth(
        "var key=Symbol();class A{get [key](){return 42;}set [key](v){this.value=v;}}var a=new A();a[key]=7;var d=Object.getOwnPropertyDescriptor(A.prototype,key);a[key]===42&&a.value===7&&d.get.name==='get '&&d.set.name==='set ';",
    );
    truth(
        "var key=Symbol('field');var calls=0;var converted={toString(){calls++;return key;}};class A{[converted]=42;static [converted]=7;}var a=new A();calls===2&&a[key]===42&&A[key]===7&&Reflect.ownKeys(a)[0]===key;",
    );
}

#[test]
fn class_keys_convert_once_with_string_hint_in_definition_order() {
    truth(
        "var log='';var key={};key[Symbol.toPrimitive]=function(hint){log+=hint;return 'method';};class A{[key](){return 42;}[log+='next'](){}}log==='stringnext'&&new A().method()===42;",
    );
    truth(
        "var calls=0;var key={toString(){calls++;throw 42;}};var caught;try{class A{[key](){}static value=(calls+=10);}}catch(e){caught=e;}caught===42&&calls===1;",
    );
}

#[test]
fn home_objects_follow_mutated_prototypes_without_following_receivers() {
    truth(
        "class A{method(){return super.value;}}Object.setPrototypeOf(A.prototype,{value:42});var method=new A().method;method.call({value:7})===42;",
    );
    truth(
        "var object={method(){return ()=>super.value;}};Object.setPrototypeOf(object,{value:42});var f=object.method();Object.setPrototypeOf(object,{value:7});f()===7;",
    );
    truth(
        "class A{static method(){return super.value;}static get value(){return super.value;}}Object.setPrototypeOf(A,{value:42});A.method()===42&&A.value===42;",
    );
}

#[test]
fn super_calls_capture_the_target_and_receiver_before_arguments() {
    truth(
        "class A{constructor(){this.value=1;}}function Other(){this.value=42;}class B extends A{constructor(){super(Object.setPrototypeOf(B,Other));}}new B().value===1;",
    );
    truth(
        "var log='';class A{}class B extends A{method(){return super.method(log+='argument');}}Object.setPrototypeOf(B.prototype,{get method(){log+='get';return function(v){return this.value+v;};}});var b=new B();b.value='receiver';b.method()==='receivergetargument'&&log==='getargument';",
    );
}

#[test]
fn super_writes_use_receiver_and_preserve_the_captured_reference() {
    truth(
        "class A{get x(){return this.value;}set x(v){this.value=v;}}class B extends A{write(){return super.x=42;}update(){return super.x++;}compound(){return super.x+=2;}}var b=new B();b.value=1;b.write()===42&&b.update()===42&&b.compound()===45&&b.value===45;",
    );
    truth(
        "var log='';class A{set x(v){log+='old';this.value=v;}}class B extends A{write(){super.x=(Object.setPrototypeOf(B.prototype,{set x(v){log+='new';}}),42);}}var b=new B();b.write();log==='old'&&b.value===42;",
    );
    truth(
        "class A{get x(){return this.value;}set x(v){this.value=v;}}class B extends A{update(){return super.x++;}logical(){return super.x??=42n;}}var b=new B();b.value=1n;b.update()===1n&&b.value===2n&&b.logical()===2n;",
    );
}

#[test]
fn fields_define_symbols_through_proxy_and_reject_denied_definitions() {
    truth(
        "var key=Symbol('field');var calls=0;var target={};var proxy=new Proxy(target,{defineProperty(t,k,d){calls++;return Reflect.defineProperty(t,k,d);}});class Identity{constructor(){return proxy;}}class A extends Identity{[key]=42;}var a=new A();a===proxy&&target[key]===42&&calls===1;",
    );
    truth(
        "var proxy=new Proxy({},{defineProperty(){return false;}});class Identity{constructor(){return proxy;}}class A extends Identity{field=42;}var caught;try{new A();}catch(e){caught=e;}caught instanceof TypeError;",
    );
}

#[test]
fn class_member_descriptors_replacement_and_anonymous_field_names() {
    truth(
        "class A{method(){return 1;}method(){return 42;}get value(){return 1;}value(){return 7;}}var a=new A();var d=Object.getOwnPropertyDescriptor(A.prototype,'method');a.method()===42&&a.value()===7&&!d.enumerable&&d.writable&&d.configurable&&Object.keys(A.prototype).length===0;",
    );
    truth(
        "var key=Symbol('named');class A{[key]=()=>42;static field=function(){};value=class{static inferred=this.name;};}var a=new A();a[key].name==='[named]'&&A.field.name==='field'&&a.value.name==='value'&&a.value.inferred==='value';",
    );
}

#[test]
fn escaped_home_objects_and_symbol_methods_keep_foreign_realms_alive() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    let method = child.eval_source("class A{method(){return super.value;}}Object.setPrototypeOf(A.prototype,{value:42});A.prototype.method;").unwrap();
    vm.set_global_checked("foreignMethod", method).unwrap();
    drop(child);
    assert!(vm.collect_cycles().skipped.is_none());
    assert!(matches!(
        vm.eval_source("foreignMethod.call({})===42;"),
        Ok(Value::Bool(true))
    ));
}

#[test]
fn class_heritage_validates_constructors_and_observable_prototypes() {
    truth(
        "class A{}class B extends null{constructor(){return {};}}Object.getPrototypeOf(A.prototype)===Object.prototype&&Object.getPrototypeOf(B.prototype)===null&&Object.getPrototypeOf(B)===Function.prototype;",
    );
    truth(
        "var calls=0;var parent=new Proxy(function(){},{get(t,k){if(k==='prototype'){calls++;throw 42;}return Reflect.get(t,k);}});var caught;try{class A extends parent{}}catch(e){caught=e;}caught===42&&calls===1;",
    );
    truth(
        "var errors=0;for(var parent of [42,{},()=>{},function*(){}]){try{class A extends parent{}}catch(e){if(e instanceof TypeError)errors++;}}errors===4;",
    );
}

#[test]
fn super_destructuring_tags_and_delete_use_the_same_reference_rules() {
    truth(
        "class A{set x(v){this.value=v;}tag(parts,v){return this.value+v;}}class B extends A{assign(){[super.x]=[42];}tagged(){return super.tag`value${7}`;}}var b=new B();b.assign();b.value===42&&b.tagged()===49;",
    );
    truth(
        "var calls=0;var key={toString(){calls++;return 'x';}};class A{method(){return delete super[key];}}var caught;try{new A().method();}catch(e){caught=e;}calls===1&&caught instanceof ReferenceError;",
    );
}

#[test]
fn computed_constructor_members_and_static_eval_keep_correct_contexts() {
    truth("class A{['constructor'](){return 42;}}new A().constructor()===42;");
    truth(
        "class A{static value=42;}class B extends A{static result=eval('super.value');static {this.block=eval('super.value');}}B.result===42&&B.block===42;",
    );
}
