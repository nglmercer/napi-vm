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
fn typed_array_methods_preserve_live_reads_and_receiver() {
    assert_tiers(
        "var a=new Uint8Array([1,2,3,4]);var same=a.copyWithin(1,0,3)===a;var log='';var found=a.findLast(function(v,i,o){log+=i;if(i===3)o[2]=9;return v===9});var index=a.findLastIndex(function(v){return v===1});var reduced=a.reduceRight(function(acc,v,i,o){if(o!==a)throw 'receiver';return acc+v},0);JSON.stringify([same,Array.from(a),found,index,reduced,log,Array.from(a.with(-1,7)),Array.from(a)]);",
        "[true,[1,1,9,3],9,1,14,\"32\",[1,1,9,7],[1,1,9,3]]",
    );
}
#[test]
fn typed_array_coercion_and_argument_validation() {
    assert_tiers(
        "var log='';var a=new Uint8Array([1,2,3]);a.fill({valueOf:function(){log+='v';return 4}},{valueOf:function(){log+='s';return 1}},{valueOf:function(){log+='e';return 2}});a.set({get length(){log+='l';return 2},get 0(){log+='a';return 8},get 1(){log+='b';return 9}},0);var bad=0;try{a.set([], -1)}catch(e){if(e instanceof RangeError)bad++}try{new BigInt64Array(0).set(new Uint8Array(0))}catch(e){if(e instanceof TypeError)bad++}try{a.with(99,Symbol())}catch(e){if(e instanceof TypeError)bad++}var empty=new Uint8Array(0);try{empty.reduceRight(function(){})}catch(e){if(e instanceof TypeError)bad++}JSON.stringify([Array.from(a),log,bad,empty.reduceRight(function(){},undefined)===undefined]);",
        "[[8,9,3],\"vselab\",4,true]",
    );
}
#[test]
fn typed_array_metadata_and_raw_byte_copy() {
    assert_tiers(
        "var names=['copyWithin','reduceRight','findLast','findLastIndex','with'];var ok=names.every(function(n){var f=Uint8Array.prototype[n];var d=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Uint8Array.prototype),n);return f.name===n&&f.length===(n==='copyWithin'||n==='with'?2:1)&&!d.enumerable&&d.writable&&d.configurable&&!Object.hasOwn(f,'prototype')});var b=new ArrayBuffer(16);var u=new Uint32Array(b);u.set([1,2146959360,2,2146959360]);var f=new Float64Array(b);f.copyWithin(1,0,1);JSON.stringify([ok,Array.from(u)]);",
        "[true,[1,2146959360,1,2146959360]]",
    );
}
#[test]
fn typed_array_resize_order() {
    assert_tiers(
        "var b=new ArrayBuffer(2,{maxByteLength:5});var a=new Uint8Array(b);a.set([1,2]);var r=a.with(4,{valueOf:function(){b.resize(5);return 9}});var cb=new ArrayBuffer(3,{maxByteLength:3});var c=new Uint8Array(cb);c.set([1,2,3]);var seen=[];c.findLast(function(v,i){seen.push(v);if(i===2)cb.resize(0);return false});JSON.stringify([Array.from(r),a.length,seen]);",
        "[[1,2],5,[3,null,null]]",
    );
}
