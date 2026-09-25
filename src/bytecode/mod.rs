//! Bytecode compiler and register VM (Phase E).
//!
//! The AST evaluator remains the reference tier: [`compiler`] translates
//! supported programs to the [`opcode`] instruction set, [`verify`]
//! validates the output, and [`vm`] executes it. Anything outside the
//! supported subset declines compilation and keeps running on the AST, so
//! behavior never changes, only the execution tier.

pub mod compiler;
pub mod constants;
pub mod function;
pub mod module;
pub mod opcode;
#[cfg(test)]
mod parity_tests;
pub mod verify;
pub mod vm;

pub use compiler::{Unsupported, compile_program};
pub use constants::{AstFunction, Constant, PropEntry, PropKind};
pub use function::{BytecodeFunction, SlotInfo, SlotKind};
pub use module::BytecodeModule;
pub use opcode::{Instr, KeySrc, Opcode, Reg, Slot, Target};
pub use verify::{VerifyError, verify_function, verify_module};
