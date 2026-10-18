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

#[test]
fn switch_preserves_case_values_through_abrupt_and_empty_completions() {
    for (body, expected) in [
        ("1; switch ('a') { case 'a': { 3; break; } default: }", "3"),
        (
            "1; switch ('a') { case 'a': 7; case 'b': break; default: }",
            "7",
        ),
        ("1; switch ('a') { case 'a': 7; case 'b': }", "7"),
        (
            "1; do { switch ('a') { case 'a': 6; continue; default: } } while (false)",
            "6",
        ),
        ("label: { 4; switch (1) { case 1: 8; break label; } }", "8"),
        ("1; switch (1) { case 1: undefined; case 2: }", "undefined"),
    ] {
        let source = format!("String(eval({body:?}))");
        assert_both(&source, expected);
    }
}

#[test]
fn switch_selects_before_default_and_skips_selectors_after_a_match() {
    assert_both(
        "var seen = []; switch (2) { default: seen.push('default'); break; case (seen.push('case'), 2): seen.push('matched'); break; } seen.join(',');",
        "case,matched",
    );
    assert_both(
        "var seen = []; switch (1) { case 1: seen.push('one'); case (seen.push('selector'), 2): seen.push('two'); } seen.join(',');",
        "one,two",
    );
}

#[test]
fn nested_statement_completion_distinguishes_empty_and_undefined() {
    for (body, expected) in [
        ("1; label: { break label; }", "1"),
        ("1; { 2; label: { break label; } }", "2"),
        ("1; label: { if (true) break label; }", "undefined"),
        ("1; label: { with ({}) { break label; } }", "undefined"),
        ("1; label: { try { break label; } finally {} }", "undefined"),
        (
            "1; label: { try { 2; } finally { break label; } }",
            "undefined",
        ),
        (
            "var i = 0; while (i++ < 2) { if (i === 1) { 7; } else { break; } }",
            "undefined",
        ),
    ] {
        assert_both(&format!("String(eval({body:?}))"), expected);
    }
}

#[test]
fn top_level_switch_and_loop_completion_matches_prepared_execution() {
    for (source, expected) in [
        ("switch (1) { case 1: 7; break; }", 7.0),
        ("do { label: { 14; break; } 15; } while (false)", 14.0),
        ("switch (1) { case 1: 7; case 2: }", 7.0),
        ("do { 8; continue; } while (false)", 8.0),
        ("label: { 9; break label; }", 9.0),
        ("10; label: { break label; }", 10.0),
        ("1; { 2; label: { break label; } }", 2.0),
        ("label: { try { 2; break label; } finally { 3; } }", 2.0),
        ("for (var value of [1]) { 11; break; }", 11.0),
        ("for (var value of [1]) { 12; continue; }", 12.0),
        (
            "do { switch (1) { case 1: 13; case 2: continue; } } while (false)",
            13.0,
        ),
    ] {
        let prepared = Interpreter::compile(source).unwrap();
        assert_eq!(
            prepared.tier(),
            napi_vm::interpreter::ExecutionTier::Bytecode,
            "{source}: {:?}",
            prepared.fallback_reason()
        );
        let mut vm = Interpreter::with_builtins();
        let compiled = vm.execute(&prepared).unwrap();
        assert!(
            matches!(compiled, Value::Number(value) if value == expected),
            "prepared {source}: {compiled:?}"
        );
        let mut parser = Parser::new(Lexer::new(source).tokenize());
        let statements = parser.parse_program().unwrap();
        let ast = Interpreter::with_builtins()
            .run_program_body(&statements)
            .unwrap();
        assert!(
            matches!(ast, Value::Number(value) if value == expected),
            "AST {source}: {ast:?}"
        );
    }
}

#[test]
fn prepared_control_boundaries_fill_empty_abrupt_completions_with_undefined() {
    for source in [
        "1; label: { if (true) break label; }",
        "1; label: { try { break label; } finally {} }",
        "1; label: { try { 2; } finally { break label; } }",
        "label: { try { 2; break label; } finally { break label; } }",
        "1; switch (1) { case 1: undefined; case 2: }",
    ] {
        let prepared = Interpreter::compile(source).unwrap();
        assert_eq!(
            prepared.tier(),
            napi_vm::interpreter::ExecutionTier::Bytecode,
            "{source}: {:?}",
            prepared.fallback_reason()
        );
        let result = Interpreter::with_builtins().execute(&prepared).unwrap();
        assert!(matches!(result, Value::Undefined), "{source}: {result:?}");
    }
}
