//! AST-to-bytecode compiler (stub).
use super::module::BytecodeModule;
use crate::parser::Statement;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub reason: &'static str,
}

pub fn compile_program(_stmts: &[Statement]) -> Result<BytecodeModule, Unsupported> {
    Err(Unsupported { reason: "stub" })
}
