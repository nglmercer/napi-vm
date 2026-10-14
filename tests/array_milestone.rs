use napi_vm::{Interpreter, Lexer, Parser, Value};

fn assert_tiers(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(
        prepared.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let result = Interpreter::with_builtins().execute(&prepared).unwrap();
    assert!(
        matches!(result, Value::String(ref s) if s == expected),
        "bytecode: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(result, Value::String(ref s) if s == expected),
        "AST: {result:?}"
    );
}

#[test]
fn array_sort_is_generic_and_uses_throwing_writes() {
    assert_tiers(
        "var proto={1:'a'};var o=Object.create(proto);o[0]='b';o.length=3;var same=Array.prototype.sort.call(o)===o;var a=[2,,1,undefined];a.sort();var bad=false;var x={0:2,1:1,length:2};Object.defineProperty(x,'1',{writable:false});try{Array.prototype.sort.call(x)}catch(e){bad=e instanceof TypeError}JSON.stringify([same,o[0],o[1],Object.hasOwn(o,2),a,Object.hasOwn(a,2),Object.hasOwn(a,3),bad,x[0],x[1]]);",
        "[true,\"a\",\"b\",false,[1,2,null,null],true,false,true,1,1]",
    );
}
#[test]
fn array_join_observes_getters_and_string_conversion() {
    assert_tiers(
        "var log='';var o={get length(){log+='l';return 3},get 0(){log+='0';return {toString:function(){log+='t';o[1]='changed';return 'a'}}},1:'old',2:null};var sep={toString:function(){log+='s';return '|'}};var result=Array.prototype.join.call(o,sep);var a=[1];a.push(a);var b=[{toString:function(){return 'z'}}];JSON.stringify([result,log,a.join(),b.join()]);",
        "[\"a|changed|\",\"ls0t\",\"1,\",\"z\"]",
    );
}
#[test]
fn array_locale_string_passes_arguments_and_validates_methods() {
    assert_tiers(
        "var log='';var item={toLocaleString:function(l,o){log+=l+o;return {toString:function(){log+='t';return 'x'}}}};var r=Array.prototype.toLocaleString.call({0:item,1:null,2:undefined,length:3},'L','O');var bad=false;try{Array.prototype.toLocaleString.call([{toLocaleString:1}])}catch(e){bad=e instanceof TypeError}JSON.stringify([r,log,bad,Array.prototype.toLocaleString.length]);",
        "[\"x,,\",\"LOt\",true,0]",
    );
}

#[test]
fn typed_array_join_uses_observable_separator_conversion() {
    assert_tiers(
        "var a=new Uint8Array([1,2]);var log='';var s={toString:function(){log+='s';return '|'}};var result=a.join(s);var bad=false;try{a.join(Symbol())}catch(e){bad=e instanceof TypeError}JSON.stringify([result,log,bad]);",
        "[\"1|2\",\"s\",true]",
    );
}
