use napi_vm::{Interpreter, Lexer, Parser, Value};

fn assert_both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    let mut vm = Interpreter::with_builtins();
    let result = vm.execute(&prepared).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == expected),
        "prepared: {result:?} for {source}"
    );
    let mut parser = Parser::new(Lexer::new(source).tokenize());
    let statements = parser.parse_program().unwrap();
    let mut ast = Interpreter::with_builtins();
    let result = ast.run_program_body(&statements).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == expected),
        "AST: {result:?} for {source}"
    );
}

#[test]
fn single_declarations_leave_prior_completion_standing() {
    assert_both("7; var a; typeof a;", "undefined");
    assert_both("var r = eval('7; var a;'); JSON.stringify(r);", "7");
    assert_both("var r = eval('var a = 2;'); typeof r;", "undefined");
    assert_both("var r = eval('9; var a = 10;'); JSON.stringify(r);", "9");
    assert_both("var r = eval('let a;'); typeof r;", "undefined");
    assert_both("var r = eval('const a = 1;'); typeof r;", "undefined");
}

#[test]
fn multi_declarator_lists_produce_empty_completion() {
    assert_both(
        "var r = eval('11; var a = 12, b;'); JSON.stringify(r);",
        "11",
    );
    assert_both(
        "var r = eval('14; var a, b = 16;'); JSON.stringify(r);",
        "14",
    );
    assert_both(
        "var r = eval('11; let a = 12, b;'); JSON.stringify(r);",
        "11",
    );
    assert_both(
        "var r = eval('11; const a = 12, b = 13;'); JSON.stringify(r);",
        "11",
    );
    assert_both("11; var a = 1, b = 2; JSON.stringify(a + b);", "3");
}

#[test]
fn var_binding_resolves_before_initializer_evaluation() {
    assert_both(
        "var obj = { test262id: 1 }; with (obj) { var test262id = delete obj.test262id; } [obj.test262id, typeof test262id].join(',');",
        "true,undefined",
    );
}
