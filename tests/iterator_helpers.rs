use napi_vm::{Interpreter, Lexer, Parser, Value};
#[test]
fn lazy_pipeline() {
    assert_tiers(
        "JSON.stringify(Iterator.from([1,2,3,4]).map(function(x){return x*2}).filter(function(x){return x>2}).drop(1).take(1).toArray());",
        "[6]",
    );
}
#[test]
fn flat_map() {
    assert_tiers(
        "JSON.stringify(Iterator.from([1,2]).flatMap(function(x){return [x,x+10]}).toArray());",
        "[1,11,2,12]",
    );
}
#[test]
fn chunks_windows() {
    assert_tiers(
        "JSON.stringify([Iterator.from([1,2,3]).chunks(2).toArray(),Iterator.from([1,2,3]).windows(2).toArray(),Iterator.from([1]).windows(3,'allow-partial').toArray()]);",
        "[[[1,2],[3]],[[1,2],[2,3]],[[1]]]",
    );
}
#[test]
fn eager_helpers() {
    assert_tiers(
        "var sum=0; Iterator.from([1,2]).forEach(function(x){sum+=x}); JSON.stringify([Iterator.from([1,2,3]).reduce(function(a,b){return a+b}),sum,Iterator.from([1,2]).some(function(x){return x===2}),Iterator.from([1,2]).every(function(x){return x>0}),Iterator.from([1,2]).find(function(x){return x===2}),Iterator.from([1,NaN]).includes(NaN),Iterator.from([1,null,3]).join('-')]);",
        "[6,3,true,true,2,true,\"1--3\"]",
    );
}
#[test]
fn concat_zip() {
    assert_tiers(
        "JSON.stringify([Iterator.concat([1],[2,3]).toArray(),Iterator.zip([[1,2],[3]]).toArray(),Iterator.zip([[1],[2,3]],{mode:'longest',padding:[9,8]}).toArray(),Iterator.zipKeyed({a:[1,2],b:[3]}).toArray()]);",
        "[[1,2,3],[[1,3]],[[1,2],[9,3]],[{\"a\":1,\"b\":3}]]",
    );
}
#[test]
fn closing_and_reentrancy() {
    assert_tiers(
        "var closed=0; var source={next:function(){return {value:1,done:false}},return:function(){closed++;return {done:true}}}; var h=Iterator.prototype.map.call(source,function(){return h.next()});var caught=false;try{h.next()}catch(e){caught=e instanceof TypeError} JSON.stringify([caught,closed,h.next().done]);",
        "[true,1,true]",
    );
}
#[test]
fn wrapper_and_disposal() {
    assert_tiers(
        "var raw={next:function(){return 5},return:function(){return 6}};var wrap=Iterator.from(raw);var disposed=Iterator.prototype[Symbol.dispose].call(raw);JSON.stringify([wrap.next(),wrap.return(),disposed===undefined,Iterator.from(new String('abc')).join('')]);",
        "[5,6,true,\"abc\"]",
    );
}
#[test]
fn invalid_callback_closes_before_next() {
    assert_tiers(
        "var log='';var raw={get next(){log+='n';return function(){}},return:function(){log+='r';return {done:true}}};try{Iterator.prototype.map.call(raw,1)}catch(e){log+='e'}JSON.stringify(log);",
        "\"re\"",
    );
}
#[test]
fn strict_zip_closes_on_mismatch() {
    assert_tiers(
        "var closed=0;var raw={next:function(){return {value:1,done:false}},return:function(){closed++;return {done:true}}};var h=Iterator.zip([[],raw],{mode:'strict'});var caught=false;try{h.next()}catch(e){caught=e instanceof TypeError}JSON.stringify([caught,closed,h.next().done]);",
        "[true,1,true]",
    );
}
#[test]
fn private_brands_are_not_properties() {
    assert_tiers(
        "var h=Iterator.from([1]).map(function(x){return x});var w=Iterator.from({next:function(){return {done:true}}});var n=0;try{h.next.call(w)}catch(e){if(e instanceof TypeError)n++}try{w.next.call(h)}catch(e){if(e instanceof TypeError)n++}try{h.next.call(new Proxy(h,{}))}catch(e){if(e instanceof TypeError)n++}JSON.stringify([n,Reflect.ownKeys(h).length]);",
        "[3,0]",
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
