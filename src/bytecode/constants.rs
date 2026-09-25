//! Owned, interpreter-state-free constants for compiled bytecode.
//!
//! A constant pool must be shareable across runtimes and threads, so it
//! carries plain data only: numbers, strings, and nested compiled
//! functions. Never raw runtime `Value`s, which may alias interpreter-owned
//! cells. The VM materializes a fresh runtime `Value` per `LoadConst`.

use std::rc::Rc;

use crate::parser::Statement;

use super::function::BytecodeFunction;
use super::opcode::{KeySrc, Reg};

/// One entry of a [`BytecodeFunction`](super::function::BytecodeFunction)
/// constant pool.
#[derive(Debug, Clone)]
pub enum Constant {
    Number(f64),
    String(String),
    Bool(bool),
    Null,
    Undefined,
    /// Cooked template-literal chunks for one `Template` instruction.
    StringList(Vec<String>),
    /// A parsed bigint literal, shared across executions (immutable).
    BigInt(Rc<crate::bigint::BigInt>),
    /// A regex literal's source and flags; compiled fresh per `LoadConst`
    /// so every evaluation gets its own `lastIndex`, like the evaluator.
    Regex { pattern: String, flags: String },
    /// A nested supported function, compiled to bytecode.
    Function(Rc<BytecodeFunction>),
    /// A nested function the compiler declined (async, generator, or an
    /// otherwise unsupported body). The VM instantiates it closed over the
    /// defining frame environment; calls run through the AST evaluator.
    AstFunction(Rc<AstFunction>),
    /// One object literal's shape for [`Instr::BuildObject`](super::opcode::Instr::BuildObject):
    /// static keys plus the registers holding dynamic keys, values, and
    /// spread sources, in source order.
    ObjectTemplate(Vec<PropEntry>),
}

/// One property of an [`Constant::ObjectTemplate`]: where its key and value
/// live plus its insertion kind. Registers are absolute, evaluated before
/// the build runs.
#[derive(Debug, Clone)]
pub struct PropEntry {
    /// Static keys name a string constant; computed keys name the register
    /// holding the *original* key value (`undefined` there skips the
    /// property, like the evaluator). `None` for spreads.
    pub key: Option<KeySrc>,
    /// The value register (data/accessor/method function) or the spread
    /// source.
    pub val: Reg,
    pub kind: PropKind,
}

/// How one template entry inserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropKind {
    Data,
    Getter,
    Setter,
    Spread,
}

/// A function kept as AST inside a compiled unit (the per-function fallback
/// of Phase E). Carries everything needed to build the same `FunctionData`
/// the AST evaluator would have built; the VM closes it over the defining
/// frame environment, like the evaluator. Capture-free by construction —
/// capturing functions decline the whole unit — because slot bindings are
/// invisible to environment chains, not because the link is unneeded: free
/// variables must still resolve lexically, not through the caller's frame.
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
