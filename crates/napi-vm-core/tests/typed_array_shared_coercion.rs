use napi_vm_core::{Interpreter, Value};
fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}
#[test]
fn explicit_length_growth_updates_shared_buffer_bounds() {
    yes(
        "var ok=true;for(var C of [Uint16Array,Float64Array,BigInt64Array]){var size=C.BYTES_PER_ELEMENT;var b=new SharedArrayBuffer(size,{maxByteLength:8*size});var a=new C(b,2*size,{valueOf(){b.grow(8*size);return 6;}});ok=ok && a.length===6 && a.byteOffset===2*size && a.buffer===b;}ok",
    );
}
#[test]
fn explicit_length_coercion_precedes_offset_bounds() {
    yes(
        "var b=new SharedArrayBuffer(8);var stop={};var caught=false;try{new Uint16Array(b,100,{valueOf(){throw stop;}});}catch(e){caught=e===stop;}caught",
    );
}
#[test]
fn alignment_precedes_length_coercion_and_omitted_length_tracks_growth() {
    yes(
        "var b=new SharedArrayBuffer(8,{maxByteLength:16});var seen=0;var caught=false;try{new Uint16Array(b,1,{valueOf(){seen++;return 1;}});}catch(e){caught=e instanceof RangeError;}var a=new Uint16Array(b,2);b.grow(16);caught && seen===0 && a.length===7",
    );
}
