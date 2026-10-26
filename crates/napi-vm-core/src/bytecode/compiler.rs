//! AST-to-bytecode compiler: Phase E scope.
//!
//! Translates a parsed program to [`BytecodeModule`] when every construct is
//! in the supported subset, and declines otherwise so the AST evaluator keeps
//! running the unit. Two decline severities exist: [`Decline::Unit`]
//! abandons the whole program (captures, `super`, top-level constructs),
//! while [`Decline::Func`] compiles the enclosing function as an AST-backed
//! constant instead (async, generators, objects, classes, `try`, `switch`,
//! destructuring, spread, optional chaining, and friends).
//!
//! Supported in Phase E:
//!
//! ```text
//! literals (number/string/bool/null/undefined), template strings
//! var/let/const, hoisting, temporal dead zone, const assignment
//! arithmetic/bitwise/comparison/logical operators, typeof/void/delete
//! ++/--, compound and logical assignment
//! if/else, while, do-while, for(;;), break/continue, return, throw
//! array literals, including elisions and iterable spreads
//! property reads/writes, calls, method calls, new
//! named/anonymous functions incl. arrows (capture-free only)
//! ```
//!
//! Anything else declines. The compiler never changes semantics: every
//! runtime behavior delegates to the same interpreter helpers the AST
//! evaluator calls, and hoisting order, completion values, and error
//! messages are reproduced instruction by instruction (see the notes at each
//! emission site). Function compilation is deferred: nested functions
//! compile after their parent's own body succeeds, so one unsupported
//! function never poisons its siblings and never wastes work on bodies that
//! are discarded.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::interpreter::{block_needs_lexical_scope, produces_completion_value};
use crate::parser::{
    AssignOp, BinOp, ClassMember, Expr, ExprOrBlock, ForBinding, ForInit, LogicalAssignOp,
    MemberName, ObjectProp, Pattern, PatternKey, Statement, SwitchCase, UnOp, VarKind,
    arrow_body_references, collect_var_names, expr_captures_identifier, pattern_names,
    statements_capture_identifier,
};

use super::constants::{
    AstFunction, ClassMemberKind, ClassMemberTemplate, ClassNameTemplate, ClassTemplate, Constant,
    ExportAllTemplate, ExportNamedTemplate, ImportTemplate, PropEntry, PropKind, SpreadEntry,
    class_key_name,
};
use super::function::{BytecodeFunction, SlotInfo, SlotKind};
use super::module::BytecodeModule;
use super::opcode::{Instr, KeySrc, Reg, Slot, Target};

/// Compilation declined: the unit stays on the AST evaluator. The reason
/// names the construct (or limit) that Phase E cannot compile yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub reason: &'static str,
}

/// Internal decline severity. `Func` becomes an AST-backed function
/// constant; `Unit` abandons the whole program. The top level maps both to
/// [`Unsupported`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decline {
    Unit(&'static str),
    Func(&'static str),
}

impl Decline {
    fn reason(self) -> &'static str {
        match self {
            Decline::Unit(reason) | Decline::Func(reason) => reason,
        }
    }
}

/// Compile a parsed program to bytecode, or decline it to the AST tier.
///
/// The caller must run [`verify_module`](super::verify::verify_module) on
/// the result before executing it; a verification failure is a compiler bug
/// and must surface loudly, never fall back silently.
pub fn compile_program(stmts: &[Statement]) -> Result<BytecodeModule, Unsupported> {
    let mut compiler = Compiler::top_level();
    match compiler.compile_top(stmts, false) {
        Ok(mut main) => {
            main.strict = crate::parser::use_strict(stmts);
            Ok(BytecodeModule {
                main: Rc::new(main),
            })
        }
        Err(decline) => Err(Unsupported {
            reason: decline.reason(),
        }),
    }
}

/// Compile a module whose declarations have already been instantiated.
pub(crate) fn compile_linked_program(stmts: &[Statement]) -> Result<BytecodeModule, Unsupported> {
    let mut compiler = Compiler::top_level();
    compiler
        .compile_top(stmts, true)
        .map(|main| BytecodeModule {
            main: Rc::new(main),
        })
        .map_err(|decline| Unsupported {
            reason: decline.reason(),
        })
}

/// How `this` resolves in the unit being compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThisMode {
    /// Non-arrow function bodies: the frame's `this` value.
    Frame,
    /// Arrows and the top level: `this` through the scope chain. A nested
    /// arrow's closure chains to the defining frame environment, which
    /// carries the `this` the arrow captures.
    Global,
}

/// One lexically nested function awaiting compilation. Functions compile
/// after their parent's own body succeeds (see the module docs); `snapshot`
/// freezes the live block bindings at the definition site so capture
/// detection sees the scope the source had, not the scope left when the
/// parent finishes.
struct Deferred<'a> {
    patch: PatchTarget,
    def: FuncDef<'a>,
    snapshot: HashSet<String>,
}

/// Where a deferred function's constant index lands once it compiles.
enum PatchTarget {
    /// Overwrite the placeholder `MakeFunction` at this address.
    MakeFunction { addr: usize },
    /// Fill the constructor slot of a class template constant.
    ClassCtor { tmpl: u16 },
    /// Fill one member's function slot of a class template constant.
    ClassMember { tmpl: u16, index: usize },
}

/// Source slices borrow the AST; synthesized slices live only through compilation.
enum SharedSlice<'a, T> {
    Borrowed(&'a [T]),
    Owned(Rc<[T]>),
}
impl<T> Clone for SharedSlice<'_, T> {
    fn clone(&self) -> Self {
        match self {
            Self::Borrowed(v) => Self::Borrowed(v),
            Self::Owned(v) => Self::Owned(v.clone()),
        }
    }
}
impl<T> std::ops::Deref for SharedSlice<'_, T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match self {
            Self::Borrowed(v) => v,
            Self::Owned(v) => v,
        }
    }
}

/// A function definition to compile, borrowed from the parsed unit.
#[derive(Clone)]
struct FuncDef<'a> {
    source_text: Option<crate::parser::SourceTextRef>,
    named_expression: bool,
    name: Option<String>,
    params: SharedSlice<'a, String>,
    body: FuncBody<'a>,
    is_arrow: bool,
    is_async: bool,
    is_generator: bool,
    is_constructor: bool,
}

#[derive(Clone)]
enum FuncBody<'a> {
    Stmts(SharedSlice<'a, Statement>),
    /// Arrow expression bodies run as `return <expr>`, like the evaluator.
    Expr(&'a Expr),
}

/// A nested-function compilation result: bytecode, or an AST fallback.
enum FuncOutcome {
    Bytecode(Rc<BytecodeFunction>),
    Ast(Rc<AstFunction>),
}

// Borrow expressions in converted assignment patterns from the original AST.
// Defaults and nested functions never require a promoted or leaked lifetime.
#[derive(Clone)]
enum PatternKeyView<'a> {
    Name(&'a str),
    Computed(&'a Expr),
}
#[derive(Clone)]
enum PatternView<'a> {
    Elision,
    Ident(&'a str),
    Member {
        object: &'a Expr,
        property: &'a Expr,
        private: bool,
    },
    Array(Vec<PatternView<'a>>),
    Object(Vec<(PatternKeyView<'a>, Option<PatternView<'a>>)>),
    Rest(Box<PatternView<'a>>),
    Default(Box<PatternView<'a>>, &'a Expr),
}
impl<'a> PatternView<'a> {
    fn from_pattern(p: &'a Pattern) -> Self {
        match p {
            Pattern::Elision => Self::Elision,
            Pattern::Ident(n) => Self::Ident(n),
            Pattern::Member {
                object,
                property,
                private,
            } => Self::Member {
                object,
                property,
                private: *private,
            },
            Pattern::Array(v) => Self::Array(v.iter().map(Self::from_pattern).collect()),
            Pattern::Object(v) => Self::Object(
                v.iter()
                    .map(|(k, v)| {
                        (
                            match k {
                                PatternKey::Name(n) => PatternKeyView::Name(n),
                                PatternKey::Computed(e) => PatternKeyView::Computed(e),
                            },
                            v.as_ref().map(Self::from_pattern),
                        )
                    })
                    .collect(),
            ),
            Pattern::Rest(v) => Self::Rest(Box::new(Self::from_pattern(v))),
            Pattern::Default(v, e) => Self::Default(Box::new(Self::from_pattern(v)), e),
        }
    }
}

/// A written-out `constructor` method, borrowed until its deferred
/// compilation runs.
struct OwnedCtor<'a> {
    params: &'a [String],
    body: &'a [Statement],
}

/// An instance field's desugared key: the static name, or the index into
/// the template's `ctor_computed_keys` holding the definition-time key.
enum FieldKey {
    Static(String),
    Private(String),
    Computed(usize),
}

/// One compile-time lexical scope: slot bindings, or the global marker.
struct Scope {
    bindings: HashMap<String, Slot>,
    /// The top-level outermost scope: names resolve to the global
    /// environment instead of slots.
    global: bool,
    /// Block bindings captured by nested closures: they live in the pushed
    /// runtime scope instead of slots, so every access resolves through
    /// the environment chain. Never populated on the function scope, which
    /// boxes through `captured` instead.
    boxed: HashSet<String>,
    /// Whether this compile scope owns a runtime environment, including an
    /// empty lexical iteration head.
    runtime: bool,
}

/// Break/continue patch lists for one breakable context under compilation
/// (a loop, a switch body, or a labeled non-loop statement).
#[derive(Clone, Copy)]
struct CompletionRegs {
    value: Reg,
    present: Option<Reg>,
}

#[derive(Default)]
struct LoopCtx {
    breaks: Vec<usize>,
    continues: Vec<usize>,
    labels: Vec<String>,
    kind: CtxKind,
    /// Cleanup-stack length when this context opened: a break/continue
    /// targeting it duplicates the cleanups above this depth.
    cleanup_depth: usize,
    completion_depth: usize,
}

/// One cleanup an abrupt exit must run, in source order: a `try` region's
/// handler (with a `finally` body to inline and/or a `for-of` iterator to
/// close), or a pushed block scope to pop. Scopes and handlers interleave,
/// so unwinding processes them innermost-first.
enum UnwindCtx<'a> {
    Handler {
        finally: Option<&'a [Statement]>,
        close_iter: Option<Reg>,
    },
    Scope,
    Pending,
    Enumerator(Reg),
}

/// What a [`LoopCtx`] entry accepts: loops take both `break` and
/// `continue`, switches and labeled blocks take `break` only.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum CtxKind {
    #[default]
    Loop,
    Switch,
    LabelBlock,
}

struct Compiler<'a> {
    code: Vec<Instr>,
    constants: Vec<Constant>,
    strings: HashMap<String, u16>,
    numbers: HashMap<u64, u16>,
    cached_true: Option<u16>,
    cached_false: Option<u16>,
    cached_null: Option<u16>,
    cached_undefined: Option<u16>,
    slots: Vec<SlotInfo>,
    scopes: Vec<Scope>,
    loops: Vec<LoopCtx>,
    completions: Vec<CompletionRegs>,
    completion_presence: HashMap<Reg, Reg>,
    /// Labels applied to the next compiled loop, pushed while compiling
    /// `label: <loop>` (labels nest, so a loop may carry several) and
    /// consumed by the loop's context.
    pending_labels: Vec<String>,
    /// Protected regions enclosing the current position: `try/finally`
    /// bodies and `for-of` iterators a break/continue must unwind.
    unwind: Vec<UnwindCtx<'a>>,
    deferred: Vec<Deferred<'a>>,
    next_reg: u16,
    max_reg: u16,
    /// Names bound to slots in enclosing *blocks*: a free variable landing
    /// here is a capture the environment chain cannot serve (block slots
    /// have no frame of their own) and declines the whole unit. Names bound
    /// at an enclosing *function* level resolve through the chain instead,
    /// because the defining function boxes them into its frame environment.
    outer_block: HashSet<String>,
    /// This function's own function-level bindings (params, hoisted vars,
    /// lexicals, functions) that a nested callable may observe. Boxed into
    /// the frame environment at call time; accesses compile to the global
    /// instruction family. Computed by [`find_captured`] before hoisting.
    captured: HashSet<String>,
    top_level: bool,
    this_mode: ThisMode,
    is_arrow: bool,
    /// A nested arrow (or this arrow's own body) reads `arguments` through
    /// the chain: the defining non-arrow function seeds it. Fed by direct
    /// references while the body compiles and by deferred arrow outcomes.
    captures_arguments: bool,
    annex_updates: HashMap<usize, (String, Binding)>,
}

impl<'a> Compiler<'a> {
    fn top_level() -> Self {
        Self {
            code: Vec::new(),
            constants: Vec::new(),
            strings: HashMap::new(),
            numbers: HashMap::new(),
            cached_true: None,
            cached_false: None,
            cached_null: None,
            cached_undefined: None,
            slots: Vec::new(),
            scopes: vec![Scope {
                bindings: HashMap::new(),
                global: true,
                boxed: HashSet::new(),
                runtime: false,
            }],
            loops: Vec::new(),
            completions: Vec::new(),
            completion_presence: HashMap::new(),
            pending_labels: Vec::new(),
            unwind: Vec::new(),
            deferred: Vec::new(),
            next_reg: 0,
            max_reg: 0,
            outer_block: HashSet::new(),
            captured: HashSet::new(),
            top_level: true,
            this_mode: ThisMode::Global,
            is_arrow: false,
            captures_arguments: false,
            annex_updates: HashMap::new(),
        }
    }

    fn for_function(outer_block: HashSet<String>, this_mode: ThisMode, is_arrow: bool) -> Self {
        Self {
            code: Vec::new(),
            constants: Vec::new(),
            strings: HashMap::new(),
            numbers: HashMap::new(),
            cached_true: None,
            cached_false: None,
            cached_null: None,
            cached_undefined: None,
            slots: Vec::new(),
            scopes: vec![Scope {
                bindings: HashMap::new(),
                global: false,
                boxed: HashSet::new(),
                runtime: false,
            }],
            loops: Vec::new(),
            completions: Vec::new(),
            completion_presence: HashMap::new(),
            pending_labels: Vec::new(),
            unwind: Vec::new(),
            deferred: Vec::new(),
            next_reg: 0,
            max_reg: 0,
            outer_block,
            captured: HashSet::new(),
            top_level: false,
            this_mode,
            is_arrow,
            captures_arguments: false,
            annex_updates: HashMap::new(),
        }
    }

    // -- allocation --------------------------------------------------------

    fn alloc_reg(&mut self) -> Result<Reg, Decline> {
        let reg = self.next_reg;
        self.next_reg = self
            .next_reg
            .checked_add(1)
            .ok_or(Decline::Func("too many registers"))?;
        self.max_reg = self.max_reg.max(self.next_reg);
        Ok(reg)
    }

    /// Reserve `count` consecutive registers (call arguments, array items).
    fn alloc_regs(&mut self, count: usize) -> Result<Reg, Decline> {
        let start = self.next_reg;
        let count = u16::try_from(count).map_err(|_| Decline::Func("too many registers"))?;
        self.next_reg = self
            .next_reg
            .checked_add(count)
            .ok_or(Decline::Func("too many registers"))?;
        self.max_reg = self.max_reg.max(self.next_reg);
        Ok(start)
    }

    /// Recycle expression temporaries above the checkpoint. Sound because
    /// values that outlive a statement (completion registers, join and loop
    /// registers, argument ranges) are always allocated below it and copied
    /// before the restore; see the emission sites.
    fn checkpoint(&self) -> u16 {
        self.next_reg
    }

    fn restore(&mut self, checkpoint: u16) {
        self.next_reg = checkpoint;
        self.completion_presence.retain(|reg, _| *reg < checkpoint);
    }

    fn alloc_slot(&mut self, name: &str, kind: SlotKind) -> Result<Slot, Decline> {
        let slot = u16::try_from(self.slots.len()).map_err(|_| Decline::Func("too many locals"))?;
        // Only function-root slots box: a block slot sharing a captured
        // name shadows it and stays direct. Single-scope allocation means
        // the root scope — blocks always push first.
        let captured = self.scopes.len() == 1 && self.captured.contains(name);
        self.slots.push(SlotInfo {
            name: name.to_string(),
            kind,
            captured,
        });
        Ok(slot)
    }

    fn push_const(&mut self, constant: Constant) -> Result<u16, Decline> {
        let index =
            u16::try_from(self.constants.len()).map_err(|_| Decline::Func("pool exhausted"))?;
        self.constants.push(constant);
        Ok(index)
    }

    fn intern_string(&mut self, value: &str) -> Result<u16, Decline> {
        if let Some(index) = self.strings.get(value) {
            return Ok(*index);
        }
        let index = self.push_const(Constant::Name(value.to_string()))?;
        self.strings.insert(value.to_string(), index);
        Ok(index)
    }

    fn intern_number(&mut self, value: f64) -> Result<u16, Decline> {
        if let Some(index) = self.numbers.get(&value.to_bits()) {
            return Ok(*index);
        }
        let index = self.push_const(Constant::Number(value))?;
        self.numbers.insert(value.to_bits(), index);
        Ok(index)
    }

    fn const_true(&mut self) -> Result<u16, Decline> {
        if let Some(index) = self.cached_true {
            return Ok(index);
        }
        let index = self.push_const(Constant::Bool(true))?;
        self.cached_true = Some(index);
        Ok(index)
    }

    fn const_false(&mut self) -> Result<u16, Decline> {
        if let Some(index) = self.cached_false {
            return Ok(index);
        }
        let index = self.push_const(Constant::Bool(false))?;
        self.cached_false = Some(index);
        Ok(index)
    }

    fn const_null(&mut self) -> Result<u16, Decline> {
        if let Some(index) = self.cached_null {
            return Ok(index);
        }
        let index = self.push_const(Constant::Null)?;
        self.cached_null = Some(index);
        Ok(index)
    }

    fn const_undefined(&mut self) -> Result<u16, Decline> {
        if let Some(index) = self.cached_undefined {
            return Ok(index);
        }
        let index = self.push_const(Constant::Undefined)?;
        self.cached_undefined = Some(index);
        Ok(index)
    }

    fn load_const(&mut self, index: u16) -> Result<Reg, Decline> {
        let dst = self.alloc_reg()?;
        self.code.push(Instr::LoadConst { dst, cst: index });
        Ok(dst)
    }

    fn load_undefined(&mut self) -> Result<Reg, Decline> {
        let index = self.const_undefined()?;
        self.load_const(index)
    }

    fn emit(&mut self, instr: Instr) {
        self.code.push(instr);
    }

    /// Emit a jump with a dummy target; the caller patches it. The dummy is
    /// always overwritten before compilation finishes, so it never executes.
    fn emit_jump(&mut self, make: impl FnOnce(Target) -> Instr) -> usize {
        let addr = self.code.len();
        self.code.push(make(0));
        addr
    }

    fn patch_jump(&mut self, addr: usize, target: usize) -> Result<(), Decline> {
        let target = u32::try_from(target).map_err(|_| Decline::Func("code too large"))?;
        match self.code.get_mut(addr) {
            Some(Instr::Jump { target: slot })
            | Some(Instr::JumpIfTrue { target: slot, .. })
            | Some(Instr::JumpIfFalse { target: slot, .. })
            | Some(Instr::JumpIfNullish { target: slot, .. })
            | Some(Instr::JumpIfNotNullish { target: slot, .. })
            | Some(Instr::PushCatch { target: slot, .. })
            | Some(Instr::PushFinally { target: slot, .. }) => {
                *slot = target;
                Ok(())
            }
            _ => Err(Decline::Func("bad jump patch")),
        }
    }

    fn here(&self) -> usize {
        self.code.len()
    }

    // -- scopes and resolution ---------------------------------------------

    /// Push a block scope, boxing `names` into a runtime scope: captured
    /// block bindings live in the pushed environment instead of slots.
    /// Scopes with no boxed names emit nothing and unwind nothing.
    fn push_scope(&mut self, boxed: HashSet<String>) {
        self.push_scope_with_runtime(boxed, false);
    }

    fn push_scope_with_runtime(&mut self, boxed: HashSet<String>, force_runtime: bool) {
        let tracked = force_runtime || !boxed.is_empty();
        self.scopes.push(Scope {
            bindings: HashMap::new(),
            global: false,
            boxed,
            runtime: tracked,
        });
        if tracked {
            self.emit(Instr::PushScope);
            self.unwind.push(UnwindCtx::Scope);
        }
    }

    /// Pop a block scope, emitting the runtime pop when it boxed names.
    /// Nested cleanups balance before this runs, so the unwind top is
    /// this scope's own entry — anything else is a compiler bug.
    fn pop_scope(&mut self) -> Result<(), Decline> {
        let scope = self
            .scopes
            .pop()
            .ok_or(Decline::Func("scope stack underflow"))?;
        if !scope.runtime {
            return Ok(());
        }
        self.emit(Instr::PopScope);
        match self.unwind.pop() {
            Some(UnwindCtx::Scope) => Ok(()),
            _ => Err(Decline::Func("unwind stack underflow")),
        }
    }

    /// Whether `name` is boxed in the innermost scope: declaration sites
    /// branch on this instead of allocating a slot.
    fn is_boxed_here(&self, name: &str) -> bool {
        self.scopes
            .last()
            .is_some_and(|scope| !scope.global && scope.boxed.contains(name))
    }

    /// Bind `name` in the innermost scope, allocating a fresh slot. Same-name
    /// duplicates within one scope share the slot the first declaration
    /// allocated, mirroring `declare`'s replace-in-place; the caller passes
    /// the final kind after hoist-order merging.
    fn declare_slot(&mut self, name: &str, kind: SlotKind) -> Result<Slot, Decline> {
        if let Some(slot) = self
            .scopes
            .last()
            .and_then(|scope| scope.bindings.get(name))
        {
            let slot = *slot;
            self.slots[slot as usize].kind = kind;
            return Ok(slot);
        }
        let slot = self.alloc_slot(name, kind)?;
        if let Some(scope) = self.scopes.last_mut() {
            scope.bindings.insert(name.to_string(), slot);
        }
        Ok(slot)
    }

    /// Resolve a name: innermost slot binding wins; the top-level outer
    /// scope and anything undeclared resolve globally (the runtime lookup
    /// reports `ReferenceError` for true misses, exactly like the AST).
    /// Captured root slots also resolve globally: the defining function
    /// boxes them into its frame environment, where the closure chain
    /// serves nested readers. Boxed block bindings resolve globally too:
    /// they live in the pushed runtime scope. Shadowing block slots stay
    /// direct, and names bound to slots in enclosing blocks decline the
    /// whole unit.
    fn resolve(&self, name: &str) -> Result<Binding, Decline> {
        for (index, scope) in self.scopes.iter().enumerate().rev() {
            if !scope.global && scope.boxed.contains(name) {
                return Ok(Binding::Global);
            }
            if let Some(slot) = scope.bindings.get(name) {
                if index == 0 && self.captured.contains(name) {
                    return Ok(Binding::Global);
                }
                return Ok(Binding::Slot(*slot));
            }
            if scope.global {
                return Ok(Binding::Global);
            }
        }
        if self.outer_block.contains(name) {
            return Err(Decline::Unit("block-scope capture needs Phase G"));
        }
        Ok(Binding::Global)
    }

    /// Live slot-bound names in enclosing *blocks*, for the capture
    /// snapshot of a nested function defined here. The function-root scope
    /// is excluded: its captured names resolve through the environment
    /// chain instead of declining.
    fn live_block_names(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        let blocks = if self.top_level { 0 } else { 1 };
        for scope in self.scopes.iter().skip(blocks) {
            if !scope.global {
                names.extend(scope.bindings.keys().cloned());
            }
        }
        names
    }
}

#[derive(Clone, Copy)]
enum CallArgs {
    Range { start: Reg, argc: u16 },
    Spread { tmpl: u16 },
}

#[derive(Clone, Copy)]
enum Binding {
    Slot(Slot),
    Global,
}

/// Lexical declarations directly in one block (nested blocks and functions
/// have their own scopes). Destructuring declarators contribute no names:
/// compiling the declarator itself declines, discarding the whole function.
fn block_lexicals(stmts: &[Statement], out: &mut Vec<(String, SlotKind)>) {
    for stmt in stmts {
        match stmt {
            Statement::VarDecl {
                kind: VarKind::Let,
                destructuring: None,
                name,
                ..
            } => {
                out.push((name.clone(), SlotKind::Let));
            }
            Statement::VarDecl {
                kind: VarKind::Const,
                destructuring: None,
                name,
                ..
            } => {
                out.push((name.clone(), SlotKind::Const));
            }
            // A pattern declaration binds every name in the pattern (the
            // declarator's own `name` is empty); `var` patterns hoist
            // through `collect_var_names` instead.
            Statement::VarDecl {
                kind: VarKind::Let,
                destructuring: Some(pattern),
                ..
            } => {
                for bound in pattern_names(pattern) {
                    out.push((bound, SlotKind::Let));
                }
            }
            Statement::VarDecl {
                kind: VarKind::Const,
                destructuring: Some(pattern),
                ..
            } => {
                for bound in pattern_names(pattern) {
                    out.push((bound, SlotKind::Const));
                }
            }
            // Class declarations bind like `let`: block-scoped, mutable,
            // dead until initialized.
            Statement::ClassDecl { name, .. } => {
                out.push((name.clone(), SlotKind::Let));
            }
            Statement::Declarations(inner) => block_lexicals(inner, out),
            _ => {}
        }
    }
}

/// Names one block binds that nested closures capture: lexicals, class
/// declarations, block functions, and directly-nested `for-in`/`of` head
/// names. Each is boxed into the pushed runtime scope instead of a slot,
/// so closures observe the entry's own cells through the chain.
fn boxed_block_names(stmts: &[Statement]) -> HashSet<String> {
    let mut candidates = Vec::new();
    let mut lexicals = Vec::new();
    block_lexicals(stmts, &mut lexicals);
    candidates.extend(lexicals.into_iter().map(|(name, _)| name));
    let mut fns = Vec::new();
    block_fn_decls(stmts, &mut fns);
    candidates.extend(fns.into_iter().map(|decl| decl.name.to_string()));
    let mut heads = Vec::new();
    block_loop_heads(stmts, &mut heads);
    candidates.extend(heads);
    candidates
        .into_iter()
        .filter(|name| statements_capture_identifier(stmts, name))
        .collect()
}

/// Captured `let`/`const` names of a C-style `for` head: candidates come
/// from the declarators (patterns included); a name boxes when a nested
/// closure anywhere in the head expressions or body captures it. `var`
/// heads stay function-scoped and never box.
fn boxed_for_head_names(
    init: Option<&ForInit>,
    test: Option<&Expr>,
    update: Option<&Expr>,
    body: &[Statement],
) -> HashSet<String> {
    fn exprs_capture(exprs: &[&Expr], body: &[Statement], name: &str) -> bool {
        exprs
            .iter()
            .any(|expr| expr_captures_identifier(expr, name))
            || statements_capture_identifier(body, name)
    }
    let mut candidates = Vec::new();
    let mut inits: Vec<&Expr> = Vec::new();
    match init {
        Some(ForInit::Var {
            kind: VarKind::Let | VarKind::Const,
            decls,
        }) => {
            for (name, init) in decls.iter() {
                candidates.push(name.clone());
                inits.extend(init.as_ref());
            }
        }
        Some(ForInit::Pattern {
            kind: VarKind::Let | VarKind::Const,
            pattern,
            init,
            trailing,
        }) => {
            candidates.extend(pattern_names(pattern));
            inits.push(init);
            for (name, init) in trailing.iter() {
                candidates.push(name.clone());
                inits.extend(init.as_ref());
            }
        }
        _ => {}
    }
    let mut exprs = inits;
    exprs.extend(test);
    exprs.extend(update);
    candidates
        .into_iter()
        .filter(|name| exprs_capture(&exprs, body, name))
        .collect()
}

/// `for-in`/`for-of` head names bound directly in one block: the head
/// assigns in the enclosing scope, so a function-level head is visible to
/// (and boxable for) nested closures exactly like a `var`.
fn block_loop_heads(stmts: &[Statement], out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Statement::ForIn { binding, .. } | Statement::ForOf { binding, .. } => {
                out.extend(binding.declared_names())
            }
            Statement::Declarations(inner) => block_loop_heads(inner, out),
            _ => {}
        }
    }
}

/// Checked address-to-target cast: code bigger than 4B instructions declines.
fn addr_target(addr: usize) -> Result<Target, Decline> {
    u32::try_from(addr).map_err(|_| Decline::Func("code too large"))
}

/// A function declaration directly in one block, borrowed for compilation.
struct FnDeclRef<'a> {
    source_text: Option<crate::parser::SourceTextRef>,
    name: &'a str,
    params: &'a [String],
    body: &'a [Statement],
    is_async: bool,
    is_generator: bool,
}

fn block_fn_decls<'a>(stmts: &'a [Statement], out: &mut Vec<FnDeclRef<'a>>) {
    for stmt in stmts {
        match stmt {
            Statement::FnDecl {
                name,
                params,
                body,
                is_async,
                is_generator,
                ..
            } => {
                out.push(FnDeclRef {
                    source_text: stmt.source_text().cloned(),
                    name,
                    params,
                    body,
                    is_async: *is_async,
                    is_generator: *is_generator,
                });
            }
            Statement::Declarations(inner) => block_fn_decls(inner, out),
            _ => {}
        }
    }
}

impl<'a> Compiler<'a> {
    // -- hoisting --------------------------------------------------------

    /// Bind a slot only when the name is absent (hoisted functions keep the
    /// existing kind, mirroring `set_binding`).
    fn declare_slot_if_absent(&mut self, name: &str, kind: SlotKind) -> Result<Slot, Decline> {
        if let Some(slot) = self
            .scopes
            .last()
            .and_then(|scope| scope.bindings.get(name))
        {
            return Ok(*slot);
        }
        self.declare_slot(name, kind)
    }

    /// Top-level hoisting in AST order: conditional `var` defines (a
    /// previous `eval` may own the name), lexical declares, eager functions.
    fn hoist_top(&mut self, stmts: &'a [Statement]) -> Result<(), Decline> {
        let mut vars = Vec::new();
        crate::parser::collect_var_declaration_names(stmts, &mut vars);
        let mut direct = Vec::new();
        block_fn_decls(stmts, &mut direct);
        vars.extend(direct.iter().map(|d| d.name.to_string()));
        for (statement, name) in crate::interpreter::annex::eligible(stmts, &[], false) {
            self.annex_updates.insert(
                statement as *const Statement as usize,
                (name.clone(), Binding::Global),
            );
            let name = self.intern_string(&name)?;
            self.emit(Instr::AnnexBInit { name });
        }
        let lexical_names = crate::parser::top_lexical_names(stmts);
        let mut seen = HashSet::new();
        for name in vars {
            if lexical_names.contains(&name) {
                continue;
            }
            if seen.insert(name.clone()) {
                let index = self.intern_string(&name)?;
                self.emit(Instr::HoistVarGlobal { name: index });
            }
        }
        let mut lexicals = Vec::new();
        block_lexicals(stmts, &mut lexicals);
        let mut seen = HashSet::new();
        for (name, kind) in lexicals {
            if seen.insert(name.clone()) {
                let index = self.intern_string(&name)?;
                let undef = self.load_undefined()?;
                self.emit(Instr::DefineGlobal {
                    name: index,
                    src: undef,
                    kind,
                    initialized: false,
                });
            }
        }
        let mut fns = Vec::new();
        block_fn_decls(stmts, &mut fns);
        for decl in fns {
            let value = self.defer_function(FuncDef {
                source_text: decl.source_text.clone(),
                named_expression: false,
                name: Some(crate::parser::declaration_display_name(decl.name).to_owned()),
                params: SharedSlice::Borrowed(decl.params),
                body: FuncBody::Stmts(SharedSlice::Borrowed(decl.body)),
                is_arrow: false,
                is_async: decl.is_async,
                is_generator: decl.is_generator,
                is_constructor: !decl.is_async && !decl.is_generator,
            })?;
            let index = self.intern_string(decl.name)?;
            self.emit(Instr::InitGlobal {
                name: index,
                src: value,
                function: true,
            });
        }
        Ok(())
    }

    /// Function-level hoisting. Slots pre-initialize from their merged kind
    /// (`var` starts defined, lexicals dead), which is exactly what
    /// `hoist_vars` + `hoist_lexical` produce on a fresh frame — so only the
    /// eager function instantiations need emission.
    fn hoist_function(
        &mut self,
        params: &'a [String],
        body: &'a [Statement],
    ) -> Result<(), Decline> {
        for param in params {
            self.declare_slot(param, SlotKind::Var)?;
        }
        let mut vars = Vec::new();
        crate::parser::collect_var_declaration_names(body, &mut vars);
        let mut direct = Vec::new();
        block_fn_decls(body, &mut direct);
        vars.extend(direct.iter().map(|d| d.name.to_string()));
        let mut seen = HashSet::new();
        for name in vars {
            if seen.insert(name.clone()) {
                // A `var` sharing a parameter's slot keeps the argument:
                // bind only when absent.
                self.declare_slot_if_absent(&name, SlotKind::Var)?;
            }
        }
        for (statement, name) in crate::interpreter::annex::eligible(body, params, !self.is_arrow) {
            let binding = self.resolve(&name)?;
            self.annex_updates.insert(
                statement as *const Statement as usize,
                (name.clone(), binding),
            );
            let name = self.intern_string(&name)?;
            self.emit(Instr::AnnexBInit { name });
        }
        let mut lexicals = Vec::new();
        block_lexicals(body, &mut lexicals);
        for (name, kind) in lexicals {
            // The lexical pass replaces whatever the `var` pass declared.
            self.declare_slot(&name, kind)?;
        }
        let mut fns = Vec::new();
        block_fn_decls(body, &mut fns);
        for decl in fns {
            let slot = self.declare_slot_if_absent(decl.name, SlotKind::Var)?;
            let value = self.defer_function(FuncDef {
                source_text: decl.source_text.clone(),
                named_expression: false,
                name: Some(crate::parser::declaration_display_name(decl.name).to_owned()),
                params: SharedSlice::Borrowed(decl.params),
                body: FuncBody::Stmts(SharedSlice::Borrowed(decl.body)),
                is_arrow: false,
                is_async: decl.is_async,
                is_generator: decl.is_generator,
                is_constructor: !decl.is_async && !decl.is_generator,
            })?;
            // Captured names live in the frame environment; the slot stays
            // an untouched placeholder.
            if self.captured.contains(decl.name) {
                let index = self.intern_string(decl.name)?;
                self.emit(Instr::InitGlobal {
                    name: index,
                    src: value,
                    function: true,
                });
            } else {
                self.emit(Instr::InitLocal { slot, src: value });
            }
        }
        Ok(())
    }

    /// Block-level hoisting, emitted at every block entry (loop bodies
    /// re-run it per iteration, restoring dead zones like the evaluator's
    /// per-iteration scopes do).
    fn hoist_block(&mut self, stmts: &'a [Statement]) -> Result<(), Decline> {
        let mut lexicals = Vec::new();
        block_lexicals(stmts, &mut lexicals);
        let mut seen = HashSet::new();
        for (name, kind) in lexicals {
            if seen.insert(name.clone()) {
                if self.is_boxed_here(&name) {
                    let index = self.intern_string(&name)?;
                    let undef = self.load_undefined()?;
                    self.emit(Instr::DefineGlobal {
                        name: index,
                        src: undef,
                        kind,
                        initialized: false,
                    });
                    continue;
                }
                let slot = self.declare_slot(&name, kind)?;
                self.emit(Instr::DeclareLocal {
                    slot,
                    kind,
                    initialized: false,
                });
            }
        }
        let mut fns = Vec::new();
        block_fn_decls(stmts, &mut fns);
        for decl in fns {
            let value = self.defer_function(FuncDef {
                source_text: decl.source_text.clone(),
                named_expression: false,
                name: Some(crate::parser::declaration_display_name(decl.name).to_owned()),
                params: SharedSlice::Borrowed(decl.params),
                body: FuncBody::Stmts(SharedSlice::Borrowed(decl.body)),
                is_arrow: false,
                is_async: decl.is_async,
                is_generator: decl.is_generator,
                is_constructor: !decl.is_async && !decl.is_generator,
            })?;
            if self.is_boxed_here(decl.name) {
                let index = self.intern_string(decl.name)?;
                self.emit(Instr::InitGlobal {
                    name: index,
                    src: value,
                    function: true,
                });
                continue;
            }
            let slot = self.declare_slot_if_absent(decl.name, SlotKind::Var)?;
            self.emit(Instr::InitLocal { slot, src: value });
        }
        Ok(())
    }

    // -- units -------------------------------------------------------------

    fn build_function(
        &mut self,
        name: Option<String>,
        parameter_count: usize,
        is_arrow: bool,
        is_constructor: bool,
    ) -> Result<BytecodeFunction, Decline> {
        let parameter_count =
            u16::try_from(parameter_count).map_err(|_| Decline::Func("too many parameters"))?;
        let local_count =
            u16::try_from(self.slots.len()).map_err(|_| Decline::Func("too many locals"))?;
        let mut code = std::mem::take(&mut self.code);
        let mut site_count = 0u32;
        for instr in &mut code {
            if let Instr::GetProp { cache, .. } | Instr::SetProp { cache, .. } = instr {
                *cache = site_count;
                site_count = site_count
                    .checked_add(1)
                    .ok_or(Decline::Func("too many property sites"))?;
            }
        }
        let caches = vec![crate::shape::PropCache::empty(); site_count as usize].into_boxed_slice();
        let needs_frame_environment = self.captures_arguments
            || self.slots.iter().any(|s| s.captured)
            || self.constants.iter().any(|c| {
                matches!(
                    c,
                    Constant::Function(_) | Constant::AstFunction(_) | Constant::ClassTemplate(_)
                )
            });
        Ok(BytecodeFunction {
            source_text: None,
            named_expression: false,
            strict: false,
            name,
            code,
            constants: std::mem::take(&mut self.constants),
            register_count: self.max_reg,
            local_count,
            parameter_count,
            upvalue_count: 0,
            slots: std::mem::take(&mut self.slots),
            is_arrow,
            is_constructor,
            captures_arguments: self.captures_arguments,
            needs_frame_environment,
            caches,
            tiers: crate::jit::TierCounters::default(),
        })
    }

    /// Record a nested function for compilation after this unit's own body
    /// succeeds, emitting a placeholder the post-pass overwrites.
    fn defer_function(&mut self, def: FuncDef<'a>) -> Result<Reg, Decline> {
        let dst = self.alloc_reg()?;
        let addr = self.here();
        self.emit(Instr::MakeFunction {
            dst,
            func: u16::MAX,
        });
        self.deferred.push(Deferred {
            patch: PatchTarget::MakeFunction { addr },
            def,
            snapshot: self.live_block_names(),
        });
        Ok(dst)
    }

    /// Compile the deferred nested functions and patch their placeholders.
    /// Runs only when the unit's own body compiled: an unsupported unit
    /// never wastes work on, or fails for, bodies it discards.
    fn finish_functions(&mut self) -> Result<(), Decline> {
        let deferred = std::mem::take(&mut self.deferred);
        for item in deferred {
            let mut outer = self.outer_block.clone();
            outer.extend(item.snapshot);
            let is_arrow = item.def.is_arrow;
            let outcome = compile_function(item.def, outer)?;
            // An arrow child's `arguments` need passes outward to this
            // scope's definer; non-arrow children bind (or decline) their
            // own. AST arrows read the defining frame the same way.
            let child_captures = match &outcome {
                FuncOutcome::Bytecode(code) => code.captures_arguments,
                FuncOutcome::Ast(ast) => ast.uses_arguments,
            };
            if is_arrow && child_captures {
                self.captures_arguments = true;
            }
            let (index, is_bytecode) = match outcome {
                FuncOutcome::Bytecode(code) => (self.push_const(Constant::Function(code))?, true),
                FuncOutcome::Ast(ast) => (self.push_const(Constant::AstFunction(ast))?, false),
            };
            match item.patch {
                PatchTarget::MakeFunction { addr } => match self.code.get_mut(addr) {
                    Some(slot @ Instr::MakeFunction { .. }) => {
                        let dst = match slot {
                            Instr::MakeFunction { dst, .. } => *dst,
                            _ => unreachable!("matched above"),
                        };
                        *slot = if is_bytecode {
                            Instr::MakeFunction { dst, func: index }
                        } else {
                            Instr::MakeAstFunction { dst, ast: index }
                        };
                    }
                    _ => return Err(Decline::Func("bad function patch")),
                },
                PatchTarget::ClassCtor { tmpl } => {
                    let Some(Constant::ClassTemplate(template)) =
                        self.constants.get_mut(tmpl as usize)
                    else {
                        return Err(Decline::Func("bad class patch"));
                    };
                    template.ctor_func = index;
                }
                PatchTarget::ClassMember {
                    tmpl,
                    index: member,
                } => {
                    let Some(Constant::ClassTemplate(template)) =
                        self.constants.get_mut(tmpl as usize)
                    else {
                        return Err(Decline::Func("bad class patch"));
                    };
                    let Some(entry) = template.members.get_mut(member) else {
                        return Err(Decline::Func("bad class patch"));
                    };
                    entry.func = Some(index);
                }
            }
        }
        Ok(())
    }

    fn compile_top(
        &mut self,
        stmts: &'a [Statement],
        linked: bool,
    ) -> Result<BytecodeFunction, Decline> {
        // Register zero is the program completion value. At this outer
        // boundary an empty result is already represented by undefined.
        let completion = self.new_completion(false)?;
        self.completions.push(completion);
        if !linked {
            self.hoist_top(stmts)?;
        }
        let checkpoint = self.checkpoint();
        self.restore(checkpoint);
        for stmt in stmts {
            let value = self.compile_stmt(stmt)?;
            self.copy_statement_completion(completion, stmt, value)?;
            self.restore(checkpoint);
        }
        self.completions.pop();
        self.finish_functions()?;
        self.build_function(None, 0, false, false)
    }

    fn compile_unit_function(&mut self, def: &'a FuncDef<'_>) -> Result<BytecodeFunction, Decline> {
        let FuncDef {
            name,
            params,
            body,
            is_arrow,
            is_constructor,
            ..
        } = def;
        // Box before hoisting: `resolve` and slot allocation both consult
        // this set while the body compiles.
        self.captured = find_captured(params, body);
        if !*is_arrow
            && matches!(body, FuncBody::Stmts(stmts) if crate::parser::stmts_need_arguments(stmts))
        {
            self.captured.extend(params.iter().cloned());
            self.captured.insert("arguments".into());
            self.captures_arguments = true;
        }
        let parameter_count = params.len();
        match body {
            FuncBody::Stmts(stmts) => {
                self.hoist_function(params, stmts)?;
                let checkpoint = self.checkpoint();
                for stmt in stmts.iter() {
                    let _ = self.compile_stmt(stmt)?;
                    self.restore(checkpoint);
                }
            }
            FuncBody::Expr(expr) => {
                for param in params.iter() {
                    self.declare_slot(param, SlotKind::Var)?;
                }
                let value = self.compile_expr(expr)?;
                self.emit(Instr::Return { src: value });
            }
        }
        self.finish_functions()?;
        let mut code =
            self.build_function(name.clone(), parameter_count, *is_arrow, *is_constructor)?;
        code.named_expression = def.named_expression;
        code.source_text = def.source_text.clone();
        code.strict = matches!(body, FuncBody::Stmts(stmts) if crate::parser::use_strict(stmts));
        Ok(code)
    }

    // -- blocks and statements ---------------------------------------------

    /// Completion values carry a presence bit: empty differs from undefined.
    fn new_completion(&mut self, empty: bool) -> Result<CompletionRegs, Decline> {
        let value = self.load_undefined()?;
        let present = if empty {
            let constant = self.const_false()?;
            let flag = self.load_const(constant)?;
            self.completion_presence.insert(value, flag);
            Some(flag)
        } else {
            None
        };
        Ok(CompletionRegs { value, present })
    }

    fn completion_regs(&self, value: Reg) -> CompletionRegs {
        CompletionRegs {
            value,
            present: self.completion_presence.get(&value).copied(),
        }
    }

    /// UpdateEmpty for a normal statement-list or case completion.
    fn copy_completion(
        &mut self,
        destination: CompletionRegs,
        source: CompletionRegs,
    ) -> Result<(), Decline> {
        let skip = source
            .present
            .map(|src| self.emit_jump(|target| Instr::JumpIfFalse { src, target }));
        self.emit(Instr::Mov {
            dst: destination.value,
            src: source.value,
        });
        if let Some(flag) = destination.present {
            let constant = self.const_true()?;
            self.emit(Instr::LoadConst {
                dst: flag,
                cst: constant,
            });
        }
        if let Some(skip) = skip {
            self.patch_jump(skip, self.here())?;
        }
        Ok(())
    }

    fn copy_statement_completion(
        &mut self,
        destination: CompletionRegs,
        statement: &Statement,
        value: Reg,
    ) -> Result<(), Decline> {
        if produces_completion_value(statement) {
            self.copy_completion(destination, self.completion_regs(value))?;
        }
        Ok(())
    }

    /// Merge the enclosing statement lists into the break/continue target
    /// before jumping over the normal completion moves.
    fn record_abrupt_completion(&mut self, target: usize) -> Result<(), Decline> {
        let depth = self.loops[target].completion_depth;
        let frames = self.completions[depth..].to_vec();
        let merged = self.new_completion(true)?;
        for frame in &frames {
            self.copy_completion(merged, *frame)?;
        }
        let destination = frames[0];
        self.emit(Instr::Mov {
            dst: destination.value,
            src: merged.value,
        });
        if let Some(flag) = destination.present {
            self.emit(Instr::Mov {
                dst: flag,
                src: merged.present.expect("empty completion flag"),
            });
        }
        Ok(())
    }

    fn compile_block(&mut self, stmts: &'a [Statement]) -> Result<Reg, Decline> {
        let completion = self.new_completion(true)?;
        self.completions.push(completion);
        let checkpoint = self.checkpoint();
        for stmt in stmts {
            let value = self.compile_stmt(stmt)?;
            self.copy_statement_completion(completion, stmt, value)?;
            self.restore(checkpoint);
        }
        self.completions.pop();
        Ok(completion.value)
    }

    /// Compile statements as a block: a fresh scope with hoisting only
    /// when the block has direct lexicals — otherwise the statements run
    /// in the enclosing scope, exactly like the evaluator. Bindings nested
    /// closures capture box into a pushed runtime scope.
    fn compile_scoped_block(&mut self, stmts: &'a [Statement]) -> Result<Reg, Decline> {
        if !block_needs_lexical_scope(stmts) {
            return self.compile_block(stmts);
        }
        self.push_scope(boxed_block_names(stmts));
        self.hoist_block(stmts)?;
        let value = self.compile_block(stmts)?;
        self.pop_scope()?;
        Ok(value)
    }

    fn compile_stmt(&mut self, stmt: &'a Statement) -> Result<Reg, Decline> {
        match stmt {
            Statement::Expr(expr) => self.compile_expr(expr),
            Statement::VarDecl {
                kind,
                name,
                init,
                destructuring,
            } => self.compile_var_decl(
                kind.clone(),
                name,
                init.as_deref(),
                destructuring.as_deref(),
            ),
            // Function identities are initialized at scope entry.
            Statement::FnDecl { name, .. } => {
                if let Some((_, target)) = self
                    .annex_updates
                    .get(&(stmt as *const Statement as usize))
                    .cloned()
                {
                    let src = self.alloc_reg()?;
                    match self.resolve(name)? {
                        Binding::Slot(slot) => self.emit(Instr::LoadLocal { dst: src, slot }),
                        Binding::Global => {
                            let key = self.intern_string(name)?;
                            self.emit(Instr::LoadGlobal {
                                dst: src,
                                name: key,
                            });
                        }
                    }
                    let name = self.intern_string(name)?;
                    self.emit(Instr::AnnexBUpdate {
                        name,
                        src,
                        slot: match target {
                            Binding::Slot(slot) => Some(slot),
                            Binding::Global => None,
                        },
                    });
                }
                self.load_undefined()
            }
            Statement::ParameterInitialization { .. } => {
                Err(Decline::Func("separate parameter environment"))
            }
            Statement::ClassInitialization { .. } => {
                Err(Decline::Func("constructor initialization environment"))
            }
            Statement::Declarations(inner) => {
                for stmt in inner {
                    let _ = self.compile_stmt(stmt)?;
                }
                self.load_undefined()
            }
            Statement::Block(stmts) => self.compile_scoped_block(stmts),
            Statement::If { test, then, else_ } => self.compile_if(test, then, else_.as_deref()),
            Statement::ResourceForOf { .. } | Statement::ResourceDeclaration { .. } => {
                Err(Decline::Func("resource disposal execution"))
            }
            Statement::With { .. } => Err(Decline::Func("with object environment")),
            Statement::While { test, body } => self.compile_while(test, body),
            Statement::DoWhile { test, body } => self.compile_do_while(test, body),
            Statement::For {
                init,
                test,
                update,
                body,
            } => self.compile_for(init.as_deref(), test.as_deref(), update.as_deref(), body),
            Statement::Break => {
                let target = self
                    .loops
                    .iter()
                    .rposition(|ctx| ctx.kind != CtxKind::LabelBlock)
                    .ok_or(Decline::Func("break outside loop"))?;
                let depth = self.loops[target].cleanup_depth;
                self.record_abrupt_completion(target)?;
                self.duplicate_unwind(depth)?;
                let address = self.emit_jump(|target| Instr::Jump { target });
                self.loops[target].breaks.push(address);
                self.load_undefined()
            }
            Statement::Continue => {
                let target = self
                    .loops
                    .iter()
                    .rposition(|ctx| ctx.kind == CtxKind::Loop)
                    .ok_or(Decline::Func("continue outside loop"))?;
                let depth = self.loops[target].cleanup_depth;
                self.record_abrupt_completion(target)?;
                self.duplicate_unwind(depth)?;
                let address = self.emit_jump(|target| Instr::Jump { target });
                self.loops[target].continues.push(address);
                self.load_undefined()
            }
            Statement::Return(expr) => {
                match expr {
                    Some(value) => {
                        let src = self.compile_expr(value)?;
                        self.emit(Instr::Return { src });
                    }
                    None => self.emit(Instr::ReturnUndefined),
                }
                self.load_undefined()
            }
            Statement::Throw(expr) => {
                let src = self.compile_expr(expr)?;
                self.emit(Instr::Throw { src });
                self.load_undefined()
            }
            Statement::Empty => self.load_undefined(),
            Statement::Try {
                body,
                catch,
                finally,
            } => self.compile_try(body, catch, finally),
            Statement::Switch { disc, cases } => self.compile_switch(disc, cases),
            Statement::ClassDecl {
                source_text: _,
                name,
                superclass,
                body,
            } => {
                let value = self.compile_class(
                    name,
                    None,
                    superclass.as_deref(),
                    body,
                    stmt.source_text(),
                )?;
                match self.resolve(name)? {
                    Binding::Slot(slot) => self.emit(Instr::InitLocal { slot, src: value }),
                    Binding::Global => {
                        let index = self.intern_string(name)?;
                        self.emit(Instr::InitGlobal {
                            name: index,
                            src: value,
                            function: false,
                        });
                    }
                }
                self.load_undefined()
            }
            Statement::ForIn {
                binding:
                    ForBinding::Declaration {
                        pattern: Pattern::Ident(name),
                        initializer: None,
                        kind,
                    },
                obj,
                body,
            } => self.compile_for_in(name, kind, obj, body),
            Statement::ForIn { .. } => Err(Decline::Func("for-in binding form")),
            Statement::ForOf {
                binding:
                    ForBinding::Declaration {
                        pattern,
                        initializer: None,
                        kind,
                    },
                iter,
                body,
                is_await,
            } => match pattern {
                Pattern::Ident(name) => {
                    self.compile_for_of(name, None, kind, iter, body, *is_await)
                }
                _ => self.compile_for_of("", Some(pattern), kind, iter, body, *is_await),
            },
            Statement::ForOf { .. } => Err(Decline::Func("for-of assignment head")),
            Statement::Labeled { label, body } => self.compile_labeled(label, body),
            Statement::LabeledBreak(label) | Statement::LabeledContinue(label) => {
                let target = self
                    .loops
                    .iter()
                    .rposition(|ctx| ctx.labels.iter().any(|known| known == label))
                    .ok_or(Decline::Func("unresolved control-flow label"))?;
                let is_continue = matches!(stmt, Statement::LabeledContinue(_));
                if is_continue && self.loops[target].kind != CtxKind::Loop {
                    return Err(Decline::Func("unresolved continue label"));
                }
                let depth = self.loops[target].cleanup_depth;
                self.record_abrupt_completion(target)?;
                self.duplicate_unwind(depth)?;
                let address = self.emit_jump(|target| Instr::Jump { target });
                if is_continue {
                    self.loops[target].continues.push(address);
                } else {
                    self.loops[target].breaks.push(address);
                }
                self.load_undefined()
            }
            Statement::Import {
                module,
                phase,
                default,
                named,
                namespace,
                attributes,
                ..
            } => {
                if !attributes.is_empty() {
                    return Err(Decline::Func("import attributes"));
                }
                // Imports bind into the running scope, which a block-level
                // import would leak out of (the VM has no block frames).
                let bound = default
                    .iter()
                    .map(String::as_str)
                    .chain(named.iter().map(|(_, local)| local.as_str()))
                    .chain(namespace.iter().map(String::as_str));
                self.require_env_module_binding(bound)?;
                let tmpl = self.push_const(Constant::ImportTemplate(ImportTemplate {
                    module: module.clone(),
                    phase: *phase,
                    default: default.clone(),
                    named: named.clone(),
                    namespace: namespace.clone(),
                }))?;
                self.emit(Instr::Import { tmpl });
                self.load_undefined()
            }
            Statement::ExportDefault(expr) => {
                let src = self.compile_named_assignment_value(expr, "default")?;
                self.emit(Instr::ExportDefault { src });
                self.load_undefined()
            }
            Statement::ExportNamed {
                specifiers,
                source,
                attributes,
            } => {
                if !attributes.is_empty() {
                    return Err(Decline::Func("export attributes"));
                }
                // Local exports publish scope cells; slots are invisible to
                // them, so only environment-bound lists compile.
                if source.is_none() {
                    self.require_env_module_binding(
                        specifiers.iter().map(|(local, _)| local.as_str()),
                    )?;
                }
                let tmpl = self.push_const(Constant::ExportNamedTemplate(ExportNamedTemplate {
                    specifiers: specifiers.clone(),
                    source: source.clone(),
                }))?;
                self.emit(Instr::ExportNamed { tmpl });
                self.load_undefined()
            }
            Statement::ExportAll {
                source,
                alias,
                attributes,
            } => {
                if !attributes.is_empty() {
                    return Err(Decline::Func("export attributes"));
                }
                let tmpl = self.push_const(Constant::ExportAllTemplate(ExportAllTemplate {
                    source: source.clone(),
                    alias: alias.clone(),
                }))?;
                self.emit(Instr::ExportAll { tmpl });
                self.load_undefined()
            }
        }
    }
}

fn pattern_requires_evaluator(pattern: &Pattern) -> bool {
    match pattern {
        Pattern::Array(_) => true,
        Pattern::Object(properties) => properties
            .iter()
            .any(|(_, pattern)| pattern.as_ref().is_some_and(pattern_requires_evaluator)),
        Pattern::Rest(pattern) => pattern_requires_evaluator(pattern),
        Pattern::Default(pattern, expression) => {
            pattern_requires_evaluator(pattern)
                || matches!(
                    expression.unparenthesized(),
                    Expr::FnExpr { name: None, .. }
                        | Expr::ArrowFn { .. }
                        | Expr::ClassExpr { name: None, .. }
                )
        }
        Pattern::Elision | Pattern::Ident(_) | Pattern::Member { .. } => false,
    }
}

/// Whether a pattern binds fresh declaration names or assigns through
/// existing bindings. `is_var` selects store-vs-initialize for declarations.
#[derive(Clone, Copy)]
enum DestructureMode {
    Decl { is_var: bool },
}

/// A prepared `for-in`/`for-of` head write: a function/block slot, or a
/// top-level global name.
enum ForHead {
    Slot(Slot),
    Global(u16),
}

impl<'a> Compiler<'a> {
    // -- declarations ----------------------------------------------------

    /// Bind one pattern leaf: a declaration initializes (`var` stores,
    /// lexicals leave the dead zone), an assignment stores through the
    /// resolved binding — mirroring the plain declarator/assignment paths.
    fn compile_pattern_ident(
        &mut self,
        name: &str,
        src: Reg,
        mode: DestructureMode,
    ) -> Result<(), Decline> {
        match (mode, self.resolve(name)?) {
            (DestructureMode::Decl { is_var: true }, Binding::Slot(slot)) => {
                self.emit(Instr::StoreLocal { slot, src });
            }
            (DestructureMode::Decl { is_var: true }, Binding::Global) => {
                let index = self.intern_string(name)?;
                self.emit(Instr::StoreGlobal { name: index, src });
            }
            (DestructureMode::Decl { .. }, Binding::Slot(slot)) => {
                self.emit(Instr::InitLocal { slot, src });
            }
            (DestructureMode::Decl { .. }, Binding::Global) => {
                let index = self.intern_string(name)?;
                self.emit(Instr::InitGlobal {
                    name: index,
                    src,
                    function: false,
                });
            }
        }
        Ok(())
    }

    /// Expand a binding pattern over the value in `val`, following the
    /// evaluator's `destructure` case for case: array sources materialize
    /// (objects become `[]`, strings split per character), the first rest
    /// element ends positional binding, object sources reject nullish
    /// inputs and snapshot their keys before named reads, defaults apply
    /// on nullish values only.
    fn compile_destructure(
        &mut self,
        pat: &'a Pattern,
        val: Reg,
        mode: DestructureMode,
    ) -> Result<(), Decline> {
        if pattern_requires_evaluator(pat) {
            return Err(Decline::Func(
                "destructuring iterator or name inference semantics",
            ));
        }
        self.compile_pattern_view(PatternView::from_pattern(pat), val, mode)
    }

    fn compile_pattern_view(
        &mut self,
        pat: PatternView<'a>,
        val: Reg,
        mode: DestructureMode,
    ) -> Result<(), Decline> {
        match pat {
            PatternView::Ident(name) => self.compile_pattern_ident(name, val, mode),
            PatternView::Member {
                object,
                property,
                private,
            } => {
                if private {
                    return Err(Decline::Func("private destructuring writes"));
                }
                if matches!(object, Expr::Super) {
                    let (base, receiver, key) = self.compile_super_reference(property)?;
                    self.emit(Instr::SetPropertyWithReceiver {
                        base,
                        receiver,
                        key,
                        value: val,
                    });
                    return Ok(());
                }
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                self.emit(Instr::SetProp {
                    cache: 0,
                    obj,
                    key,
                    val,
                });
                Ok(())
            }
            PatternView::Array(elements) => {
                let arr = self.alloc_reg()?;
                self.emit(Instr::ToDestructArray { dst: arr, src: val });
                for (index, elem) in elements.iter().enumerate() {
                    if let PatternView::Rest(inner) = elem {
                        let from =
                            u16::try_from(index).map_err(|_| Decline::Func("code too large"))?;
                        let rest = self.alloc_reg()?;
                        self.emit(Instr::RestArray {
                            dst: rest,
                            src: arr,
                            from,
                        });
                        return self.compile_pattern_view((**inner).clone(), rest, mode);
                    }
                    let position = self.intern_number(index as f64)?;
                    let key = self.load_const(position)?;
                    let found = self.alloc_reg()?;
                    self.emit(Instr::GetProp {
                        private: false,
                        cache: 0,
                        dst: found,
                        obj: arr,
                        key,
                    });
                    self.compile_pattern_view(elem.clone(), found, mode)?;
                }
                Ok(())
            }
            PatternView::Object(props) => {
                let checked = self.alloc_reg()?;
                self.emit(Instr::CheckDestructObject {
                    dst: checked,
                    src: val,
                });
                let mut taken = Vec::new();
                for (key, sub) in props {
                    if let PatternKeyView::Name(name) = key
                        && name == "..."
                        && let Some(PatternView::Rest(target)) = sub
                    {
                        let template = taken
                            .iter()
                            .map(|reg| SpreadEntry {
                                elision: false,
                                spread: false,
                                prepared: false,
                                reg: *reg,
                            })
                            .collect();
                        let tmpl = self.push_const(Constant::SpreadTemplate(template))?;
                        let taken_array = self.alloc_reg()?;
                        self.emit(Instr::BuildArray {
                            dst: taken_array,
                            tmpl,
                        });
                        let rest = self.alloc_reg()?;
                        self.emit(Instr::RestObject {
                            dst: rest,
                            src: checked,
                            taken: taken_array,
                        });
                        self.compile_pattern_view(*target, rest, mode)?;
                        continue;
                    }
                    let key_reg = match key {
                        PatternKeyView::Name(name) => {
                            let index = self.intern_string(name)?;
                            self.load_const(index)?
                        }
                        PatternKeyView::Computed(expr) => {
                            let value = self.compile_expr(expr)?;
                            let key = self.alloc_reg()?;
                            self.emit(Instr::PropertyKey {
                                dst: key,
                                src: value,
                            });
                            key
                        }
                    };
                    taken.push(key_reg);
                    let found = self.alloc_reg()?;
                    self.emit(Instr::GetProp {
                        private: false,
                        cache: 0,
                        dst: found,
                        obj: checked,
                        key: key_reg,
                    });
                    match sub {
                        Some(next) => self.compile_pattern_view(next.clone(), found, mode)?,
                        None => match key {
                            PatternKeyView::Name(name) => {
                                self.compile_pattern_ident(name, found, mode)?;
                            }
                            PatternKeyView::Computed(_) => {
                                return Err(Decline::Func("invalid destructuring target"));
                            }
                        },
                    }
                }
                Ok(())
            }
            // A rest element only binds at the top of an array pattern; a
            // bare one anywhere else falls through, like the evaluator.
            PatternView::Elision | PatternView::Rest(_) => Ok(()),
            PatternView::Default(inner, default) => {
                let value = self.alloc_reg()?;
                self.emit(Instr::Mov {
                    dst: value,
                    src: val,
                });
                let undefined = self.load_undefined()?;
                let needs_default = self.alloc_reg()?;
                self.emit(Instr::Binary {
                    dst: needs_default,
                    op: BinOp::Seq,
                    lhs: val,
                    rhs: undefined,
                });
                let has = self.emit_jump(|target| Instr::JumpIfFalse {
                    src: needs_default,
                    target,
                });
                let fallback = self.compile_expr(default)?;
                self.emit(Instr::Mov {
                    dst: value,
                    src: fallback,
                });
                self.patch_jump(has, self.here())?;
                self.compile_pattern_view(*inner, value, mode)
            }
        }
    }

    /// Module statements that bind or publish scope cells compile only
    /// when every touched name lives in the environment: block slots have
    /// no cells to publish, and a slot-bound import would shadow invisibly.
    fn require_env_module_binding<'n>(
        &self,
        names: impl Iterator<Item = &'n str>,
    ) -> Result<(), Decline> {
        if self.scopes.len() != 1 {
            return Err(Decline::Func("scoped modules need Phase G"));
        }
        for name in names {
            if !matches!(self.resolve(name)?, Binding::Global) {
                return Err(Decline::Func("scoped modules need Phase G"));
            }
        }
        Ok(())
    }

    /// Compile a class declaration or expression to a `BuildClass`.
    /// The walk mirrors the evaluator's member order exactly: the
    /// superclass first, then each member's computed name and static
    /// initializer inline. Methods defer like nested functions; instance
    /// fields desugar into the constructor body, like the evaluator.
    fn compile_class(
        &mut self,
        name: &str,
        expr_name: Option<String>,
        superclass: Option<&'a Expr>,
        body: &'a [ClassMember],
        source_text: Option<&crate::parser::SourceTextRef>,
    ) -> Result<Reg, Decline> {
        // Heritage, computed names and method closures share one class scope.
        let mut boxed = HashSet::new();
        if !name.is_empty() {
            boxed.insert(name.to_owned());
        }
        self.push_scope_with_runtime(boxed, true);
        let name_constant = if name.is_empty() {
            None
        } else {
            Some(self.intern_string(name)?)
        };
        *self.code.last_mut().expect("class scope was emitted") = Instr::ClassScope {
            name: name_constant,
        };
        let snapshot = self.live_block_names();
        let superclass = superclass.map(|expr| self.compile_expr(expr)).transpose()?;
        let super_proto = if let Some(superclass) = superclass {
            let dst = self.alloc_reg()?;
            self.emit(Instr::ClassHeritage { dst, superclass });
            Some(dst)
        } else {
            None
        };
        let private_declarations = crate::parser::class_private_declarations(body);
        let names = self.push_const(Constant::StringList(
            private_declarations
                .iter()
                .map(|name| name.as_str().into())
                .collect(),
        ))?;
        self.emit(Instr::ClassPrivateEnvironment { names });

        let mut members = Vec::new();
        let mut blocks = Vec::new();
        let mut ctor_computed_keys = Vec::new();
        let mut deferred = Vec::new();
        // Instance field keys in field order: a static name, or the index
        // into `ctor_computed_keys` holding the evaluated key.
        let mut instance_fields: Vec<(FieldKey, Option<&'a Expr>)> = Vec::new();
        let mut private_fields = Vec::new();
        let mut ctor: Option<OwnedCtor<'a>> = None;

        for member in body {
            match member {
                ClassMember::Method {
                    source_text: _,
                    name: member_name,
                    is_static: st,
                    params,
                    body: method_body,
                    is_async,
                    is_generator,
                } => {
                    let template = self.compile_member_name(member_name)?;
                    self.check_static_method_key(&template, *st)?;
                    // Only a written-out `constructor` is the constructor,
                    // like the evaluator; a computed "constructor" stays a
                    // plain method.
                    let is_ctor =
                        !st && matches!(member_name, MemberName::Static(n) if n == "constructor");
                    if is_ctor {
                        ctor = Some(OwnedCtor {
                            params,
                            body: method_body,
                        });
                        continue;
                    }
                    let display = match &template {
                        ClassNameTemplate::Static(key) | ClassNameTemplate::Private(key) => {
                            Some(key.clone())
                        }
                        // The builder names computed members once the key
                        // value is known.
                        ClassNameTemplate::Computed(_) => None,
                    };
                    deferred.push((
                        PatchTarget::ClassMember {
                            tmpl: u16::MAX,
                            index: members.len(),
                        },
                        FuncDef {
                            source_text: member.source_text().cloned(),
                            named_expression: false,
                            name: display,
                            params: SharedSlice::Borrowed(params),
                            body: FuncBody::Stmts(SharedSlice::Borrowed(method_body)),
                            is_arrow: false,
                            is_async: *is_async,
                            is_generator: *is_generator,
                            is_constructor: false,
                        },
                    ));
                    members.push(ClassMemberTemplate {
                        kind: ClassMemberKind::Method,
                        is_static: *st,
                        name: template,
                        func: Some(u16::MAX),
                    });
                }
                ClassMember::Field {
                    name: field_name,
                    is_static: st,
                    init,
                } => {
                    let template = self.compile_member_name(field_name)?;
                    if *st {
                        blocks.push(super::constants::ClassStaticTemplate::Field {
                            name: template,
                            init: init.clone().unwrap_or(Expr::Undefined),
                        });
                    } else {
                        if let ClassNameTemplate::Private(name) = &template {
                            private_fields.push(name.clone());
                        }
                        let key = match template {
                            ClassNameTemplate::Private(key) => FieldKey::Private(key),
                            ClassNameTemplate::Static(key) => FieldKey::Static(key),
                            ClassNameTemplate::Computed(reg) => {
                                let index = ctor_computed_keys.len();
                                ctor_computed_keys.push(reg);
                                FieldKey::Computed(index)
                            }
                        };
                        instance_fields.push((key, init.as_ref()));
                    }
                }
                ClassMember::Getter {
                    source_text: _,
                    name: member_name,
                    is_static: st,
                    body: getter_body,
                } => {
                    let template = self.compile_member_name(member_name)?;
                    self.check_static_method_key(&template, *st)?;
                    let display = match &template {
                        ClassNameTemplate::Static(key) | ClassNameTemplate::Private(key) => {
                            Some(format!("get {key}"))
                        }
                        ClassNameTemplate::Computed(_) => None,
                    };
                    deferred.push((
                        PatchTarget::ClassMember {
                            tmpl: u16::MAX,
                            index: members.len(),
                        },
                        FuncDef {
                            source_text: member.source_text().cloned(),
                            named_expression: false,
                            name: display,
                            params: SharedSlice::Borrowed(&[]),
                            body: FuncBody::Stmts(SharedSlice::Borrowed(getter_body)),
                            is_arrow: false,
                            is_async: false,
                            is_generator: false,
                            is_constructor: false,
                        },
                    ));
                    members.push(ClassMemberTemplate {
                        kind: ClassMemberKind::Getter,
                        is_static: *st,
                        name: template,
                        func: Some(u16::MAX),
                    });
                }
                ClassMember::Setter {
                    source_text: _,
                    name: member_name,
                    param,
                    is_static: st,
                    body: setter_body,
                } => {
                    let template = self.compile_member_name(member_name)?;
                    self.check_static_method_key(&template, *st)?;
                    let display = match &template {
                        ClassNameTemplate::Static(key) | ClassNameTemplate::Private(key) => {
                            Some(format!("set {key}"))
                        }
                        ClassNameTemplate::Computed(_) => None,
                    };
                    deferred.push((
                        PatchTarget::ClassMember {
                            tmpl: u16::MAX,
                            index: members.len(),
                        },
                        FuncDef {
                            source_text: member.source_text().cloned(),
                            named_expression: false,
                            name: display,
                            params: SharedSlice::Borrowed(std::slice::from_ref(param)),
                            body: FuncBody::Stmts(SharedSlice::Borrowed(setter_body)),
                            is_arrow: false,
                            is_async: false,
                            is_generator: false,
                            is_constructor: false,
                        },
                    ));
                    members.push(ClassMemberTemplate {
                        kind: ClassMemberKind::Setter,
                        is_static: *st,
                        name: template,
                        func: Some(u16::MAX),
                    });
                }
                ClassMember::StaticBlock { body: block_body } => {
                    // Static blocks always run on the AST tier: the builder
                    // hands their bodies to assembly, like the evaluator.
                    // They read this scope's `arguments`, so a mention seeds
                    // it (over-approximating, like hoisting).
                    if crate::parser::stmts_need_arguments(block_body) {
                        self.captures_arguments = true;
                    }
                    let ast = build_ast_function(&FuncDef {
                        source_text: None,
                        named_expression: false,
                        name: None,
                        params: SharedSlice::Borrowed(&[]),
                        body: FuncBody::Stmts(SharedSlice::Borrowed(block_body)),
                        is_arrow: false,
                        is_async: false,
                        is_generator: false,
                        is_constructor: false,
                    });
                    blocks.push(super::constants::ClassStaticTemplate::Block(
                        self.push_const(Constant::AstFunction(ast))?,
                    ));
                }
            }
        }

        let (ctor_params, ctor_body) = Self::class_ctor_body(
            superclass.is_some(),
            ctor,
            &instance_fields,
            body.iter().any(|m| {
                matches!(
                    m,
                    ClassMember::Method {
                        name: MemberName::Private(_),
                        is_static: false,
                        ..
                    } | ClassMember::Getter {
                        name: MemberName::Private(_),
                        is_static: false,
                        ..
                    } | ClassMember::Setter {
                        name: MemberName::Private(_),
                        is_static: false,
                        ..
                    }
                )
            }),
        );
        let ctor_length = crate::parser::formal_parameter_length(&ctor_params, &ctor_body);
        deferred.push((
            PatchTarget::ClassCtor { tmpl: u16::MAX },
            FuncDef {
                source_text: None,
                named_expression: false,
                // The evaluator names the constructor after the class.
                name: Some(crate::parser::declaration_display_name(name).to_owned()),
                params: ctor_params,
                body: FuncBody::Stmts(ctor_body),
                is_arrow: false,
                is_async: false,
                is_generator: false,
                is_constructor: false,
            },
        ));

        let tmpl = self.push_const(Constant::ClassTemplate(ClassTemplate {
            source_text: source_text.cloned(),
            private_fields,
            private_declarations,
            name: crate::parser::declaration_display_name(name).to_owned(),
            expr_name,
            superclass,
            super_proto,
            ctor_func: u16::MAX,
            ctor_length,
            ctor_computed_keys,
            members,
            blocks,
        }))?;
        let dst = self.alloc_reg()?;
        self.emit(Instr::BuildClass { dst, tmpl });
        self.pop_scope()?;
        for (mut patch, def) in deferred {
            match &mut patch {
                PatchTarget::ClassCtor { tmpl: slot }
                | PatchTarget::ClassMember { tmpl: slot, .. } => *slot = tmpl,
                PatchTarget::MakeFunction { .. } => unreachable!("class defers class patches"),
            }
            self.deferred.push(Deferred {
                patch,
                def,
                snapshot: snapshot.clone(),
            });
        }
        Ok(dst)
    }

    fn check_static_method_key(
        &mut self,
        key: &ClassNameTemplate,
        is_static: bool,
    ) -> Result<(), Decline> {
        if !is_static {
            return Ok(());
        }
        let key = match key {
            ClassNameTemplate::Private(_) => return Ok(()),
            ClassNameTemplate::Computed(reg) => *reg,
            ClassNameTemplate::Static(name) => {
                if name != "prototype" {
                    return Ok(());
                }
                let index = self.intern_string(name)?;
                self.load_const(index)?
            }
        };
        self.emit(Instr::CheckClassStaticKey { key });
        Ok(())
    }

    /// A member name as written, or compiled and coerced when computed.
    /// Coercing now matches the evaluator, which keys members at class
    /// definition time rather than at first use.
    fn compile_member_name(&mut self, name: &'a MemberName) -> Result<ClassNameTemplate, Decline> {
        match name {
            MemberName::Private(key) => Ok(ClassNameTemplate::Private(key.clone())),
            MemberName::Static(key) => Ok(ClassNameTemplate::Static(key.clone())),
            MemberName::Computed(expr) => {
                let src = self.compile_expr(expr)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::PropertyKey { dst, src });
                Ok(ClassNameTemplate::Computed(dst))
            }
        }
    }

    /// The constructor's parameter list and body: the written one with
    /// instance fields desugared ahead of it, the implicit derived
    /// Implicit constructors forward arguments through shared constructor entry.
    /// Owned bodies promote to the compilation lifetime, like converted
    /// destructuring patterns.
    fn class_ctor_body(
        is_derived: bool,
        ctor: Option<OwnedCtor<'a>>,
        instance_fields: &[(FieldKey, Option<&'a Expr>)],
        has_private_instance_elements: bool,
    ) -> (SharedSlice<'a, String>, SharedSlice<'a, Statement>) {
        let implicit_derived = is_derived && ctor.is_none();
        let (params, body) = match ctor {
            Some(own) => (
                SharedSlice::Borrowed(own.params),
                SharedSlice::Borrowed(own.body),
            ),
            None if is_derived => (
                SharedSlice::Owned(Rc::from(vec!["...args".to_string()])),
                SharedSlice::Borrowed(&[]),
            ),
            None => (SharedSlice::Borrowed(&[]), SharedSlice::Borrowed(&[])),
        };
        let mut full = Vec::with_capacity(instance_fields.len() + body.len());
        for (key, init) in instance_fields {
            let (property, computed) = match key {
                FieldKey::Static(field) => (Expr::String(crate::JsString::from_key(field)), true),
                FieldKey::Private(field) => (Expr::String(crate::JsString::from_key(field)), false),
                // Evaluated at class definition time; the builder binds the
                // value into the constructor's scope under this key.
                FieldKey::Computed(index) => (Expr::Identifier(class_key_name(*index)), true),
            };
            full.push(Statement::Expr(Expr::Assignment {
                target: Box::new(Expr::Member {
                    object: Box::new(Expr::This),
                    property: Box::new(property),
                    computed,
                }),
                op: AssignOp::Assign,
                value: Box::new(init.cloned().unwrap_or(Expr::Undefined)),
            }));
        }
        let fields = full;
        let mut full = Vec::new();
        if is_derived || !fields.is_empty() || has_private_instance_elements {
            full.push(Statement::ClassInitialization {
                derived: is_derived,
                forward_rest: implicit_derived.then(|| "args".into()),
                fields,
            });
        }
        full.extend(body.iter().cloned());
        (params, SharedSlice::Owned(Rc::from(full)))
    }

    fn compile_var_decl(
        &mut self,
        kind: VarKind,
        name: &str,
        init: Option<&'a Expr>,
        destructuring: Option<&'a Pattern>,
    ) -> Result<Reg, Decline> {
        if let Some(pattern) = destructuring {
            let src = match init {
                Some(value) => self.compile_expr(value)?,
                None => self.load_undefined()?,
            };
            self.compile_destructure(
                pattern,
                src,
                DestructureMode::Decl {
                    is_var: matches!(kind, VarKind::Var),
                },
            )?;
            return self.load_undefined();
        }
        // `var` declarators assign through the scope chain (bare ones only
        // touch dead-zone-merged bindings); `let`/`const` declarators
        // initialize in place, defaulting to `undefined`.
        match (kind, self.resolve(name)?) {
            (VarKind::Var, Binding::Slot(slot)) => match init {
                Some(value) => {
                    let src = self.compile_named_assignment_value(value, name)?;
                    self.emit(Instr::StoreLocal { slot, src });
                }
                None => self.emit(Instr::BareVarLocal { slot }),
            },
            (VarKind::Var, Binding::Global) => match init {
                Some(value) => {
                    let src = self.compile_named_assignment_value(value, name)?;
                    let index = self.intern_string(name)?;
                    self.emit(Instr::StoreGlobal { name: index, src });
                }
                None => {
                    let index = self.intern_string(name)?;
                    self.emit(Instr::BareVarGlobal { name: index });
                }
            },
            (VarKind::Let | VarKind::Const, Binding::Slot(slot)) => {
                let src = match init {
                    Some(value) => self.compile_named_assignment_value(value, name)?,
                    None => self.load_undefined()?,
                };
                self.emit(Instr::InitLocal { slot, src });
            }
            (VarKind::Let | VarKind::Const, Binding::Global) => {
                let src = match init {
                    Some(value) => self.compile_named_assignment_value(value, name)?,
                    None => self.load_undefined()?,
                };
                let index = self.intern_string(name)?;
                self.emit(Instr::InitGlobal {
                    name: index,
                    src,
                    function: false,
                });
            }
        }
        self.load_undefined()
    }

    // -- control flow ------------------------------------------------------

    fn compile_if(
        &mut self,
        test: &'a Expr,
        then: &'a [Statement],
        else_: Option<&'a [Statement]>,
    ) -> Result<Reg, Decline> {
        let cond = self.compile_expr(test)?;
        let join = self.load_undefined()?;
        self.completions.push(CompletionRegs {
            value: join,
            present: None,
        });
        let else_jump = self.emit_jump(|target| Instr::JumpIfFalse { src: cond, target });
        let then_value = self.compile_scoped_block(then)?;
        self.emit(Instr::Mov {
            dst: join,
            src: then_value,
        });
        let end_jump = self.emit_jump(|target| Instr::Jump { target });
        self.patch_jump(else_jump, self.here())?;
        match else_ {
            Some(stmts) => {
                let else_value = self.compile_scoped_block(stmts)?;
                self.emit(Instr::Mov {
                    dst: join,
                    src: else_value,
                });
            }
            None => {
                let undef = self.load_undefined()?;
                self.emit(Instr::Mov {
                    dst: join,
                    src: undef,
                });
            }
        }
        self.patch_jump(end_jump, self.here())?;
        self.completions.pop();
        Ok(join)
    }

    /// Patch a finished loop's break/continue lists to their targets.
    fn finish_loop(
        &mut self,
        ctx: LoopCtx,
        continue_target: usize,
        end: usize,
    ) -> Result<(), Decline> {
        for addr in ctx.continues {
            self.patch_jump(addr, continue_target)?;
        }
        for addr in ctx.breaks {
            self.patch_jump(addr, end)?;
        }
        Ok(())
    }

    /// Inline the cleanups a break/continue crosses when leaving the
    /// protected regions above `depth`: `for-of` closes and `finally`
    /// bodies innermost-first. Pop each handler before its cleanup so
    /// cleanup errors cannot land in the same protected region again.
    /// `return` needs none of this: it unwinds as an error the VM handlers
    /// intercept.
    fn duplicate_unwind(&mut self, depth: usize) -> Result<(), Decline> {
        let mut index = self.unwind.len();
        while index > depth {
            index -= 1;
            let (finally, close_iter) = match &self.unwind[index] {
                UnwindCtx::Scope => {
                    self.emit(Instr::PopScope);
                    continue;
                }
                UnwindCtx::Enumerator(state) => {
                    let state = *state;
                    self.emit(Instr::EnumEnd { state });
                    continue;
                }
                UnwindCtx::Pending => {
                    self.emit(Instr::PopPending);
                    continue;
                }
                UnwindCtx::Handler {
                    finally,
                    close_iter,
                } => (*finally, *close_iter),
            };
            self.emit(Instr::PopHandler);
            if let Some(iter) = close_iter {
                self.emit(Instr::CloseIterator {
                    src: iter,
                    unwind: false,
                });
            }
            if let Some(body) = finally {
                // The inline copy runs outside its own region: compile it
                // with the crossed regions (including this one) truncated
                // away, so abrupt exits inside it do not unwind twice.
                // Scopes above the handler pop first (they were crossed
                // getting here), so the copy observes the handler's depth.
                let mut crossed = self.unwind.split_off(index);
                self.compile_finally_body(body)?;
                self.unwind.append(&mut crossed);
            }
        }
        Ok(())
    }

    /// One `finally` copy as a block; the block value is discarded, like
    /// the evaluator's, while abrupt exits propagate.
    fn compile_finally_body(&mut self, body: &'a [Statement]) -> Result<(), Decline> {
        let completion = self.new_completion(false)?;
        self.completions.push(completion);
        let _ = self.compile_scoped_block(body)?;
        self.completions.pop();
        Ok(())
    }

    /// `try/catch/finally`. The body runs under a catch handler (when a
    /// `catch` clause exists) nested inside a finally handler (when a
    /// `finally` clause exists); pads run the catch body, the cleanup
    /// copies, and rethrow. `return` unwinds through the handlers, while
    /// `break`/`continue` duplicate the cleanups inline at each exit.
    fn compile_try(
        &mut self,
        body: &'a [Statement],
        catch: &'a Option<(Option<Pattern>, Vec<Statement>)>,
        finally: &'a Option<Vec<Statement>>,
    ) -> Result<Reg, Decline> {
        if matches!(catch, Some((Some(pattern), _)) if !matches!(pattern, Pattern::Ident(_))) {
            return Err(Decline::Func("catch binding pattern"));
        }
        let value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov {
            dst: value,
            src: undef,
        });
        if catch.is_none() && finally.is_none() {
            let body_value = self.compile_scoped_block(body)?;
            self.emit(Instr::Mov {
                dst: value,
                src: body_value,
            });
            return Ok(value);
        }
        self.completions.push(CompletionRegs {
            value,
            present: None,
        });
        let err = self.alloc_reg()?;
        let mut finally_addr = None;
        let mut catch_addr = None;
        if finally.is_some() {
            finally_addr = Some(self.emit_jump(|target| Instr::PushFinally { target, dst: err }));
            self.unwind.push(UnwindCtx::Handler {
                finally: finally.as_deref(),
                close_iter: None,
            });
        }
        if catch.is_some() {
            catch_addr = Some(self.emit_jump(|target| Instr::PushCatch { target, dst: err }));
            self.unwind.push(UnwindCtx::Handler {
                finally: None,
                close_iter: None,
            });
        }
        let body_value = self.compile_scoped_block(body)?;
        self.emit(Instr::Mov {
            dst: value,
            src: body_value,
        });
        if catch.is_some() {
            self.emit(Instr::PopHandler);
            self.unwind
                .pop()
                .ok_or(Decline::Func("unwind stack underflow"))?;
        }
        let normal = self.emit_jump(|target| Instr::Jump { target });
        let catch_pad = self.here();
        if let Some((param, catch_body)) = catch {
            let mut boxed = boxed_block_names(catch_body);
            let param = match param {
                Some(Pattern::Ident(name)) => Some(name.as_str()),
                None => None,
                _ => unreachable!("catch pattern declined before emission"),
            };
            if let Some(param) = param
                && statements_capture_identifier(catch_body, param)
            {
                boxed.insert(param.to_owned());
            }
            self.push_scope(boxed);
            if let Some(param) = param {
                if self.is_boxed_here(param) {
                    let index = self.intern_string(param)?;
                    self.emit(Instr::DefineGlobal {
                        name: index,
                        src: err,
                        kind: SlotKind::Catch,
                        initialized: true,
                    });
                } else {
                    let slot = self.declare_slot(param, SlotKind::Catch)?;
                    self.emit(Instr::InitLocal { slot, src: err });
                }
            }
            self.hoist_block(catch_body)?;
            let catch_value = self.compile_block(catch_body)?;
            self.emit(Instr::Mov {
                dst: value,
                src: catch_value,
            });
            self.pop_scope()?;
        }
        let finally_run = self.here();
        if finally.is_some() {
            self.emit(Instr::PopHandler);
            self.unwind
                .pop()
                .ok_or(Decline::Func("unwind stack underflow"))?;
            if let Some(body) = finally.as_deref() {
                self.compile_finally_body(body)?;
            }
        }
        let end = self.emit_jump(|target| Instr::Jump { target });
        let finally_pad = self.here();
        if let Some(body) = finally.as_deref() {
            self.unwind.push(UnwindCtx::Pending);
            self.compile_finally_body(body)?;
            self.unwind.pop();
            self.emit(Instr::Rethrow);
        }
        let done = self.here();
        if let Some(addr) = catch_addr {
            self.patch_jump(addr, catch_pad)?;
        }
        if let Some(addr) = finally_addr {
            self.patch_jump(addr, finally_pad)?;
        }
        self.patch_jump(normal, finally_run)?;
        self.patch_jump(end, done)?;
        self.completions.pop();
        Ok(value)
    }

    /// `label: body`. A labeled loop hands the label to the loop's
    /// context (so `break label`/`continue label` reach it); any other
    /// labeled statement runs inside a `LabelBlock` context whose breaks
    /// preserve both the value and presence of the abrupt completion.
    fn compile_labeled(&mut self, label: &'a str, body: &'a Statement) -> Result<Reg, Decline> {
        // Only a directly-wrapped loop takes the label (the evaluator's
        // loops likewise take a single pending label on entry); an outer
        // label of a nested chain stays a `LabelBlock` with its own
        // completion result.
        if matches!(
            body,
            Statement::While { .. }
                | Statement::DoWhile { .. }
                | Statement::For { .. }
                | Statement::ForIn { .. }
                | Statement::ForOf { .. }
        ) {
            self.pending_labels.push(label.to_string());
            return self.compile_stmt(body);
        }
        let completion = self.new_completion(true)?;
        self.loops.push(LoopCtx {
            labels: vec![label.to_string()],
            kind: CtxKind::LabelBlock,
            cleanup_depth: self.unwind.len(),
            completion_depth: self.completions.len(),
            ..Default::default()
        });
        self.completions.push(completion);
        let value = self.compile_stmt(body)?;
        self.copy_statement_completion(completion, body, value)?;
        self.completions.pop();
        let ctx = self
            .loops
            .pop()
            .ok_or(Decline::Func("loop stack underflow"))?;
        let end = self.here();
        for address in ctx.breaks {
            self.patch_jump(address, end)?;
        }
        Ok(completion.value)
    }

    /// Lexical iteration heads use a boxed TDZ environment, replaced before
    /// each binding initialization so closures retain their iteration's cell.
    fn define_iteration_head(&mut self, names: &[String], kind: &VarKind) -> Result<(), Decline> {
        for name in names {
            let index = self.intern_string(name)?;
            let undef = self.load_undefined()?;
            self.emit(Instr::DefineGlobal {
                name: index,
                src: undef,
                kind: if *kind == VarKind::Const {
                    SlotKind::Const
                } else {
                    SlotKind::Let
                },
                initialized: false,
            });
        }
        Ok(())
    }

    fn reset_iteration_head(&mut self, names: &[String], kind: &VarKind) -> Result<(), Decline> {
        if *kind != VarKind::Var {
            self.emit(Instr::PopScope);
            self.emit(Instr::PushScope);
            self.define_iteration_head(names, kind)?;
        }
        Ok(())
    }

    /// Resolve a head after its lexical scope has been prepared.
    fn prepare_for_head(&mut self, name: &str) -> Result<ForHead, Decline> {
        // Var names are already hoisted. Their iteration assignments resolve
        // through intervening scopes, including an Annex B catch parameter.
        // Lexical heads resolve to the fresh environment installed above.
        match self.resolve(name)? {
            Binding::Slot(slot) => Ok(ForHead::Slot(slot)),
            Binding::Global => Ok(ForHead::Global(self.intern_string(name)?)),
        }
    }

    /// Var heads assign an existing binding; lexical heads initialize it.
    fn bind_for_head(&mut self, head: &ForHead, src: Reg, kind: &VarKind) {
        match head {
            ForHead::Slot(slot) if *kind == VarKind::Var => {
                self.emit(Instr::StoreLocal { slot: *slot, src })
            }
            ForHead::Slot(slot) => self.emit(Instr::InitLocal { slot: *slot, src }),
            ForHead::Global(name) => {
                if *kind == VarKind::Var {
                    self.emit(Instr::StoreGlobal { name: *name, src });
                } else {
                    self.emit(Instr::InitGlobal {
                        name: *name,
                        src,
                        function: false,
                    });
                }
            }
        }
    }

    /// `for (name in obj)` shares a lazy descriptor-aware enumerator with AST.
    fn compile_for_in(
        &mut self,
        name: &'a str,
        kind: &VarKind,
        obj: &'a Expr,
        body: &'a [Statement],
    ) -> Result<Reg, Decline> {
        let names = vec![name.to_string()];
        if *kind != VarKind::Var {
            self.push_scope_with_runtime(names.iter().cloned().collect(), true);
            self.define_iteration_head(&names, kind)?;
        }
        let head = self.prepare_for_head(name)?;
        let source = self.compile_expr(obj)?;
        let keys = self.alloc_reg()?;
        self.emit(Instr::EnumKeys {
            dst: keys,
            src: source,
        });
        self.unwind.push(UnwindCtx::Enumerator(keys));
        let loop_value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov {
            dst: loop_value,
            src: undef,
        });
        let top = self.here();
        self.emit(Instr::LoopHead);
        let key = self.alloc_reg()?;
        let done = self.alloc_reg()?;
        self.emit(Instr::EnumNext {
            state: keys,
            dst: key,
            done,
        });
        let end_jump = self.emit_jump(|target| Instr::JumpIfTrue { src: done, target });
        self.loops.push(LoopCtx {
            labels: std::mem::take(&mut self.pending_labels),
            cleanup_depth: self.unwind.len(),
            completion_depth: self.completions.len(),
            ..Default::default()
        });
        self.completions.push(CompletionRegs {
            value: loop_value,
            present: None,
        });
        self.reset_iteration_head(&names, kind)?;
        self.bind_for_head(&head, key, kind);
        let body_value = self.compile_scoped_block(body)?;
        self.copy_completion(
            CompletionRegs {
                value: loop_value,
                present: None,
            },
            self.completion_regs(body_value),
        )?;
        let increment = self.here();
        self.emit(Instr::Jump {
            target: addr_target(top)?,
        });
        self.completions.pop();
        let ctx = self
            .loops
            .pop()
            .ok_or(Decline::Func("loop stack underflow"))?;
        let end = self.here();
        self.patch_jump(end_jump, end)?;
        self.finish_loop(ctx, increment, end)?;
        self.emit(Instr::EnumEnd { state: keys });
        self.unwind.pop();
        if *kind != VarKind::Var {
            self.pop_scope()?;
        }
        Ok(loop_value)
    }

    /// `for (name of iter)` (and pattern heads, destructured per
    /// iteration). Early exits close the iterator: `break` through a close
    /// pad, unwinding through a finally handler; exhaustion and `continue`
    /// do not close.
    fn compile_for_of(
        &mut self,
        name: &'a str,
        pattern: Option<&'a Pattern>,
        kind: &VarKind,
        iter: &'a Expr,
        body: &'a [Statement],
        is_await: bool,
    ) -> Result<Reg, Decline> {
        if is_await {
            return Err(Decline::Func("async needs Phase G"));
        }
        let names = match pattern {
            Some(pattern) => pattern_names(pattern),
            None => vec![name.to_string()],
        };
        if *kind != VarKind::Var {
            self.push_scope_with_runtime(names.iter().cloned().collect(), true);
            self.define_iteration_head(&names, kind)?;
        }
        // Heads prepare before the iterable evaluates, like declarations.
        let head = match pattern {
            Some(_) => None,
            None => Some(self.prepare_for_head(name)?),
        };
        if let Some(pattern) = pattern {
            // Declare each head name; the per-iteration desugar resolves
            // them back like any declaration.
            for bound in pattern_names(pattern) {
                self.prepare_for_head(&bound)?;
            }
        }
        let source = self.compile_expr(iter)?;
        let iterator = self.alloc_reg()?;
        let next = self.alloc_reg()?;
        self.emit(Instr::ForOfInit {
            iter: iterator,
            next,
            src: source,
        });
        let loop_value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov {
            dst: loop_value,
            src: undef,
        });
        let scratch = self.alloc_reg()?;
        self.unwind.push(UnwindCtx::Handler {
            finally: None,
            close_iter: Some(iterator),
        });
        self.loops.push(LoopCtx {
            labels: std::mem::take(&mut self.pending_labels),
            cleanup_depth: self.unwind.len(),
            completion_depth: self.completions.len(),
            ..Default::default()
        });
        self.completions.push(CompletionRegs {
            value: loop_value,
            present: None,
        });
        let top = self.here();
        self.emit(Instr::LoopHead);
        let done = self.alloc_reg()?;
        let yielded = self.alloc_reg()?;
        self.emit(Instr::IterNext {
            done,
            value: yielded,
            iter: iterator,
            next,
        });
        let exhausted = self.emit_jump(|target| Instr::JumpIfTrue { src: done, target });
        // IteratorStep/IteratorValue failures do not close the iterator.
        // Protect only binding initialization and the body of each iteration.
        let unwind_pad = self.emit_jump(|target| Instr::PushFinally {
            target,
            dst: scratch,
        });
        self.reset_iteration_head(&names, kind)?;
        match (&head, pattern) {
            (Some(head), None) => self.bind_for_head(head, yielded, kind),
            (None, Some(pattern)) => {
                self.compile_destructure(
                    pattern,
                    yielded,
                    DestructureMode::Decl {
                        is_var: *kind == VarKind::Var,
                    },
                )?;
            }
            _ => return Err(Decline::Func("bad for-of head")),
        }
        let body_value = self.compile_scoped_block(body)?;
        self.copy_completion(
            CompletionRegs {
                value: loop_value,
                present: None,
            },
            self.completion_regs(body_value),
        )?;
        let continue_pad = self.here();
        self.emit(Instr::PopHandler);
        self.emit(Instr::Jump {
            target: addr_target(top)?,
        });
        self.completions.pop();
        let ctx = self
            .loops
            .pop()
            .ok_or(Decline::Func("loop stack underflow"))?;
        self.unwind
            .pop()
            .ok_or(Decline::Func("unwind stack underflow"))?;
        let drained = self.here();
        let end_jump = self.emit_jump(|target| Instr::Jump { target });
        let close_pad = self.here();
        self.emit(Instr::PopHandler);
        self.emit(Instr::CloseIterator {
            src: iterator,
            unwind: false,
        });
        let over_pad = self.emit_jump(|target| Instr::Jump { target });
        self.patch_jump(unwind_pad, self.here())?;
        self.emit(Instr::CloseIterator {
            src: iterator,
            unwind: true,
        });
        self.emit(Instr::Rethrow);
        let end = self.here();
        self.patch_jump(exhausted, drained)?;
        self.patch_jump(end_jump, end)?;
        self.patch_jump(over_pad, end)?;
        for addr in ctx.continues {
            self.patch_jump(addr, continue_pad)?;
        }
        for addr in ctx.breaks {
            self.patch_jump(addr, close_pad)?;
        }
        if *kind != VarKind::Var {
            self.pop_scope()?;
        }
        Ok(loop_value)
    }

    /// `switch (disc) { ... }`. One scope hosts every case's lexical hoists
    /// (shared switch scope, matching the evaluator); strict-equality tests
    /// dispatch to case bodies in order with fallthrough, `break` exits the
    /// switch, and a completed switch evaluates to `undefined`.
    fn compile_switch(&mut self, disc: &'a Expr, cases: &'a [SwitchCase]) -> Result<Reg, Decline> {
        let scrutinee = self.compile_expr(disc)?;
        // One scope across all cases: a name captured in any case boxes.
        let mut boxed = HashSet::new();
        for case in cases {
            boxed.extend(boxed_block_names(&case.body));
        }
        self.push_scope(boxed);
        for case in cases {
            self.hoist_block(&case.body)?;
        }
        let value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov {
            dst: value,
            src: undef,
        });
        let mut tests = Vec::new();
        let mut default = None;
        for (index, case) in cases.iter().enumerate() {
            match &case.test {
                Some(test) => {
                    let test_value = self.compile_expr(test)?;
                    let matched = self.alloc_reg()?;
                    self.emit(Instr::Binary {
                        dst: matched,
                        op: BinOp::Seq,
                        lhs: scrutinee,
                        rhs: test_value,
                    });
                    tests.push((
                        self.emit_jump(|target| Instr::JumpIfTrue {
                            src: matched,
                            target,
                        }),
                        index,
                    ));
                }
                None => {
                    if default.is_none() {
                        default = Some(index);
                    }
                }
            }
        }
        let no_match = self.emit_jump(|target| Instr::Jump { target });
        self.loops.push(LoopCtx {
            kind: CtxKind::Switch,
            cleanup_depth: self.unwind.len(),
            completion_depth: self.completions.len(),
            ..Default::default()
        });
        self.completions.push(CompletionRegs {
            value,
            present: None,
        });
        let mut bodies = Vec::with_capacity(cases.len());
        for case in cases {
            bodies.push(self.here());
            let case_value = self.compile_block(&case.body)?;
            self.copy_completion(
                CompletionRegs {
                    value,
                    present: None,
                },
                self.completion_regs(case_value),
            )?;
        }
        self.completions.pop();
        let ctx = self
            .loops
            .pop()
            .ok_or(Decline::Func("loop stack underflow"))?;
        debug_assert!(ctx.continues.is_empty());
        let end = self.here();
        for addr in ctx.breaks {
            self.patch_jump(addr, end)?;
        }
        for (addr, index) in tests {
            self.patch_jump(addr, bodies[index])?;
        }
        match default {
            Some(index) => self.patch_jump(no_match, bodies[index])?,
            None => self.patch_jump(no_match, end)?,
        }
        self.pop_scope()?;
        Ok(value)
    }

    fn compile_while(&mut self, test: &'a Expr, body: &'a [Statement]) -> Result<Reg, Decline> {
        let loop_value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov {
            dst: loop_value,
            src: undef,
        });
        // Budget first, then the test — the evaluator's order — so a
        // `continue` re-entering here consumes exactly once per iteration.
        let test_addr = self.here();
        self.emit(Instr::LoopHead);
        let cond = self.compile_expr(test)?;
        let end_jump = self.emit_jump(|target| Instr::JumpIfFalse { src: cond, target });
        self.loops.push(LoopCtx {
            labels: std::mem::take(&mut self.pending_labels),
            cleanup_depth: self.unwind.len(),
            completion_depth: self.completions.len(),
            ..Default::default()
        });
        self.completions.push(CompletionRegs {
            value: loop_value,
            present: None,
        });
        let body_value = self.compile_scoped_block(body)?;
        self.copy_completion(
            CompletionRegs {
                value: loop_value,
                present: None,
            },
            self.completion_regs(body_value),
        )?;
        self.emit(Instr::Jump {
            target: addr_target(test_addr)?,
        });
        self.completions.pop();
        let ctx = self
            .loops
            .pop()
            .ok_or(Decline::Func("loop stack underflow"))?;
        let end = self.here();
        self.patch_jump(end_jump, end)?;
        self.finish_loop(ctx, test_addr, end)?;
        Ok(loop_value)
    }

    fn compile_do_while(&mut self, test: &'a Expr, body: &'a [Statement]) -> Result<Reg, Decline> {
        let loop_value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov {
            dst: loop_value,
            src: undef,
        });
        let body_addr = self.here();
        self.emit(Instr::LoopHead);
        self.loops.push(LoopCtx {
            labels: std::mem::take(&mut self.pending_labels),
            cleanup_depth: self.unwind.len(),
            completion_depth: self.completions.len(),
            ..Default::default()
        });
        self.completions.push(CompletionRegs {
            value: loop_value,
            present: None,
        });
        let body_value = self.compile_scoped_block(body)?;
        self.copy_completion(
            CompletionRegs {
                value: loop_value,
                present: None,
            },
            self.completion_regs(body_value),
        )?;
        // A `continue` lands here, after the body: the test runs without
        // consuming the budget a second time, like the evaluator.
        let test_addr = self.here();
        let cond = self.compile_expr(test)?;
        self.emit(Instr::JumpIfTrue {
            src: cond,
            target: addr_target(body_addr)?,
        });
        self.completions.pop();
        let ctx = self
            .loops
            .pop()
            .ok_or(Decline::Func("loop stack underflow"))?;
        let end = self.here();
        self.finish_loop(ctx, test_addr, end)?;
        Ok(loop_value)
    }

    fn compile_for(
        &mut self,
        init: Option<&'a ForInit>,
        test: Option<&'a Expr>,
        update: Option<&'a Expr>,
        body: &'a [Statement],
    ) -> Result<Reg, Decline> {
        // The head owns a scope, like the evaluator's pushed loop scope.
        // Captured lexical heads box into a runtime environment. `let`
        // rotates before the first test and each update, so
        // each iteration's closures observe their own cells.
        let boxed = boxed_for_head_names(init, test, update, body);
        self.push_scope(boxed.clone());
        if let Some(init) = init {
            self.prepare_for_lexicals(init)?;
            self.compile_for_init(init)?;
        }
        let rotating = if matches!(
            init,
            Some(
                ForInit::Var {
                    kind: VarKind::Let,
                    ..
                } | ForInit::Pattern {
                    kind: VarKind::Let,
                    ..
                }
            )
        ) {
            boxed.clone()
        } else {
            HashSet::new()
        };
        if !rotating.is_empty() {
            self.rotate_head_scope(&rotating)?;
        }
        let loop_value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov {
            dst: loop_value,
            src: undef,
        });
        let test_addr = self.here();
        self.emit(Instr::LoopHead);
        let end_jump = if let Some(test) = test {
            let cond = self.compile_expr(test)?;
            Some(self.emit_jump(|target| Instr::JumpIfFalse { src: cond, target }))
        } else {
            None
        };
        self.loops.push(LoopCtx {
            labels: std::mem::take(&mut self.pending_labels),
            cleanup_depth: self.unwind.len(),
            completion_depth: self.completions.len(),
            ..Default::default()
        });
        self.completions.push(CompletionRegs {
            value: loop_value,
            present: None,
        });
        let body_value = self.compile_scoped_block(body)?;
        self.copy_completion(
            CompletionRegs {
                value: loop_value,
                present: None,
            },
            self.completion_regs(body_value),
        )?;
        // A `continue` runs the update, then the budgeted test. With a
        // boxed head, the scope rotates first: the update assigns the next
        // iteration's cells, so the current iteration's closures keep the
        // values they observed — like the evaluator's per-iteration copy.
        let continue_addr = self.here();
        if !rotating.is_empty() {
            self.rotate_head_scope(&rotating)?;
        }
        if let Some(update) = update {
            let _ = self.compile_expr(update)?;
        }
        self.emit(Instr::Jump {
            target: addr_target(test_addr)?,
        });
        self.completions.pop();
        let ctx = self
            .loops
            .pop()
            .ok_or(Decline::Func("loop stack underflow"))?;
        // Breaks unwind to above the head scope, then land on the pop pad
        // with everything else; the head scope pops exactly once per exit.
        let pad = self.here();
        self.pop_scope()?;
        if let Some(addr) = end_jump {
            self.patch_jump(addr, pad)?;
        }
        self.finish_loop(ctx, continue_addr, pad)?;
        Ok(loop_value)
    }

    /// Rotate a boxed head scope between iterations: read the head values
    /// out of the old scope, replace it, and define them in the new one.
    fn rotate_head_scope(&mut self, boxed: &HashSet<String>) -> Result<(), Decline> {
        let mut names: Vec<&String> = boxed.iter().collect();
        names.sort();
        let mut temps = Vec::with_capacity(names.len());
        for name in &names {
            let tmp = self.alloc_reg()?;
            let index = self.intern_string(name)?;
            self.emit(Instr::LoadGlobal {
                dst: tmp,
                name: index,
            });
            temps.push(tmp);
        }
        self.emit(Instr::PopScope);
        self.emit(Instr::PushScope);
        for (name, tmp) in names.iter().zip(temps) {
            let index = self.intern_string(name)?;
            self.emit(Instr::DefineGlobal {
                name: index,
                src: tmp,
                kind: SlotKind::Let,
                initialized: true,
            });
        }
        Ok(())
    }

    /// Plain identifier declarators in a `for` head, shared by `Var`
    /// heads and the trailing declarators of pattern heads.
    fn compile_for_decls(
        &mut self,
        kind: &VarKind,
        decls: &'a [(String, Option<Expr>)],
    ) -> Result<(), Decline> {
        for (name, init) in decls {
            if *kind == VarKind::Var && init.is_none() {
                continue;
            }
            match kind {
                // Head `var`s live in the function scope (hoisted);
                // only initialized declarators assign a value.
                VarKind::Var => {
                    let src = match init {
                        Some(value) => self.compile_named_assignment_value(value, name)?,
                        None => self.load_undefined()?,
                    };
                    match self.resolve(name)? {
                        Binding::Slot(slot) => {
                            self.emit(Instr::StoreLocal { slot, src });
                        }
                        Binding::Global => {
                            let index = self.intern_string(name)?;
                            self.emit(Instr::StoreGlobal { name: index, src });
                        }
                    }
                }
                VarKind::Let | VarKind::Const => {
                    let slot_kind = if matches!(kind, VarKind::Const) {
                        SlotKind::Const
                    } else {
                        SlotKind::Let
                    };
                    if self.is_boxed_here(name) {
                        let index = self.intern_string(name)?;
                        let undef = self.load_undefined()?;
                        self.emit(Instr::DefineGlobal {
                            name: index,
                            src: undef,
                            kind: slot_kind,
                            initialized: false,
                        });
                        let src = match init {
                            Some(value) => self.compile_named_assignment_value(value, name)?,
                            None => self.load_undefined()?,
                        };
                        self.emit(Instr::InitGlobal {
                            name: index,
                            src,
                            function: false,
                        });
                        continue;
                    }
                    let slot = self.declare_slot(name, slot_kind)?;
                    self.emit(Instr::DeclareLocal {
                        slot,
                        kind: slot_kind,
                        initialized: false,
                    });
                    let src = match init {
                        Some(value) => self.compile_named_assignment_value(value, name)?,
                        None => self.load_undefined()?,
                    };
                    self.emit(Instr::InitLocal { slot, src });
                }
            }
        }
        Ok(())
    }

    /// Create every lexical head binding before any initializer runs.
    fn prepare_for_lexicals(&mut self, init: &'a ForInit) -> Result<(), Decline> {
        let (kind, names) = match init {
            ForInit::Var { kind, decls } => (
                kind.clone(),
                decls
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>(),
            ),
            ForInit::Pattern {
                kind,
                pattern,
                trailing,
                ..
            } => {
                let mut names = pattern_names(pattern);
                names.extend(trailing.iter().map(|(name, _)| name.clone()));
                (kind.clone(), names)
            }
            _ => return Ok(()),
        };
        if kind == VarKind::Var {
            return Ok(());
        }
        let kind = if kind == VarKind::Const {
            SlotKind::Const
        } else {
            SlotKind::Let
        };
        for name in names {
            if self.is_boxed_here(&name) {
                let name = self.intern_string(&name)?;
                let value = self.load_undefined()?;
                self.emit(Instr::DefineGlobal {
                    name,
                    src: value,
                    kind,
                    initialized: false,
                });
            } else {
                let slot = self.declare_slot(&name, kind)?;
                self.emit(Instr::DeclareLocal {
                    slot,
                    kind,
                    initialized: false,
                });
            }
        }
        Ok(())
    }

    fn compile_for_init(&mut self, init: &'a ForInit) -> Result<(), Decline> {
        match init {
            ForInit::Var { kind, decls } => self.compile_for_decls(kind, decls),
            ForInit::Pattern {
                kind,
                pattern,
                init,
                trailing,
            } => {
                let src = self.compile_expr(init)?;
                if !matches!(kind, VarKind::Var) {
                    let slot_kind = if matches!(kind, VarKind::Const) {
                        SlotKind::Const
                    } else {
                        SlotKind::Let
                    };
                    for bound in pattern_names(pattern) {
                        if self.is_boxed_here(&bound) {
                            let index = self.intern_string(&bound)?;
                            let undef = self.load_undefined()?;
                            self.emit(Instr::DefineGlobal {
                                name: index,
                                src: undef,
                                kind: slot_kind,
                                initialized: false,
                            });
                            continue;
                        }
                        let slot = self.declare_slot(&bound, slot_kind)?;
                        self.emit(Instr::DeclareLocal {
                            slot,
                            kind: slot_kind,
                            initialized: false,
                        });
                    }
                }
                self.compile_destructure(
                    pattern,
                    src,
                    DestructureMode::Decl {
                        is_var: matches!(kind, VarKind::Var),
                    },
                )?;
                self.compile_for_decls(kind, trailing)?;
                Ok(())
            }
            ForInit::Expr(expr) => {
                let _ = self.compile_expr(expr)?;
                Ok(())
            }
        }
    }
}

impl<'a> Compiler<'a> {
    // -- expressions -------------------------------------------------------

    fn compile_expr(&mut self, expr: &'a Expr) -> Result<Reg, Decline> {
        let target = match expr {
            Expr::Assignment { target, .. } | Expr::LogicalAssignment { target, .. } => {
                Some(target.as_ref())
            }
            Expr::Unary {
                op: UnOp::Inc | UnOp::Dec,
                operand,
                ..
            } => Some(operand.as_ref()),
            _ => None,
        };
        if matches!(target, Some(Expr::Member { property, computed: false, .. })
            if matches!(property.as_ref(), Expr::String(name) if name.to_key().starts_with('#')))
        {
            return Err(Decline::Func("private field writes"));
        }
        match expr {
            Expr::LegacyLiteral(inner) | Expr::Parenthesized(inner) => self.compile_expr(inner),
            Expr::Number(value) => {
                let index = self.intern_number(*value)?;
                self.load_const(index)
            }
            Expr::String(value) | Expr::EscapedString(value) => {
                let index = self.push_const(Constant::String(value.clone()))?;
                self.load_const(index)
            }
            Expr::Bool(true) => {
                let index = self.const_true()?;
                self.load_const(index)
            }
            Expr::Bool(false) => {
                let index = self.const_false()?;
                self.load_const(index)
            }
            Expr::Null => {
                let index = self.const_null()?;
                self.load_const(index)
            }
            Expr::Undefined | Expr::Elision => self.load_undefined(),
            Expr::Identifier(name) => self.compile_identifier(name),
            Expr::Array { items, .. } => self.compile_array(items),
            Expr::Binary { op, left, right } => self.compile_binary(*op, left, right),
            Expr::Unary {
                op,
                operand,
                prefix,
            } => self.compile_unary(*op, operand, *prefix),
            Expr::Call { callee, args } => {
                if Self::continues_optional_chain(expr) {
                    self.compile_chain_expression(expr)
                } else {
                    self.compile_call(callee, args)
                }
            }
            Expr::Member {
                object,
                property,
                computed,
            } => {
                if Self::continues_optional_chain(expr) {
                    return self.compile_chain_expression(expr);
                }
                if matches!(object.as_ref(), Expr::Super) {
                    let key = self.compile_super_property_key(property)?;
                    let dst = self.alloc_reg()?;
                    self.emit(Instr::SuperMember { dst, key });
                    return Ok(dst);
                }
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::GetProp {
                    private: !computed && matches!(property.as_ref(), Expr::String(key) if key.to_key().starts_with('#')),
                    cache: 0,
                    dst,
                    obj,
                    key,
                });
                Ok(dst)
            }
            Expr::Assignment { target, op, value } => self.compile_assignment(target, *op, value),
            Expr::LogicalAssignment { target, op, value } => {
                self.compile_logical_assignment(target, *op, value)
            }
            Expr::Conditional {
                test,
                consequent,
                alternate,
            } => {
                let cond = self.compile_expr(test)?;
                let join = self.alloc_reg()?;
                let else_jump = self.emit_jump(|target| Instr::JumpIfFalse { src: cond, target });
                let then_value = self.compile_expr(consequent)?;
                self.emit(Instr::Mov {
                    dst: join,
                    src: then_value,
                });
                let end_jump = self.emit_jump(|target| Instr::Jump { target });
                self.patch_jump(else_jump, self.here())?;
                let else_value = self.compile_expr(alternate)?;
                self.emit(Instr::Mov {
                    dst: join,
                    src: else_value,
                });
                self.patch_jump(end_jump, self.here())?;
                Ok(join)
            }
            Expr::ArrowFn {
                source_text: _,
                params,
                body,
                is_async,
            } => {
                let body = match body.as_ref() {
                    ExprOrBlock::Block(stmts) => FuncBody::Stmts(SharedSlice::Borrowed(stmts)),
                    ExprOrBlock::Expr(expr) => FuncBody::Expr(expr),
                };
                self.defer_function(FuncDef {
                    source_text: expr.source_text().cloned(),
                    named_expression: false,
                    name: None,
                    params: SharedSlice::Borrowed(params),
                    body,
                    is_arrow: true,
                    is_async: *is_async,
                    is_generator: false,
                    is_constructor: false,
                })
            }
            Expr::FnExpr {
                source_text: _,
                name,
                params,
                body,
                is_async,
                is_generator,
            } => self.defer_function(FuncDef {
                source_text: expr.source_text().cloned(),
                named_expression: name.is_some(),
                name: name.clone(),
                params: SharedSlice::Borrowed(params),
                body: FuncBody::Stmts(SharedSlice::Borrowed(body)),
                is_arrow: false,
                is_async: *is_async,
                is_generator: *is_generator,
                is_constructor: !is_async && !is_generator,
            }),
            Expr::New { callee, args } => {
                let callee = self.compile_expr(callee)?;
                let arguments = self.compile_call_arguments(args)?;
                let dst = self.alloc_reg()?;
                match arguments {
                    CallArgs::Range { start, argc } => self.emit(Instr::Construct {
                        dst,
                        callee,
                        args: start,
                        argc,
                    }),
                    CallArgs::Spread { tmpl } => {
                        self.emit(Instr::ConstructSpread { dst, callee, tmpl })
                    }
                }
                Ok(dst)
            }
            Expr::Template { quasis, exprs } => self.compile_template(quasis, exprs),
            Expr::This => self.compile_this(),
            Expr::Object { props, .. } => self.compile_object(props),
            Expr::ClassExpr {
                source_text: _,
                name,
                superclass,
                body,
            } => self.compile_class(
                name.as_deref().unwrap_or(""),
                name.clone(),
                superclass.as_deref(),
                body,
                expr.source_text(),
            ),
            Expr::TaggedTemplate {
                site,
                tag,
                cooked,
                raw,
                exprs,
            } => self.compile_tagged(*site, tag, cooked, raw, exprs),
            Expr::Super => self.raise_bare_super(),
            Expr::Spread(inner) => self.compile_expr(inner),
            Expr::DynamicImport {
                specifier,
                options,
                phase,
            } => {
                if options.is_some() {
                    return Err(Decline::Func("dynamic import options"));
                }
                if *phase != crate::parser::ImportPhase::Evaluation {
                    return Err(Decline::Func("non-evaluation import phases"));
                }
                let src = self.compile_expr(specifier)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::DynamicImport { dst, src });
                Ok(dst)
            }
            Expr::NewTarget => {
                let dst = self.alloc_reg()?;
                self.emit(Instr::NewTarget { dst });
                Ok(dst)
            }
            Expr::ImportMeta => {
                let dst = self.alloc_reg()?;
                self.emit(Instr::ImportMeta { dst });
                Ok(dst)
            }
            Expr::Await(value) => {
                let src = self.compile_expr(value)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::Await { dst, src });
                Ok(dst)
            }
            Expr::Yield(_) | Expr::YieldFrom(_) => Err(Decline::Func("generators need Phase G")),
            Expr::BigIntLiteral(digits) => {
                match crate::bigint::BigInt::parse(digits) {
                    Ok(value) => {
                        let index = self.push_const(Constant::BigInt(Rc::new(value)))?;
                        self.load_const(index)
                    }
                    // The AST fallback reports the malformed literal.
                    Err(_) => Err(Decline::Func("invalid bigint literal")),
                }
            }
            Expr::Regex(pattern, flags) => {
                let index = self.push_const(Constant::Regex {
                    pattern: pattern.clone(),
                    flags: flags.clone(),
                })?;
                self.load_const(index)
            }
            Expr::OptionalChain { .. } => self.compile_chain_expression(expr),
        }
    }

    /// Identifier reads resolve lexical bindings, including shadowed `undefined`.
    /// Real functions seed their own arguments object; arrows capture the
    /// enclosing one through the environment chain.
    fn compile_identifier(&mut self, name: &str) -> Result<Reg, Decline> {
        if name == "arguments" && !self.top_level {
            self.captures_arguments = true;
        }
        let dst = self.alloc_reg()?;
        match self.resolve(name)? {
            Binding::Slot(slot) => self.emit(Instr::LoadLocal { dst, slot }),
            Binding::Global => {
                let index = self.intern_string(name)?;
                self.emit(Instr::LoadGlobal { dst, name: index });
            }
        }
        Ok(dst)
    }

    fn compile_object(&mut self, props: &'a [ObjectProp]) -> Result<Reg, Decline> {
        let mut template = Vec::with_capacity(props.len());
        for prop in props {
            match prop {
                // Shorthand uses the same binding resolution as an identifier.
                ObjectProp::CoverInitializedName { .. } => {
                    return Err(Decline::Func("object assignment default"));
                }
                ObjectProp::Shorthand(name) => {
                    let val = self.alloc_reg()?;
                    match self.resolve(name)? {
                        Binding::Slot(slot) => {
                            self.emit(Instr::LoadLocal { dst: val, slot });
                        }
                        Binding::Global => {
                            let index = self.intern_string(name)?;
                            self.emit(Instr::LoadGlobal {
                                dst: val,
                                name: index,
                            });
                        }
                    }
                    let key = self.intern_string(name)?;
                    template.push(PropEntry {
                        key: Some(KeySrc::Const(key)),
                        val,
                        kind: PropKind::Data,
                    });
                }
                ObjectProp::KeyValue(key, expression) => {
                    // NamedEvaluation matches the evaluator: an anonymous
                    // function or class value takes the key as its name.
                    // `__proto__` never infers a name.
                    let val = if key == "__proto__" {
                        self.compile_expr(expression)?
                    } else {
                        self.compile_named_assignment_value(expression, key)?
                    };
                    let kind = if key == "__proto__" {
                        PropKind::Prototype
                    } else {
                        PropKind::Data
                    };
                    let key = self.intern_string(key)?;
                    template.push(PropEntry {
                        key: Some(KeySrc::Const(key)),
                        val,
                        kind,
                    });
                }
                ObjectProp::ComputedMethod { .. } => {
                    return Err(Decline::Func("computed object method"));
                }
                ObjectProp::Computed(key_expression, value_expression) => {
                    match key_expression {
                        // Statically known keys skip the normalization check.
                        Expr::String(key) | Expr::EscapedString(key) => {
                            let display = key.to_key();
                            let val =
                                self.compile_named_assignment_value(value_expression, &display)?;
                            let key = self.intern_string(&display)?;
                            template.push(PropEntry {
                                key: Some(KeySrc::Const(key)),
                                val,
                                kind: PropKind::Data,
                            });
                        }
                        Expr::Number(key) => {
                            let display = crate::format::ecmascript_number_string(*key);
                            let val =
                                self.compile_named_assignment_value(value_expression, &display)?;
                            let key = self.intern_string(&display)?;
                            template.push(PropEntry {
                                key: Some(KeySrc::Const(key)),
                                val,
                                kind: PropKind::Data,
                            });
                        }
                        _ => {
                            let key = self.compile_expr(key_expression)?;
                            let normalized = self.alloc_reg()?;
                            self.emit(Instr::PropertyKey {
                                dst: normalized,
                                src: key,
                            });
                            let val = self.compile_expr(value_expression)?;
                            // The display name is only known at runtime here;
                            // rename anonymous functions and classes then.
                            if matches!(
                                value_expression.unparenthesized(),
                                Expr::FnExpr { name: None, .. }
                                    | Expr::ArrowFn { .. }
                                    | Expr::ClassExpr { name: None, .. }
                            ) {
                                self.emit(Instr::InferName {
                                    dst: val,
                                    key: normalized,
                                });
                            }
                            template.push(PropEntry {
                                key: Some(KeySrc::Reg(normalized)),
                                val,
                                kind: PropKind::Data,
                            });
                        }
                    }
                }
                ObjectProp::Method {
                    source_text: _,
                    name,
                    params,
                    body,
                    is_async,
                    is_generator,
                } => {
                    let val = self.defer_function(FuncDef {
                        source_text: prop.source_text().cloned(),
                        named_expression: false,
                        name: Some(name.clone()),
                        params: SharedSlice::Borrowed(params),
                        body: FuncBody::Stmts(SharedSlice::Borrowed(body)),
                        is_arrow: false,
                        is_async: *is_async,
                        is_generator: *is_generator,
                        // Methods are never constructors.
                        is_constructor: false,
                    })?;
                    let key = self.intern_string(name)?;
                    template.push(PropEntry {
                        key: Some(KeySrc::Const(key)),
                        val,
                        kind: PropKind::Method,
                    });
                }
                ObjectProp::Getter {
                    source_text: _,
                    name,
                    body,
                } => {
                    let val = self.defer_function(FuncDef {
                        source_text: prop.source_text().cloned(),
                        named_expression: false,
                        name: Some(format!("get {name}")),
                        params: SharedSlice::Borrowed(&[]),
                        body: FuncBody::Stmts(SharedSlice::Borrowed(body)),
                        is_arrow: false,
                        is_async: false,
                        is_generator: false,
                        is_constructor: false,
                    })?;
                    let key = self.intern_string(name)?;
                    template.push(PropEntry {
                        key: Some(KeySrc::Const(key)),
                        val,
                        kind: PropKind::Getter,
                    });
                }
                ObjectProp::Setter {
                    source_text: _,
                    name,
                    param,
                    body,
                } => {
                    let val = self.defer_function(FuncDef {
                        source_text: prop.source_text().cloned(),
                        named_expression: false,
                        name: Some(format!("set {name}")),
                        params: SharedSlice::Borrowed(std::slice::from_ref(param)),
                        body: FuncBody::Stmts(SharedSlice::Borrowed(body)),
                        is_arrow: false,
                        is_async: false,
                        is_generator: false,
                        is_constructor: false,
                    })?;
                    let key = self.intern_string(name)?;
                    template.push(PropEntry {
                        key: Some(KeySrc::Const(key)),
                        val,
                        kind: PropKind::Setter,
                    });
                }
                ObjectProp::Spread(expression) => {
                    let src = self.compile_expr(expression)?;
                    // Copy immediately: getters/traps and abrupt completions
                    // precede every later property expression. The private
                    // snapshot has only data properties, so final assembly has
                    // no further guest-observable source reads.
                    let snapshot_template =
                        self.push_const(Constant::ObjectTemplate(vec![PropEntry {
                            key: None,
                            val: src,
                            kind: PropKind::Spread,
                        }]))?;
                    let snapshot = self.alloc_reg()?;
                    self.emit(Instr::BuildObject {
                        dst: snapshot,
                        tmpl: snapshot_template,
                    });
                    template.push(PropEntry {
                        key: None,
                        val: snapshot,
                        kind: PropKind::Spread,
                    });
                }
            }
        }
        let tmpl = self.push_const(Constant::ObjectTemplate(template))?;
        let dst = self.alloc_reg()?;
        self.emit(Instr::BuildObject { dst, tmpl });
        Ok(dst)
    }

    fn compile_array(&mut self, items: &'a [Expr]) -> Result<Reg, Decline> {
        if items
            .iter()
            .any(|item| matches!(item, Expr::Spread(_) | Expr::Elision))
        {
            let mut template = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Expr::Spread(inner) => {
                        let src = self.compile_expr(inner)?;
                        let reg = self.alloc_reg()?;
                        self.emit(Instr::ExpandSpread { dst: reg, src });
                        template.push(SpreadEntry {
                            elision: false,
                            spread: true,
                            prepared: true,
                            reg,
                        });
                    }
                    _ => {
                        let reg = self.compile_expr(item)?;
                        template.push(SpreadEntry {
                            elision: matches!(item, Expr::Elision),
                            spread: false,
                            prepared: false,
                            reg,
                        });
                    }
                }
            }
            let tmpl = self.push_const(Constant::SpreadTemplate(template))?;
            let dst = self.alloc_reg()?;
            self.emit(Instr::BuildArray { dst, tmpl });
            return Ok(dst);
        }
        // Dense literals use the compact register-range instruction.
        let start = self.alloc_regs(items.len())?;
        for (i, item) in items.iter().enumerate() {
            let reg = self.compile_expr(item)?;
            self.emit(Instr::Mov {
                dst: start + i as u16,
                src: reg,
            });
        }
        let dst = self.alloc_reg()?;
        self.emit(Instr::NewArray {
            dst,
            args: start,
            argc: items.len() as u16,
        });
        Ok(dst)
    }

    fn compile_binary(
        &mut self,
        op: BinOp,
        left: &'a Expr,
        right: &'a Expr,
    ) -> Result<Reg, Decline> {
        if op == BinOp::In
            && let Expr::Identifier(name) = left
            && name.starts_with('#')
        {
            let obj = self.compile_expr(right)?;
            let name = self.intern_string(name)?;
            let dst = self.alloc_reg()?;
            self.emit(Instr::PrivateIn { dst, obj, name });
            return Ok(dst);
        }
        match op {
            BinOp::And | BinOp::Or | BinOp::Nullish => {
                let lhs = self.compile_expr(left)?;
                let join = self.alloc_reg()?;
                self.emit(Instr::Mov {
                    dst: join,
                    src: lhs,
                });
                let end_jump = match op {
                    BinOp::And => self.emit_jump(|target| Instr::JumpIfFalse { src: lhs, target }),
                    BinOp::Or => self.emit_jump(|target| Instr::JumpIfTrue { src: lhs, target }),
                    _ => self.emit_jump(|target| Instr::JumpIfNotNullish { src: lhs, target }),
                };
                let rhs = self.compile_expr(right)?;
                self.emit(Instr::Mov {
                    dst: join,
                    src: rhs,
                });
                self.patch_jump(end_jump, self.here())?;
                Ok(join)
            }
            BinOp::Comma => {
                let _ = self.compile_expr(left)?;
                self.compile_expr(right)
            }
            _ => {
                let lhs = self.compile_expr(left)?;
                let rhs = self.compile_expr(right)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::Binary { dst, op, lhs, rhs });
                Ok(dst)
            }
        }
    }

    fn compile_unary(&mut self, op: UnOp, operand: &'a Expr, prefix: bool) -> Result<Reg, Decline> {
        let operand = operand.unparenthesized();
        match op {
            UnOp::Inc | UnOp::Dec => self.compile_inc_dec(op, operand, prefix),
            UnOp::Delete => self.compile_delete(operand),
            UnOp::Typeof => self.compile_typeof(operand),
            _ => {
                let src = self.compile_expr(operand)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::Unary { dst, op, src });
                Ok(dst)
            }
        }
    }

    fn compile_super_property_key(&mut self, property: &'a Expr) -> Result<Reg, Decline> {
        let receiver = self.alloc_reg()?;
        self.emit(Instr::LoadGlobalThis { dst: receiver });
        self.compile_expr(property)
    }

    fn compile_super_reference(&mut self, property: &'a Expr) -> Result<(Reg, Reg, Reg), Decline> {
        let src = self.compile_super_property_key(property)?;
        let base = self.alloc_reg()?;
        let receiver = self.alloc_reg()?;
        let key = self.alloc_reg()?;
        self.emit(Instr::SuperReference {
            base,
            receiver,
            key,
            src,
        });
        Ok((base, receiver, key))
    }

    fn compile_inc_dec(
        &mut self,
        op: UnOp,
        operand: &'a Expr,
        prefix: bool,
    ) -> Result<Reg, Decline> {
        if let Expr::Member {
            object, property, ..
        } = operand.unparenthesized()
            && matches!(object.as_ref(), Expr::Super)
        {
            let (base, receiver, key) = self.compile_super_reference(property)?;
            let src = self.alloc_reg()?;
            self.emit(Instr::GetPropertyWithReceiver {
                dst: src,
                base,
                receiver,
                key,
            });
            let previous = self.alloc_reg()?;
            let updated = self.alloc_reg()?;
            self.emit(Instr::NumericUpdate {
                previous,
                updated,
                src,
                increment: op == UnOp::Inc,
            });
            self.emit(Instr::SetPropertyWithReceiver {
                base,
                receiver,
                key,
                value: updated,
            });
            return Ok(if prefix { updated } else { previous });
        }
        let delta = if op == UnOp::Inc { 1 } else { -1 };
        match operand.unparenthesized() {
            Expr::Identifier(name) => {
                let dst = self.alloc_reg()?;
                match self.resolve(name)? {
                    Binding::Slot(slot) => {
                        self.emit(Instr::IncLocal {
                            dst,
                            slot,
                            delta,
                            prefix,
                        });
                    }
                    Binding::Global => {
                        let index = self.intern_string(name)?;
                        self.emit(Instr::IncGlobal {
                            dst,
                            name: index,
                            delta,
                            prefix,
                        });
                    }
                }
                Ok(dst)
            }
            Expr::Member {
                object, property, ..
            } => {
                if matches!(object.as_ref(), Expr::Super) {
                    return self.raise_bare_super();
                }
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::IncProp {
                    dst,
                    obj,
                    key,
                    delta,
                    prefix,
                });
                Ok(dst)
            }
            // Annex B call targets evaluate for their effects, then throw
            // before ToNumeric. A returned object must not be coerced.
            _ => {
                let _ = self.compile_expr(operand)?;
                let msg = self.intern_string("ReferenceError: invalid assignment target")?;
                self.emit(Instr::Raise { msg });
                self.load_undefined()
            }
        }
    }

    fn compile_delete(&mut self, operand: &'a Expr) -> Result<Reg, Decline> {
        if Self::continues_optional_chain(operand)
            && let Expr::Member {
                object, property, ..
            }
            | Expr::OptionalChain {
                object, property, ..
            } = operand
        {
            let index = self.const_true()?;
            let dst = self.load_const(index)?;
            let mut ends = Vec::new();
            let obj = self.compile_chain_value(object, &mut ends)?;
            if matches!(operand, Expr::OptionalChain { .. }) {
                ends.push(self.emit_jump(|target| Instr::JumpIfNullish { src: obj, target }));
            }
            let key = self.compile_expr(property)?;
            self.emit(Instr::DelProp { dst, obj, key });
            for end in ends {
                self.patch_jump(end, self.here())?;
            }
            return Ok(dst);
        }
        match operand {
            Expr::Member {
                object, property, ..
            } => {
                if matches!(object.as_ref(), Expr::Super) {
                    // `this` resolution throws for an unbound receiver,
                    // then the key expression still evaluates, then the
                    // ReferenceError — without any ToPropertyKey conversion.
                    let this_value = self.alloc_reg()?;
                    self.emit(Instr::LoadGlobalThis { dst: this_value });
                    let _ = self.compile_expr(property)?;
                    let msg =
                        self.intern_string("ReferenceError: Cannot delete a super property")?;
                    self.emit(Instr::Raise { msg });
                    return self.load_undefined();
                }
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::DelProp { dst, obj, key });
                Ok(dst)
            }
            // Declared bindings are not configurable: a slot-bound name is
            // always bound, so it deletes to `false` without a lookup.
            Expr::Identifier(name) => {
                let dst = self.alloc_reg()?;
                match self.resolve(name)? {
                    Binding::Slot(_) => {
                        let index = self.const_false()?;
                        self.emit(Instr::LoadConst { dst, cst: index });
                    }
                    Binding::Global => {
                        let index = self.intern_string(name)?;
                        self.emit(Instr::DelGlobal { dst, name: index });
                    }
                }
                Ok(dst)
            }
            Expr::OptionalChain {
                object, property, ..
            } => {
                let obj = self.compile_expr(object)?;
                let dst = self.alloc_reg()?;
                let end = self.emit_jump(|target| Instr::JumpIfNullish { src: obj, target });
                let key = self.compile_expr(property)?;
                self.emit(Instr::DelProp { dst, obj, key });
                let over = self.emit_jump(|target| Instr::Jump { target });
                self.patch_jump(end, self.here())?;
                let index = self.const_true()?;
                self.emit(Instr::LoadConst { dst, cst: index });
                self.patch_jump(over, self.here())?;
                Ok(dst)
            }
            _ => {
                let _ = self.compile_expr(operand)?;
                let index = self.const_true()?;
                self.load_const(index)
            }
        }
    }

    /// `typeof` tolerates missing bindings but rejects uninitialized ones.
    fn compile_typeof(&mut self, operand: &'a Expr) -> Result<Reg, Decline> {
        if let Expr::Identifier(name) = operand.unparenthesized() {
            let dst = self.alloc_reg()?;
            match self.resolve(name)? {
                Binding::Slot(slot) => self.emit(Instr::TypeofLocal { dst, slot }),
                Binding::Global => {
                    let index = self.intern_string(name)?;
                    self.emit(Instr::TypeofGlobal { dst, name: index });
                }
            }
            return Ok(dst);
        }
        let src = self.compile_expr(operand)?;
        let dst = self.alloc_reg()?;
        self.emit(Instr::Unary {
            dst,
            op: UnOp::Typeof,
            src,
        });
        Ok(dst)
    }

    /// `` tag`a${x}b` `` desugars to `tag(parts, x)` where `parts` is a
    /// fresh cooked-strings array carrying the raw chunks per evaluation.
    /// Resolve the tag before substitutions, matching the evaluator.
    fn compile_tagged(
        &mut self,
        site: u64,
        tag: &'a Expr,
        cooked: &'a [Option<crate::JsString>],
        raw: &'a [crate::JsString],
        exprs: &'a [Expr],
    ) -> Result<Reg, Decline> {
        // A parenthesized optional chain may produce undefined as the tag;
        // substitutions still execute before the ensuing callability error.
        let callee = self.load_undefined()?;
        let this = self.load_undefined()?;
        let mut ends = Vec::new();
        let (function, receiver) = self.compile_call_reference(tag, &mut ends)?;
        self.emit(Instr::Mov {
            dst: callee,
            src: function,
        });
        if let Some(receiver) = receiver {
            self.emit(Instr::Mov {
                dst: this,
                src: receiver,
            });
        }
        for end in ends {
            self.patch_jump(end, self.here())?;
        }
        let receiver = Some(this);
        let parts = self.alloc_reg()?;
        self.emit(Instr::LoadTemplate { dst: parts, site });
        let cached = self.emit_jump(|target| Instr::JumpIfNotNullish { src: parts, target });
        let mut template = Vec::with_capacity(cooked.len());
        for part in cooked {
            let constant = part
                .as_ref()
                .map(|part| Constant::String(part.clone()))
                .unwrap_or(Constant::Undefined);
            let index = self.push_const(constant)?;
            let reg = self.load_const(index)?;
            template.push(SpreadEntry {
                elision: false,
                spread: false,
                prepared: false,
                reg,
            });
        }
        let tmpl = self.push_const(Constant::SpreadTemplate(template))?;
        self.emit(Instr::BuildArray { dst: parts, tmpl });
        let mut raw_template = Vec::with_capacity(raw.len());
        for part in raw {
            let index = self.push_const(Constant::String(part.clone()))?;
            let reg = self.load_const(index)?;
            raw_template.push(SpreadEntry {
                elision: false,
                spread: false,
                prepared: false,
                reg,
            });
        }
        let tmpl = self.push_const(Constant::SpreadTemplate(raw_template))?;
        let raw_parts = self.alloc_reg()?;
        self.emit(Instr::BuildArray {
            dst: raw_parts,
            tmpl,
        });
        self.emit(Instr::FreezeTemplate {
            site,
            cooked: parts,
            raw: raw_parts,
        });

        self.patch_jump(cached, self.here())?;
        let mut values = Vec::with_capacity(exprs.len());
        for expr in exprs {
            values.push(self.compile_expr(expr)?);
        }
        let start = self.alloc_regs(1 + values.len())?;
        self.emit(Instr::Mov {
            dst: start,
            src: parts,
        });
        for (i, value) in values.iter().enumerate() {
            self.emit(Instr::Mov {
                dst: start + 1 + i as u16,
                src: *value,
            });
        }
        let argc = 1 + values.len() as u16;
        let dst = self.alloc_reg()?;
        if let Some(this) = receiver {
            self.emit(Instr::CallMethod {
                dst,
                callee,
                this,
                args: start,
                argc,
            });
        } else {
            self.emit(Instr::Call {
                dst,
                callee,
                args: start,
                argc,
            });
        }

        Ok(dst)
    }

    /// Calls evaluate arguments before the callee, like the evaluator (and
    /// unlike the specification — this engine's order is load-bearing).
    /// The current `this`: the frame's for plain functions, the global
    /// one at top level. Arrows inside functions capture it lexically.
    fn compile_this(&mut self) -> Result<Reg, Decline> {
        match self.this_mode {
            ThisMode::Frame => {
                let dst = self.alloc_reg()?;
                self.emit(Instr::LoadThis { dst });
                Ok(dst)
            }
            ThisMode::Global => {
                let dst = self.alloc_reg()?;
                self.emit(Instr::LoadGlobalThis { dst });
                Ok(dst)
            }
        }
    }

    /// A bare `super` (or a reference through one): the evaluator fails
    /// reference evaluation, after any earlier side effects already ran.
    /// The returned register never receives a value; the raise diverges.
    fn raise_bare_super(&mut self) -> Result<Reg, Decline> {
        let msg = self.intern_string("'super' must be called as a function")?;
        self.emit(Instr::Raise { msg });
        self.alloc_reg()
    }

    fn compile_call_reference(
        &mut self,
        callee: &'a Expr,
        ends: &mut Vec<usize>,
    ) -> Result<(Reg, Option<Reg>), Decline> {
        match callee {
            Expr::Member {
                object,
                property,
                computed,
            } => {
                if matches!(object.as_ref(), Expr::Super) {
                    let key = self.compile_super_property_key(property)?;
                    let function = self.alloc_reg()?;
                    self.emit(Instr::SuperMember { dst: function, key });
                    return Ok((function, Some(self.compile_this()?)));
                }
                let receiver = self.compile_chain_value(object, ends)?;
                let key = self.compile_expr(property)?;
                let function = self.alloc_reg()?;
                self.emit(Instr::GetProp {
                    private: !computed && matches!(property.as_ref(), Expr::String(name) if name.to_key().starts_with('#')),
                    cache: 0,
                    dst: function,
                    obj: receiver,
                    key,
                });
                Ok((function, Some(receiver)))
            }
            Expr::OptionalChain {
                object,
                property,
                computed,
            } => {
                if !computed && matches!(property.as_ref(), Expr::Undefined) {
                    let reference = self.compile_call_reference(object, ends)?;
                    ends.push(self.emit_jump(|target| Instr::JumpIfNullish {
                        src: reference.0,
                        target,
                    }));
                    return Ok(reference);
                }
                let receiver = self.compile_chain_value(object, ends)?;
                ends.push(self.emit_jump(|target| Instr::JumpIfNullish {
                    src: receiver,
                    target,
                }));
                let key = self.compile_expr(property)?;
                let function = self.alloc_reg()?;
                self.emit(Instr::GetProp {
                    private: !computed && matches!(property.as_ref(), Expr::String(name) if name.to_key().starts_with('#')),
                    cache: 0,
                    dst: function,
                    obj: receiver,
                    key,
                });
                Ok((function, Some(receiver)))
            }
            Expr::Parenthesized(inner) => {
                if !Self::continues_optional_chain(inner) {
                    return self.compile_call_reference(inner, ends);
                }
                let function = self.load_undefined()?;
                let receiver = self.load_undefined()?;
                let mut local_ends = Vec::new();
                let (value, this) = self.compile_call_reference(inner, &mut local_ends)?;
                self.emit(Instr::Mov {
                    dst: function,
                    src: value,
                });
                if let Some(this) = this {
                    self.emit(Instr::Mov {
                        dst: receiver,
                        src: this,
                    });
                }
                for end in local_ends {
                    self.patch_jump(end, self.here())?;
                }
                Ok((function, this.map(|_| receiver)))
            }
            Expr::Call { callee, args } => {
                Ok((self.compile_call_in_chain(callee, args, ends)?, None))
            }
            Expr::Identifier(name) if matches!(self.resolve(name)?, Binding::Global) => {
                let dst = self.alloc_reg()?;
                let receiver = self.alloc_reg()?;
                let name = self.intern_string(name)?;
                self.emit(Instr::LoadCallBinding {
                    dst,
                    receiver,
                    name,
                });
                Ok((dst, Some(receiver)))
            }
            other => Ok((self.compile_expr(other)?, None)),
        }
    }

    /// Spread iterators run at their argument position, before later arguments.
    fn compile_call_arguments(&mut self, args: &'a [Expr]) -> Result<CallArgs, Decline> {
        if args.iter().any(|arg| matches!(arg, Expr::Spread(_))) {
            let mut template = Vec::with_capacity(args.len());
            for arg in args {
                let (reg, spread) = if let Expr::Spread(inner) = arg {
                    let src = self.compile_expr(inner)?;
                    let dst = self.alloc_reg()?;
                    self.emit(Instr::ExpandSpread { dst, src });
                    (dst, true)
                } else {
                    (self.compile_expr(arg)?, false)
                };
                template.push(SpreadEntry {
                    elision: false,
                    spread,
                    prepared: spread,
                    reg,
                });
            }
            Ok(CallArgs::Spread {
                tmpl: self.push_const(Constant::SpreadTemplate(template))?,
            })
        } else {
            let start = self.alloc_regs(args.len())?;
            for (index, arg) in args.iter().enumerate() {
                let src = self.compile_expr(arg)?;
                self.emit(Instr::Mov {
                    dst: start + index as u16,
                    src,
                });
            }
            Ok(CallArgs::Range {
                start,
                argc: args.len() as u16,
            })
        }
    }

    fn compile_call(&mut self, callee: &'a Expr, args: &'a [Expr]) -> Result<Reg, Decline> {
        let mut ends = Vec::new();
        let result = self.compile_call_in_chain(callee, args, &mut ends)?;
        for end in ends {
            self.patch_jump(end, self.here())?;
        }
        Ok(result)
    }

    fn compile_call_in_chain(
        &mut self,
        callee: &'a Expr,
        args: &'a [Expr],
        ends: &mut Vec<usize>,
    ) -> Result<Reg, Decline> {
        let direct_eval =
            matches!(callee.unparenthesized(), Expr::Identifier(name) if name == "eval");
        if direct_eval && (!self.top_level || self.scopes.len() != 1) {
            return Err(Decline::Func("direct eval requires a lexical environment"));
        }
        let dst = self.alloc_reg()?;
        let is_super = matches!(callee.unparenthesized(), Expr::Super);
        let (function, receiver) = if is_super {
            let function = self.alloc_reg()?;
            self.emit(Instr::SuperConstructor { dst: function });
            (function, None)
        } else {
            self.compile_call_reference(callee, ends)?
        };
        let arguments = self.compile_call_arguments(args)?;
        let instruction = match (arguments, receiver) {
            (CallArgs::Range { start, argc }, _) if is_super => Instr::SuperCall {
                dst,
                callee: function,
                args: start,
                argc,
            },
            (CallArgs::Spread { tmpl }, _) if is_super => Instr::SuperCallSpread {
                dst,
                callee: function,
                tmpl,
            },
            (CallArgs::Range { start, argc }, _) if direct_eval => Instr::DirectEval {
                dst,
                callee: function,
                this: receiver,
                args: start,
                argc,
            },
            (CallArgs::Spread { tmpl }, _) if direct_eval => Instr::DirectEvalSpread {
                dst,
                callee: function,
                this: receiver,
                tmpl,
            },
            (CallArgs::Range { start, argc }, Some(this)) => Instr::CallMethod {
                dst,
                callee: function,
                this,
                args: start,
                argc,
            },
            (CallArgs::Spread { tmpl }, Some(this)) => Instr::MethodSpread {
                dst,
                callee: function,
                this,
                tmpl,
            },
            (CallArgs::Range { start, argc }, None) => Instr::Call {
                dst,
                callee: function,
                args: start,
                argc,
            },
            (CallArgs::Spread { tmpl }, None) => Instr::CallSpread {
                dst,
                callee: function,
                tmpl,
            },
        };
        self.emit(instruction);
        Ok(dst)
    }

    fn continues_optional_chain(expression: &Expr) -> bool {
        match expression {
            Expr::OptionalChain { .. } => true,
            Expr::Member { object, .. } => Self::continues_optional_chain(object),
            Expr::Call { callee, .. } => Self::continues_optional_chain(callee),
            _ => false,
        }
    }

    fn compile_chain_value(
        &mut self,
        expression: &'a Expr,
        ends: &mut Vec<usize>,
    ) -> Result<Reg, Decline> {
        match expression {
            Expr::Member { .. } | Expr::OptionalChain { .. } => {
                Ok(self.compile_call_reference(expression, ends)?.0)
            }
            Expr::Call { callee, args } => self.compile_call_in_chain(callee, args, ends),
            _ => self.compile_expr(expression),
        }
    }

    fn compile_chain_expression(&mut self, expression: &'a Expr) -> Result<Reg, Decline> {
        let result = self.load_undefined()?;
        let mut ends = Vec::new();
        let value = self.compile_chain_value(expression, &mut ends)?;
        self.emit(Instr::Mov {
            dst: result,
            src: value,
        });
        for end in ends {
            self.patch_jump(end, self.here())?;
        }
        Ok(result)
    }

    /// Value first, then the target reference — the evaluator's order.
    fn compile_assignment(
        &mut self,
        target: &'a Expr,
        op: AssignOp,
        value: &'a Expr,
    ) -> Result<Reg, Decline> {
        let identifier_target = matches!(target, Expr::Identifier(_));
        let target = target.unparenthesized();
        if let Expr::Member {
            object, property, ..
        } = target
            && matches!(object.as_ref(), Expr::Super)
        {
            let (base, receiver, key) = if op == AssignOp::Assign {
                let raw_key = self.compile_super_property_key(property)?;
                let undefined = self.const_undefined()?;
                let src = self.load_const(undefined)?;
                let base = self.alloc_reg()?;
                let receiver = self.alloc_reg()?;
                let key = self.alloc_reg()?;
                self.emit(Instr::SuperReference {
                    base,
                    receiver,
                    key,
                    src,
                });
                (base, receiver, raw_key)
            } else {
                self.compile_super_reference(property)?
            };
            let current = if op.bin_op().is_some() {
                let dst = self.alloc_reg()?;
                self.emit(Instr::GetPropertyWithReceiver {
                    dst,
                    base,
                    receiver,
                    key,
                });
                Some(dst)
            } else {
                None
            };
            let rhs = self.compile_expr(value)?;
            let value = if let Some(op) = op.bin_op() {
                let dst = self.alloc_reg()?;
                self.emit(Instr::Binary {
                    dst,
                    op,
                    lhs: current.expect("compound value"),
                    rhs,
                });
                dst
            } else {
                rhs
            };
            let key = if op == AssignOp::Assign {
                let normalized = self.alloc_reg()?;
                self.emit(Instr::PropertyKey {
                    dst: normalized,
                    src: key,
                });
                normalized
            } else {
                key
            };
            self.emit(Instr::SetPropertyWithReceiver {
                base,
                receiver,
                key,
                value,
            });
            return Ok(value);
        }
        if let Some(bin) = op.bin_op() {
            match target {
                Expr::Identifier(name) => {
                    let current = self.alloc_reg()?;
                    let binding = self.resolve(name)?;
                    let reference = match binding {
                        Binding::Slot(slot) => {
                            self.emit(Instr::LoadLocal { dst: current, slot });
                            None
                        }
                        Binding::Global => {
                            let base = self.alloc_reg()?;
                            let property = self.alloc_reg()?;
                            let name = self.intern_string(name)?;
                            self.emit(Instr::ReadAssignmentBinding {
                                dst: current,
                                base,
                                property,
                                name,
                            });
                            Some((base, property, name))
                        }
                    };
                    let rhs = self.compile_expr(value)?;
                    let dst = self.alloc_reg()?;
                    self.emit(Instr::Binary {
                        dst,
                        op: bin,
                        lhs: current,
                        rhs,
                    });
                    match binding {
                        Binding::Slot(slot) => self.emit(Instr::StoreLocal { slot, src: dst }),
                        Binding::Global => {
                            let (base, property, name) =
                                reference.expect("captured global reference");
                            self.emit(Instr::WriteAssignmentBinding {
                                base,
                                property,
                                name,
                                src: dst,
                            });
                        }
                    }
                    return Ok(dst);
                }
                Expr::Member {
                    object,
                    property,
                    computed,
                } => {
                    let obj = self.compile_expr(object)?;
                    let raw_key = self.compile_expr(property)?;
                    self.emit(Instr::CheckDestructObject { dst: obj, src: obj });
                    let private = !computed
                        && matches!(property.as_ref(), Expr::String(key) if key.to_key().starts_with('#'));
                    let key = if private {
                        raw_key
                    } else {
                        let key = self.alloc_reg()?;
                        self.emit(Instr::PropertyKey {
                            dst: key,
                            src: raw_key,
                        });
                        key
                    };
                    let current = self.alloc_reg()?;
                    self.emit(Instr::GetProp {
                        private,
                        dst: current,
                        obj,
                        key,
                        cache: 0,
                    });
                    let rhs = self.compile_expr(value)?;
                    let dst = self.alloc_reg()?;
                    self.emit(Instr::Binary {
                        dst,
                        op: bin,
                        lhs: current,
                        rhs,
                    });
                    self.emit(Instr::WriteAssignmentProperty {
                        obj,
                        key,
                        src: dst,
                        private,
                    });
                    return Ok(dst);
                }
                _ => return Err(Decline::Func("invalid compound assignment target")),
            }
        }
        if op == AssignOp::Assign {
            match target {
                Expr::Identifier(name) => {
                    let binding = self.resolve(name)?;
                    let reference = if matches!(binding, Binding::Global) {
                        let base = self.alloc_reg()?;
                        let property = self.alloc_reg()?;
                        let name = self.intern_string(name)?;
                        self.emit(Instr::ResolveAssignmentBinding {
                            base,
                            property,
                            name,
                        });
                        Some((base, property, name))
                    } else {
                        None
                    };
                    let rhs = if identifier_target {
                        self.compile_named_assignment_value(value, name)?
                    } else {
                        self.compile_expr(value)?
                    };
                    match binding {
                        Binding::Slot(slot) => self.emit(Instr::StoreLocal { slot, src: rhs }),
                        Binding::Global => {
                            let (base, property, name) = reference.unwrap();
                            self.emit(Instr::WriteAssignmentBinding {
                                base,
                                property,
                                name,
                                src: rhs,
                            });
                        }
                    }
                    return Ok(rhs);
                }
                Expr::Member {
                    object,
                    property,
                    computed,
                } => {
                    let obj = self.compile_expr(object)?;
                    let raw_key = self.compile_expr(property)?;
                    let private = !computed
                        && matches!(property.as_ref(),Expr::String(key) if key.to_key().starts_with('#'));
                    let key = raw_key;
                    let rhs = self.compile_expr(value)?;
                    self.emit(Instr::WriteAssignmentProperty {
                        obj,
                        key,
                        src: rhs,
                        private,
                    });
                    return Ok(rhs);
                }
                _ => (),
            }
        }
        let rhs = self.compile_expr(value)?;
        match target {
            Expr::Identifier(name) => {
                let dst = self.alloc_reg()?;
                match (self.resolve(name)?, op == AssignOp::Assign) {
                    (Binding::Slot(slot), true) => {
                        self.emit(Instr::StoreLocal { slot, src: rhs });
                        Ok(rhs)
                    }
                    (Binding::Global, true) => {
                        let index = self.intern_string(name)?;
                        self.emit(Instr::StoreGlobal {
                            name: index,
                            src: rhs,
                        });
                        Ok(rhs)
                    }
                    (Binding::Slot(slot), false) => {
                        self.emit(Instr::CompoundLocal { dst, slot, op, rhs });
                        Ok(dst)
                    }
                    (Binding::Global, false) => {
                        let index = self.intern_string(name)?;
                        self.emit(Instr::CompoundGlobal {
                            dst,
                            name: index,
                            op,
                            rhs,
                        });
                        Ok(dst)
                    }
                }
            }
            Expr::Member {
                object, property, ..
            } => {
                if matches!(object.as_ref(), Expr::Super) {
                    return self.raise_bare_super();
                }
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                if op == AssignOp::Assign {
                    self.emit(Instr::SetProp {
                        cache: 0,
                        obj,
                        key,
                        val: rhs,
                    });
                    Ok(rhs)
                } else {
                    let dst = self.alloc_reg()?;
                    self.emit(Instr::CompoundProp {
                        dst,
                        obj,
                        key,
                        op,
                        rhs,
                    });
                    Ok(dst)
                }
            }
            Expr::Array { .. } | Expr::Object { .. } => {
                Err(Decline::Func("destructuring assignment ordering"))
            }
            _ => Err(Decline::Func("invalid assignment target")),
        }
    }

    fn compile_named_assignment_value(
        &mut self,
        value: &'a Expr,
        name: &str,
    ) -> Result<Reg, Decline> {
        match value.unparenthesized() {
            Expr::FnExpr {
                source_text: _,
                name: None,
                params,
                body,
                is_async,
                is_generator,
            } => self.defer_function(FuncDef {
                source_text: value.source_text().cloned(),
                named_expression: false,
                name: Some(name.into()),
                params: SharedSlice::Borrowed(params),
                body: FuncBody::Stmts(SharedSlice::Borrowed(body)),
                is_arrow: false,
                is_async: *is_async,
                is_generator: *is_generator,
                is_constructor: !is_async && !is_generator,
            }),
            Expr::ArrowFn {
                source_text: _,
                params,
                body,
                is_async,
            } => self.defer_function(FuncDef {
                source_text: value.source_text().cloned(),
                named_expression: false,
                name: Some(name.into()),
                params: SharedSlice::Borrowed(params),
                body: match body.as_ref() {
                    ExprOrBlock::Block(stmts) => FuncBody::Stmts(SharedSlice::Borrowed(stmts)),
                    ExprOrBlock::Expr(expr) => FuncBody::Expr(expr),
                },
                is_arrow: true,
                is_async: *is_async,
                is_generator: false,
                is_constructor: false,
            }),
            Expr::ClassExpr {
                source_text: _,
                name: None,
                superclass,
                body,
            } => {
                let dst =
                    self.compile_class("", None, superclass.as_deref(), body, value.source_text())?;
                let template = self
                    .code
                    .iter()
                    .rev()
                    .find_map(|instruction| match instruction {
                        Instr::BuildClass { dst: result, tmpl } if *result == dst => Some(*tmpl),
                        _ => None,
                    })
                    .expect("compiled class template");
                if let Constant::ClassTemplate(template) = &mut self.constants[template as usize] {
                    template.name = name.into();
                }
                Ok(dst)
            }
            _ => self.compile_expr(value),
        }
    }

    fn compile_logical_assignment(
        &mut self,
        target: &'a Expr,
        op: LogicalAssignOp,
        value: &'a Expr,
    ) -> Result<Reg, Decline> {
        let infer_name = matches!(target, Expr::Identifier(_));
        let target = target.unparenthesized();
        // Read the current value, skip the write when it already decides.
        let join = self.alloc_reg()?;
        if let Expr::Member {
            object, property, ..
        } = target.unparenthesized()
            && matches!(object.as_ref(), Expr::Super)
        {
            let (base, receiver, key) = self.compile_super_reference(property)?;
            let current = self.alloc_reg()?;
            self.emit(Instr::GetPropertyWithReceiver {
                dst: current,
                base,
                receiver,
                key,
            });
            self.emit(Instr::Mov {
                dst: join,
                src: current,
            });
            let end = self.logical_skip_jump(op, current)?;
            let value = self.compile_expr(value)?;
            self.emit(Instr::SetPropertyWithReceiver {
                base,
                receiver,
                key,
                value,
            });
            self.emit(Instr::Mov {
                dst: join,
                src: value,
            });
            self.patch_jump(end, self.here())?;
            return Ok(join);
        }
        let end_jump = match target {
            Expr::Identifier(name) => {
                let binding = self.resolve(name)?;
                let current = self.alloc_reg()?;
                let reference = match binding {
                    Binding::Slot(slot) => {
                        self.emit(Instr::LoadLocal { dst: current, slot });
                        None
                    }
                    Binding::Global => {
                        let base = self.alloc_reg()?;
                        let property = self.alloc_reg()?;
                        let name = self.intern_string(name)?;
                        self.emit(Instr::ReadAssignmentBinding {
                            dst: current,
                            base,
                            property,
                            name,
                        });
                        Some((base, property, name))
                    }
                };
                self.emit(Instr::Mov {
                    dst: join,
                    src: current,
                });
                let jump = self.logical_skip_jump(op, current)?;
                let rhs = if infer_name {
                    self.compile_named_assignment_value(value, name)?
                } else {
                    self.compile_expr(value)?
                };
                match binding {
                    Binding::Slot(slot) => self.emit(Instr::StoreLocal { slot, src: rhs }),
                    Binding::Global => {
                        let (base, property, name) = reference.unwrap();
                        self.emit(Instr::WriteAssignmentBinding {
                            base,
                            property,
                            name,
                            src: rhs,
                        });
                    }
                }
                self.emit(Instr::Mov {
                    dst: join,
                    src: rhs,
                });
                jump
            }
            Expr::Member {
                object,
                property,
                computed,
            } => {
                if matches!(object.as_ref(), Expr::Super) {
                    return self.raise_bare_super();
                }
                let obj = self.compile_expr(object)?;
                let raw_key = self.compile_expr(property)?;
                self.emit(Instr::CheckDestructObject { dst: obj, src: obj });
                let private = !computed
                    && matches!(property.as_ref(), Expr::String(name) if name.to_key().starts_with('#'));
                let key = if private {
                    raw_key
                } else {
                    let key = self.alloc_reg()?;
                    self.emit(Instr::PropertyKey {
                        dst: key,
                        src: raw_key,
                    });
                    key
                };
                let current = self.alloc_reg()?;
                self.emit(Instr::GetProp {
                    private,
                    cache: 0,
                    dst: current,
                    obj,
                    key,
                });
                self.emit(Instr::Mov {
                    dst: join,
                    src: current,
                });
                let jump = self.logical_skip_jump(op, current)?;
                let rhs = self.compile_expr(value)?;
                self.emit(Instr::WriteAssignmentProperty {
                    obj,
                    key,
                    src: rhs,
                    private,
                });
                self.emit(Instr::Mov {
                    dst: join,
                    src: rhs,
                });
                jump
            }
            _ => return Err(Decline::Func("invalid assignment target")),
        };
        self.patch_jump(end_jump, self.here())?;
        Ok(join)
    }

    /// Jump over a logical-assignment write when the current value decides.
    fn logical_skip_jump(&mut self, op: LogicalAssignOp, current: Reg) -> Result<usize, Decline> {
        match op {
            LogicalAssignOp::And => Ok(self.emit_jump(|target| Instr::JumpIfFalse {
                src: current,
                target,
            })),
            LogicalAssignOp::Or => Ok(self.emit_jump(|target| Instr::JumpIfTrue {
                src: current,
                target,
            })),
            LogicalAssignOp::Nullish => Ok(self.emit_jump(|target| Instr::JumpIfNotNullish {
                src: current,
                target,
            })),
        }
    }

    fn compile_template(
        &mut self,
        quasis: &[crate::JsString],
        exprs: &'a [Expr],
    ) -> Result<Reg, Decline> {
        let start = self.alloc_regs(exprs.len())?;
        for (i, expr) in exprs.iter().enumerate() {
            let reg = self.compile_expr(expr)?;
            self.emit(Instr::Mov {
                dst: start + i as u16,
                src: reg,
            });
        }
        let quasis = self.push_const(Constant::StringList(quasis.to_vec()))?;
        let dst = self.alloc_reg()?;
        self.emit(Instr::Template {
            dst,
            quasis,
            args: start,
            argc: exprs.len() as u16,
        });
        Ok(dst)
    }
}

// -- nested functions --------------------------------------------------------

fn has_rest_param(params: &[String]) -> bool {
    params.iter().any(|param| param.starts_with("..."))
}

fn has_dup_params(params: &[String]) -> bool {
    let mut seen = HashSet::new();
    params.iter().any(|param| !seen.insert(param))
}

/// Names bound at this function level that a nested callable may observe.
/// Covers exactly what hoisting declares (params, vars, lexicals, function
/// declarations): a missed name would leave a nested reader with no cell in
/// the frame environment. Over-approximation (shadowed occurrences inside
/// nested bodies) only boxes more.
fn find_captured(params: &[String], body: &FuncBody) -> HashSet<String> {
    match body {
        FuncBody::Stmts(stmts) => {
            let mut names: Vec<&str> = params.iter().map(String::as_str).collect();
            let mut vars = Vec::new();
            collect_var_names(stmts, &mut vars);
            names.extend(vars.iter().map(String::as_str));
            let mut lexicals = Vec::new();
            block_lexicals(stmts, &mut lexicals);
            names.extend(lexicals.iter().map(|(name, _)| name.as_str()));
            let mut fns = Vec::new();
            block_fn_decls(stmts, &mut fns);
            names.extend(fns.iter().map(|decl| decl.name));
            let mut heads = Vec::new();
            block_loop_heads(stmts, &mut heads);
            names.extend(heads.iter().map(String::as_str));
            names
                .into_iter()
                .filter(|name| statements_capture_identifier(stmts, name))
                .map(str::to_string)
                .collect()
        }
        FuncBody::Expr(expr) => params
            .iter()
            .filter(|param| expr_captures_identifier(expr, param))
            .cloned()
            .collect(),
    }
}

/// Compile one nested function: bytecode when supported, otherwise an
/// AST-backed constant. Either tier closes over the defining scope, whose
/// captured bindings box into environments (frame cells for function
/// scope, pushed scopes for blocks). A capture the analysis cannot box —
/// a slot in an unscoped nested block — declines the whole unit instead.
fn compile_function(
    def: FuncDef<'_>,
    outer_block: HashSet<String>,
) -> Result<FuncOutcome, Decline> {
    if def.is_async
        || def.is_generator
        || has_rest_param(&def.params)
        || has_dup_params(&def.params)
    {
        return Ok(FuncOutcome::Ast(build_ast_function(&def)));
    }
    let this_mode = if def.is_arrow {
        ThisMode::Global
    } else {
        ThisMode::Frame
    };
    let mut compiler = Compiler::for_function(outer_block, this_mode, def.is_arrow);
    match compiler.compile_unit_function(&def) {
        Ok(bytecode) => Ok(FuncOutcome::Bytecode(Rc::new(bytecode))),
        Err(Decline::Func(_)) => Ok(FuncOutcome::Ast(build_ast_function(&def))),
        Err(unit @ Decline::Unit(_)) => Err(unit),
    }
}

/// Build the AST fallback for one function, mirroring the evaluator's
/// function-creation arms. The VM closes it over the defining frame
/// environment; capture-free-ness holds because capturing functions decline
/// the whole unit (slot bindings are invisible to environment chains).
fn build_ast_function(def: &FuncDef<'_>) -> Rc<AstFunction> {
    let body = match &def.body {
        FuncBody::Stmts(stmts) => stmts.to_vec(),
        FuncBody::Expr(expr) => vec![Statement::Return(Some(Box::new((**expr).clone())))],
    };
    let uses_arguments = if def.is_arrow {
        arrow_body_references(&ExprOrBlock::Block(body.clone()), "arguments")
    } else {
        crate::parser::stmts_need_arguments(&body)
    };
    Rc::new(AstFunction {
        source_text: def.source_text.clone(),
        named_expression: def.named_expression,
        name: def.name.clone(),
        params: def.params.to_vec(),
        body: Rc::new(body),
        is_arrow: def.is_arrow,
        is_constructor: def.is_constructor,
        is_async: def.is_async,
        is_generator: def.is_generator,
        uses_arguments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_cached;

    fn compile(source: &str) -> Result<BytecodeModule, Unsupported> {
        let stmts = parse_cached(source).expect("test source must parse");
        compile_program(&stmts)
    }

    fn reason(source: &str) -> &'static str {
        match compile(source) {
            Ok(_) => "compiled",
            Err(unsupported) => unsupported.reason,
        }
    }

    #[test]
    fn stage_one_programs_compile() {
        for source in [
            "1 + 2 * 3;",
            "let x = 1; x += 2; x++;",
            "function fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); } fib(10);",
            "let t = 0; for (let i = 0; i < 10; i++) { t += i; } t;",
            "let i = 0; while (i < 5) { i++; } i;",
            "let x = 1; let y = 2; x > y ? x : y;",
            "function f(a, b) { let c = a + b; return c; } f(1, 2);",
            "let s = `hello ${1 + 2}`; s;",
            "let a = [1, 2, 3]; a[0] + a[2];",
            "Math.max(1, 2);",
            "let o = Math; o.max(3, 4);",
            "function g() { return 1; } g();",
            "const f = (x) => x * 2; f(21);",
            "new Array(3);",
            "let x = 1; x ??= 2; x;",
            "let n = 5; n--;",
            "delete Math.foo;",
            "typeof x;",
        ] {
            assert_eq!(reason(source), "compiled", "source: {source}");
        }
    }

    #[test]
    fn unsupported_constructs_decline() {
        for (source, expected) in [
            ("try { f(); } catch (e) { g(); }", "compiled"),
            ("switch (x) { case 1: y(); }", "compiled"),
            ("class C {}", "compiled"),
            ("for (let k in o) { f(k); }", "compiled"),
            ("for (const v of a) { f(v); }", "compiled"),
            ("let o = { a: 1 };", "compiled"),
            (
                "let [a] = b;",
                "destructuring iterator or name inference semantics",
            ),
            ("f(...args);", "compiled"),
            ("let a = [...b];", "compiled"),
            ("a?.b;", "compiled"),
            ("import x from 'm';", "compiled"),
            ("export default 1;", "compiled"),
            ("outer: for (;;) { break outer; }", "compiled"),
            ("async function f() {} f();", "compiled"), // per-function fallback
            ("function* g() {}", "compiled"),           // per-function fallback
            ("function f() { return arguments; }", "compiled"), // arguments objects compile
            ("let x = 10n;", "compiled"),
            ("let r = /ab+c/;", "compiled"),
        ] {
            assert_eq!(reason(source), expected, "source: {source}");
        }
    }

    #[test]
    fn function_level_captures_compile() {
        // The defining function boxes `x` into its frame environment.
        assert_eq!(
            reason("function g() { let x = 1; function f() { return x; } return f(); } g();"),
            "compiled"
        );
        // Top-level names are globals, not captures.
        assert_eq!(
            reason("let x = 1; function f() { return x; } f();"),
            "compiled"
        );
        // Nested declarations recurse through the chain, not slots.
        assert_eq!(
            reason("function g() { function f() { return f; } return f(); } g();"),
            "compiled"
        );
    }

    #[test]
    fn block_captures_box_into_pushed_scopes() {
        // Captured block bindings live in pushed runtime scopes now.
        assert_eq!(
            reason("{ let y = 1; function f() { return y; } }"),
            "compiled"
        );
        assert_eq!(
            reason("function g() { { let y = 1; function f() { return y; } } }"),
            "compiled"
        );
        assert_eq!(
            reason("function g() { for (let i = 0; i < 1; i++) { function f() { return i; } } }"),
            "compiled"
        );
        // Arrows inheriting `this` or `arguments` from a function.
        assert_eq!(
            reason("function g() { const f = () => this; return f; }"),
            "compiled"
        );
        assert_eq!(
            reason("function g() { const f = () => arguments; return f; }"),
            "compiled"
        );
        // A lexical iteration head has its own boxed scope even inside
        // surrounding blocks that need no runtime environment.
        assert_eq!(
            reason(
                "function g(o) { let r = []; { let z = 1; { for (let k in o) { r.push(() => k + z); } } } return r; }"
            ),
            "compiled"
        );
    }

    #[test]
    fn fibonacci_disassembly() {
        let module =
            compile("function fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); }")
                .unwrap();
        crate::bytecode::verify::verify_module(&module).expect("compiler output verifies");
        // Undefined, the binding name, and one hoisted function identity.
        assert_eq!(module.main.constants.len(), 3);
        let mut functions = module
            .main
            .constants
            .iter()
            .filter_map(|constant| match constant {
                Constant::Function(func) => Some(func),
                _ => None,
            });
        let first = functions.next().unwrap();
        assert!(functions.next().is_none());
        // The body loads the parameter slot and recurses through the global.
        let disassembly = first.disassemble();
        assert!(disassembly.contains("LOAD_LOCAL r0, s0"), "{disassembly}");
        assert!(disassembly.contains("CALL"), "{disassembly}");
        assert_eq!(first.parameter_count, 1);
        assert_eq!(first.slots.len(), 1);
    }

    fn nested_functions(source: &str) -> Vec<Constant> {
        let module = compile(source).expect("test source must compile");
        module
            .main
            .constants
            .iter()
            .filter(|c| matches!(c, Constant::Function(_) | Constant::AstFunction(_)))
            .cloned()
            .collect()
    }

    #[test]
    fn nested_functions_carry_arrow_and_constructor_flags() {
        let consts = nested_functions("const f = () => 1;");
        assert_eq!(consts.len(), 1);
        match &consts[0] {
            Constant::Function(code) => {
                assert!(code.is_arrow);
                assert!(!code.is_constructor);
            }
            other => panic!("expected bytecode function, got {other:?}"),
        }
        // Function declarations instantiate only at scope entry.
        let consts = nested_functions("function g(){}");
        assert_eq!(consts.len(), 1);
        for c in &consts {
            match c {
                Constant::Function(code) => {
                    assert!(!code.is_arrow);
                    assert!(code.is_constructor);
                }
                other => panic!("expected bytecode function, got {other:?}"),
            }
        }
        let consts = nested_functions("async function h(){}");
        assert_eq!(consts.len(), 1);
        assert!(consts.iter().all(|c| matches!(c, Constant::AstFunction(_))));
    }

    #[test]
    fn recursive_and_self_referencing_functions_compile() {
        // Declarations recurse through the enclosing scope in both tiers.
        let module =
            compile("function fib(n){ return n < 2 ? n : fib(n - 1) + fib(n - 2); } fib(10);")
                .expect("recursive declaration must compile");
        assert!(
            module
                .main
                .constants
                .iter()
                .any(|c| matches!(c, Constant::Function(_)))
        );
        // Named expressions resolve their own name outward (the evaluator
        // has no intermediate self-scope), so self-reference compiles too.
        compile("(function bar(){ return typeof bar; })();")
            .expect("self-referencing expression must compile");
    }
    #[test]
    fn class_super_writes_compile_to_receiver_operations_without_method_fallback() {
        let statements = crate::parser::parse_cached("class A{get x(){return 1;}set x(v){}}class B extends A{method(){super.x+=2;return super.x++;}}").unwrap();
        let module = compile_program(&statements).unwrap();
        let method = module.main.constants.iter().find_map(|constant| {
            let Constant::ClassTemplate(template) = constant else { return None; };
            let member = template.members.iter().find(|member| matches!(&member.name, ClassNameTemplate::Static(name) if name == "method"))?;
            match &module.main.constants[member.func.unwrap() as usize] {
                Constant::Function(function) => Some(function),
                _ => panic!("super writes must not use method fallback"),
            }
        }).expect("compiled method");
        assert!(
            method
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instr::SuperReference { .. }))
        );
        assert!(
            method
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instr::SetPropertyWithReceiver { .. }))
        );
        assert!(
            method
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instr::NumericUpdate { .. }))
        );
    }
    #[test]
    fn constructor_spread_and_iterator_snapshots_compile_without_fallback() {
        let module = compile("function C(a,b){this.value=a+b;}new C(...[35],7);").unwrap();
        assert!(
            module
                .main
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instr::ConstructSpread { .. }))
        );
        assert!(
            module
                .main
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instr::ExpandSpread { .. }))
        );
        assert!(module.main.constants.iter().any(|constant| matches!(constant, Constant::SpreadTemplate(entries) if entries.iter().any(|entry| entry.spread && entry.prepared))));
    }
    #[test]
    fn module_templates_preserve_utf16_requests_for_link_time_validation() {
        let module = compile(r"import '\uD800';export * from '\uDC00';")
            .expect("valid ModuleSpecifier strings");
        crate::bytecode::verify::verify_module(&module).expect("UTF-16 module templates verify");
        assert!(module.main.constants.iter().any(|constant| matches!(constant, Constant::ImportTemplate(template) if template.module.units()==[0xd800])));
        assert!(module.main.constants.iter().any(|constant| matches!(constant, Constant::ExportAllTemplate(template) if template.source.units()==[0xdc00])));
    }
}
