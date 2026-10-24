//! Evaluate a flat sequence of empty blocks without allocating tokens, AST
//! nodes or lexical environments. Every other source uses the normal parser.
use crate::{Interpreter, JsString, VmErr};

pub(super) fn try_empty_blocks(vm: &mut Interpreter, source: &JsString) -> Result<bool, VmErr> {
    let mut open = false;
    let mut blocks = 0;
    for (offset, &unit) in source.units().iter().enumerate() {
        if offset % 4096 == 0 {
            vm.check_execution()?;
        }
        match unit {
            9..=13 | 32 => {}
            123 if !open => open = true,
            125 if open => {
                open = false;
                blocks += 1;
            }
            _ => return Ok(false),
        }
    }
    if open || blocks == 0 {
        return Ok(false);
    }
    // Validate the whole source before charging execution fuel: malformed
    // suffixes must still fail parsing before any statement runs. Preserve
    // the AST evaluator's one-fuel-unit charge for each empty BlockStatement.
    for _ in 0..blocks {
        vm.consume_fuel(1)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_eval_still_exhausts_fuel() {
        let mut vm = Interpreter::with_builtins();
        vm.set_fuel_budget(2);
        assert!(try_empty_blocks(&mut vm, &"{}{}{}".into()).is_err());
        assert_eq!(vm.execution_budget().fuel, 0);
    }
    #[test]
    fn rejected_sources_do_not_consume_fuel() {
        let mut vm = Interpreter::with_builtins();
        vm.set_fuel_budget(2);
        for source in ["{}{", "}{", "{{}}", "{};", "{}/*x*/", "{}x", "\u{feff}{}"] {
            assert!(!try_empty_blocks(&mut vm, &source.into()).unwrap());
            assert_eq!(vm.execution_budget().fuel, 2);
        }
    }
    #[test]
    fn long_empty_sources_observe_cancellation() {
        let mut vm = Interpreter::with_builtins();
        vm.cancellation_token().cancel();
        assert!(try_empty_blocks(&mut vm, &"{}".repeat(10_000).into()).is_err());
    }
}
