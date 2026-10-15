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
fn data_view_conversions_are_ordered_and_reads_preserve_bytes() {
    assert_tiers(
        r#"var b=new ArrayBuffer(8),v=new DataView(b),log=[];v.setUint16({valueOf:function(){log.push('index');return 1}},{valueOf:function(){log.push('value');return 4660}},false);var n=v.getUint16({valueOf:function(){log.push('read');return 1}});var raw=new Uint8Array(b);var errors=[];try{v.setUint32(100,{valueOf:function(){throw new TypeError('value')}})}catch(e){errors.push(e.name+':'+e.message)}try{v.getUint8(Infinity)}catch(e){errors.push(e.name)}JSON.stringify([n,raw[1],raw[2],log,errors,v.getUint8(),Object.getOwnPropertyDescriptor(DataView.prototype,'byteLength').get.call(v),Object.prototype.toString.call(v),DataView.prototype.getUint16.length,DataView.prototype.setUint16.length]);"#,
        r#"[4660,18,52,["index","value","read"],["TypeError:value","RangeError"],0,8,"[object DataView]",1,2]"#,
    );
}

#[test]
fn data_view_bigint_and_resize_use_observable_coercion() {
    assert_tiers(
        r#"var b=new ArrayBuffer(8,{maxByteLength:8}),v=new DataView(b,0,8);v.setBigInt64(0,{valueOf:function(){return -2n}},true);var log=[];var errors=[];try{v.setUint8({valueOf:function(){log.push('index');return 0}},{valueOf:function(){log.push('value');b.resize(0);return 1}})}catch(e){errors.push(e.name)}try{Object.getOwnPropertyDescriptor(DataView.prototype,'byteOffset').get.call(v)}catch(e){errors.push(e.name)}var b2=new ArrayBuffer(8),v2=new DataView(b2);v2.setBigUint64(0,true);JSON.stringify([log,errors,v2.getBigUint64(0).toString(),Object.getOwnPropertyDescriptor(DataView.prototype,'buffer').get.call(v)===b]);"#,
        r#"[["index","value"],["TypeError","TypeError"],"1",true]"#,
    );
}

#[test]
fn data_view_float16_rounds_binary64_directly_with_even_ties() {
    assert_tiers(
        r#"var v=new DataView(new ArrayBuffer(2)),r=[];for(var n of [1+2**-11,1+3*2**-11,2**-25,3*2**-25,65504,65520,-0,-Infinity,NaN]){v.setFloat16(0,n);r.push([v.getUint16(0),String(v.getFloat16(0)),Object.is(v.getFloat16(0),-0)])}JSON.stringify(r);"#,
        r#"[[15360,"1",false],[15362,"1.001953125",false],[0,"0",false],[2,"1.1920928955078125e-7",false],[31743,"65504",false],[31744,"Infinity",false],[32768,"0",true],[64512,"-Infinity",false],[32256,"NaN",false]]"#,
    );
}

#[test]
fn typed_array_callbacks_read_live_values_and_keep_the_original_receiver() {
    assert_tiers(
        r#"var a=new Int16Array([1,2,3]),log=[];a.forEach(function(v,k,t){log.push([v,k,t===a]);if(k===0)a[1]=20});var last=a.findLast(function(v,k,t){return t===a&&v<10});var idx=a.findLastIndex(function(v){return v===20});var reduction=a.reduceRight(function(acc,v,k,t){if(k===2)t[1]=7;return acc+v},0);JSON.stringify([log,last,idx,reduction,Array.from(a),Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Int16Array.prototype),'length').get.call(a)]);"#,
        r#"[[[1,0,true],[20,1,true],[3,2,true]],3,1,11,[1,7,3],3]"#,
    );
}

#[test]
fn typed_array_callbacks_keep_initial_length_when_resized() {
    assert_tiers(
        r#"var b=new ArrayBuffer(3,{maxByteLength:3}),a=new Uint8Array(b);a.set([1,2,3]);var seen=[];a.forEach(function(v,k,t){seen.push([String(v),t===a]);if(k===0)b.resize(1)});var errors=[];try{new Uint8Array(0).reduceRight(function(a,b){return a+b})}catch(e){errors.push(e.name)}JSON.stringify([seen,errors,new Uint8Array([1,2]).reduceRight(function(a,b){return a-b}),Uint8Array.prototype.findLast.length]);"#,
        r#"[[["1",true],["undefined",true],["undefined",true]],["TypeError"],1,1]"#,
    );
}

#[test]
fn typed_array_copy_within_and_reverse_mutate_the_original_view() {
    assert_tiers(
        r#"var a=new Uint16Array([1,2,3,4]);var same=a.copyWithin(1,0,3)===a;var reversed=a.reverse()===a;var b=new ArrayBuffer(4,{maxByteLength:4}),v=new Uint8Array(b);v.set([1,2,3,4]);var result=v.copyWithin({valueOf:function(){b.resize(3);return 1}},0,3);JSON.stringify([same,reversed,Array.from(a),result===v,Array.from(v)]);"#,
        r#"[true,true,[3,2,1,1],true,[1,1,2]]"#,
    );
}
