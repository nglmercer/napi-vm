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
fn math_added_functions_and_metadata() {
    assert_tiers(
        "var names=['acos','asin','atan','atan2','acosh','asinh','atanh','cosh','sinh','tanh','expm1','log1p'];var ok=true;var errors=0;for(var i=0;i<names.length;i++){var f=Math[names[i]];var d=Object.getOwnPropertyDescriptor(Math,names[i]);ok=ok&&f.name===names[i]&&f.length===(names[i]==='atan2'?2:1)&&d.writable&&!d.enumerable&&d.configurable;try{f(1n)}catch(e){if(e instanceof TypeError)errors++}}JSON.stringify([ok,errors,Math.acos(1),Math.asin(0),Math.atan(0),Math.atan2(0,1),Math.acosh(1),Math.asinh(0),Math.atanh(0),Math.cosh(0),Math.sinh(0),Math.tanh(0),Math.expm1(0),Math.log1p(0),Object.is(Math.sinh(-0),-0),Object.is(Math.expm1(-0),-0),Math.tanh(Infinity),Number.isNaN(Math.acosh(0))]);",
        "[true,12,0,0,0,0,0,0,0,1,0,0,0,0,true,true,1,true]",
    );
    assert_tiers(
        "var log='';var y={valueOf:function(){log+='y';return NaN}};var x={valueOf:function(){log+='x';return 1}};Math.atan2(y,x);JSON.stringify([log,Math.atan2(-0,-1)===-Math.PI,Object.is(Math.atan2(-0,1),-0)]);",
        "[\"yx\",true,true]",
    );
}
#[test]
fn copying_arrays_densify_holes_and_skip_deleted_getters() {
    assert_tiers(
        "var a=[1,,3];var b=a.toReversed();var c=a.toSpliced(1,1,4,5);JSON.stringify([b,Object.hasOwn(b,1),a,c,[1,2,3].toSpliced(),[1,2,3].toSpliced(1),[1,2,3].toSpliced(1,undefined,4)]);",
        "[[3,null,1],true,[1,null,3],[1,4,5,3],[1,2,3],[1],[1,4,2,3]]",
    );
    assert_tiers(
        "var log='';var o={length:3,get 0(){log+='a';return 1},get 1(){throw Error('deleted')},get 2(){log+='c';return 3}};var a=Array.prototype.toSpliced.call(o,1,1,2);JSON.stringify([a,log,Array.prototype.toSpliced.length,Array.prototype.toReversed.length,Array.prototype.copyWithin.length,Array.prototype[Symbol.unscopables].toSpliced]);",
        "[[1,2,3],\"ac\",2,0,2,true]",
    );
}
#[test]
fn copy_within_overlap_holes_and_receiver_validation() {
    assert_tiers(
        "var a=[1,2,3,4];var same=a.copyWithin(1,0,3)===a;var b=[1,,3];b.copyWithin(0,1);var bad=0;try{Array.prototype.copyWithin.call(null,0)}catch(e){if(e instanceof TypeError)bad++}JSON.stringify([a,same,b,Object.hasOwn(b,0),bad]);",
        "[[1,1,2,3],true,[null,3,3],false,1]",
    );
    assert_tiers(
        "var o={length:9007199254740991,9007199254740989:7,9007199254740990:8};JSON.stringify(Array.prototype.toSpliced.call(o,0,9007199254740989));",
        "[7,8]",
    );
}
#[test]
fn exact_number_formatting_and_signed_zero() {
    assert_tiers(
        "JSON.stringify([(2.5).toExponential(0),(9.99).toExponential(1),(1e21).toExponential(),(-0).toExponential(2),(5e-324).toExponential(2),(1.25).toPrecision(2),(9.99).toPrecision(2),(1e-7).toPrecision(2),(-0).toPrecision(),(1e21).toPrecision(undefined),(1.005).toFixed(2)]);",
        "[\"3e+0\",\"1.0e+1\",\"1e+21\",\"0.00e+0\",\"4.94e-324\",\"1.3\",\"10\",\"1.0e-7\",\"0\",\"1e+21\",\"1.00\"]",
    );
    assert_tiers(
        "var calls=0;var a={valueOf:function(){calls++;return 101}};var bad=false;try{Number.prototype.toExponential.call({},a)}catch(e){bad=e instanceof TypeError}var inf=(Infinity).toExponential(a);var range=false;try{(1).toExponential(101)}catch(e){range=e instanceof RangeError}JSON.stringify([bad,calls,inf,range,Number.prototype.toExponential.name,Number.prototype.toExponential.length]);",
        "[true,1,\"Infinity\",true,\"toExponential\",1]",
    );
}

#[test]
fn strict_named_function_assignment_uses_callers_strictness() {
    assert_tiers(
        "var f=function loop(){'use strict';try{loop=1;}catch(e){return e instanceof TypeError;}};JSON.stringify([f()]);",
        "[true]",
    );
}
