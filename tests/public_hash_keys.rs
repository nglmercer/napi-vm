use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let program = Interpreter::compile(source).unwrap();
    assert_eq!(
        program.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let value = Interpreter::with_builtins().execute(&program).unwrap();
    assert!(
        matches!(&value,Value::String(s) if s==expected),
        "bytecode: {value:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let value = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&value,Value::String(s) if s==expected),
        "AST: {value:?}"
    );
}

#[test]
fn public_hash_keys_participate_in_enumeration_copying_and_json() {
    both(
        "var o={'#visible':1};var keys='';for(var k in o)keys+=k;var {...rest}=o;[Object.keys(o).join(','),keys,Object.keys({...o}).join(','),Object.keys(Object.assign({},o)).join(','),JSON.stringify(rest)].join('|')",
        "#visible|#visible|#visible|#visible|{\"#visible\":1}",
    );
}

#[test]
fn public_and_private_fields_with_matching_names_stay_distinct() {
    both(
        "class C{#visible=7;['#visible']=9;['#constructor']=11;read(){return this.#visible;}}var c=new C();[c.read(),c['#visible'],Object.keys(c).join(','),Object.getOwnPropertyDescriptor(c,'#constructor').enumerable,JSON.stringify(c)].join('|')",
        "7|9|#visible,#constructor|true|{\"#visible\":9,\"#constructor\":11}",
    );
}

#[test]
fn arrays_keep_public_hash_named_properties() {
    both(
        "var a=[];a['#visible']=3;[Object.keys(a).join(','),Object.getOwnPropertyNames(a).join(','),Object.keys({...a}).join(',')].join('|')",
        "#visible|length,#visible|#visible",
    );
}
