use napi_vm_core::{Interpreter, Value};

fn check_both(source: &str) {
    let mut vm = Interpreter::with_builtins();
    let program = Interpreter::compile(source).unwrap();
    let result = vm.eval_source(source);
    assert!(
        matches!(result, Ok(Value::Bool(true))),
        "compiled: {source}: {result:?}, tier {:?}",
        program.tier()
    );
    let mut lexer = napi_vm_core::Lexer::new(source);
    let body = napi_vm_core::Parser::new_with_spans(lexer.tokenize_with_spans())
        .parse_program()
        .unwrap();
    let mut vm = Interpreter::with_builtins();
    let result = vm.run_program_body(&body);
    assert!(
        matches!(result, Ok(Value::Bool(true))),
        "AST: {source}: {result:?}"
    );
}

#[test]
fn builtins_keep_their_state_and_derived_prototype() {
    check_both(
        "var log='';function Other(){}var target=new Proxy(Other,{get(object,key){if(key==='prototype'){log+='P';}return object[key];}});var input={valueOf(){log+='V';return 4;}};Reflect.construct(ArrayBuffer,[input],target);log==='VP';",
    );
    check_both(
        "var reads=0;function Other(){}var target=new Proxy(Other,{get(object,key){if(key==='prototype'){reads++;}return object[key];}});var caught=false;try{Reflect.construct(Promise,[1],target);}catch(e){caught=e instanceof TypeError;}caught&&reads===0;",
    );
    check_both(
        "var target;class Child extends Promise{}var value=new Child(function(resolve){target=new.target;resolve(1);});value instanceof Child&&target===undefined;",
    );
    check_both(
        "var reads=0;function Other(){}var target=new Proxy(Other,{get(object,key){if(key==='prototype'){reads++;}return object[key];}});Reflect.construct(Map,[],target);reads===1;",
    );
    check_both(
        "class Child extends Array{x=42;}var value=new Child(1,2);value instanceof Child&&value instanceof Array&&Array.isArray(value)&&value.length===2&&value[0]===1&&value.x===42;",
    );
    check_both(
        "class Child extends Date{x=42;}var value=new Child(0);value instanceof Child&&value instanceof Date&&value.getTime()===0&&value.x===42;",
    );
    check_both(
        "class Child extends Uint8Array{x=42;}var value=new Child(2);value[0]=7;value instanceof Child&&value instanceof Uint8Array&&value.length===2&&value[0]===7&&value.x===42;",
    );
    check_both(
        "class Child extends ArrayBuffer{x=42;}var value=new Child(2);value instanceof Child&&value instanceof ArrayBuffer&&value.byteLength===2&&value.x===42;",
    );
    check_both(
        "class Child extends Number{x=42;}var value=new Child(7);value instanceof Child&&value instanceof Number&&value.valueOf()===7&&value.x===42;",
    );
    check_both(
        "class Child extends Object{valueOf(){return 42;}}var original={};var value=new Child(original);value!==original&&value instanceof Child&&value.valueOf()===42;",
    );
}

#[test]
fn instance_private_fields_have_lexical_identity_and_own_storage() {
    check_both(
        "class Base{constructor(value){return value;}}class Child extends Base{#x;m(){var init=()=>new Child(this);var source={get a(){init();return 42;}};({a:this.#x}=source);return this.#x;}}Child.prototype.m.call({})===42;",
    );
    check_both(
        "class Base{#x=1;read(){return this.#x;}}class Child extends Base{#x=2;other(){return this.#x;}}var value=new Child();value.read()===1&&value.other()===2&&Reflect.ownKeys(value).length===0;",
    );
    check_both(
        "class Base{#x=1;read(value){return value.#x;}}var instance=new Base();var fake={'#x':1};var caught=false;try{instance.read(fake);}catch(e){caught=e instanceof TypeError;}caught;",
    );
    check_both(
        "class Base{#x=1;update(){this.#x++;this.#x+=2;this.#x&&=7;return this.#x;}}new Base().update()===7;",
    );
    check_both("class Base{#x=()=>42;read(){return this.#x();}}new Base().read()===42;");
    check_both(
        "var shared={};class Base{constructor(){return shared;}}class Child extends Base{#x=1;}new Child();var caught=false;try{new Child();}catch(e){caught=e instanceof TypeError;}caught;",
    );
}

#[test]
fn derived_constructors_bind_this_only_after_super() {
    check_both(
        "var iterator={next(){return {done:false};},return(){this.initialize();return {done:true};},[Symbol.iterator](){return this;}};class Base{}class Child extends Base{constructor(){iterator.initialize=()=>super();for(var value of iterator){return;}}}new Child() instanceof Child;",
    );
    check_both(
        "class Base{constructor(){this.target=new.target;}}class Child extends Base{x=42;constructor(){return ()=>super();}}var construct=new Child();var value=construct();value.x===42&&value.target===Child;",
    );
    check_both(
        "var reads=0;class Base{get x(){reads++;return 1;}}class Child extends Base{constructor(){var caught=false;try{super.x;}catch(e){caught=e instanceof ReferenceError;}super();this.ok=caught&&reads===0;}}new Child().ok;",
    );
    check_both(
        "class Base{}class Child extends Base{constructor(){var caught=false;try{this;}catch(e){caught=e instanceof ReferenceError;}super();this.ok=caught;}}new Child().ok;",
    );
    check_both(
        "class Base{}class Child extends Base{constructor(){}}var caught=false;try{new Child();}catch(e){caught=e instanceof ReferenceError;}caught;",
    );
    check_both(
        "class Base{}class Child extends Base{constructor(){return 1;}}var caught=false;try{new Child();}catch(e){caught=e instanceof TypeError;}caught;",
    );
    check_both(
        "var object={ok:true};class Base{}class Child extends Base{constructor(){return object;}}new Child()===object;",
    );
    check_both(
        "var calls=0;class Base{constructor(){calls++;}}class Child extends Base{constructor(){super();try{super();}catch(e){this.ok=e instanceof ReferenceError;}}}new Child().ok&&calls===2;",
    );
    check_both(
        "class Base{}class Child extends Base{constructor(read=()=>this){var caught=false;try{read();}catch(e){caught=e instanceof ReferenceError;}super();this.ok=caught&&read()===this;}}new Child().ok;",
    );
    check_both(
        "class Base{}class Child extends Base{constructor(){var run=()=>super();run();this.ok=true;}}new Child().ok;",
    );
    check_both(
        "class Base{}class Child extends Base{constructor(){var caught=false;try{eval('this');}catch(e){caught=e instanceof ReferenceError;}super();this.ok=caught&&eval('this')===this;}}new Child().ok;",
    );
}

#[test]
fn fields_follow_construction_and_use_own_property_definitions() {
    check_both(
        "var fields=0;class Base{}class Child extends Base{x=(()=>{fields++;throw new Error('field');})();constructor(){var first=false,second=false;try{super();}catch(e){first=e.message==='field';}try{super();}catch(e){second=e instanceof ReferenceError;}this.ok=first&&second;}}new Child().ok&&fields===1;",
    );
    check_both(
        "var object={};function Base(){return object;}class Child extends Base{x=42;}new Child()===object&&object.x===42;",
    );
    check_both(
        "var log='';class Base{x=(log+='B');}class Middle extends Base{x=(log+='M');}class Child extends Middle{x=(log+='C');}new Child();log==='BMC';",
    );
    check_both(
        "var log='';class Base{constructor(){log+='B';}}class Child extends Base{x=(log+='F');constructor(){log+='C';super();log+='A';}}new Child();log==='CBFA';",
    );
    check_both(
        "var object={};class Base{constructor(){return object;}}class Child extends Base{x=42;constructor(){var value=super();this.ok=value===object;}}var value=new Child();value===object&&value.x===42&&value.ok;",
    );
    check_both(
        "var object={};class Base{constructor(){return object;}}class Child extends Base{x=42;}new Child()===object&&object.x===42;",
    );
    check_both(
        "var writes=0;class Base{set x(value){writes++;}}class Child extends Base{x=42;}var value=new Child();var descriptor=Object.getOwnPropertyDescriptor(value,'x');writes===0&&descriptor.value===42&&descriptor.writable&&descriptor.enumerable&&descriptor.configurable;",
    );
    check_both("var value=7;class Base{x=value;constructor(value){}}new Base(99).x===7;");
    check_both(
        "var log='';class Base{x=(log+='F');constructor(value=(log+='P')){log+='B';}}new Base();log==='FPB';",
    );
    check_both(
        "var fields=0;class Base{}class Child extends Base{x=fields++;constructor(){return {};}}new Child();fields===0;",
    );
}

#[test]
fn class_construction_uses_new_target_prototype() {
    check_both(
        "class Base {constructor(){this.target=new.target;}} function Other(){} var value=Reflect.construct(Base,[],Other);Object.getPrototypeOf(value)===Other.prototype&&value.target===Other;",
    );
    check_both(
        "class Base {} class Child extends Base {} function Other(){} var value=Reflect.construct(Child,[],Other);Object.getPrototypeOf(value)===Other.prototype;",
    );
    check_both(
        "class Base {} function Other(){} Object.defineProperty(Other,'prototype',{value:1});Object.getPrototypeOf(Reflect.construct(Base,[],Other))===Object.prototype;",
    );
}

#[test]
fn proxy_construct_requires_an_object_result() {
    check_both(
        "var reads=0;var p=new Proxy(()=>{}, {get construct(){reads++;return function(){return {};};}});var caught=false;try{new p();}catch(e){caught=e instanceof TypeError;}caught&&reads===0;",
    );
    check_both(
        "var p=new Proxy(function(){},{construct(){return 1;}});var caught=false;try{new p();}catch(e){caught=e instanceof TypeError;}caught;",
    );
    check_both(
        "var result={ok:true};var p=new Proxy(function(){},{construct(){return result;}});new p()===result;",
    );
    check_both("function Target(){this.ok=true;}var p=new Proxy(Target,{});new p().ok===true;");
    check_both(
        "var handler={get construct(){throw new Error('trap getter');}};var p=new Proxy(function(){},handler);var caught=false;try{new p();}catch(e){caught=e.message==='trap getter';}caught;",
    );
    check_both(
        "var p=new Proxy(function(){},{construct:1});var caught=false;try{new p();}catch(e){caught=e instanceof TypeError;}caught;",
    );
    check_both(
        "function Target(){this.ok=true;}var p=new Proxy(Target,{construct:null});new p().ok===true;",
    );
}

#[test]
fn invalid_descriptor_getter_stops_before_reading_setter() {
    check_both(
        "var reads=0;var p=new Proxy({}, {getOwnPropertyDescriptor(){return {get:1,get set(){reads++;throw new Error('setter read');}};}});var caught=false;try{Object.getOwnPropertyDescriptor(p,'x');}catch(e){caught=e instanceof TypeError;}caught&&reads===0;",
    );
}

#[test]
fn new_target_tracks_construction_without_leaking_to_ordinary_calls() {
    check_both(
        "function F(){this.target=new.target;} var instance=new F(); instance.target===F && F.call({})===undefined;",
    );
    check_both(
        "function G(){return new.target;} function F(){this.inner=G();this.target=new.target;} var result=new F();result.inner===undefined && result.target===F;",
    );
    check_both(
        "function F(){this.target=new.target;} function Other(){} var value=Reflect.construct(F,[],Other);value.target===Other && Object.getPrototypeOf(value)===Other.prototype;",
    );
    check_both(
        "function F(){this.target=new.target;} var Bound=F.bind(null);var instance=new Bound();instance.target===F;",
    );
    check_both(
        "function F(){this.target=()=>new.target;} var instance=new F();instance.target()===F;",
    );
    check_both("function F(){return ()=>new.target;} F()()===undefined;");
    check_both("function F(){this.arrow=(...args)=>new.target;}new F().arrow()===F;");
    check_both(
        "function F(){this.target=eval('new.target');} var instance=new F();instance.target===F;",
    );
    check_both(
        "class Parent { constructor(){this.target=new.target;} } class Child extends Parent {} var instance=new Child();instance.target===Child;",
    );
}

#[test]
fn strict_and_context_errors_are_rejected_before_execution() {
    for source in [
        "new.target;",
        "function f(){new.target=1;}",
        "function f(){(new.target)=1;}",
        "function f(){new.target++;}",
        "function f(){++new.target;}",
        "function f(){new.target--;}",
        "function f(){--new.target;}",
        "(()=>new.target)();",
        "return 1;",
        "break;",
        "continue;",
        "while(true){function f(){break;}}",
        "label: {continue label;}",
        "label: label: while(false){}",
        "switch(1){default:break;default:break;}",
        "'use strict'; var eval;",
        "'use strict'; var arguments;",
        "'use strict'; eval=1;",
        "'use strict'; arguments++;",
        "'use strict';delete missing;",
        "function f(a,a){'use strict';}",
        "'use strict';function f(a,a){}",
        "function f(a=1){'use strict';}",
        "function f(a,a=1){}",
        "function f({a},a){}",
        "function f(...a){'use strict';}",
        "(a,a)=>a;",
        "({method(a,a){}});",
        "class C {method(a,a){}}",
        "function f(a){let a;}",
        "let a;let a;",
        "{let a;const a=1;}",
        "switch(1){case 1:let a;break;case 2:let a;}",
    ] {
        assert!(
            Interpreter::compile(source).is_err(),
            "accepted invalid source: {source}"
        );
    }
}

#[test]
fn valid_sloppy_bindings_and_nested_control_flow_remain_accepted() {
    for source in [
        "function f(a,a){return a;} f(1,2);",
        "var eval=1;var arguments=2;",
        "let a;{let a;}",
        "while(false){break;continue;}",
        "outer: inner: while(false){continue outer;}",
        "switch(1){case 1:break;default:break;}",
        "function f(a){'other';'use strict';return a;}",
        "function F(){return ()=>new.target;}",
        "class C {static value=new.target;}",
    ] {
        assert!(
            Interpreter::compile(source).is_ok(),
            "rejected valid source: {source}"
        );
    }
}

#[test]
fn global_descriptors_distinguish_builtins_var_properties_and_lexical_bindings() {
    check_both(
        "var d=Object.getOwnPropertyDescriptor(globalThis,'WeakMap');d.value===WeakMap && d.writable && !d.enumerable && d.configurable;",
    );
    check_both(
        "var d=Object.getOwnPropertyDescriptor(globalThis,'undefined');d.value===undefined && !d.writable && !d.enumerable && !d.configurable;",
    );
    check_both(
        "var user=42;var d=Object.getOwnPropertyDescriptor(globalThis,'user');d.value===42 && d.writable && d.enumerable && !d.configurable;",
    );
    check_both(
        "let lexicalOnly=42;globalThis.lexicalOnly===undefined && !Object.hasOwn(globalThis,'lexicalOnly') && Object.getOwnPropertyDescriptor(globalThis,'lexicalOnly')===undefined;",
    );
    check_both(
        "Object.getOwnPropertyDescriptors(globalThis).WeakSet.value===WeakSet && !Object.prototype.propertyIsEnumerable.call(globalThis,'WeakSet');",
    );
}

#[test]
fn dynamic_function_constructor_uses_function_grammar_and_validates_parameters() {
    check_both("Function('a=42','return a;')()===42 && Function('a,b','return a+b;')(1,2)===3;");
    check_both("var F=Function('return new.target;'); F()===undefined && new F()===F;");
    check_both(
        r#"var failed=0;for(var args of [['a,a',"'use strict';"],['a=1',"'use strict';"],['a){};var injected=1;function b(', ''],['', '} var injected=1; {']]){try{Function.apply(null,args);}catch(e){if(e.name==='SyntaxError')failed++;}}failed===4 && typeof injected==='undefined';"#,
    );
}

#[test]
fn indirect_eval_does_not_inherit_locals_or_new_target() {
    check_both(
        "var local='global';function F(){var local='function';return (0,eval)('local');} F()==='global';",
    );
    check_both(
        "function F(){var caught=false;try{(0,eval)('new.target');}catch(e){caught=e.name==='SyntaxError';}this.caught=caught;}new F().caught;",
    );
    check_both("function F(){var local=42;return eval('local');}F()===42;");
    check_both("var eval=function(){return 42;};eval('new.target')===42;");
}

#[test]
fn array_pattern_elisions_do_not_bind_scratch_names_or_consume_the_rest() {
    check_both("let [,,answer]=[1,2,42];answer===42 && typeof hole==='undefined';");
    check_both("var answer=0;[,,answer]=[1,2,42];answer===42 && typeof hole==='undefined';");
    check_both("let [first,...rest]=[]; first===undefined && rest.length===0;");
    assert!(Interpreter::compile("let [...rest,]=[];").is_err());
    assert!(Interpreter::compile("let [...rest,other]=[];").is_err());
}

#[test]
fn global_property_writes_preserve_attributes_and_deletion_does_not_reveal_builtins() {
    check_both(
        "var original=WeakMap;globalThis.WeakMap=42;var d=Object.getOwnPropertyDescriptor(globalThis,'WeakMap');var deleted=delete globalThis.WeakMap;d.value===42 && !d.enumerable && deleted && !Object.hasOwn(globalThis,'WeakMap') && typeof WeakMap==='undefined';",
    );
    check_both(
        "globalThis.undefined=42;globalThis.Infinity=42;globalThis.NaN=42;undefined===void 0 && Infinity===1/0 && Number.isNaN(NaN) && !(delete globalThis.undefined);",
    );
    check_both("var declared=42;!(delete globalThis.declared) && declared===42;");
}

#[test]
fn intrinsic_prototypes_survive_global_constructor_replacement_and_deletion() {
    check_both(
        "var original=Object;var prototype=Object.prototype;Object=42;original.getPrototypeOf({})===prototype;",
    );
    check_both(
        "var original=Array;var prototype=Array.prototype;delete globalThis.Array;var values=[1,2];original.isArray(values) && values.map(x=>x+1).join(',')==='2,3' && Object.getPrototypeOf(values)===prototype;",
    );
    check_both(
        "var prototype=Function.prototype;delete globalThis.Function;var fn=function(){};Object.getPrototypeOf(fn)===prototype;",
    );
}

#[test]
fn strict_directives_require_literal_unescaped_spelling() {
    for source in [
        r"'use\x20strict'; var eval;",
        r"('use strict'); var eval;",
        r"'use\x20strict';function f(a,a){}",
    ] {
        assert!(
            Interpreter::compile(source).is_ok(),
            "not a strict directive: {source}"
        );
    }
    assert!(Interpreter::compile(r"'other\x20directive'; 'use strict';var eval;").is_err());
    check_both(r"'use\x20strict'==='use strict';");
}

#[test]
fn script_this_numeric_coercion_and_property_receivers_follow_the_language_rules() {
    check_both("this===globalThis && (()=>this)()===globalThis;");
    check_both(
        "Number.isNaN(+undefined) && Number.isNaN(-undefined) && Number.isNaN(+(function(){}));",
    );
    check_both(
        "var calls=0;var obj={valueOf(){calls++;return 42;}}; +obj===42 && -obj===-42 && calls===2 && -Object(1n)===-1n;",
    );
    check_both(
        "var errors=0;for(var value of [Symbol(),1n]){try{+value;}catch(e){if(e.name==='TypeError')errors++;}}errors===2;",
    );
    check_both(
        "class Parent {get value(){return this.answer;}}class Child extends Parent {constructor(){super();this.answer=42;this.result=super.value;}}new Child().result===42;",
    );
    check_both(
        "var obj={get value(){return this.answer;}};Reflect.get(obj,'value',{answer:42})===42;",
    );
    check_both(
        "var seen;var receiver={};var obj=new Proxy({}, {get:function(t,k,r){seen=r;return 42;}});Reflect.get(obj,'value',receiver)===42 && seen===receiver;",
    );
}

#[test]
fn escaped_accessor_names_and_global_enumeration_keep_property_semantics() {
    check_both(
        r"var obj={get ['str\u0069ng'](){return 1;}};Object.getOwnPropertyDescriptor(obj,'string').get.name==='get string';",
    );
    check_both(
        "var leaked=false;for(var key in this){if(key==='Math'||key==='Infinity'||key==='Object')leaked=true;}!leaked;",
    );
}

#[test]
fn function_this_mode_is_captured_and_eval_strictness_is_scoped() {
    for source in [
        "function f(){return this;}f()===globalThis && f.call(null)===globalThis;",
        "function f(){'use strict';return this;}f()===undefined && f.call(null)===null && f.call(42)===42;",
        "'use strict';function f(){return this;}f()===undefined;",
        "function strictOuter(){'use strict';return function(){return this;};}var f=strictOuter();f()===undefined;",
        "function loose(){return this;}function strict(){'use strict';return loose();}strict()===globalThis;",
        "function f(){return this;}var n=f.call(42);typeof n==='object' && n.valueOf()===42 && Object.getPrototypeOf(n)===Number.prototype;",
        "var arrow=()=>this;arrow.call({})===globalThis;",
        "function f(){'use strict';return ()=>this;}var arrow=f.call(42);arrow.call({})===42;",
        "Function('return this')()===globalThis && Function('\"use strict\";return this')()===undefined;",
        "function f(){'use strict';var x=1;eval('var x=2');return x;}f()===1;",
        "function f(){var x=1;eval('\"use strict\";var x=2');return x;}f()===1;",
        "function f(){'use strict';try{eval('var eval=1');}catch(e){return e.name==='SyntaxError';}return false;}f();",
        "'use strict';var indirect=eval;indirect('var evalResult=17');evalResult===17;",
        "function f(){var x=1;eval('x=2');return x;}f()===2;",
        "class C {method(){return this;}}var method=new C().method;method()===undefined;",
    ] {
        check_both(source);
    }
}

#[test]
fn generator_receiver_and_strict_writes_preserve_execution_context() {
    for source in [
        "function* f(){yield this;}f().next().value===globalThis;",
        "function* f(){'use strict';yield this;}f().next().value===undefined && f.call(17).next().value===17;",
        "function* f(){yield arguments[0];}f(17).next().value===17;",
        "function f(){createdBySloppyCall=17;}f();globalThis.createdBySloppyCall===17;",
        "function f(){'use strict';try{undeclaredStrictName=1;}catch(e){return e.name==='ReferenceError';}return false;}f();",
        "Infinity=17;Infinity===1/0;",
        "function f(){'use strict';try{Infinity=17;}catch(e){return e.name==='TypeError';}return false;}f();",
        "function f(){'use strict';try{globalThis.Infinity=17;}catch(e){return e.name==='TypeError';}return false;}f();",
        "var obj=Object.defineProperty({},'x',{value:1});function f(){'use strict';try{obj.x=2;}catch(e){return e.name==='TypeError';}return false;}f() && obj.x===1;",
        "var obj=Object.preventExtensions({});function f(){'use strict';try{obj.x=2;}catch(e){return e.name==='TypeError';}return false;}f();",
        "var obj=new Proxy({}, {set:function(){return false;}});function f(){'use strict';try{obj.x=2;}catch(e){return e.name==='TypeError';}return false;}f();",
    ] {
        check_both(source);
    }
}

#[test]
fn assignment_targets_and_var_lexical_conflicts_are_early_errors() {
    for source in [
        "1=2;",
        "true++;",
        "++null;",
        "f()=1;",
        "this=1;",
        "(a+b)=1;",
        "obj?.x=1;",
        "[a]+=b;",
        "({a})&&=b;",
        "[...a,b]=values;",
        "({method(){}}=obj);",
        "let x;var x;",
        "const x=1;{var x;}",
        "{let x;{var x;}}",
        "let f;function f(){}",
        "function f(){let x;var x;}",
    ] {
        assert!(Interpreter::compile(source).is_err(), "accepted: {source}");
    }
    for source in [
        "var x=1;{let x=2;}",
        "let x=1;function f(){var x=2;}",
        "var a,b;[a,b]=[1,2];",
        "var a;({a}= {a:1});",
        "var a;[a=1]=[];",
    ] {
        assert!(Interpreter::compile(source).is_ok(), "rejected: {source}");
    }
}

#[test]
fn parse_goals_keep_validation_and_caches_separate() {
    use napi_vm_core::parser::{ParseGoal, parse_cached_with_goal};
    for source in ["export const goalProbe=1;", "import.meta;"] {
        assert!(parse_cached_with_goal(source, ParseGoal::Module).is_ok());
        assert!(parse_cached_with_goal(source, ParseGoal::Script).is_err());
        assert!(parse_cached_with_goal(source, ParseGoal::Module).is_ok());
    }
    assert!(Interpreter::compile("export default class Example {}").is_ok());
    assert!(Interpreter::compile("export const grouped=1;").is_ok());
    let sloppy = "var eval=1;";
    assert!(parse_cached_with_goal(sloppy, ParseGoal::Script).is_ok());
    assert!(parse_cached_with_goal(sloppy, ParseGoal::Module).is_err());
    for source in [
        "if(true){export const x=1;}",
        "function f(){import.meta; import 'x';}",
    ] {
        assert!(parse_cached_with_goal(source, ParseGoal::Module).is_err());
    }
    let script = parse_cached_with_goal("var cachedGoal=1;", ParseGoal::Script).unwrap();
    let module = parse_cached_with_goal("var cachedGoal=1;", ParseGoal::Module).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&script, &module));
    assert!(std::sync::Arc::ptr_eq(
        &script,
        &parse_cached_with_goal("var cachedGoal=1;", ParseGoal::Script).unwrap()
    ));
}

#[test]
fn class_strictness_does_not_escape_and_accessors_are_paired() {
    check_both("class C{};classSloppy=1;classSloppy===1;");
    check_both(
        "class C{constructor(){this.receiver=this;try{classMissing=1;}catch(e){this.strict=e instanceof ReferenceError;}}}var c=new C();c.strict && c.receiver===c;",
    );
    check_both("class C{get x(){return this._x;}set x(v){this._x=v;}}var c=new C();c.x=7;c.x===7;");
    check_both("class C{set x(v){this._x=v;}get x(){return this._x;}}var c=new C();c.x=9;c.x===9;");
}

#[test]
fn global_parser_aliases_inherited_names_and_missing_private_receivers() {
    check_both("Number.parseFloat===parseFloat && Number.parseInt===parseInt;");
    check_both(
        "'use strict';toString=Object.prototype.toString;toString===Object.prototype.toString;",
    );
    check_both("'use strict';Object.prototype.inheritedName=7;inheritedName===7;");
    check_both(
        "var rejected=false;try{class C{get #x(){throw 1;}[this.#x]=1;}}catch(e){rejected=e instanceof TypeError;}rejected;",
    );
    check_both("var object={'#x':7};object['#x']===7;");
}

#[test]
fn eval_variables_use_the_enclosing_variable_environment() {
    check_both(
        "var before=function(){return x;};var run=true;var test,body,increment;for(var _=eval('var x=1;');run&&(test=function(){return x;});increment=function(){return x;}){body=function(){return x;};run=false;}var x=2;before()===2&&test()===2&&body()===2&&increment()===2;",
    );
    check_both(
        "function f(){let marker=0;{let inner=1;eval('var x=7;let hidden=9;function g(){return x;}');}return x===7&&g()===7&&typeof hidden==='undefined';}f();",
    );
    check_both(
        "function f(){var x=1;eval('\"use strict\";var x=2;function local(){}');return x===1&&typeof local==='undefined';}f();",
    );
    check_both("var old=NaN;eval('var NaN;');Number.isNaN(NaN)&&Number.isNaN(old);");
    check_both(
        "let conflict=1;var caught=false;try{eval('var newBinding;var conflict;');}catch(e){caught=e instanceof SyntaxError;}caught&&typeof newBinding==='undefined'&&conflict===1;",
    );
}

#[test]
fn eval_global_declarations_validate_before_creating_bindings() {
    check_both(
        "var threw=false;try{eval('var shouldNotExist;function earlier(){}function NaN(){}');}catch(e){threw=e instanceof TypeError;}threw&&Object.getOwnPropertyDescriptor(globalThis,'shouldNotExist')===undefined&&Object.getOwnPropertyDescriptor(globalThis,'earlier')===undefined;",
    );
    check_both(
        "var indirect=eval;var threw=false;try{indirect('function earlier(){}function NaN(){}');}catch(e){threw=e instanceof TypeError;}threw&&Object.getOwnPropertyDescriptor(globalThis,'earlier')===undefined;",
    );
}

#[test]
fn proxy_descriptors_run_traps_and_omit_absent_properties() {
    check_both(
        "var calls=0;var p=new Proxy({}, {ownKeys(){return ['missing'];},getOwnPropertyDescriptor(target,key){calls++;return undefined;}});var d=Object.getOwnPropertyDescriptors(p);calls===1&&!('missing' in d);",
    );
    check_both(
        "var target={x:1};var p=new Proxy(target,{});Object.getOwnPropertyDescriptor(p,'x').value===1;",
    );
    check_both(
        "var proto={enumerable:1,configurable:1,value:42,writable:1};var p=new Proxy({}, {getOwnPropertyDescriptor(){return Object.create(proto);}});var d=Object.getOwnPropertyDescriptor(p,'x');d!==proto&&d.value===42&&d.enumerable===true&&d.configurable===true&&d.writable===true;",
    );
    check_both(
        "var target={};Object.defineProperty(target,'x',{value:1});var p=new Proxy(target,{getOwnPropertyDescriptor(){return undefined;}});var caught=false;try{Object.getOwnPropertyDescriptor(p,'x');}catch(e){caught=e instanceof TypeError;}caught;",
    );
    check_both(
        "var p=new Proxy({}, {getOwnPropertyDescriptor:1});var caught=false;try{Object.getOwnPropertyDescriptor(p,'x');}catch(e){caught=e instanceof TypeError;}caught;",
    );
    check_both(
        "var p=new Proxy({}, {getOwnPropertyDescriptor(){throw new Error('trap');}});var caught=false;try{Object.getOwnPropertyDescriptors(new Proxy(p,{ownKeys(){return ['x'];}}));}catch(e){caught=e.message==='trap';}caught;",
    );
}

#[test]
fn proxy_descriptor_completion_preserves_undefined_accessors() {
    check_both(
        "var p=new Proxy({}, {getOwnPropertyDescriptor(){return {get:undefined,configurable:true};}});var d=Object.getOwnPropertyDescriptor(p,'x');('get' in d)&&('set' in d)&&!('value' in d)&&!('writable' in d)&&d.get===undefined&&d.set===undefined&&d.enumerable===false;",
    );
    check_both(
        "var getter=function original(){return 1;};var p=new Proxy({}, {getOwnPropertyDescriptor(){return {get:getter,configurable:true};}});var d=Object.getOwnPropertyDescriptor(p,'x');d.get===getter&&d.get.name==='original';",
    );
}

#[test]
fn class_static_blocks_own_separate_variable_environments() {
    check_both(
        "var x='outer';var first,second;class C{static{var x='first';first=function(){return x;};}static{var x='second';second=function(){return x;};}}x==='outer'&&first()==='first'&&second()==='second';",
    );
    check_both(
        "var x=1;class C{static{var x=2;eval('var x=3;');this.x=x;}static{this.y=x;}}x===1&&C.x===2&&C.y===1;",
    );
}

#[test]
fn parameter_defaults_run_before_body_declarations() {
    check_both(
        "var old=globalThis.arguments;var count=0;const f=(p=eval(\"var arguments='param'\"))=>{let arguments='local';if(arguments==='local')count++;};f();count===1&&globalThis.arguments===old;",
    );
    check_both("var x='outer';function f(a=()=>x){let x='body';return a();}f()==='outer';");
    check_both(
        "function f(a=1,read=()=>a){var a=2;return [a,read()];}var r=f();r[0]===2&&r[1]===1;",
    );
    check_both("function f(a=1,b=a+1){return b;}f()===2;");
    check_both("function f({x}={x:1},b=x+1){var x=7;return b===2&&x===7;}f();");
    check_both(
        "var caught=false;try{(function(a=a){return a;})();}catch(e){caught=e instanceof ReferenceError;}caught;",
    );
    check_both(
        "var caught=false;try{(function(a=b,b=1){return a;})();}catch(e){caught=e instanceof ReferenceError;}caught;",
    );
    check_both(
        "var outer=1;function f(a=eval('var outer=2;'),read=()=>outer){var outer=3;return read()===2&&outer===3;}f()&&outer===1;",
    );
    assert!(Interpreter::compile("function f({x}){let x;}").is_err());
}

#[test]
fn generators_initialize_parameters_at_creation_and_retain_their_scope() {
    check_both(
        "var calls=0;function* g(a=(calls++,1),read=()=>a){var a=2;yield read();yield a;}var iterator=g();var before=calls;before===1&&iterator.next().value===1&&iterator.next().value===2&&calls===1;",
    );
    check_both(
        "var caught=false;try{(function* g(a=a){yield a;})();}catch(e){caught=e instanceof ReferenceError;}caught;",
    );
    check_both(
        "var calls=0;function* g({x}={x:(calls++,7)}){yield x;}var iterator=g();calls===1&&iterator.next().value===7&&calls===1;",
    );
    check_both(
        "var seen=0;class C{field=seen;constructor(a=(seen=3)){this.a=a;}}var c=new C();c.a===3&&c.field===0;",
    );
}

#[test]
fn function_lengths_stop_before_default_and_rest_parameters() {
    check_both(
        "function a(x,y=1,z){}function b({x},y=1){}function c(x,...rest){}class C{field=1;constructor(x,y=1){}}a.length===1&&b.length===1&&c.length===1&&C.length===1&&((x=1)=>x).length===0;",
    );
}

#[test]
fn non_simple_parameter_bindings_block_sloppy_eval_redeclarations() {
    check_both(
        "var caught=false;try{(function(p=eval('var arguments')){let arguments;})();}catch(e){caught=e instanceof SyntaxError;}caught&&typeof globalThis.arguments==='undefined';",
    );
    check_both(
        "var caught=false;try{(function(p=eval('var p')){})();}catch(e){caught=e instanceof SyntaxError;}caught;",
    );
    check_both(
        "var caught=false;try{({f(p=eval('var arguments=1')){let arguments;}}).f();}catch(e){caught=e instanceof SyntaxError;}caught;",
    );
}

#[test]
fn formal_arguments_parameters_replace_the_implicit_arguments_binding() {
    check_both("function f(arguments=1){return arguments;}f(7)===7&&f()===1;");
    check_both("function f({arguments}={arguments:9}){return arguments;}f()===9;");
}

#[test]
fn base_fields_initialize_before_parameter_defaults_in_their_defining_scope() {
    check_both("class A{#x='hello';constructor(p=this.#x){this.value=p;}}new A().value==='hello';");
    check_both(
        "function field(){throw 10;}function parameter(){throw 20;}class A{x=field();constructor(p=parameter()){}}var result;try{new A();}catch(e){result=e;}result===10;",
    );
    check_both(
        "var x='outer';class A{field=x;constructor(x='parameter'){this.argument=x;}}var a=new A();a.field==='outer'&&a.argument==='parameter';",
    );
}

#[test]
fn typed_array_elements_reject_invalid_descriptors_and_indices() {
    check_both(
        "var a=new Uint8Array(1);!Reflect.defineProperty(a,'-0',{value:1})&&!Reflect.defineProperty(a,'1',{value:1})&&!Reflect.defineProperty(a,'0',{writable:false})&&Reflect.defineProperty(a,'0',{value:42})&&a[0]===42;",
    );
    check_both(
        "var accessed=false;var F=function(){}.bind(null);Object.defineProperty(F,'prototype',{get(){accessed=true;throw 1;}});var caught=false;try{Reflect.construct(Uint8Array,[Symbol()],F);}catch(e){caught=e instanceof TypeError;}caught&&!accessed;",
    );
}

#[test]
fn regexp_compile_replaces_pattern_and_resets_last_index() {
    check_both(
        "var re=/a/g;re.lastIndex=4;re.compile('b','i')===re&&re.source==='b'&&re.flags==='i'&&re.lastIndex===0&&re.test('B')&&!re.test('a');",
    );
    check_both(
        "var re=/a/;var copy=Reflect.construct(RegExp,[re],Object.defineProperty(function(){}.bind(null),'prototype',{get(){re.compile('b');return RegExp.prototype;}}));copy.source==='a'&&re.source==='b';",
    );
}

#[test]
fn typed_array_constructor_inheritance_retains_shared_instance_methods() {
    check_both(
        "var base=Object.getPrototypeOf(Float32Array);class Derived extends base{constructor(){return Reflect.construct(Float32Array,[1],new.target);}}var a=new Derived();typeof a.slice==='function'&&typeof Derived.prototype.slice==='function'&&a.slice(0).length===1;",
    );
}

#[test]
fn typed_array_metadata_and_accessors_validate_the_receiver() {
    check_both(
        "var T=Object.getPrototypeOf(Uint8Array);var d=Object.getOwnPropertyDescriptor(T,'length');T.name==='TypedArray'&&d.value===0&&!d.writable&&!d.enumerable&&d.configurable;",
    );
    check_both(
        "var p=Object.getPrototypeOf(Uint8Array).prototype;var caught=0;for(var k of ['buffer','length','byteLength','byteOffset']){try{p[k];}catch(e){if(e instanceof TypeError)caught++;}}caught===4;",
    );
    check_both(
        "class Re extends RegExp{}var re=new Re('a');var caught=false;try{re.compile('b');}catch(e){caught=e instanceof TypeError;}caught&&re.source==='a';",
    );
}
