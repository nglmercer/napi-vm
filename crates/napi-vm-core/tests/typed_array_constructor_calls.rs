use napi_vm_core::{Interpreter, Value};
#[test]
fn concrete_typed_array_calls_reject_before_argument_coercion() {
    let mut vm = Interpreter::with_builtins();
    let value=vm.eval_source("var ok=true;var reads=0;var arg=new Proxy({},{get(){reads++;throw 99;}});for(var C of [Int8Array,Uint8Array,Uint8ClampedArray,Int16Array,Uint16Array,Int32Array,Uint32Array,Float32Array,Float64Array,BigInt64Array,BigUint64Array]){for(var args of [[],[3],[arg],[new ArrayBuffer(8),arg],[new C(1)]]){var caught=false;try{Reflect.apply(C,null,args);}catch(e){caught=e instanceof TypeError;}ok=ok && caught;}}ok && reads===0").unwrap();
    assert!(matches!(value, Value::Bool(true)));
}
#[test]
fn typed_array_construction_and_subclassing_remain_available() {
    let mut vm = Interpreter::with_builtins();
    let value=vm.eval_source("class Bytes extends Uint8Array{};class Bigs extends BigInt64Array{};var a=new Bytes([1,258]);var b=new Bigs([3n]);var c=Reflect.construct(Uint16Array,[2],Bytes);a instanceof Bytes && a[1]===2 && b instanceof Bigs && b[0]===3n && c instanceof Bytes && c.length===2 && new Float64Array().length===0").unwrap();
    assert!(matches!(value, Value::Bool(true)));
}
