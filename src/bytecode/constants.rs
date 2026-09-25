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
    Regex {
        pattern: String,
        flags: String,
    },
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
    /// One spread-bearing argument or element list, in source order, for
    /// [`Instr::CallSpread`](super::opcode::Instr::CallSpread),
    /// [`Instr::MethodSpread`](super::opcode::Instr::MethodSpread), and
    /// [`Instr::BuildArray`](super::opcode::Instr::BuildArray).
    SpreadTemplate(Vec<SpreadEntry>),
    /// One class definition for [`Instr::BuildClass`](super::opcode::Instr::BuildClass):
    /// the constructor and members with their function constants, plus the
    /// registers holding the runtime-evaluated superclass, computed names,
    /// and static initializers.
    ClassTemplate(ClassTemplate),
    /// One `import` statement for [`Instr::Import`](super::opcode::Instr::Import).
    ImportTemplate(ImportTemplate),
    /// One `export { ... }` statement for [`Instr::ExportNamed`](super::opcode::Instr::ExportNamed).
    ExportNamedTemplate(ExportNamedTemplate),
    /// One `export * [as ns] from` statement for [`Instr::ExportAll`](super::opcode::Instr::ExportAll).
    ExportAllTemplate(ExportAllTemplate),
}

/// Scope binding for one computed instance-field key, in field order.
/// The space makes it unwritable as an identifier, so user code can never
/// collide with it; shared by the class compiler (synthetic field
/// assignments) and the class builder (which binds the values).
pub fn class_key_name(index: usize) -> String {
    format!("__class key {index}__")
}

/// A member name as written (`Static`) or evaluated when the class is
/// defined (`Computed`, holding the register with the key value).
#[derive(Debug, Clone)]
pub enum ClassNameTemplate {
    Static(String),
    Computed(Reg),
}

/// One non-constructor class member: a method, accessor, or static field.
/// Instance fields desugar into the constructor before this template is
/// built, so only their computed keys (bound into the constructor's scope
/// under `__class key {i}__`, in order) appear here.
#[derive(Debug, Clone)]
pub struct ClassMemberTemplate {
    pub kind: ClassMemberKind,
    pub is_static: bool,
    pub name: ClassNameTemplate,
    /// Method/accessor function constant (bytecode or AST fallback).
    pub func: Option<u16>,
    /// Static field initializer value.
    pub value: Option<Reg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassMemberKind {
    Method,
    Getter,
    Setter,
    Field,
}

/// A class definition template. Function-constant slots start as `u16::MAX`
/// placeholders the deferred pass overwrites once each method compiles.
#[derive(Debug, Clone)]
pub struct ClassTemplate {
    pub name: String,
    /// A class *expression's* own name, bound in a child scope around the
    /// definition (declarations bind in the enclosing scope instead).
    pub expr_name: Option<String>,
    pub superclass: Option<Reg>,
    pub ctor_func: u16,
    pub ctor_length: usize,
    /// Computed instance-field keys, in field order, bound into the
    /// constructor's scope for the desugared field assignments to read.
    pub ctor_computed_keys: Vec<Reg>,
    pub members: Vec<ClassMemberTemplate>,
    /// Static-block bodies as AST-function constants.
    pub blocks: Vec<u16>,
}

/// One `import` statement: the module specifier plus the local names to
/// bind (default, `(imported, local)` pairs, namespace).
#[derive(Debug, Clone)]
pub struct ImportTemplate {
    pub module: String,
    pub default: Option<String>,
    pub named: Vec<(String, String)>,
    pub namespace: Option<String>,
}

/// One `export { ... }` statement: `(local, exported)` pairs, optionally
/// re-exported from another module.
#[derive(Debug, Clone)]
pub struct ExportNamedTemplate {
    pub specifiers: Vec<(String, String)>,
    pub source: Option<String>,
}

/// One `export * [as ns] from 'm'` statement.
#[derive(Debug, Clone)]
pub struct ExportAllTemplate {
    pub source: String,
    pub alias: Option<String>,
}

/// One element of a [`Constant::SpreadTemplate`]: a register plus whether
/// it spreads (splice semantics) or passes as one value.
#[derive(Debug, Clone, Copy)]
pub struct SpreadEntry {
    pub spread: bool,
    pub reg: Reg,
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
