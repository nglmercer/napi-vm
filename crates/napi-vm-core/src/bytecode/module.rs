//! A compiled top-level program: the bytecode half of `Executable`.
//!
//! In Phase E a module is exactly one compiled unit: the top-level
//! statements as a zero-parameter function. Phase G grows this into a
//! registry of compiled modules with export tables; the type exists now so
//! `PreparedProgram` does not change shape again later.

use std::rc::Rc;

use super::function::BytecodeFunction;

/// Top-level compiled unit. Nested functions live in the constant pools of
/// `main` (transitively); the VM starts execution at address zero.
#[derive(Debug, Clone)]
pub struct BytecodeModule {
    pub main: Rc<BytecodeFunction>,
}
