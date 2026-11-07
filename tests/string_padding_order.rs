use napi_vm::{Interpreter, Lexer, Parser, Value};

#[test]
fn padding_coerces_fill_before_checking_allocation_limit() {
    let source = r#"
        var log = '';
        var empty = { toString: function() { log += 'e'; return ''; } };
        var throwing = { toString: function() { log += 't'; throw 42; } };
        var a = 'x'.padStart(100000000, empty);
        var b = 'x'.padEnd(100000000, '');
        var caught;
        try { 'x'.padEnd(100000000, throwing); } catch (e) { caught = e; }
        var limited = false;
        try { 'x'.padStart(100000000, 'a'); } catch (e) { limited = e instanceof RangeError; }
        JSON.stringify([a, b, caught, log, limited]);
    "#;
    let prepared = Interpreter::compile(source).unwrap();
    let bytecode = Interpreter::with_builtins().execute(&prepared).unwrap();
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let ast = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    for result in [bytecode, ast] {
        assert!(
            matches!(result, Value::String(ref s) if s == r#"["x","x",42,"et",true]"#),
            "{result:?}"
        );
    }
}
