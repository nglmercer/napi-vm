use napi_vm::{Interpreter, Lexer, Parser, Value};
fn both(source: &str, expected: &str) {
    let program = Interpreter::compile(source).unwrap();
    let result = Interpreter::with_builtins().execute(&program).unwrap();
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
fn repeated_patterns_keep_instance_state_and_compile_mutations_independent() {
    both(
        r#"
        function literal(){return /a/g;}
        var a=literal(), b=literal(); a.lastIndex=4; a.note=1;
        b.exec('a'); var c=literal(); a.compile('b','g');
        var d=new RegExp('a','g');
        JSON.stringify([a!==b,b.lastIndex,c.lastIndex,b.note===undefined,
          a.source,b.source,c.source,d.source,d.lastIndex]);
    "#,
        "[true,1,0,true,\"b\",\"a\",\"a\",\"a\",0]",
    );
}
#[test]
fn cached_constructor_patterns_still_observe_coercion_and_invalid_flags() {
    both(
        r#"
        var calls=0; var p={toString:function(){calls++;return 'a';}};
        var f={toString:function(){calls++;return 'g';}};
        var a=new RegExp(p,f); var b=new RegExp(p,f);
        var invalid=0;
        for(var i=0;i<2;i++){try{new RegExp('a','gg');}catch(e){if(e instanceof SyntaxError)invalid++;}}
        JSON.stringify([calls,a!==b,invalid,new RegExp('','g').source,
          new RegExp('a','i').ignoreCase]);
    "#,
        "[4,true,2,\"(?:)\",true]",
    );
}
