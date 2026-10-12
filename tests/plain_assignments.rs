use napi_vm::{Interpreter, Lexer, Parser, Value};
#[test]
fn member_target_precedes_rhs() {
    assert_tiers(
        "var log='';var a={};var b={};var target=a;function rhs(){log+='r';target=b;return 5}function object(){log+='o';return target}var key={toString:function(){log+='k';return 'x'}};object()[key]=rhs();JSON.stringify([log,a.x,b.x]);",
        "[\"ork\",5,null]",
    );
}
#[test]
fn plain_assignment_does_not_get() {
    assert_tiers(
        "var log='';var o={get x(){log+='g';throw 0},set x(v){log+='s'+v}};o.x=3;JSON.stringify(log);",
        "\"s3\"",
    );
}
#[test]
fn names_of_anonymous_rhs() {
    assert_tiers(
        "var a;var b;var c;a=function(){};b=()=>1;c=class {};JSON.stringify([a.name,b.name,c.name]);",
        "[\"a\",\"b\",\"c\"]",
    );
}
#[test]
fn rhs_runs_before_const_failure() {
    assert_tiers(
        "const x=1;var n=0;try{x=(n++,3)}catch(e){}JSON.stringify([n,x]);",
        "[1,1]",
    );
}
#[test]
fn null_target_runs_rhs_before_write_error() {
    assert_tiers(
        "var log='';var key={toString:function(){log+='k';return 'x'}};try{null[key]=(log+='r',1)}catch(e){}JSON.stringify(log);",
        "\"rk\"",
    );
}
#[test]
fn private_field_assignment() {
    assert_tiers(
        "class C{#x=1;set(v){this.#x=v;return this.#x}}JSON.stringify(new C().set(7));",
        "7",
    );
}
#[test]
fn parenthesized_target_keeps_anonymous_name() {
    assert_tiers("var fn;(fn)=function(){};JSON.stringify(fn.name);", "\"\"");
}
#[test]
fn super_key_conversion_follows_rhs() {
    assert_tiers(
        "var log='';var key={toString:function(){log+='k';return 'x'}};class C extends Object{set(){super[key]=(log+='r',7);return this.x}}JSON.stringify([new C().set(),log]);",
        "[7,\"rk\"]",
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
