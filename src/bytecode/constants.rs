//! Owned, interpreter-state-free constants for compiled bytecode.
//!
//! A constant pool must be shareable across runtimes and threads, so it
//! carries plain data only: numbers, strings, and nested compiled
//! functions. Never raw runtime `Value`s, which may alias interpreter-owned
//! cells. The VM materializes a fresh runtime `Value` per `LoadConst`.

use std::rc::Rc;

use crate::parser::Statement;

use super::function::BytecodeFunction;

/// One entry of a [`BytecodeFunction`](super::function::BytecodeFunction)
/// constant pool.
#[derive(Debug, Clone)]
pub enum Constant {
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Undefined,
    /// A nested supported function, compiled to bytecode.
    Function(Rc<BytecodeFunction>),
    /// A nested function the compiler declined (async, generator, or an
    /// otherwise unsupported body). Capture-free by construction, so the VM
    /// instantiates it as a plain AST-backed function with no closure
    /// environment; calls run through the AST evaluator.
    AstFunction(Rc<AstFunction>),
}

/// A function kept as AST inside a compiled unit (the per-function fallback
/// of Phase E). Carries everything needed to build the same `FunctionData`
/// the AST evaluator would have built, minus the closure environment, which
/// is always empty here: capturing functions decline the whole unit.
#[derive(Debug, Clone)]
pub struct AstFunction {
    pub name: Option<String>,
    pub params: Vec<String>,
    pub body: Rc<Vec<Statement>>,
    pub is_arrow: bool,
    pub is_constructor: bool,
    pub is_async: bool,
    pub is_generator: bool,
    pub uses_arguments: bool,
}
