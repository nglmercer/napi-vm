use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let compiled = Interpreter::compile(source).unwrap();
    let value = Interpreter::with_builtins().execute(&compiled).unwrap();
    assert!(
        matches!(&value, Value::String(s) if s == expected),
        "prepared: {value:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let value = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&value, Value::String(s) if s == expected),
        "AST: {value:?}"
    );
}

#[test]
fn computed_accessor_literals_use_ecmascript_number_keys_and_names() {
    both(
        "var value=1;var o={get [0.0000001](){return value;},set [0.0000001](v){value=v;}};var d=Object.getOwnPropertyDescriptor(o,'1e-7');o['1e-7']=2;[o['1e-7'],d.get.name,d.set.name,Object.keys(o)].join('|')",
        "2|get 1e-7|set 1e-7|1e-7",
    );
}

#[test]
fn computed_methods_match_runtime_keys_at_decimal_and_exponent_boundaries() {
    both(
        "var o={[1e-7](){return 1;},[1e-6](){return 2;},[1e20](){return 3;},[1e21](){return 4;},[5e-324](){return 5;}};[o['1e-7'](),o['0.000001'](),o['100000000000000000000'](),o['1e+21'](),o['5e-324'](),o['1e+21'].name].join(',')",
        "1,2,3,4,5,1e+21",
    );
}

#[test]
fn literal_and_dynamic_keys_define_the_same_property() {
    both(
        "var key=1e-7;var o={[0.0000001](){return 'literal';},[key](){return 'dynamic';}};[Object.keys(o).length,o['1e-7'](),o['1e-7'].name].join(',')",
        "1,dynamic,1e-7",
    );
}

#[test]
fn object_class_and_destructuring_literal_keys_share_canonical_conversion() {
    both(
        "var o={0.0000001:3,1e21:4};var {0.0000001:a,1e21:b}=o;class C{0.0000001(){return a;}get 1e21(){return b;}}var c=new C();[Object.keys(o).join(':'),c['1e-7'](),c['1e+21'],c['1e-7'].name,Object.getOwnPropertyDescriptor(C.prototype,'1e+21').get.name].join('|')",
        "1e-7:1e+21|3|4|1e-7|get 1e+21",
    );
}

#[test]
fn literal_computed_accessors_remain_on_verified_bytecode() {
    let program = Interpreter::compile("var o={get [1e-7](){return 1;}};o['1e-7']").unwrap();
    assert_eq!(
        program.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
}
