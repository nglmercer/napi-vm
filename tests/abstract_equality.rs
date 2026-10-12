use napi_vm::{Interpreter, Lexer, Parser, Value};

#[test]
fn relational_operators_coerce_left_then_right_with_number_hint() {
    assert_tiers(
        "var log = ''; function operand(name,value){return {[Symbol.toPrimitive]:function(hint){log += name+hint+';'; return value;}};} var a = operand('a',1); var b = operand('b',2); var results = [a < b, a > b, a <= b, a >= b]; JSON.stringify([results,log]);",
        "[[true,false,true,false],\"anumber;bnumber;anumber;bnumber;anumber;bnumber;anumber;bnumber;\"]",
    );
}

#[test]
fn relational_operators_preserve_abrupt_completion_and_symbol_errors() {
    assert_tiers(
        "var log = ''; var left = {valueOf:function(){log += 'l'; throw new Error('stop');}}; var right = {valueOf:function(){log += 'r'; return 2;}}; var caught = ''; try{left > right;}catch(e){caught=e.message;} var symbolError = false; try{Symbol('x') <= right;}catch(e){symbolError=e instanceof TypeError;} JSON.stringify([log,caught,symbolError]);",
        "[\"lr\",\"stop\",true]",
    );
}

#[test]
fn relational_operators_parse_bigint_strings_and_preserve_utf16_order() {
    assert_tiers(
        "JSON.stringify([16n <= '0x10', '0x10' >= 16n, 9007199254740993n > '9007199254740992', 0n >= '', 1n < '1.5', 1n <= '1_0', 1n >= '+0x1', '2' > '10', '\\ud800' < '\\ue000', undefined < 0, undefined >= 0, null <= false]);",
        "[true,true,true,true,false,false,false,true,true,false,false,true]",
    );
}

#[test]
fn mixed_numeric_comparisons_preserve_large_integer_and_fraction_order() {
    assert_tiers(
        "var huge = 1n << 2000n; JSON.stringify([9007199254740993n > 9007199254740992, -9007199254740993n < -9007199254740992, 0n < 0.5, 0n > -0.5, 1n > 0.5, -1n < -0.5, huge == Infinity, huge < Infinity, -huge > -Infinity, huge > Number.MAX_VALUE, 1n == NaN]);",
        "[true,true,true,true,true,true,false,true,true,true,false]",
    );
}

#[test]
fn equality_uses_numeric_string_and_boolean_conversions() {
    assert_tiers(
        "JSON.stringify([false == '', true == '1', true == 'true', false == 'false', 0 == '  ', 16 == '0x10', 2 == '0b10', 1 == '\\ufeff1\\u00a0', 1 == '\\u00851', null == 0, undefined == false, null == undefined, NaN == NaN]);",
        "[true,true,false,false,true,true,true,true,false,false,false,true,false]",
    );
}

#[test]
fn equality_converts_only_the_object_operand_with_default_hint() {
    assert_tiers(
        "var log = ''; var object = {[Symbol.toPrimitive]:function(hint){log += hint; return 3;}}; var left = object == '3'; var right = true != object; JSON.stringify([left, right, log, Object(1) == 1, Object('x') == 'x', Object(false) == false, [] == 0]);",
        "[true,true,\"defaultdefault\",true,true,true,true]",
    );
}

#[test]
fn equality_preserves_object_identity_and_skips_nullish_coercion() {
    assert_tiers(
        "var calls = 0; var a = {[Symbol.toPrimitive]:function(){calls++; throw new Error('unexpected');}}; var b = {}; JSON.stringify([a == a, a == b, a != b, a == null, undefined == a, a === 1, calls]);",
        "[true,false,true,false,false,false,0]",
    );
}

#[test]
fn equality_propagates_abrupt_coercion_and_uses_ordinary_method_order() {
    assert_tiers(
        "var log = ''; var object = {valueOf:function(){log += 'v'; return {};}, toString:function(){log += 's'; return '2';}}; var result = object == 2; var caught = ''; try {({[Symbol.toPrimitive]:function(){throw new Error('stop');}}) != 1;} catch(e){caught = e.message;} var rejected = false; try {({[Symbol.toPrimitive]:function(){return {};}}) == 1;} catch(e){rejected = e instanceof TypeError;} JSON.stringify([result,log,caught,rejected]);",
        "[true,\"vs\",\"stop\",true]",
    );
}

#[test]
fn equality_compares_bigints_without_lossy_string_number_conversion() {
    assert_tiers(
        "var key = Symbol('x'); JSON.stringify([9007199254740993n == '9007199254740993', 9007199254740993n == 9007199254740992, 1n == true, 0n == false, 0n == '', 16n == '0x10', 16n == '+0x10', 1n == '1_0', 1n == '1.0', 1n == null, 1n == Infinity, Object(2n) == '2', Object(key) == key, key == 1]);",
        "[true,false,true,true,true,true,false,false,false,false,false,true,true,false]",
    );
}

#[test]
fn string_to_bigint_grammar_is_shared_with_bigint_conversion() {
    assert_tiers(
        "var invalid = ['1_0','+0x10','-0b1','1.0','\\u00851','\\ud800']; var errors = 0; for(var i = 0; i < invalid.length; i++){try{BigInt(invalid[i]);}catch(e){if(e instanceof SyntaxError)errors++;}} JSON.stringify([errors, String(BigInt('\\ufeff16\\u00a0')), String(BigInt('-10')), String(BigInt('')), String(BigInt('0o10'))]);",
        "[6,\"16\",\"-10\",\"0\",\"8\"]",
    );
}

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
