use napi_vm::{Interpreter, Lexer, Parser, Value};

fn assert_both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(
        prepared.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let bytecode = Interpreter::with_builtins().execute(&prepared).unwrap();
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let ast = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    for result in [bytecode, ast] {
        assert!(
            matches!(&result, Value::String(s) if s == expected),
            "{result:?}"
        );
    }
}

#[test]
fn species_result_rechecked_after_each_callback() {
    assert_both(
        "var r=[99,88];var a=[1,2];a.constructor={};a.constructor[Symbol.species]=function(){return r};var gets=0;a.map(function(v,i){if(i===0)Object.defineProperty(r,'1',{get:function(){gets++;return 5},configurable:true});return v*10});var d=Object.getOwnPropertyDescriptor(r,'1');JSON.stringify([r,gets,d.writable,d.enumerable,d.configurable]);",
        "[[10,20],0,true,true,true]",
    );
    assert_both(
        "var r=[99];Object.preventExtensions(r);var a=[1,2];a.constructor={};a.constructor[Symbol.species]=function(){return r};var bad=false;try{a.map(function(v){return v*10})}catch(e){bad=e instanceof TypeError}JSON.stringify([bad,r]);",
        "[true,[10]]",
    );
    assert_both(
        "var r=[99];Object.defineProperty(r,'length',{writable:false});var a=[1,2];a.constructor={};a.constructor[Symbol.species]=function(){return r};var bad=false;try{a.map(function(v){return v*10})}catch(e){bad=e instanceof TypeError}JSON.stringify([bad,r]);",
        "[true,[10]]",
    );
    assert_both(
        "var r=[99,88];var a=[1,2];a.constructor={};a.constructor[Symbol.species]=function(){return r};var bad=false;try{a.map(function(v,i){if(i===1)Object.freeze(r);return v*10})}catch(e){bad=e instanceof TypeError}JSON.stringify([bad,r]);",
        "[true,[10,88]]",
    );
}

#[test]
fn proxies_and_protected_properties_keep_define_semantics() {
    assert_both(
        "var log=[];var r=[];var p=new Proxy(r,{defineProperty:function(t,k,d){log.push([k,d.writable,d.enumerable,d.configurable]);return Reflect.defineProperty(t,k,d)}});var a=[1,2];a.constructor={};a.constructor[Symbol.species]=function(){return p};a.filter(function(){return true});JSON.stringify([r,log]);",
        "[[1,2],[[\"0\",true,true,true],[\"1\",true,true,true]]]",
    );
    assert_both(
        "var r=[99];Object.defineProperty(r,'0',{configurable:false});var a=[1];a.constructor={};a.constructor[Symbol.species]=function(){return r};var bad=false;try{a.map(function(){return 10})}catch(e){bad=e instanceof TypeError}JSON.stringify([bad,r]);",
        "[true,[99]]",
    );
    assert_both(
        "var r=[];Object.defineProperty(r,'0',{value:99,writable:false,enumerable:false,configurable:true});var a=[1];a.constructor={};a.constructor[Symbol.species]=function(){return r};a.map(function(){return 10});var d=Object.getOwnPropertyDescriptor(r,'0');JSON.stringify([r,d.writable,d.enumerable,d.configurable]);",
        "[[10],true,true,true]",
    );
}

#[test]
fn holes_and_function_values_remain_data_properties() {
    assert_both(
        "var calls=0;var source={get x(){calls++;return 7}};var f=Object.getOwnPropertyDescriptor(source,'x').get;var a=[1,,3].map(function(){return f});var d=Object.getOwnPropertyDescriptor(a,'0');var filtered=a.filter(function(){return true});JSON.stringify([a.length,Object.hasOwn(a,1),a[0]===f,filtered.length,filtered[1]===f,calls,d.writable,d.enumerable,d.configurable]);",
        "[3,false,true,2,true,0,true,true,true]",
    );
}
