use napi_vm_core::{Interpreter, Value, parser::parse_cached};

fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(matches!(vm.eval_source(source).unwrap(), Value::Bool(true)));
    let mut vm = Interpreter::with_builtins();
    assert!(matches!(
        vm.run_program_body(&parse_cached(source).unwrap()).unwrap(),
        Value::Bool(true)
    ));
}

#[test]
fn bigint_number_rounding_retains_a_low_sticky_bit() {
    yes(
        "var tie=(1n<<255n)+(1n<<202n);Number(tie)===2**255 && Number(tie+1n)>Number(tie) && Number(-tie-1n)===-Number(tie+1n)",
    );
}

#[test]
fn inverse_hyperbolic_tangent_is_accurate_and_odd_at_the_boundary() {
    yes(
        "var x=0.9999983310699463;Math.abs(Math.atanh(-x)+6.998237084679027)<2e-15 && Math.atanh(-x)===-Math.atanh(x) && Object.is(Math.atanh(-0),-0) && Math.atanh(1)===Infinity && Math.atanh(-1)===-Infinity && Number.isNaN(Math.atanh(2))",
    );
}
