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
