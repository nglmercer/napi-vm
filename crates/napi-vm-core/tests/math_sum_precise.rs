use napi_vm_core::{Interpreter, Value};
fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}
#[test]
fn exact_sum_preserves_cancellation_subnormals_and_rounding() {
    yes(
        "Math.sumPrecise([1e308,1e308,0.1,0.1,0.1,-1e308,-1e308])===0.30000000000000004 && Math.sumPrecise([1e30,0.1,-1e30])===0.1 && Math.sumPrecise([Number.MIN_VALUE,Number.MIN_VALUE])===2*Number.MIN_VALUE && Math.sumPrecise([1,2**-53])===1 && Math.sumPrecise([1,2**-53,Number.MIN_VALUE])===1+Number.EPSILON && Math.sumPrecise([Number.MAX_VALUE,Number.MAX_VALUE])===Infinity",
    );
}
#[test]
fn special_values_and_zero_signs_are_preserved() {
    yes(
        "Object.is(Math.sumPrecise([]),-0) && Object.is(Math.sumPrecise([-0,-0]),-0) && Object.is(Math.sumPrecise([-0,0]),0) && Object.is(Math.sumPrecise([1,-1]),0) && Number.isNaN(Math.sumPrecise([Infinity,-Infinity])) && Number.isNaN(Math.sumPrecise([NaN])) && Math.sumPrecise([-Infinity,1])===-Infinity",
    );
}
#[test]
fn nonnumbers_close_iterator_without_coercion_even_after_nan() {
    yes(
        "var reads=0,closed=0;var bad={valueOf(){reads++;return 1;}};var caught=false;function* values(){try{yield NaN;yield bad;}finally{closed++;}}try{Math.sumPrecise(values());}catch(e){caught=e instanceof TypeError;}caught && closed===1 && reads===0",
    );
}
#[test]
fn iterator_abrupt_completion_and_native_metadata() {
    yes(
        "var stop={};var caught=false;try{Math.sumPrecise({[Symbol.iterator](){return {next(){throw stop;},return(){throw 99;}};}});}catch(e){caught=e===stop;}var d=Object.getOwnPropertyDescriptor(Math.sumPrecise,'length');caught && d.value===1 && !d.writable && !d.enumerable && d.configurable && Math.sumPrecise.name==='sumPrecise'",
    );
}
