use napi_vm_core::{Interpreter, Lexer, Parser, Value};
fn both(source: &str) {
    for ast in [true, false] {
        let mut vm = Interpreter::with_builtins();
        let result = if ast {
            let mut parser = Parser::new(Lexer::new(source).tokenize());
            vm.run_program_body(&parser.parse())
        } else {
            let program = Interpreter::compile(source).unwrap();
            assert_eq!(
                program.tier(),
                napi_vm_core::interpreter::ExecutionTier::Bytecode
            );
            vm.execute(&program)
        };
        assert!(
            matches!(result, Ok(Value::Bool(true))),
            "AST={ast}: {result:?}"
        );
    }
}
#[test]
fn cooked_and_raw_arrays_are_frozen_with_standard_raw_descriptor() {
    both(
        "function tag(parts){var d=Object.getOwnPropertyDescriptor(parts,'raw');return Object.isFrozen(parts) && Object.isFrozen(parts.raw) && !d.writable && !d.enumerable && !d.configurable && parts[0]==='a' && parts.raw[0]==='a' && !Reflect.set(parts,'0','bad') && !Reflect.deleteProperty(parts,'raw') && !Reflect.defineProperty(parts.raw,'extra',{value:1});}tag`a${7}b`",
    );
}
#[test]
fn construction_uses_intrinsics_even_if_guest_freeze_is_replaced() {
    both(
        "var check=Object.isFrozen;Object.freeze=function(){throw 99;};function tag(parts){return check(parts) && check(parts.raw);}tag`x`",
    );
}
