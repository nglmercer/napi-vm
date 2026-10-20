use napi_vm_core::{Interpreter, Value};
fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}
#[test]
fn same_buffer_species_copies_forward_instead_of_snapshotting() {
    yes(
        "var ok=true;for(var C of [Uint8Array,Float64Array,BigInt64Array]){var values=C===BigInt64Array?[10n,20n,30n,40n,50n,60n]:[10,20,30,40,50,60];var a=new C(values);a.constructor={[Symbol.species]:function(){return new C(a.buffer,2*C.BYTES_PER_ELEMENT);}};var r=a.slice(1,4);ok=ok && r.join(',')==='20,20,20,60';}ok",
    );
}
#[test]
fn resized_zero_copy_count_does_not_read_past_buffer_end() {
    yes(
        "var ok=true;for(var C of [Uint16Array,Float64Array]){for(var D of [C,Uint8Array]){var b=new ArrayBuffer(32,{maxByteLength:32});var a=new C(b);var original=a.length;a.constructor=D;var r=a.slice({valueOf(){b.resize(0);return 1;}});ok=ok && r.length===original-1 && r.every(x=>x===0);}}ok",
    );
}
#[test]
fn same_kind_copy_preserves_nan_payload_bytes() {
    yes(
        "var b=new ArrayBuffer(8);var bytes=new Uint8Array(b);bytes.set([1,2,3,4,5,6,248,127]);var a=new Float64Array(b);var r=a.slice();new Uint8Array(r.buffer).join(',')===bytes.join(',')",
    );
}
