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
fn instance_private_methods_are_branded_before_fields_and_remain_read_only() {
    truth(
        "class A{#value=this.#method();#method(){return 42;}read(){return this.#value;}method(){return this.#method;}write(){this.#method=1;}}var a=new A();var b=new A();var caught;try{a.write();}catch(e){caught=e;}a.read()===42&&a.method()===b.method()&&caught instanceof TypeError&&Object.getOwnPropertyNames(A.prototype).join(',')==='read,method,write,constructor';",
    );
}

#[test]
fn private_accessor_pairs_use_the_original_receiver_and_enforce_missing_halves() {
    truth(
        "class A{#value=1;get #access(){return this.#value;}set #access(value){this.#value=value;}get(){return this.#access;}set(value){this.#access=value;}}var a=new A();a.set(42);a.get()===42;",
    );
    truth(
        "class A{get #readOnly(){return 1;}set #writeOnly(value){}read(){return this.#writeOnly;}write(){this.#readOnly=1;}}var a=new A();var errors=0;try{a.read();}catch(e){if(e instanceof TypeError)errors++;}try{a.write();}catch(e){if(e instanceof TypeError)errors++;}errors===2;",
    );
}

#[test]
fn repeated_class_evaluations_allocate_distinct_private_method_and_accessor_names() {
    for member in [
        "#x(){return 1;}read(obj){return obj.#x;}",
        "get #x(){return 1;}read(obj){return obj.#x;}",
        "set #x(v){}read(obj){obj.#x=1;}",
    ] {
        truth(&format!(
            "function make(){{return class{{{member}}};}}var A=make();var B=make();var caught;try{{new A().read(new B());}}catch(e){{caught=e;}}caught instanceof TypeError;"
        ));
    }
}

#[test]
fn static_private_elements_are_branded_on_the_defining_class() {
    truth(
        "function make(){return class{static #field=1;static #method(){return 42;}static get #access(){return this.#field;}static set #access(value){this.#field=value;}static read(obj){return obj.#field;}static method(obj){return obj.#method;}static get(){return this.#access;}static set(value){this.#access=value;}};}var A=make();var B=make();A.set(7);var errors=0;try{A.read(B);}catch(e){if(e instanceof TypeError)errors++;}try{A.method(B);}catch(e){if(e instanceof TypeError)errors++;}class Derived extends A{}try{Derived.get();}catch(e){if(e instanceof TypeError)errors++;}A.get()===7&&B.get()===1&&A.method(A)()===42&&errors===3&&!Object.getOwnPropertyNames(A).some(k=>k[0]==='#');",
    );
}

#[test]
fn private_brand_checks_use_slot_presence_without_proxy_traps() {
    truth(
        "class Identity{constructor(object){return object;}}class A extends Identity{#field;#method(){}get #access(){return 1;}static has(obj){return [#field in obj,#method in obj,#access in obj];}}var traps=0;var target={};var pair=Proxy.revocable(target,{has(){traps++;throw 1;},get(){traps++;throw 2;}});var proxy=pair.proxy;pair.revoke();new A(proxy);var caught;try{A.has(1);}catch(e){caught=e;}A.has(proxy).every(Boolean)&&A.has(target).every(v=>!v)&&traps===0&&caught instanceof TypeError;",
    );
}

#[test]
fn nested_classes_do_not_install_outer_private_methods_on_their_receivers() {
    truth(
        "class Outer{#method(){}make(){return class Inner{static check(obj){return #method in obj;}};}}var outer=new Outer();var Inner=outer.make();Inner.check(outer)&&!Inner.check(new Inner());",
    );
}

#[test]
fn static_initializers_observe_class_this_methods_and_source_order() {
    truth(
        "var order='';class A{static #field=(order+='1',1);static {order+='b';this.#field=2;}static #next=(order+='2',this.#field);static #method(){return this.#next;}static value=(order+='3',this.#method());static get(){return this.#field;}static self=A;}order==='1b23'&&A.value===2&&A.get()===2&&A.self===A;",
    );
    truth(
        "class A{static #method(){return 42;}static value=this.#method();static has(){return #method in this;}}A.value===42&&A.has();",
    );
}

#[test]
fn class_name_bindings_are_initialized_before_static_methods_run() {
    truth(
        "class A{static method(){return A;}static self=this.method();}var Original=A;A=undefined;Original.self===Original&&Original.method()===Original;",
    );
    truth(
        "var C=1;var caught;try{var value=class C extends C{};}catch(e){caught=e;}caught instanceof ReferenceError;",
    );
}

#[test]
fn static_field_contexts_use_undefined_new_target_and_constructor_super_base() {
    truth(
        "class Parent{static answer=42;}class Child extends Parent{static value=super.answer;static target=new.target;static method(){return super.answer;}}Child.value===42&&Child.target===undefined&&Child.method()===42;",
    );
}

#[test]
fn class_heritage_closures_share_the_initialized_internal_name_binding() {
    truth("var read;class A extends (read=()=>A, Object){static value=read();}A.value===A;");
    truth(
        "var read;var Value=class A extends (read=()=>A, Object){static value=read();};Value.value===Value;",
    );
}

#[test]
fn class_computed_names_use_private_names_and_the_class_name_tdz() {
    truth("class A{#field;[#field in {}](){return 42;}}new A().false()===42;");
    truth(
        "var A=1;var caught;try{var Value=class A{[A](){}};}catch(e){caught=e;}caught instanceof ReferenceError;",
    );
}
