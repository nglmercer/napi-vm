//! Register-VM instruction set (Phase E).
//!
//! The VM is a register machine: expressions evaluate into an unbounded
//! (verifier-bounded) set of per-frame registers, while named bindings live
//! in per-frame local slots or, for top-level outer declarations, in the
//! interpreter's global environment. Short-circuiting, loops, and branches
//! lower to explicit jumps; everything with intricate JavaScript semantics
//! (operators, property access, calls) delegates to the same interpreter
//! helpers the AST evaluator uses, so both tiers share one semantics
//! implementation.
//!
//! Operands are typed (`Reg`, `Slot`, pool indices, jump targets) rather
//! than packed words: the verifier checks every index against its table, so
//! malformed bytecode can fail verification but never causes unchecked
//! indexing or Rust UB.

use std::fmt;

use crate::parser::{AssignOp, BinOp, UnOp};

use super::function::SlotKind;

/// An expression-temporary register within one frame.
pub type Reg = u16;

/// A named-binding slot within one frame.
pub type Slot = u16;

/// A jump destination: an index into the function's instruction stream.
pub type Target = u32;

/// A property key that is either a pooled string or a computed register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySrc {
    Const(u16),
    Reg(Reg),
}

/// One executable instruction with typed operands.
#[derive(Debug, Clone, PartialEq)]
pub enum Instr {
    // -- data movement --------------------------------------------------
    /// `dst = constants[cst]` (fresh runtime value per execution).
    LoadConst { dst: Reg, cst: u16 },
    /// `dst = frame.register[src]`.
    Mov { dst: Reg, src: Reg },
    /// `dst = slots[slot]`, enforcing the temporal dead zone.
    LoadLocal { dst: Reg, slot: Slot },
    /// `slots[slot] = src`, enforcing const assignment rules.
    StoreLocal { slot: Slot, src: Reg },
    /// Hoisting declaration: reset the slot to `undefined` with a new kind,
    /// mirroring `Environment::declare` exactly (no checks, replaces).
    DeclareLocal { slot: Slot, kind: SlotKind, initialized: bool },
    /// Hoisting/declarator initialization: set value and mark initialized,
    /// keeping the slot kind, mirroring `try_set`/`initialize` (no checks).
    InitLocal { slot: Slot, src: Reg },
    /// Hoisted top-level function binding: `set_binding` through the global
    /// environment (keeps kind, quota-checked, no const check).
    InitGlobal { name: u16, src: Reg },
    /// Top-level `var` hoisting: define `undefined` only when no binding
    /// exists yet, mirroring `hoist_vars` (a previous `eval` may own it).
    HoistVarGlobal { name: u16 },
    /// A bare `var x;` on a slot: no-op when initialized, dead-zone error
    /// otherwise (a merged `let` may still be uninitialized).
    BareVarLocal { slot: Slot },
    /// A bare `var x;` on a global binding: same rule via the environment.
    BareVarGlobal { name: u16 },
    /// `dst = global.lookup(name)`: `ReferenceError` when missing or dead.
    LoadGlobal { dst: Reg, name: u16 },
    /// Assign through the scope chain, creating an implicit global when
    /// missing (sloppy mode, like the AST evaluator).
    StoreGlobal { name: u16, src: Reg },
    /// Declare a top-level binding (hoisting), with quota enforcement.
    DefineGlobal {
        name: u16,
        src: Reg,
        kind: SlotKind,
        initialized: bool,
    },
    /// `dst` = the frame's `this` value (non-arrow functions).
    LoadThis { dst: Reg },
    /// `dst` = the global environment's `this` (top level / top-level arrows).
    LoadGlobalThis { dst: Reg },
    /// `dst = typeof global.lookup(name)`, evaluating to `"undefined"`
    /// for missing names instead of throwing.
    TypeofGlobal { dst: Reg, name: u16 },
    /// `dst = typeof slots[slot]`, still enforcing the dead zone.
    TypeofLocal { dst: Reg, slot: Slot },

    // -- operators (all delegate to the AST evaluator's helpers) ---------
    /// `dst = lhs <op> rhs`. Short-circuit operators (`&&`, `||`, `??`)
    /// and `,` never appear here: the compiler lowers them to jumps.
    Binary { dst: Reg, op: BinOp, lhs: Reg, rhs: Reg },
    /// `dst = <op> src`. `++`/`--`/`delete` never appear here: they need
    /// targets, so they have dedicated instructions below.
    Unary { dst: Reg, op: UnOp, src: Reg },
    /// Read-modify-write `slot <op>= rhs` with the AST evaluator's exact
    /// check order (writability before coercion before write).
    CompoundLocal { dst: Reg, slot: Slot, op: AssignOp, rhs: Reg },
    /// Read-modify-write `name <op>= rhs` on a global binding.
    CompoundGlobal { dst: Reg, name: u16, op: AssignOp, rhs: Reg },
    /// Read-modify-write `obj[key] <op>= rhs` on a property.
    CompoundProp {
        dst: Reg,
        obj: Reg,
        key: Reg,
        op: AssignOp,
        rhs: Reg,
    },
    /// `++`/`--` on a local slot. `delta` is +1 or -1; `prefix` selects
    /// whether `dst` receives the new value or the old one.
    IncLocal { dst: Reg, slot: Slot, delta: i8, prefix: bool },
    /// `++`/`--` on a global binding.
    IncGlobal { dst: Reg, name: u16, delta: i8, prefix: bool },
    /// `++`/`--` on a property.
    IncProp {
        dst: Reg,
        obj: Reg,
        key: Reg,
        delta: i8,
        prefix: bool,
    },
    /// `dst = delete obj[key]`.
    DelProp { dst: Reg, obj: Reg, key: Reg },
    /// `dst = delete name` on a global binding.
    DelGlobal { dst: Reg, name: u16 },

    // -- control flow ----------------------------------------------------
    Jump { target: Target },
    JumpIfTrue { src: Reg, target: Target },
    JumpIfFalse { src: Reg, target: Target },
    JumpIfNullish { src: Reg, target: Target },
    JumpIfNotNullish { src: Reg, target: Target },
    /// Loop back-edge marker: consumes one loop-budget iteration (the same
    /// budget the AST evaluator's loops consume, so runaway loops raise the
    /// same `RangeError` on both tiers) plus instruction fuel.
    LoopHead,
    Return { src: Reg },
    ReturnUndefined,
    Throw { src: Reg },

    // -- properties, calls, allocation -----------------------------------
    /// `dst = obj[key]` through the full lookup chain (proxies included).
    GetProp { dst: Reg, obj: Reg, key: Reg },
    /// `obj[key] = val` through the full assignment path.
    SetProp { obj: Reg, key: Reg, val: Reg },
    /// `dst = callee(...args)`: `argc` registers starting at `args`.
    Call { dst: Reg, callee: Reg, args: Reg, argc: u16 },
    /// `dst = callee.call(this, ...args)`: a method call keeps its receiver.
    CallMethod {
        dst: Reg,
        callee: Reg,
        this: Reg,
        args: Reg,
        argc: u16,
    },
    /// `dst` = template with `quasis` cooked chunks interpolating `argc`
    /// evaluated values starting at `args`.
    Template { dst: Reg, quasis: u16, args: Reg, argc: u16 },
    /// `dst = new callee(...args)`.
    Construct { dst: Reg, callee: Reg, args: Reg, argc: u16 },
    /// `dst = {}`: a fresh ordinary object. Superseded by [`Instr::BuildObject`]
    /// (single-shot construction needs no live intermediate); retained as
    /// valid IR, never emitted.
    NewObject { dst: Reg },
    /// Define one own property on a live object under construction.
    /// Superseded by [`Instr::BuildObject`]; retained as valid IR, never
    /// emitted. Note there is no `__proto__` switching anywhere: the
    /// evaluator stores it as ordinary data, and so would this.
    SetOwnProp { obj: Reg, key: KeySrc, val: Reg },
    /// `dst = [args..args+argc]` (no holes, no spread in Phase E).
    NewArray { dst: Reg, args: Reg, argc: u16 },
    /// `dst` = a new bytecode-backed function from `constants[func]`.
    MakeFunction { dst: Reg, func: u16 },
    /// `dst` = a new AST-backed function from `constants[ast]` (the
    /// per-function fallback, closed over the defining frame environment
    /// like any other function).
    MakeAstFunction { dst: Reg, ast: u16 },
    /// `dst` = the normalized form of computed key `src`: strings as-is,
    /// numbers stringified, symbols mapped to their slot keys, anything
    /// else `undefined` (those properties are skipped without evaluating
    /// their values, like the evaluator).
    NormalKey { dst: Reg, src: Reg },
    /// `dst` = one object literal from `constants[tmpl]`: keys, values, and
    /// spread sources already evaluated into registers. Insertion, accessor
    /// pairing, spread, symbols, and the property-count limit all follow
    /// the evaluator's construction exactly.
    BuildObject { dst: Reg, tmpl: u16 },
    /// `dst` = the named global, or `undefined` when missing or
    /// uninitialized. Shorthand properties never throw.
    LoadGlobalSoft { dst: Reg, name: u16 },
    /// `dst` = the slot value, or `undefined` when uninitialized.
    LoadLocalSoft { dst: Reg, slot: Slot },
    /// `dst = callee(...spread_args)`: like [`Instr::Call`], but the
    /// argument list in `constants[tmpl]` splices array-valued spread
    /// elements and passes anything else as one argument.
    CallSpread { dst: Reg, callee: Reg, tmpl: u16 },
    /// Method-call form of [`Instr::CallSpread`].
    MethodSpread { dst: Reg, callee: Reg, this: Reg, tmpl: u16 },
    /// `dst` = one array literal from `constants[tmpl]`: plain elements
    /// plus spreads (arrays splice, strings spread per character, anything
    /// else drains the iterator protocol).
    BuildArray { dst: Reg, tmpl: u16 },
    /// `dst` = `src` materialized for an array destructuring pattern:
    /// arrays clone, strings split per character (length-checked), plain
    /// objects and anything else become `[]`.
    ToDestructArray { dst: Reg, src: Reg },
    /// `dst` = `src` sliced from element `from` for an array-rest element.
    /// `src` always holds an array the compiler materialized just before.
    RestArray { dst: Reg, src: Reg, from: u16 },
    /// Throw a `TypeError` when `src` is nullish (object patterns cannot
    /// destructure `null`/`undefined`); otherwise `dst` snapshots `src`'s
    /// own enumerable string keys (only objects have any) for a later
    /// [`Instr::RestObject`].
    CheckDestructObject { dst: Reg, src: Reg },
    /// `dst` = object of the `src` properties named by the `keys` array
    /// minus the `taken` array's keys, each read through the normal member
    /// path. Implements `{ ...rest }`.
    RestObject { dst: Reg, src: Reg, keys: Reg, taken: Reg },
    /// `dst` = own enumerable keys of `src` as a string array (proxy traps
    /// honored). Drives `for-in`.
    EnumKeys { dst: Reg, src: Reg },
    /// Initialize `for-of`: `iter` = the iterator for `src`,
    /// `next` = its `next` method (an error when absent).
    ForOfInit { iter: Reg, next: Reg, src: Reg },
    /// One `for-of` step: call `next` on `iter`; `done` reports truthy
    /// `done` (missing counts as done), `value` the yielded value.
    IterNext { done: Reg, value: Reg, iter: Reg, next: Reg },
    /// Close `src` as a `for-of` iterator (runs a suspended generator's
    /// `finally` blocks); anything else ignores it.
    CloseIterator { src: Reg },
    /// Push a catch handler: a catchable error (`Throw`, `Msg`,
    /// `RuntimeError`) abandons the protected region, stores the catch
    /// value (thrown value, or a converted error object) in `dst`, and
    /// resumes at `target` with the handler popped.
    PushCatch { target: Target, dst: Reg },
    /// Push a finally handler: like [`Instr::PushCatch`] but also
    /// intercepts `return` unwinding, landing at `target` to run cleanup
    /// and [`Instr::Rethrow`]. Neither kind intercepts `break`,
    /// `continue`, or abandonment. `dst` receives the catch value (or
    /// `undefined` for a `return` landing) for uniformity; pads ignore it.
    PushFinally { target: Target, dst: Reg },
    /// Pop the innermost handler: the protected region completed.
    PopHandler,
    /// Re-raise the error a handler just landed with, after cleanup ran.
    Rethrow,
    /// `dst` = `super[key]`: a lookup on the superclass prototype from
    /// the enclosing method's scope (an error outside one).
    SuperMember { dst: Reg, key: Reg },
    /// `dst` = `super(args)`: invoke the superclass constructor on the
    /// current `this` (an error outside a derived constructor).
    SuperCall { dst: Reg, args: Reg, argc: u16 },
    /// Spread-argument form of [`Instr::SuperCall`].
    SuperCallSpread { dst: Reg, tmpl: u16 },
    /// Raise `constants[msg]` as a runtime error. Used where the
    /// evaluator fails during reference evaluation (a bare `super`, an
    /// assignment through one) after running the earlier side effects.
    Raise { msg: u16 },
    /// `dst` = the class defined by `constants[tmpl]`: methods close over
    /// a scope carrying the superclass prototype, the constructor over one
    /// carrying the superclass constructor, and static blocks run once the
    /// class value exists. Member functions come from the template's
    /// constants (bytecode or AST fallback each).
    BuildClass { dst: Reg, tmpl: u16 },
    /// `dst` = `src` converted to a property key, like a computed class
    /// member name evaluated when the class is defined.
    PropertyKey { dst: Reg, src: Reg },
    /// Run the `import` statement in `constants[tmpl]`: resolve and
    /// evaluate the module, then bind its exports as live cells.
    Import { tmpl: u16 },
    /// Publish `src` as this module's default export.
    ExportDefault { src: Reg },
    /// Run the `export { ... }` statement in `constants[tmpl]`.
    ExportNamed { tmpl: u16 },
    /// Run the `export * [as ns] from` statement in `constants[tmpl]`.
    ExportAll { tmpl: u16 },
    /// `dst` = the promise for the namespace of module `src`.
    DynamicImport { dst: Reg, src: Reg },
    /// `dst` = this module's `import.meta` object.
    ImportMeta { dst: Reg },
}

/// The discriminant of [`Instr`], for classification without operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Opcode {
    LoadConst,
    Mov,
    LoadLocal,
    StoreLocal,
    DeclareLocal,
    InitLocal,
    InitGlobal,
    HoistVarGlobal,
    BareVarLocal,
    BareVarGlobal,
    LoadGlobal,
    StoreGlobal,
    DefineGlobal,
    LoadThis,
    LoadGlobalThis,
    TypeofGlobal,
    TypeofLocal,
    Binary,
    Unary,
    CompoundLocal,
    CompoundGlobal,
    CompoundProp,
    IncLocal,
    IncGlobal,
    IncProp,
    DelProp,
    DelGlobal,
    Jump,
    JumpIfTrue,
    JumpIfFalse,
    JumpIfNullish,
    JumpIfNotNullish,
    LoopHead,
    Return,
    ReturnUndefined,
    Throw,
    GetProp,
    SetProp,
    Call,
    CallMethod,
    Template,
    Construct,
    NewObject,
    SetOwnProp,
    NewArray,
    MakeFunction,
    MakeAstFunction,
    NormalKey,
    BuildObject,
    LoadGlobalSoft,
    LoadLocalSoft,
    CallSpread,
    MethodSpread,
    BuildArray,
    ToDestructArray,
    RestArray,
    CheckDestructObject,
    RestObject,
    EnumKeys,
    ForOfInit,
    IterNext,
    CloseIterator,
    PushCatch,
    PushFinally,
    PopHandler,
    Rethrow,
    SuperMember,
    SuperCall,
    SuperCallSpread,
    Raise,
    BuildClass,
    PropertyKey,
    Import,
    ExportDefault,
    ExportNamed,
    ExportAll,
    DynamicImport,
    ImportMeta,
}

impl Instr {
    /// The discriminant of this instruction.
    pub fn opcode(&self) -> Opcode {
        match self {
            Instr::LoadConst { .. } => Opcode::LoadConst,
            Instr::Mov { .. } => Opcode::Mov,
            Instr::LoadLocal { .. } => Opcode::LoadLocal,
            Instr::StoreLocal { .. } => Opcode::StoreLocal,
            Instr::DeclareLocal { .. } => Opcode::DeclareLocal,
            Instr::InitLocal { .. } => Opcode::InitLocal,
            Instr::InitGlobal { .. } => Opcode::InitGlobal,
            Instr::HoistVarGlobal { .. } => Opcode::HoistVarGlobal,
            Instr::BareVarLocal { .. } => Opcode::BareVarLocal,
            Instr::BareVarGlobal { .. } => Opcode::BareVarGlobal,
            Instr::LoadGlobal { .. } => Opcode::LoadGlobal,
            Instr::StoreGlobal { .. } => Opcode::StoreGlobal,
            Instr::DefineGlobal { .. } => Opcode::DefineGlobal,
            Instr::LoadThis { .. } => Opcode::LoadThis,
            Instr::LoadGlobalThis { .. } => Opcode::LoadGlobalThis,
            Instr::TypeofGlobal { .. } => Opcode::TypeofGlobal,
            Instr::TypeofLocal { .. } => Opcode::TypeofLocal,
            Instr::Binary { .. } => Opcode::Binary,
            Instr::Unary { .. } => Opcode::Unary,
            Instr::CompoundLocal { .. } => Opcode::CompoundLocal,
            Instr::CompoundGlobal { .. } => Opcode::CompoundGlobal,
            Instr::CompoundProp { .. } => Opcode::CompoundProp,
            Instr::IncLocal { .. } => Opcode::IncLocal,
            Instr::IncGlobal { .. } => Opcode::IncGlobal,
            Instr::IncProp { .. } => Opcode::IncProp,
            Instr::DelProp { .. } => Opcode::DelProp,
            Instr::DelGlobal { .. } => Opcode::DelGlobal,
            Instr::Jump { .. } => Opcode::Jump,
            Instr::JumpIfTrue { .. } => Opcode::JumpIfTrue,
            Instr::JumpIfFalse { .. } => Opcode::JumpIfFalse,
            Instr::JumpIfNullish { .. } => Opcode::JumpIfNullish,
            Instr::JumpIfNotNullish { .. } => Opcode::JumpIfNotNullish,
            Instr::LoopHead => Opcode::LoopHead,
            Instr::Return { .. } => Opcode::Return,
            Instr::ReturnUndefined => Opcode::ReturnUndefined,
            Instr::Throw { .. } => Opcode::Throw,
            Instr::GetProp { .. } => Opcode::GetProp,
            Instr::SetProp { .. } => Opcode::SetProp,
            Instr::Call { .. } => Opcode::Call,
            Instr::CallMethod { .. } => Opcode::CallMethod,
            Instr::Template { .. } => Opcode::Template,
            Instr::Construct { .. } => Opcode::Construct,
            Instr::NewObject { .. } => Opcode::NewObject,
            Instr::SetOwnProp { .. } => Opcode::SetOwnProp,
            Instr::NewArray { .. } => Opcode::NewArray,
            Instr::MakeFunction { .. } => Opcode::MakeFunction,
            Instr::MakeAstFunction { .. } => Opcode::MakeAstFunction,
            Instr::NormalKey { .. } => Opcode::NormalKey,
            Instr::BuildObject { .. } => Opcode::BuildObject,
            Instr::LoadGlobalSoft { .. } => Opcode::LoadGlobalSoft,
            Instr::LoadLocalSoft { .. } => Opcode::LoadLocalSoft,
            Instr::CallSpread { .. } => Opcode::CallSpread,
            Instr::MethodSpread { .. } => Opcode::MethodSpread,
            Instr::BuildArray { .. } => Opcode::BuildArray,
            Instr::ToDestructArray { .. } => Opcode::ToDestructArray,
            Instr::RestArray { .. } => Opcode::RestArray,
            Instr::CheckDestructObject { .. } => Opcode::CheckDestructObject,
            Instr::RestObject { .. } => Opcode::RestObject,
            Instr::EnumKeys { .. } => Opcode::EnumKeys,
            Instr::ForOfInit { .. } => Opcode::ForOfInit,
            Instr::IterNext { .. } => Opcode::IterNext,
            Instr::CloseIterator { .. } => Opcode::CloseIterator,
            Instr::PushCatch { .. } => Opcode::PushCatch,
            Instr::PushFinally { .. } => Opcode::PushFinally,
            Instr::PopHandler => Opcode::PopHandler,
            Instr::Rethrow => Opcode::Rethrow,
            Instr::SuperMember { .. } => Opcode::SuperMember,
            Instr::SuperCall { .. } => Opcode::SuperCall,
            Instr::SuperCallSpread { .. } => Opcode::SuperCallSpread,
            Instr::Raise { .. } => Opcode::Raise,
            Instr::BuildClass { .. } => Opcode::BuildClass,
            Instr::PropertyKey { .. } => Opcode::PropertyKey,
            Instr::Import { .. } => Opcode::Import,
            Instr::ExportDefault { .. } => Opcode::ExportDefault,
            Instr::ExportNamed { .. } => Opcode::ExportNamed,
            Instr::ExportAll { .. } => Opcode::ExportAll,
            Instr::DynamicImport { .. } => Opcode::DynamicImport,
            Instr::ImportMeta { .. } => Opcode::ImportMeta,
        }
    }

    /// Fuel cost of one execution, per the §22 budget table. Plain moves
    /// are free; allocation and calls cost more. Exact numbers are a
    /// starting point for benchmark tuning, not a final schedule.
    pub fn cost(&self) -> u64 {
        match self.opcode() {
            Opcode::Call | Opcode::CallMethod | Opcode::CallSpread | Opcode::MethodSpread => 5,
            Opcode::Construct => 8,
            Opcode::NewObject | Opcode::NewArray | Opcode::BuildObject | Opcode::BuildArray => 10,
            Opcode::GetProp | Opcode::SetProp | Opcode::SetOwnProp => 2,
            Opcode::Mov
            | Opcode::Jump
            | Opcode::JumpIfTrue
            | Opcode::JumpIfFalse
            | Opcode::JumpIfNullish
            | Opcode::JumpIfNotNullish
            | Opcode::LoopHead => 0,
            _ => 1,
        }
    }
}

impl fmt::Display for Instr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instr::LoadConst { dst, cst } => write!(f, "LOAD_CONST r{dst}, c{cst}"),
            Instr::Mov { dst, src } => write!(f, "MOV r{dst}, r{src}"),
            Instr::LoadLocal { dst, slot } => write!(f, "LOAD_LOCAL r{dst}, s{slot}"),
            Instr::StoreLocal { slot, src } => write!(f, "STORE_LOCAL s{slot}, r{src}"),
            Instr::DeclareLocal { slot, kind, initialized } => {
                write!(f, "DECLARE_LOCAL s{slot}, {kind:?}, init={initialized}")
            }
            Instr::InitLocal { slot, src } => write!(f, "INIT_LOCAL s{slot}, r{src}"),
            Instr::InitGlobal { name, src } => write!(f, "INIT_GLOBAL c{name}, r{src}"),
            Instr::HoistVarGlobal { name } => write!(f, "HOIST_VAR_GLOBAL c{name}"),
            Instr::BareVarLocal { slot } => write!(f, "BARE_VAR_LOCAL s{slot}"),
            Instr::BareVarGlobal { name } => write!(f, "BARE_VAR_GLOBAL c{name}"),
            Instr::LoadGlobal { dst, name } => write!(f, "LOAD_GLOBAL r{dst}, c{name}"),
            Instr::StoreGlobal { name, src } => write!(f, "STORE_GLOBAL c{name}, r{src}"),
            Instr::DefineGlobal {
                name,
                src,
                kind,
                initialized,
            } => write!(
                f,
                "DEFINE_GLOBAL c{name}, r{src}, {kind:?}, init={initialized}"
            ),
            Instr::LoadThis { dst } => write!(f, "LOAD_THIS r{dst}"),
            Instr::LoadGlobalThis { dst } => write!(f, "LOAD_GLOBAL_THIS r{dst}"),
            Instr::TypeofGlobal { dst, name } => write!(f, "TYPEOF_GLOBAL r{dst}, c{name}"),
            Instr::TypeofLocal { dst, slot } => write!(f, "TYPEOF_LOCAL r{dst}, s{slot}"),
            Instr::Binary { dst, op, lhs, rhs } => {
                write!(f, "BINARY r{dst}, {op:?}, r{lhs}, r{rhs}")
            }
            Instr::Unary { dst, op, src } => write!(f, "UNARY r{dst}, {op:?}, r{src}"),
            Instr::CompoundLocal { dst, slot, op, rhs } => {
                write!(f, "COMPOUND_LOCAL r{dst}, s{slot}, {op:?}, r{rhs}")
            }
            Instr::CompoundGlobal { dst, name, op, rhs } => {
                write!(f, "COMPOUND_GLOBAL r{dst}, c{name}, {op:?}, r{rhs}")
            }
            Instr::CompoundProp { dst, obj, key, op, rhs } => {
                write!(f, "COMPOUND_PROP r{dst}, r{obj}, r{key}, {op:?}, r{rhs}")
            }
            Instr::IncLocal { dst, slot, delta, prefix } => {
                write!(f, "INC_LOCAL r{dst}, s{slot}, {delta}, prefix={prefix}")
            }
            Instr::IncGlobal { dst, name, delta, prefix } => {
                write!(f, "INC_GLOBAL r{dst}, c{name}, {delta}, prefix={prefix}")
            }
            Instr::IncProp { dst, obj, key, delta, prefix } => {
                write!(f, "INC_PROP r{dst}, r{obj}, r{key}, {delta}, prefix={prefix}")
            }
            Instr::DelProp { dst, obj, key } => write!(f, "DEL_PROP r{dst}, r{obj}, r{key}"),
            Instr::DelGlobal { dst, name } => write!(f, "DEL_GLOBAL r{dst}, c{name}"),
            Instr::Jump { target } => write!(f, "JUMP @{target}"),
            Instr::JumpIfTrue { src, target } => write!(f, "JUMP_IF_TRUE r{src}, @{target}"),
            Instr::JumpIfFalse { src, target } => write!(f, "JUMP_IF_FALSE r{src}, @{target}"),
            Instr::JumpIfNullish { src, target } => {
                write!(f, "JUMP_IF_NULLISH r{src}, @{target}")
            }
            Instr::JumpIfNotNullish { src, target } => {
                write!(f, "JUMP_IF_NOT_NULLISH r{src}, @{target}")
            }
            Instr::LoopHead => write!(f, "LOOP_HEAD"),
            Instr::Return { src } => write!(f, "RETURN r{src}"),
            Instr::ReturnUndefined => write!(f, "RETURN_UNDEFINED"),
            Instr::Throw { src } => write!(f, "THROW r{src}"),
            Instr::GetProp { dst, obj, key } => write!(f, "GET_PROP r{dst}, r{obj}, r{key}"),
            Instr::SetProp { obj, key, val } => write!(f, "SET_PROP r{obj}, r{key}, r{val}"),
            Instr::Call { dst, callee, args, argc } => {
                write!(f, "CALL r{dst}, r{callee}, r{args}..r{args}+{argc}")
            }
            Instr::CallMethod { dst, callee, this, args, argc } => {
                write!(f, "CALL_METHOD r{dst}, r{callee}, this=r{this}, r{args}..r{args}+{argc}")
            }
            Instr::Template { dst, quasis, args, argc } => {
                write!(f, "TEMPLATE r{dst}, c{quasis}, r{args}..r{args}+{argc}")
            }
            Instr::Construct { dst, callee, args, argc } => {
                write!(f, "CONSTRUCT r{dst}, r{callee}, r{args}..r{args}+{argc}")
            }
            Instr::NewObject { dst } => write!(f, "NEW_OBJECT r{dst}"),
            Instr::SetOwnProp { obj, key, val } => match key {
                KeySrc::Const(c) => write!(f, "SET_OWN_PROP r{obj}, c{c}, r{val}"),
                KeySrc::Reg(r) => write!(f, "SET_OWN_PROP r{obj}, r{r}, r{val}"),
            },
            Instr::NewArray { dst, args, argc } => {
                write!(f, "NEW_ARRAY r{dst}, r{args}..r{args}+{argc}")
            }
            Instr::MakeFunction { dst, func } => write!(f, "MAKE_FUNCTION r{dst}, c{func}"),
            Instr::MakeAstFunction { dst, ast } => write!(f, "MAKE_AST_FUNCTION r{dst}, c{ast}"),
            Instr::NormalKey { dst, src } => write!(f, "NORMAL_KEY r{dst}, r{src}"),
            Instr::BuildObject { dst, tmpl } => write!(f, "BUILD_OBJECT r{dst}, c{tmpl}"),
            Instr::LoadGlobalSoft { dst, name } => write!(f, "LOAD_GLOBAL_SOFT r{dst}, c{name}"),
            Instr::LoadLocalSoft { dst, slot } => write!(f, "LOAD_LOCAL_SOFT r{dst}, s{slot}"),
            Instr::CallSpread { dst, callee, tmpl } => {
                write!(f, "CALL_SPREAD r{dst}, r{callee}, c{tmpl}")
            }
            Instr::MethodSpread { dst, callee, this, tmpl } => {
                write!(f, "METHOD_SPREAD r{dst}, r{callee}, r{this}, c{tmpl}")
            }
            Instr::BuildArray { dst, tmpl } => write!(f, "BUILD_ARRAY r{dst}, c{tmpl}"),
            Instr::ToDestructArray { dst, src } => write!(f, "TO_DESTRUCT_ARRAY r{dst}, r{src}"),
            Instr::RestArray { dst, src, from } => write!(f, "REST_ARRAY r{dst}, r{src}, {from}"),
            Instr::CheckDestructObject { dst, src } => {
                write!(f, "CHECK_DESTRUCT_OBJECT r{dst}, r{src}")
            }
            Instr::RestObject { dst, src, keys, taken } => {
                write!(f, "REST_OBJECT r{dst}, r{src}, r{keys}, r{taken}")
            }
            Instr::EnumKeys { dst, src } => write!(f, "ENUM_KEYS r{dst}, r{src}"),
            Instr::ForOfInit { iter, next, src } => {
                write!(f, "FOR_OF_INIT r{iter}, r{next}, r{src}")
            }
            Instr::IterNext { done, value, iter, next } => {
                write!(f, "ITER_NEXT r{done}, r{value}, r{iter}, r{next}")
            }
            Instr::CloseIterator { src } => write!(f, "CLOSE_ITERATOR r{src}"),
            Instr::PushCatch { target, dst } => write!(f, "PUSH_CATCH @{target}, r{dst}"),
            Instr::PushFinally { target, dst } => write!(f, "PUSH_FINALLY @{target}, r{dst}"),
            Instr::PopHandler => write!(f, "POP_HANDLER"),
            Instr::Rethrow => write!(f, "RETHROW"),
            Instr::SuperMember { dst, key } => write!(f, "SUPER_MEMBER r{dst}, r{key}"),
            Instr::SuperCall { dst, args, argc } => {
                write!(f, "SUPER_CALL r{dst}, r{args}..r{args}+{argc}")
            }
            Instr::SuperCallSpread { dst, tmpl } => write!(f, "SUPER_CALL_SPREAD r{dst}, c{tmpl}"),
            Instr::Raise { msg } => write!(f, "RAISE c{msg}"),
            Instr::BuildClass { dst, tmpl } => write!(f, "BUILD_CLASS r{dst}, c{tmpl}"),
            Instr::PropertyKey { dst, src } => write!(f, "PROPERTY_KEY r{dst}, r{src}"),
            Instr::Import { tmpl } => write!(f, "IMPORT c{tmpl}"),
            Instr::ExportDefault { src } => write!(f, "EXPORT_DEFAULT r{src}"),
            Instr::ExportNamed { tmpl } => write!(f, "EXPORT_NAMED c{tmpl}"),
            Instr::ExportAll { tmpl } => write!(f, "EXPORT_ALL c{tmpl}"),
            Instr::DynamicImport { dst, src } => write!(f, "DYNAMIC_IMPORT r{dst}, r{src}"),
            Instr::ImportMeta { dst } => write!(f, "IMPORT_META r{dst}"),
        }
    }
}
