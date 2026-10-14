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
fn set_composition_results_and_predicates() {
    assert_tiers(
        "var a=new Set([1,2,3]);var b=new Set([3,4]);JSON.stringify([Array.from(a.union(b)),Array.from(a.intersection(b)),Array.from(a.difference(b)),Array.from(a.symmetricDifference(b)),new Set([3]).isSubsetOf(a),a.isSupersetOf(new Set([1,2])),a.isDisjointFrom(new Set([5])),a.isDisjointFrom(b)]);",
        "[[1,2,3,4],[3],[1,2],[1,2,4],true,true,true,false]",
    );
}
#[test]
fn set_record_order_live_receiver_and_iterator_close() {
    assert_tiers(
        "var log='';var a=new Set([1,2]);var other={get size(){log+='s';return {valueOf:function(){log+='n';return 10}}},get has(){log+='h';return function(k){if(k===1){a.delete(2);a.add(3)}log+=k;return true}},get keys(){log+='k';return function(){throw 'unused'}}};var result=a.intersection(other);var closed=0;var o={size:1,has:function(){},keys:function(){return {next:function(){return {value:9,done:false}},return:function(){closed++;return {}}}}};var sup=a.isSupersetOf(o);JSON.stringify([Array.from(result),log,sup,closed]);",
        "[[1,3],\"snhk13\",false,1]",
    );
}
#[test]
fn set_composition_ignores_species_and_adder_and_deduplicates() {
    assert_tiers(
        "var a=new Set([1]);a.constructor={};Object.defineProperty(a.constructor,Symbol.species,{get:function(){throw 'species'}});var add=Set.prototype.add;Set.prototype.add=function(){throw 'add'};var r=a.union({size:3,has:function(){return false},keys:function(){return [2,2,-0].values()}});Set.prototype.add=add;var bad=0;try{Set.prototype.union.call(new Map(),new Set())}catch(e){if(e instanceof TypeError)bad++}try{a.union({size:NaN})}catch(e){if(e instanceof TypeError)bad++}try{a.union({size:-1})}catch(e){if(e instanceof RangeError)bad++}JSON.stringify([Array.from(r),Object.getPrototypeOf(r)===Set.prototype,bad,Object.is(Array.from(r)[2],0)]);",
        "[[1,2,0],true,3,true]",
    );
}
