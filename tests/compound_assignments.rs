use napi_vm::{Interpreter, Lexer, Parser, Value};
#[test]
fn reference_order() {
    assert_tiers(
        "var log='';var o={get x(){log+='g';return {valueOf:function(){log+='l';return 4}}},set x(v){log+='s'+v}};var key={toString:function(){log+='k';return 'x'}};function rhs(){log+='r';return {valueOf:function(){log+='v';return 3}}}o[key]+=rhs();JSON.stringify(log);",
        "\"kgrlvs7\"",
    );
}
#[test]
fn snapshot_identifier() {
    assert_tiers(
        "var x=4;function rhs(){x=100;return 3}x*=rhs();JSON.stringify(x);",
        "12",
    );
}
#[test]
fn logical_key_once() {
    assert_tiers(
        "var n=0;var key={toString:function(){n++;return 'x'}};var o={x:0};o[key] ||= 7;JSON.stringify([n,o.x]);",
        "[1,7]",
    );
}
#[test]
fn all_operators() {
    assert_tiers(
        "var a=8;a+=2;a-=1;a*=2;a/=3;a%=5;a**=3;a<<=1;a>>=1;a>>>=1;a|=8;a&=9;a^=2;JSON.stringify(a);",
        "10",
    );
}
#[test]
fn constant_write_follows_conversion() {
    assert_tiers(
        "var log='';const x={valueOf:function(){log+='l';return 2}};try{x+={valueOf:function(){log+='r';return 3}}}catch(e){if(e instanceof TypeError)log+='e'}JSON.stringify(log);",
        "\"lre\"",
    );
}
#[test]
fn bigint_compound() {
    assert_tiers(
        "var x=8n;x+=2n;x*=3n;x>>=1n;var mixed=false;try{x+=1}catch(e){mixed=e instanceof TypeError}JSON.stringify([String(x),mixed]);",
        "[\"15\",true]",
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
