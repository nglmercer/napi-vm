use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    let result = Interpreter::with_builtins().execute(&prepared).unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "prepared {source}: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "AST {source}: {result:?}"
    );
}

#[test]
fn reads_and_calls_perform_one_resolution_and_a_separate_value_check() {
    both(
        "var log=[],target={x:1,f:function(){'use strict';return this===proxy;}};var proxy=new Proxy(target,{has:function(t,k){log.push('has:'+String(k));return Reflect.has(t,k);},get:function(t,k,r){log.push('get:'+String(k));return Reflect.get(t,k,r);}});with(proxy){x;}var first=log.join('|');log=[];var result;with(proxy){result=f();}first+';'+log.filter(x=>x!=='has:result').join('|')+';'+result",
        "has:x|get:Symbol(Symbol.unscopables)|has:x|get:x;has:f|get:Symbol(Symbol.unscopables)|has:f|get:f;true",
    );
}

#[test]
fn missing_call_bindings_are_tested_only_once_per_environment() {
    both(
        "var log=[],proxy=new Proxy({},{has:function(t,k){log.push(k);return false;}});with(proxy){Object();}log.join(',')",
        "Object",
    );
}

#[test]
fn assignments_and_updates_do_not_resolve_again() {
    both(
        "var log=[],proxy=new Proxy({x:1},{has:function(t,k){if(k==='x')log.push('has');return Reflect.has(t,k);},get:function(t,k,r){if(k==='x')log.push('get');if(k===Symbol.unscopables)log.push('unscopables');return Reflect.get(t,k,r);},set:function(t,k,v,r){log.push('set');return Reflect.set(t,k,v,r);}});with(proxy){x=2;x+=3;x++;}log.join(',')",
        "has,unscopables,has,set,has,unscopables,has,get,has,set,has,unscopables,has,get,has,set",
    );
}

#[test]
fn disappearing_bindings_do_not_fall_back_to_an_outer_binding() {
    both(
        "var x='outer';var env={x:1,get [Symbol.unscopables](){delete this.x;return null;}};var value;with(env){value=x;}String(value)+','+x",
        "undefined,outer",
    );
}

#[test]
fn strict_closures_reject_a_binding_deleted_by_unscopables() {
    both(
        "var env={x:1,get [Symbol.unscopables](){delete this.x;return null;}},f;with(env){f=function(){'use strict';return x;};}var result;try{f();}catch(e){result=e.name;}result",
        "ReferenceError",
    );
}

#[test]
fn rhs_changes_do_not_redirect_the_captured_assignment_base() {
    both(
        "var x=10,env={x:1},run=function(){delete env.x;env[Symbol.unscopables]={x:true};return 3;};with(env){x+=run();}env.x+','+x",
        "4,10",
    );
}

#[test]
fn calls_preserve_the_base_even_if_getters_hide_the_binding() {
    both(
        "var env={get f(){this[Symbol.unscopables]={f:true};return function(){'use strict';return this===env;};}};var result;with(env){result=f();}String(result)",
        "true",
    );
}

#[test]
fn deleted_bindings_are_recreated_by_sloppy_assignments() {
    both(
        "var x=10,env={x:1,get [Symbol.unscopables](){delete this.x;return null;}};with(env){x=3;}env.x+','+x",
        "3,10",
    );
}

#[test]
fn excluded_names_and_declarative_bindings_keep_their_order() {
    both(
        "var x=10,env={x:1,[Symbol.unscopables]:{x:true}};var result;with(env){let x=2;result=x;}result+','+x+','+env.x",
        "2,10,1",
    );
}

#[test]
fn with_global_object_retains_its_receiver() {
    both(
        "function f(){'use strict';return this===globalThis;}var result;with(globalThis){result=f();}String(result)",
        "true",
    );
}

#[test]
fn global_identifier_calls_keep_an_undefined_receiver_in_bytecode() {
    let source = "function f(){'use strict';return this===undefined;}String(f())";
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(
        prepared.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    both(source, "true");
}

#[test]
fn optional_calls_and_template_tags_keep_the_with_receiver() {
    both(
        "var env={f:function(){'use strict';return this===env;},tag:function(){'use strict';return this===env;}},values;with(env){values=[f?.(),tag`x`];}values.join(',')",
        "true,true",
    );
}
