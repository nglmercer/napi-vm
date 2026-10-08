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

#[test]
fn no_in_loop_heads_restore_in_for_nested_grammar_productions() {
    for source in [
        "for(C=class{get ['x' in {}](){return 1;}};;)break;",
        "for(C=class{static set ['x' in {}](x){}};;)break;",
        "for(x={['x' in {}]:1};;)break;",
        "for(x={key:'x' in {}};;)break;",
        "for(x=['x' in {}];;)break;",
        "for(x=f('x' in {});;)break;",
        "for(x=new F('x' in {});;)break;",
        "for(x=a['x' in {}];;)break;",
        "for(x=a?.['x' in {}];;)break;",
        "for(x=a?.('x' in {});;)break;",
        "for(x=true?'x' in {}:false;;)break;",
        "for(x=function(){return 'x' in {}; };;)break;",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
    assert!(!parses(
        "class C{#x;m(){for(#x in {};;)break;}}",
        ParseGoal::Script
    ));
}

#[test]
fn resource_and_import_call_grammar_rejects_invalid_statement_positions() {
    for source in [
        "do using x=1; while(false);",
        "async function f(){do await using x=1;while(false);}",
        "for(using x of []){var x;}",
        "async function f(){for(await using x of []){var x;}}",
        "let\nlet;",
        ".0000000001n;",
        ".0_e1;",
        "async()=>await new import.defer('x');",
        "async()=>await new import.source('x').value;",
        "async()=>await import.defer(...['x']);",
        "async()=>await import.source(...['x']);",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "async function f(){await using[x];}",
        "let async=11;",
        "new (import('x'));",
        "new (import.source('x')).value;",
        "let\nx=1;",
        "for(x=()=>{return 'x' in {};};;)break;",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
}

#[test]
fn dynamic_function_parse_errors_precede_new_target_prototype_access() {
    let mut vm = napi_vm_core::Interpreter::with_builtins();
    let result = vm.eval_source(
        r#"
        let reads=0;
        let target=Object.defineProperty(function(){}.bind(), 'prototype', {
            get(){reads++;return null;}
        });
        let constructors=[Function,(async function(){}).constructor,
            (function*(){}).constructor,(async function*(){}).constructor];
        let errors=0;
        for(let ctor of constructors){
            try{Reflect.construct(ctor,['@error'],target);}catch(e){
                if(e instanceof SyntaxError)errors++;
            }
        }
        reads===0 && errors===4;
    "#,
    );
    assert!(
        matches!(result, Ok(napi_vm_core::Value::Bool(true))),
        "{result:?}"
    );
}

#[test]
fn for_in_assignment_heads_are_retained_and_validated() {
    for source in [
        "class C{#x;m(){for(#x in []){}}}",
        "function* f(){for({yield} in []){}}",
        "for(1 in {}){}",
        "for(x=0 in {}){}",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "let x;for(x in {a:1}){};x==='a';",
        "let o={};for(o.x in {a:1}){};o.x==='a';",
        "let x;for([x] in {abc:1}){};x==='a';",
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
fn lexical_declarations_require_statement_list_positions() {
    for source in [
        "if(true)const x=1;",
        "while(false)class C{}",
        "label:class C{}",
        "label:const x=1;",
        "label:using x=null;",
        "async function f(){label:await using x=null;}",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "if(true){const x=1;}",
        "while(false){class C{}}",
        "label:{const x=1;}",
        "label:{using x=null;}",
        "async function f(){label:{await using x=null;}}",
        "label:function f(){}",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
}

#[test]
fn computed_object_methods_keep_method_context_and_accessor_metadata() {
    for source in [
        "({[key](){return super.x;}});",
        "({get [key](){return super.x;},set [key](x){super.x=x;}});",
        "({async [key](){await 1;return super.x;},*[key](){yield super.x;}});",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
    for source in [
        "({[key]:function(){return super.x;}});",
        "({[key](){super();}});",
        "({get [key](x){}});",
        "({set [key](){}});",
        "({set [key](...x){}});",
        "({async [key](){yield 1;}});",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "let key='x';let v=0;let o={get [key](){return v;},set [key](x){v=x;}};o.x=7;o.x===7;",
        "let key=Symbol('x');let o={[key](){return 7;}};o[key]()===7;",
        "let key='x';let o={[key](){return 7;}};o.x.name==='x';",
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
fn assignment_rest_comma_and_lexical_loop_bindings_are_early_errors() {
    for source in [
        "for([...x,] in []){}",
        "for([...x=1] in []){}",
        "for({...x,} in []){}",
        "([...x,]=[]);",
        "({...x,}={});",
        "([...x=1]=[]);",
        "for(let let in {}){}",
        "for(const let in {}){}",
        "for(let let of []){}",
        "for(const let of []){}",
        "for(let [let] of []){}",
        "for(const {let} in {}){}",
        "for(x in {})label:other:function f(){}",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "[...x,];",
        "({...x,});",
        "[...x=1];",
        "for([...x] in []){}",
        "for({...x} in []){}",
        "let x;for([x,] in []){}",
        "let x;for({x,} in []){}",
        "for(var let in {}){}",
        "label:other:function f(){}",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
}

#[test]
fn parameter_defaults_and_pattern_bound_names_cannot_disappear_during_recovery() {
    for source in [
        "class C{async f(x=await){}}",
        "function f(x=){}",
        "function f({x}=){}",
        "function f(a,a=1){}",
        "function f(a,{x:a}){}",
        "function f({x},{x}){}",
        "class C{f({x},{x}){}}",
        "const f=({x},{x})=>x;",
        "const [...x=1]=[];",
        "function f([...x=1]){}",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "function f(a,a){}",
        "function f(a=1,b=2){}",
        "function f({x},{y}){}",
        "function f([...x]){}",
        "const [...x]=[];",
        "class C{async f(x=async()=>await 1){}}",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
}

#[test]
fn annex_b_function_statement_positions_preserve_strict_and_block_boundaries() {
    for source in [
        "while(false)function f(){}",
        "for(;;)function f(){}",
        "with({})function f(){}",
        "if(true)async function f(){}",
        "if(true)function* f(){}",
        "label:async function f(){}",
        "'use strict';if(true)function f(){}",
        "'use strict';label:function f(){}",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "if(true)function f(){}",
        "if(true){}else function f(){}",
        "label:function f(){}",
        "while(false){function f(){}}",
        "if(true)switch(1){case 1:const x=1;}",
        "if(true)function f(){'use strict';function g(){}}",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
}

#[test]
fn catch_patterns_validate_bound_names_and_declaration_conflicts() {
    for source in [
        "try{}catch({x,x}){}",
        "try{}catch([x,x]){}",
        "try{}catch({x,y:x}){}",
        "try{}catch({x}){let x;}",
        "try{}catch([x]){const x=1;}",
        "try{}catch({x}){class x{}}",
        "try{}catch({x}){function x(){}}",
        "try{}catch({x}){var x;}",
        "try{}catch([x]){if(true){var x;}}",
        "try{}catch(x=1){}",
        "try{}catch([...x=1]){}",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "try{}catch({x,y}){}",
        "try{}catch([x=1,...rest]){}",
        "try{}catch({x,...rest}){}",
        "try{}catch({x}){var y;}",
        "try{}catch({x}){{let x;}}",
        "try{}catch(x){var x;}",
        "'use strict';try{}catch(x){var x;}",
        "try{}catch{}",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
}

#[test]
fn catch_pattern_initialization_observes_tdz_and_restores_scope_on_abrupt_completion() {
    for source in [
        "let f;try{throw{x:3,y:4};}catch({x,y}){x=8;f=()=>x+y;}f()===12&&typeof x==='undefined';",
        "let result=0;try{throw[undefined,2,3];}catch([x=1,...rest]){result=x+rest[0]+rest[1];}result===6;",
        "let result=0;try{throw{x:1,y:2};}catch({x,...rest}){result=x+rest.y;}result===3;",
        "let x='outer',ok=false;try{try{throw{};}catch({x=x}){}}catch(e){ok=e instanceof ReferenceError;}ok&&x==='outer';",
        "let x='outer',seen=0;try{try{throw{get x(){throw 9;}};}catch({x}){seen=1;}finally{seen=2;}}catch(e){seen+=e;}x==='outer'&&seen===11;",
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
fn iteration_heads_keep_declaration_and_assignment_grammar_distinct() {
    for source in [
        "for(x of [],[]){}",
        "for(async of []){}",
        "for(const x;;){}",
        "for(const [x]=[],y;;){}",
        "for(var [x];;){}",
        "for(let x of []){var x;}",
        "for(const {x} in {}){if(false){var x;}}",
        "for(let [x,x] of []){}",
        "for(const x=1 in {}){}",
        "'use strict';for(var x=1 in {}){}",
        "for(var {x}={} in {}){}",
        "for(var [x]=[] of []){}",
        "for(x+1 of []){}",
        "for([x,...y,] of []){}",
        "for({x,...y,z} of []){}",
        "for await(x in {}){}",
    ] {
        assert!(!parses(source, ParseGoal::Script), "accepted {source}");
    }
    for source in [
        "for(x of ([],[])){}",
        "for((async) of []){}",
        "async function f(){for await(async of []){}}",
        "for(var x=1 in {}){}",
        "for(var [x,x] of []){}",
        "for(let x of []){function f(){var x;}}",
        "for(x of []){}",
        "for(obj.x of []){}",
        "for([x,...rest] of []){}",
        "for({x:y} of []){}",
        "for([x] in {}){}",
        "async function f(){for await([x] of []){}}",
    ] {
        assert!(parses(source, ParseGoal::Script), "rejected {source}");
    }
}

#[test]
fn iteration_assignment_targets_update_bindings_and_close_on_failure() {
    for source in [
        "let x=0,sum=0;for(x of [1,2,3]){sum+=x;}x===3&&sum===6;",
        "let x=0,rest;for([x,...rest] of [[1,2,3]]){}x===1&&rest.length===2&&rest[1]===3;",
        "let obj={x:0};for(obj.x of [2,4]){}obj.x===4;",
        "let y;for({x:y} of [{x:9}]){}y===9;",
        "let x;for([x] in {ab:1}){}x==='a';",
        "let calls=0,x;for(var i=(calls++,7) in {a:1}){x=i;}calls===1&&x==='a';",
        "let closed=false,ok=false;const x=0;function* g(){try{yield 1;}finally{closed=true;}}try{for(x of g()){} }catch(e){ok=e instanceof TypeError;}closed&&ok;",
        "let ok=false;try{[x]=[1];let x;}catch(e){ok=e instanceof ReferenceError;}ok;",
    ] {
        let mut vm = napi_vm_core::Interpreter::with_builtins();
        let result = vm.eval_source(source);
        assert!(
            matches!(result, Ok(napi_vm_core::Value::Bool(true))),
            "{source}: {result:?}"
        );
    }
}
