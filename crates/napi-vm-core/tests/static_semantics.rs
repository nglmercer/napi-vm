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
