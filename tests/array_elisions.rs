use napi_vm::{Interpreter, Lexer, Parser, Value};

fn assert_tiers(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(
        prepared.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
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
fn elisions_contribute_length_without_creating_properties() {
    assert_tiers(
        "var a = [, undefined, , 4,,]; JSON.stringify([a.length, Object.keys(a), 0 in a, 1 in a, 2 in a, 4 in a, Object.getOwnPropertyDescriptor(a, '0')]);",
        "[5,[\"1\",\"3\"],false,true,false,false,null]",
    );
    assert_tiers(
        "var a = [,,,]; var b = [undefined]; JSON.stringify([a.length, Object.keys(a), b.length, Object.keys(b)]);",
        "[3,[],1,[\"0\"]]",
    );
}

#[test]
fn spreads_materialize_holes_but_preserve_surrounding_elisions() {
    assert_tiers(
        "var a = [, ...[, 2], , ...[3], ,]; JSON.stringify([a.length, Object.keys(a), a]);",
        "[6,[\"1\",\"2\",\"4\"],[null,null,2,null,3,null]]",
    );
}

#[test]
fn elisions_observe_inherited_elements_without_owning_them() {
    assert_tiers(
        "Array.prototype[0] = 7; var a = [,]; var b = [...a]; JSON.stringify([a[0], 0 in a, Object.hasOwn(a, 0), b[0], Object.hasOwn(b, 0)]);",
        "[7,true,false,7,true]",
    );
}

#[test]
fn elided_destructuring_skips_iterator_values() {
    // Destructuring assignment intentionally declines to the shared AST
    // evaluator (see docs/destructuring-assignment-checkpoint.md), so this
    // case checks both execution paths agree without requiring bytecode.
    let source = "var a = 0; [, a] = [3, 4]; JSON.stringify([a]);";
    let prepared = Interpreter::compile(source).unwrap();
    let mut prepared_vm = Interpreter::with_builtins();
    let result = prepared_vm.execute(&prepared).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == "[4]"),
        "prepared: {result:?}"
    );
    let mut parser = Parser::new(Lexer::new(source).tokenize());
    let statements = parser.parse_program().unwrap();
    let mut ast = Interpreter::with_builtins();
    let result = ast.run_program_body(&statements).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == "[4]"),
        "AST: {result:?}"
    );
}

#[test]
fn searches_visit_inherited_accessors_and_read_elements_live() {
    assert_tiers(
        "Object.defineProperty(Array.prototype, '0', {set: function(value){}, configurable:true}); var a = [,]; JSON.stringify([a.indexOf(undefined), a.lastIndexOf(undefined), a.includes(undefined)]);",
        "[0,0,true]",
    );
    assert_tiers(
        "var a = [1, 2, 3]; Object.defineProperty(a, '0', {get: function(){ a[1] = 7; return 1; }}); JSON.stringify([a.indexOf(7), a.lastIndexOf(7), a.includes(7), [,].indexOf(undefined)]);",
        "[1,1,true,-1]",
    );
}

#[test]
fn searches_coerce_indices_and_support_array_like_receivers() {
    assert_tiers(
        "var a = {0: 'x', 1: 'y', length: 2}; var index = {valueOf: function(){ return 1; }}; JSON.stringify([Array.prototype.indexOf.call(a, 'y', index), Array.prototype.lastIndexOf.call(a, 'y', index), Array.prototype.includes.call(a, 'y', index)]);",
        "[1,1,true]",
    );
    assert_tiers(
        "var count = 0; var index = {valueOf: function(){ count++; throw new Error('unexpected'); }}; var a = []; JSON.stringify([a.indexOf(1, index), a.lastIndexOf(1, index), a.includes(1, index), count]);",
        "[-1,-1,false,0]",
    );
}
