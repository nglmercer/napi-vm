use napi_vm_core::{Interpreter, Value};

#[test]
fn repeated_capture_substitutions_stop_at_the_string_limit() {
    let mut vm = Interpreter::with_builtins();
    let result = vm
        .eval_source(
            "var input='x'.repeat(1 << 20); var replacement='$1'.repeat(16384); var threw=false; try { input.replace(/(.+)/g, replacement); } catch (error) { threw=error instanceof RangeError; } threw",
        )
        .unwrap();

    assert!(matches!(result, Value::Bool(true)));
}
