use napi_vm::{Interpreter, Lexer, Parser, Value};

fn assert_tiers(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    let mut bytecode = Interpreter::with_builtins();
    let result = bytecode.execute(&prepared).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == expected),
        "bytecode: {result:?}"
    );
    let mut parser = Parser::new(Lexer::new(source).tokenize());
    let statements = parser.parse_program().unwrap();
    let mut ast = Interpreter::with_builtins();
    let result = ast.run_program_body(&statements).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == expected),
        "AST: {result:?}"
    );
}

#[test]
fn declaration_names_anonymous_functions() {
    assert_tiers(
        "var a = function() {}; let b = function() {}; const c = function() {}; [a.name, b.name, c.name].join(',');",
        "a,b,c",
    );
}

#[test]
fn declaration_names_arrows_classes_and_generators() {
    assert_tiers(
        "var a = () => 1; let b = class {}; const c = function*() {}; var d = async function() {}; let e = async () => 1; [a.name, b.name, c.name, d.name, e.name].join(',');",
        "a,b,c,d,e",
    );
}

#[test]
fn declaration_does_not_rename_named_functions() {
    assert_tiers(
        "var a = function named() {}; let b = class Named {}; [a.name, b.name].join(',');",
        "named,Named",
    );
}

#[test]
fn declaration_name_inference_only_for_direct_anonymous_definitions() {
    assert_tiers(
        "function make() { return function() {}; } var a = make(); var b = (function() {}); [a.name, b.name].join(',');",
        ",b",
    );
}
