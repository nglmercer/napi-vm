use napi_vm_core::{Interpreter, Value};
fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}
#[test]
fn noncanonical_numeric_names_are_ordinary_properties() {
    yes(
        "var ok=true;for(var C of [Uint8Array,Float64Array,BigInt64Array]){var a=new C(1);for(var key of ['1000000000000000000000','0.0000001','1.0','+1']){ok=ok && Reflect.defineProperty(a,key,{value:'own',writable:true,enumerable:true,configurable:true}) && a[key]==='own' && key in a && Object.getOwnPropertyDescriptor(a,key).value==='own';Object.defineProperty(a,key,{get(){return 'getter';}});ok=ok && a[key]==='getter' && Reflect.deleteProperty(a,key) && !(key in a);}}ok",
    );
}
#[test]
fn canonical_exponent_names_remain_integer_indexed() {
    yes(
        "var ok=true;for(var C of [Uint8Array,BigInt64Array]){var a=new C(1);for(var key of ['1e+21','1e-7','-0','NaN','Infinity']){ok=ok && !Reflect.defineProperty(a,key,{value:1}) && !Object.hasOwn(a,key) && a[key]===undefined;}}ok",
    );
}
