use napi_vm::{Interpreter, Lexer, Parser, Value};
fn both(source: &str, expected: &str) {
    let p = Interpreter::compile(source).unwrap();
    let result = Interpreter::with_builtins().execute(&p).unwrap();
    assert!(
        matches!(result, Value::String(ref s) if s == expected),
        "compiled: {result:?}"
    );
    let body = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&body)
        .unwrap();
    assert!(
        matches!(result, Value::String(ref s) if s == expected),
        "AST: {result:?}"
    );
}
#[test]
fn globals_keep_descriptors_and_observe_rhs_redefinition() {
    both(
        r#"
        var count = 1; count++; count += 4;
        var saved = Object.getOwnPropertyDescriptor(globalThis, 'count');
        Object.defineProperty(globalThis,'count',{writable:false});
        count = 20; var strict = false;
        try { (function(){'use strict'; count = 30;})(); } catch(e){strict=e instanceof TypeError;}
        var seen = 0; globalThis.dynamic = 1;
        dynamic = (Object.defineProperty(globalThis,'dynamic',{
          get:function(){return seen;},set:function(v){seen=v;},configurable:true}), 9);
        JSON.stringify([count,saved.writable,saved.enumerable,saved.configurable,strict,seen,dynamic]);
    "#,
        "[6,true,true,false,true,9,9]",
    );
}
#[test]
fn deleted_resolved_globals_keep_strict_reference_errors_and_with_receivers() {
    both(
        r#"
        globalThis.removable = 1; var strict = false;
        try {(function(){'use strict'; removable = (delete globalThis.removable, 2);})();}
        catch(e){strict=e instanceof ReferenceError;}
        globalThis.again = 1; again = (delete globalThis.again, 3);
        var o = {again: 8}; with(o){again++;}
        JSON.stringify([strict,Object.hasOwn(globalThis,'removable'),again,o.again]);
    "#,
        "[true,false,3,9]",
    );
}
#[test]
fn native_calls_keep_receivers_and_conversion_errors() {
    both(
        r#"
        var calls = 0;
        var value = {valueOf:function(){calls++;return 65;}};
        var text = String.fromCharCode(value);
        var threw = false;
        try {String.fromCharCode(Symbol());}catch(e){threw=e instanceof TypeError;}
        var array = Array.prototype.slice.call({0:7,length:1});
        JSON.stringify([text,calls,threw,array]);
    "#,
        "[\"A\",1,true,[7]]",
    );
}
