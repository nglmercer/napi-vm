use napi_vm::{Interpreter, Lexer, Parser, Value};

#[test]
fn binary_numeric_operators_invoke_conversions_in_order() {
    assert_tiers(
        "var log = ''; var defaults = 0; var numbers = 0; function operand(name,value){return {[Symbol.toPrimitive]:function(hint){log += name; if(hint === 'default') defaults++; if(hint === 'number') numbers++; return value;}};} var a = operand('a',8); var b = operand('b',2); JSON.stringify([[a+b,a-b,a*b,a/b,a%b,a**b,a&b,a|b,a^b,a<<b,a>>b,a>>>b],log,defaults,numbers]);",
        "[[10,6,16,4,0,64,0,10,10,32,2,2],\"abababababababababababab\",2,22]",
    );
}

#[test]
fn binary_numeric_operators_keep_bigint_results_and_reject_unsigned_shift() {
    assert_tiers(
        "var a = Object(8n); var b = Object(2n); var error = false; try{a>>>b;}catch(e){error=e instanceof TypeError;} JSON.stringify([[String(a+b),String(a-b),String(a*b),String(a/b),String(a%b),String(a**b),String(a&b),String(a|b),String(a^b),String(a<<b),String(a>>b)],error]);",
        "[[\"10\",\"6\",\"16\",\"4\",\"0\",\"64\",\"0\",\"10\",\"10\",\"32\",\"2\"],true]",
    );
}

#[test]
fn numeric_conversion_of_symbol_precedes_later_numeric_operand() {
    assert_tiers(
        "var calls = 0; var right = {valueOf:function(){calls++; return 2;}}; var errors = 0; try{Symbol('x') * right;}catch(e){if(e instanceof TypeError)errors++;} var before = calls; try{Symbol('x') + right;}catch(e){if(e instanceof TypeError)errors++;} JSON.stringify([errors,before,calls]);",
        "[2,0,1]",
    );
}

#[test]
fn mixed_numeric_type_errors_follow_both_operand_conversions() {
    assert_tiers(
        "var log = ''; var left = {[Symbol.toPrimitive]:function(hint){log += 'l'+hint+';'; return 1n;}}; var right = {[Symbol.toPrimitive]:function(hint){log += 'r'+hint+';'; return 2;}}; var error = false; try{left * right;}catch(e){error=e instanceof TypeError;} JSON.stringify([error,log]);",
        "[true,\"lnumber;rnumber;\"]",
    );
}

#[test]
fn abrupt_numeric_conversion_stops_later_conversion() {
    assert_tiers(
        "var calls = 0; var left = {valueOf:function(){throw new Error('stop');}}; var right = {valueOf:function(){calls++; return 2;}}; var message = ''; try{left ** right;}catch(e){message=e.message;} JSON.stringify([message,calls]);",
        "[\"stop\",0]",
    );
}

#[test]
fn numeric_binary_operators_use_ecmascript_string_number_grammar() {
    assert_tiers(
        "JSON.stringify(['\\ufeff2\\u00a0'*3, '0x10'<<1, 8/'0b10', '08'|0, Number.isNaN('\\u00852'*3), 1/(-0*1) === -Infinity]);",
        "[6,32,4,8,true,true]",
    );
}

#[test]
fn exponentiation_special_cases_follow_number_exponentiate() {
    assert_tiers(
        "JSON.stringify([Number.isNaN((-1) ** Infinity), Number.isNaN((-1) ** -Infinity), Number.isNaN(1 ** NaN), Number.isNaN(Math.pow(-1, Infinity)), (NaN ** 0), ((-1) ** 2), (2 ** -1)]);",
        "[true,true,true,true,1,1,0.5]",
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
