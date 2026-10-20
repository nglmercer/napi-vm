use napi_vm_core::{Interpreter, Value};
fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}
#[test]
fn typed_array_string_conversion_shares_the_array_intrinsic() {
    yes(
        "var p=Object.getPrototypeOf(Uint8Array.prototype);var d=Object.getOwnPropertyDescriptor(p,'toString');p.toString===Array.prototype.toString && d.writable && !d.enumerable && d.configurable && p.toString.call({join(){return this.value;},value:'generic'})==='generic' && new Uint8Array([1,2]).toString()==='1,2'",
    );
}
#[test]
fn typed_array_from_arity_does_not_limit_mapping_arguments() {
    yes(
        "var C=Object.getPrototypeOf(Uint8Array);var d=Object.getOwnPropertyDescriptor(C.from,'length');var receiver={add:2};var a=Uint8Array.from([1,2],function(x,i){return x+i+this.add;},receiver);d.value===1 && !d.writable && !d.enumerable && d.configurable && a[0]===3 && a[1]===5",
    );
}
