use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(
        prepared.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let value = Interpreter::with_builtins().execute(&prepared).unwrap();
    assert!(
        matches!(&value, Value::String(s) if s == expected),
        "prepared: {value:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let value = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&value, Value::String(s) if s == expected),
        "AST: {value:?}"
    );
}

#[test]
fn empty_accessors_keep_their_descriptor_kind_on_objects_and_arrays() {
    both(
        "var results=[];for(var o of [{},[]]){Object.defineProperty(o,'1',{get:undefined,set:undefined,enumerable:true,configurable:true});var d=Object.getOwnPropertyDescriptor(o,'1');results.push(d.hasOwnProperty('get'),d.hasOwnProperty('set'),d.hasOwnProperty('value'),o[1]);Object.defineProperty(o,'1',{value:3});d=Object.getOwnPropertyDescriptor(o,'1');results.push(d.hasOwnProperty('get'),d.value);}results.join(',')",
        "true,true,false,,false,3,true,true,false,,false,3",
    );
}

#[test]
fn nonconfigurable_empty_accessors_can_be_redefined_identically() {
    both(
        "var result=[];for(var o of [{},[]]){Object.defineProperty(o,'1',{set:undefined});result.push(Reflect.defineProperty(o,'1',{set:undefined}));result.push(Reflect.defineProperty(o,'1',{value:1}));result.push(Reflect.set(o,'1',2));result.push(o[1]);}result.join(',')",
        "true,false,false,,true,false,false,",
    );
}

#[test]
fn paired_getters_and_setters_keep_their_identities() {
    both(
        "var o={},value=1;function g(){return value;}function s(v){value=v;}Object.defineProperty(o,'x',{get:g,set:s,configurable:true});var d=Object.getOwnPropertyDescriptor(o,'x');Object.defineProperty(o,'x',{enumerable:true});var e=Object.getOwnPropertyDescriptor(o,'x');o.x=3;[d.get===g,d.set===s,e.get===g,e.set===s,o.x].join(',')",
        "true,true,true,true,3",
    );
}

#[test]
fn clearing_both_sides_and_deleting_removes_stale_kind_metadata() {
    both(
        "var result=[];for(var o of [{},[]]){Object.defineProperty(o,'1',{get:function(){return 1;},set:function(v){},configurable:true});Object.defineProperty(o,'1',{get:undefined,set:undefined});var d=Object.getOwnPropertyDescriptor(o,'1');result.push(d.get,d.set,d.hasOwnProperty('value'));delete o[1];o[1]=undefined;d=Object.getOwnPropertyDescriptor(o,'1');result.push(d.hasOwnProperty('value'),d.hasOwnProperty('get'));}result.join(',')",
        ",,false,true,false,,,false,true,false",
    );
}

#[test]
fn inherited_empty_accessors_block_assignment_without_creating_own_properties() {
    both(
        "var p={};Object.defineProperty(p,'x',{get:undefined,set:undefined});var o=Object.create(p);var ok=Reflect.set(p,'x',1,o);o.x=2;var strictError;try{(function(){'use strict';o.x=3;})();}catch(e){strictError=e.name;}[ok,o.hasOwnProperty('x'),strictError].join(',')",
        "false,false,TypeError",
    );
}

#[test]
fn mapped_argument_conversion_to_an_empty_accessor_disconnects_the_parameter() {
    both(
        "function f(a){Object.defineProperty(arguments,'0',{get:undefined,set:undefined});a=2;var d=Object.getOwnPropertyDescriptor(arguments,'0');return [a,arguments[0],d.hasOwnProperty('get'),d.hasOwnProperty('value')].join(',');}f(1)",
        "2,,true,false",
    );
}
