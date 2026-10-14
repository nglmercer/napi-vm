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
fn clz32_wraps_and_handles_nonfinite_inputs() {
    assert_tiers(
        "JSON.stringify([Math.clz32(),Math.clz32(undefined),Math.clz32(NaN),Math.clz32(Infinity),Math.clz32(-Infinity),Math.clz32(-0),Math.clz32(1),Math.clz32(-1),Math.clz32(4294967297),Math.clz32('2'),Math.clz32(0.9),Math.clz32(-2147483648)]);",
        "[32,32,32,32,32,32,31,0,31,30,32,0]",
    );
}

#[test]
fn imul_wraps_the_product_and_coerces_both_arguments() {
    assert_tiers(
        "JSON.stringify([Math.imul(),Math.imul(2),Math.imul(4294967295,5),Math.imul(2147483647,2),Math.imul(4294967297,3),Math.imul(-2.9,3.9),Math.imul(Infinity,2),Object.is(Math.imul(-0,2),0)]);",
        "[0,0,-5,-2,3,-6,0,true]",
    );
    assert_tiers(
        "var log=''; var a={valueOf:function(){log+='a';return NaN}};var b={valueOf:function(){log+='b';return 2}};JSON.stringify([Math.imul(a,b),log]);",
        "[0,\"ab\"]",
    );
}

#[test]
fn fround_preserves_special_values_and_rounds_ties_to_even() {
    assert_tiers(
        "JSON.stringify([Number.isNaN(Math.fround()),Number.isNaN(Math.fround(NaN)),Object.is(Math.fround(-0),-0),Math.fround(Infinity)===Infinity,Math.fround(-Infinity)===-Infinity,Math.fround(1e40)===Infinity,Math.fround(1+Math.pow(2,-24)),Math.fround(1+3*Math.pow(2,-24)),Math.fround(Math.pow(2,-149)),Object.is(Math.fround(-Math.pow(2,-150)),-0),Math.fround('1.5')]);",
        "[true,true,true,true,true,true,1,1.000000238418579,1.401298464324817e-45,true,1.5]",
    );
}

#[test]
fn math_methods_reject_symbols_and_bigints_and_propagate_coercion_errors() {
    assert_tiers(
        "var count=0;var methods=[Math.clz32,Math.imul,Math.fround];for(var i=0;i<methods.length;i++){for(var j=0;j<2;j++){try{methods[i](j===0?1n:Symbol())}catch(e){if(e instanceof TypeError)count++}}}var log='';try{Math.imul({valueOf:function(){log+='a';throw 7}},{valueOf:function(){log+='b';return 1}})}catch(e){log+=e}JSON.stringify([count,log]);",
        "[6,\"a7\"]",
    );
}

#[test]
fn min_max_coerce_after_nan_and_preserve_later_exceptions() {
    assert_tiers(
        "var log='';var x={valueOf:function(){log+='x';return 1}};var y={valueOf:function(){log+='y';throw 7}};var a=Number.isNaN(Math.min(NaN,x));var b=Number.isNaN(Math.max(NaN,x));try{Math.min(NaN,y)}catch(e){log+=e}try{Math.max(NaN,y)}catch(e){log+=e}JSON.stringify([a,b,log,Object.is(Math.min(0,-0),-0),Object.is(Math.max(-0,0),0)]);",
        "[true,true,\"xxy7y7\",true,true]",
    );
}

#[test]
fn new_math_methods_use_standard_metadata_and_descriptors() {
    assert_tiers(
        "var result=[];var names=['clz32','imul','fround'];for(var i=0;i<names.length;i++){var n=names[i];var f=Math[n];var d=Object.getOwnPropertyDescriptor(Math,n);var l=Object.getOwnPropertyDescriptor(f,'length');var p=Object.getOwnPropertyDescriptor(f,'name');var bad=false;try{new f()}catch(e){bad=e instanceof TypeError}result.push([f.name,f.length,d.writable,d.enumerable,d.configurable,l.writable,l.enumerable,l.configurable,p.writable,p.enumerable,p.configurable,bad])}JSON.stringify(result);",
        "[[\"clz32\",1,true,false,true,false,false,true,false,false,true,true],[\"imul\",2,true,false,true,false,false,true,false,false,true,true],[\"fround\",1,true,false,true,false,false,true,false,false,true,true]]",
    );
}

#[test]
fn math_has_a_nonwritable_configurable_symbol_tag() {
    assert_tiers(
        "var d=Object.getOwnPropertyDescriptor(Math,Symbol.toStringTag);JSON.stringify([Math[Symbol.toStringTag],Object.prototype.toString.call(Math),d.writable,d.enumerable,d.configurable]);",
        "[\"Math\",\"[object Math]\",false,false,true]",
    );
}
