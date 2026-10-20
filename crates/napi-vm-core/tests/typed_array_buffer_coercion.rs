use napi_vm_core::{Interpreter, Value};
fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}
#[test]
fn detachment_during_offset_and_length_coercion_rejects() {
    yes(
        "var ok=true;for(var C of [Uint8Array,Float64Array,BigInt64Array]){for(var which of ['offset','length']){var b=new ArrayBuffer(32);var coercion={valueOf(){b.transfer();return 1;}};var caught=false;try{if(which==='offset')new C(b,{valueOf(){b.transfer();return C.BYTES_PER_ELEMENT;}});else new C(b,0,coercion);}catch(e){caught=e instanceof TypeError;}ok=ok && caught;}}ok",
    );
}
#[test]
fn coercion_and_alignment_precede_detachment_and_bounds_checks() {
    yes(
        "var b=new ArrayBuffer(8);b.transfer();var stop={};var caught=false;try{new Uint16Array(b,{valueOf(){throw stop;}});}catch(e){caught=e===stop;}var misaligned=false;try{new Uint16Array(b,1,{valueOf(){throw 99;}});}catch(e){misaligned=e instanceof RangeError;}caught && misaligned",
    );
    yes(
        "var b=new ArrayBuffer(8);var stop={};var caught=false;try{new Uint16Array(b,100,{valueOf(){throw stop;}});}catch(e){caught=e===stop;}caught",
    );
}
#[test]
fn length_coercion_observes_current_resizable_buffer_size() {
    yes(
        "var b=new ArrayBuffer(8,{maxByteLength:32});var a=new Uint16Array(b,4,{valueOf(){b.resize(16);return 6;}});a.length===6 && a.byteOffset===4 && a.buffer===b",
    );
    yes(
        "var b=new ArrayBuffer(16,{maxByteLength:32});var caught=false;try{new Uint16Array(b,0,{valueOf(){b.resize(4);return 4;}});}catch(e){caught=e instanceof RangeError;}caught",
    );
}
