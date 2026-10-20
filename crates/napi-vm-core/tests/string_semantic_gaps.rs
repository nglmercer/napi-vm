use napi_vm_core::{Interpreter, Value};
fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}

#[test]
fn raw_length_uses_number_hint_and_rejects_symbol_and_bigint() {
    yes(
        r#"var ok=true;for(var length of [Symbol(),1n]){var threw=false;try{String.raw({raw:{length}});}catch(e){threw=e instanceof TypeError;}ok=ok && threw;}var log=[];var raw={get length(){log.push('length');return {[Symbol.toPrimitive](hint){log.push(hint);return 2;}};},get 0(){log.push('zero');return 'a';},get 1(){log.push('one');return 'b';}};var substitution={toString(){log.push('sub');return '-';}};ok && String.raw({raw},substitution)==='a-b' && log.join(',')==='length,number,zero,sub,one'"#,
    );
}

#[test]
fn raw_boxes_both_template_and_raw_before_getters() {
    yes(
        r#"var templateReceiver,rawReceiver;Object.defineProperty(Number.prototype,'raw',{get:function(){'use strict';templateReceiver=this;return 9;}});Object.defineProperty(Number.prototype,'length',{get:function(){'use strict';rawReceiver=this;return 1;}});Object.defineProperty(Number.prototype,'0',{get:function(){'use strict';return this===rawReceiver?'boxed':'wrong';}});String.raw(7)==='boxed' && typeof templateReceiver==='object' && templateReceiver.valueOf()===7 && typeof rawReceiver==='object' && rawReceiver.valueOf()===9"#,
    );
}

#[test]
fn raw_observes_logical_length_and_abrupt_getters() {
    yes(
        r#"var stop={};var count=0;var caught=false;try{String.raw({raw:{length:Infinity,get 0(){count++;throw stop;}}});}catch(e){caught=e===stop;}var empty=String.raw({raw:{length:-1,get 0(){throw 99;}}},{toString(){throw 98;}});caught && count===1 && empty===''"#,
    );
}

#[test]
fn starts_with_has_standard_native_metadata() {
    yes(
        r#"var f=String.prototype.startsWith;var d=Object.getOwnPropertyDescriptor(f,'length');d.value===1 && !d.writable && !d.enumerable && d.configurable && f.name==='startsWith' && Object.getPrototypeOf(f)===Function.prototype && f.call('abc','b',1)"#,
    );
}

#[test]
fn legacy_hex_escapes_restore_partial_digits_and_allow_identity() {
    yes(
        r#"new RegExp('\\x1Z').test('x1Z') && new RegExp('\\u12G4').test('u12G4') && /[\x1Z]/.test('Z') && /[\u12G4]/.test('G') && /\x/.test('x') && new RegExp('\\u{2}').test('uu') && 'x'.split(/\x/).join(',')===','"#,
    );
}

#[test]
fn unicode_hex_escapes_remain_strict_and_decode_valid_pairs() {
    yes(
        r#"var ok=/\x41/u.test('A') && /\u0041/u.test('A') && /\u{1F600}/u.test('😀') && /\uD83D\uDE00/u.test('😀');for(var flags of ['u','v']){for(var source of ['\\x','\\x1Z','\\u12G4','\\u{}','\\u{110000}']){var caught=false;try{new RegExp(source,flags);}catch(e){caught=e instanceof SyntaxError;}ok=ok && caught;}}ok"#,
    );
}
