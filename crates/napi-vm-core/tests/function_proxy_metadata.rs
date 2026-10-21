use napi_vm_core::{Interpreter, Value};
fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}
#[test]
fn function_constructors_have_ordered_own_metadata() {
    yes(
        "var ok=true;for(var C of [Function,(async function(){}).constructor,(function*(){}).constructor,(async function*(){}).constructor]){var names=Object.getOwnPropertyNames(C);var l=Object.getOwnPropertyDescriptor(C,'length');var n=Object.getOwnPropertyDescriptor(C,'name');ok=ok && names[0]==='length' && names[1]==='name' && l.value===1 && !l.writable && !l.enumerable && l.configurable && typeof n.value==='string' && n.value!=='' && !n.writable && !n.enumerable && n.configurable;}ok",
    );
}
#[test]
fn proxy_constructor_inherits_function_methods_without_public_prototype() {
    yes(
        "var names=Object.getOwnPropertyNames(Proxy);var l=Object.getOwnPropertyDescriptor(Proxy,'length');var n=Object.getOwnPropertyDescriptor(Proxy,'name');Object.getPrototypeOf(Proxy)===Function.prototype && !Object.hasOwn(Proxy,'prototype') && names[0]==='length' && names[1]==='name' && l.value===2 && !l.writable && !l.enumerable && l.configurable && n.value==='Proxy' && !n.writable && !n.enumerable && n.configurable && Proxy.toString()===Function.prototype.toString.call(Proxy) && Proxy.toString().includes('[native code]')",
    );
}
#[test]
fn proxy_binding_preserves_construction_and_requires_new() {
    yes(
        "var P=Proxy.bind(null,{value:42},{});var object=new P();var rejected=false;try{P();}catch(e){rejected=e instanceof TypeError;}object.value===42 && rejected && Function('return 7')()===7",
    );
}
