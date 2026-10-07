use napi_vm_core::parser::ParseGoal;
use napi_vm_core::{Lexer, Parser};

fn parses(source: &str, goal: ParseGoal) -> bool {
    Parser::new_with_spans(
        Lexer::new(source)
            .with_module_goal(goal == ParseGoal::Module)
            .tokenize_with_spans(),
    )
    .parse_program_with_goal(goal)
    .is_ok()
}

#[test]
fn async_generator_and_parameter_contexts_reject_invalid_source() {
    for source in [
        "await 1;",
        "function f(){await 1;}",
        "function f(){yield 1;}",
        "async function f(a=await 1){}",
        "function* f(a=yield 1){}",
        "async function* f(a=yield* []){}",
        "async function* f(a=await 1){}",
        "async function f(){function g(){await 1;}}",
        "function* f(){function g(){yield 1;}}",
        "async function f(){(()=>await 1);}",
        "function* f(){(()=>yield 1);}",
        "function f(){for await (var x of []){}}",
        "class C{static{await 1;}}",
        "class C{static{yield 1;}}",
        "class C{static{var await=1;}}",
        "class C{static{let await=1;}}",
        "class C{static{const await=1;}}",
        "class C{static{var [await]=[];}}",
        "class C{x=arguments;}",
        "class C{static{arguments;}}",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
}

#[test]
fn contextual_identifiers_and_nested_functions_remain_valid() {
    for source in [
        "var await=1; await++;",
        "var yield=1; yield++;",
        "function f(await,yield){return await+yield;}",
        "async function f(){await 1;}",
        "function* f(){yield 1;yield* [];}",
        "async function* f(){await 1;yield 2;yield* [];}",
        "async function f(){function g(){var await=1;return await;}}",
        "function* f(){function g(){var yield=1;return yield;}}",
        "({async f(){await 1;},*g(){yield 1;},async *h(){yield await 1;}});",
        "class C{async f(){await 1;} *g(){yield 1;} async *h(){yield await 1;}}",
        "async function f(a=async()=>await 1){}",
        "function* f(a=function*(){yield 1;}){}",
        "function* f(){(function yield(){});}",
        "function f(...[x]){return x;}",
        "class C{x=await;}",
        "class C{x=function(){return arguments;};}",
        "class C{static{(async()=>await 1);(function*(){yield 1;});}}",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
    assert!(parses("await 1;", ParseGoal::Module));
    assert!(!parses("var await;", ParseGoal::Module));
}

#[test]
fn super_and_new_target_follow_lexical_function_boundaries() {
    for source in [
        "super();",
        "super.x;",
        "super[x];",
        "new.target;",
        "()=>new.target;",
        "class C{constructor(){super();}}",
        "class C extends B{m(){super();}}",
        "class C extends B{constructor(){function f(){super();}}}",
        "class C{m(){function f(){super.x;}}}",
        "({m(){super();}});",
        "class C extends B{constructor(){super?.x;}}",
        "class C{#x;m(){super.#x;}}",
        "class C{m(){return super;}}",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "class C extends B{constructor(){super();(()=>super());}}",
        "class C{m(){super.x;super[x];(()=>super.x);}}",
        "({m(){super.x;},get x(){return super.x;},set x(v){super.x=v;}});",
        "function f(){(()=>new.target);}",
        "class C{x=new.target;static{new.target;super.x;}}",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
}

#[test]
fn private_names_and_class_element_early_errors() {
    for source in [
        "obj.#x;",
        "#x in obj;",
        "class C{m(){return this.#x;}}",
        "class C{#x;#x;}",
        "class C{#x;static #x;}",
        "class C{#x(){}#x;}",
        "class C{get #x(){}get #x(){}}",
        "class C{get #x(){}static set #x(v){}}",
        "class C{#constructor;}",
        "class C{constructor(){}constructor(){}}",
        "class C{async constructor(){}}",
        "class C{*constructor(){}}",
        "class C{get constructor(){}}",
        "class C{constructor=1;}",
        "class C{static prototype(){}}",
        "class C{static prototype=1;}",
        "class C{#x;m(){delete this.#x;}}",
        "class C{#x;m(){return #x;}}",
        "class C{get x(a){}}",
        "class C{set x(){}}",
        "class C{set x(a,b){}}",
        "class C{set x(...a){}}",
        "({get x(a){}});",
        "({set x(){}});",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "class C{#x;m(){return this.#x;}}",
        "class C{#x;m(obj){return #x in obj;}}",
        "class C{#x;m(){return class D{m(obj){return obj.#x;}};}}",
        "class C{#x;m(){return class D{#x;m(obj){return obj.#x;}};}}",
        "class C{get #x(){}set #x(v){}}",
        "class C{static get #x(){}static set #x(v){}}",
        "class C{static constructor(){}['constructor'](){};}",
        "class C{set x({v}){}}",
        "({set x(v=1){}});",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
}

#[test]
fn declarations_imports_exports_and_catch_conflicts() {
    for source in [
        "{function x(){}let x;}",
        "try{}catch(x){let x;}",
        "switch(0){case 0:let x;case 1:var x;}",
        "let let;",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "import x from 'm';let x;",
        "import {x,x as x} from 'm';",
        "function x(){}function x(){}",
        "export {missing};",
        "let x;export{x};export{x};",
        "export default 1;export default 2;",
    ] {
        assert!(!parses(source, ParseGoal::Module), "accepted {source}");
    }
    for source in [
        "var x;export{x};",
        "export{x}from'm';",
        "export function x(){}",
        "import x from'm';export{x};",
    ] {
        assert!(parses(source, ParseGoal::Module), "rejected {source}");
    }
    assert!(parses("try{}catch(x){var x;}", ParseGoal::Script));
    assert!(parses("{function x(){}function x(){}}", ParseGoal::Script));
    assert!(!parses(
        "'use strict';{function x(){}function x(){}}",
        ParseGoal::Script
    ));
    assert!(!parses(
        "{function* x(){}function* x(){}}",
        ParseGoal::Script
    ));
}

#[test]
fn direct_eval_inherits_private_and_super_context_but_ordinary_functions_do_not() {
    for source in [
        "class C{#x=7;m(){return eval('this.#x');}}new C().m()===7;",
        "class A{m(){return 7;}}class B extends A{m(){return eval('super.m()');}}new B().m()===7;",
        "class A{constructor(){this.x=7;}}class B extends A{constructor(){eval('super()');}}new B().x===7;",
        "class A{m(){return 7;}}class B extends A{m(){return (()=>eval('super.m()'))();}}new B().m()===7;",
        "class A{m(){return 7;}}class B extends A{m(){function f(){try{eval('super.m()');}catch(e){return e instanceof SyntaxError;}}return f();}}new B().m();",
        "class C{#x=7;m(){try{(0,eval)('this.#x');}catch(e){return e instanceof SyntaxError;}}}new C().m();",
        "class C{static set #x(v){this.value=v;}static m(){eval('this.#x=7');}}C.m();C.value===7;",
        "function f(...[x]){return x;}f(7)===7;",
    ] {
        let mut vm = napi_vm_core::Interpreter::with_builtins();
        assert!(
            matches!(vm.eval_source(source), Ok(napi_vm_core::Value::Bool(true))),
            "failed {source}"
        );
    }
}

#[test]
fn line_terminators_and_parameter_delimiters_are_grammar_boundaries() {
    for source in [
        "var x=1 var y=2;",
        "throw\n1;",
        "x\n=>x;",
        "(x)\n=>x;",
        "function f(a b){}",
        "function f(if){}",
        "function f(...a,b){}",
        "function f(...a,){}",
        "f(1 2);",
        "new F(1 2);",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "var x=1\nvar y=2;",
        "function f(){return\n1;}",
        "function* g(){yield\n1;}",
        "async\nfunction f(){}",
        "x\n++y;",
        "function f(a,b,){}",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
}

#[test]
fn annex_b_comments_follow_the_lexical_goal_and_function_parameter_goal() {
    assert!(parses("<!-- comment\nvar x;", ParseGoal::Script));
    assert!(parses("\n--> comment\nvar x;", ParseGoal::Script));
    assert!(!parses("<!-- comment\nvar x;", ParseGoal::Module));
    assert!(!parses("\n--> comment\nvar x;", ParseGoal::Module));
    let mut vm = napi_vm_core::Interpreter::with_builtins();
    assert!(
        vm.eval_source("Function('<!--','');Function('\\n-->','');")
            .is_ok()
    );
    assert!(!parses("throw\u{2028}1;", ParseGoal::Script));
    assert!(parses("var x=1\u{2029}var y=2;", ParseGoal::Script));
}

#[test]
fn named_function_expressions_and_dynamic_function_kinds_keep_their_own_context() {
    for source in [
        "var f=function loop(n){return n ? loop(n-1) : 7;}; f(3)===7 && typeof loop==='undefined';",
        "var f=function loop(){loop=1;return typeof loop;};f()==='function';",
        "var f=function loop(){'use strict';try{loop=1;}catch(e){return e instanceof TypeError;}};f();",
        "function f(){return f;}var old=f;f=7;old()===7;",
        "var A=(async function(){}).constructor;A('a','await a').length===1 && typeof AsyncFunction==='undefined';",
        "var G=(function*(){}).constructor;G('yield 7')().next().value===7 && typeof GeneratorFunction==='undefined';",
        "var G=(async function*(){}).constructor;G('yield await 7').length===0;",
        "try{Function('await 1');false;}catch(e){e instanceof SyntaxError;}",
        "try{(async function(){}).constructor('a=await 1','');false;}catch(e){e instanceof SyntaxError;}",
        "Function(undefined,'return 7')()===7;",
    ] {
        let mut vm = napi_vm_core::Interpreter::with_builtins();
        let result = vm.eval_source(source);
        assert!(
            matches!(result, Ok(napi_vm_core::Value::Bool(true))),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn full_corpus_grammar_regressions_have_explicit_boundaries() {
    for source in [
        "class C{#x;constructor(){for(#x in value;;)break;}}",
        "class C{#x;m(){#x in ()=>{};}}",
        "class C{static{((x=await)=>0);}}",
        "'use strict';({yield});",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "do{}while(false) var x;",
        "foo(.1_2e1_0);",
        "var a=1;/*\u{2028}*/a++;",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
}

#[test]
fn with_syntax_uses_object_records_and_restores_lexical_scope() {
    assert!(!parses("'use strict';with({}){}", ParseGoal::Script));
    for source in [
        "var x=1;var o={x:2};with(o){x=3;x++;}x===1 && o.x===4;",
        "var x=1;var o={x:2,[Symbol.unscopables]:{x:true}};with(o){x=3;}x===3 && o.x===2;",
        "var x=1;var o={x:2};try{with(o){throw x;}}catch(e){}x===1;",
        "var o={x:7,f(){return this.x;}};var r;with(o){r=f();}r===7;",
        "var o={x:1};with(o){delete x;}!('x' in o);",
        "var x=1;with({x:2}){let x=3;}x===1;",
        "var x=1;var o={get x(){return 7;}};var r;with(o){r=x;}r===7;",
    ] {
        let mut vm = napi_vm_core::Interpreter::with_builtins();
        let result = vm.eval_source(source);
        assert!(
            matches!(result, Ok(napi_vm_core::Value::Bool(true))),
            "{source}: {result:?}"
        );
    }
}

#[test]
fn newer_syntax_retains_metadata_and_reports_runtime_gaps_explicitly() {
    for source in [
        "async function f(){await using x=null,y=null;}",
        "{using x=null;}",
        "(async()=>{await import.defer('x');});",
        "(()=>import.source('x'));",
        "for(using x of []){}",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
    for source in [
        "import x from 'x' with {type:'json'};",
        "import 'x' with {type:'json',};",
    ] {
        assert!(parses(source, ParseGoal::Module), "rejected {source}");
    }
    for source in [
        "import x from 'x' with {type:'json',type:'json'};",
        "import 'x' with {type:1};",
    ] {
        assert!(!parses(source, ParseGoal::Module), "accepted {source}");
    }
    for source in ["{x;using x=null;}", "let x={};for(using x of [x]){}"] {
        let mut vm = napi_vm_core::Interpreter::with_builtins();
        let result = vm.eval_source(source);
        assert!(
            result.is_err_and(|e| e.to_string().contains("ReferenceError")),
            "{source}"
        );
    }
    let mut vm = napi_vm_core::Interpreter::with_builtins();
    assert!(vm.eval_source("{using x=null;}").is_err_and(|e| {
        e.to_string()
            .contains("resource disposal execution is not implemented")
    }));
}
