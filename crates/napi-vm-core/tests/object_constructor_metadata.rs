use napi_vm_core::{Interpreter, Value};
#[test]
fn object_constructor_has_function_prototype_and_ordered_metadata() {
    let mut vm = Interpreter::with_builtins();
    let result=vm.eval_source("var names=Object.getOwnPropertyNames(Object);var length=Object.getOwnPropertyDescriptor(Object,'length');var name=Object.getOwnPropertyDescriptor(Object,'name');Object.getPrototypeOf(Object)===Function.prototype && Function.prototype.isPrototypeOf(Object) && names[0]==='length' && names[1]==='name' && length.value===1 && !length.writable && !length.enumerable && length.configurable && name.value==='Object' && !name.writable && !name.enumerable && name.configurable").unwrap();
    assert!(matches!(result, Value::Bool(true)));
}
#[test]
fn object_metadata_does_not_change_call_construct_or_subclass_behavior() {
    let mut vm = Interpreter::with_builtins();
    assert!(matches!(vm.eval_source("var input={};class Derived extends Object{};var value=new Derived(input);Object(input)===input && new Object(input)===input && value!==input && value instanceof Derived && Object(7).valueOf()===7 && Object.prototype.constructor===Object && Object().constructor===Object").unwrap(),Value::Bool(true)));
}
