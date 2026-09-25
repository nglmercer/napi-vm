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
//! plain array literals (holes become undefined, like the parser)
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
    AssignOp, BinOp, ClassMember, Expr, ExprOrBlock, ForInit, LogicalAssignOp, MemberName,
    ObjectProp, Pattern, PatternKey, Statement, SwitchCase, UnOp, VarKind,
    arrow_body_references, collect_var_names, expr_captures_identifier, expr_to_pattern,
    pattern_names, statements_capture_identifier, stmts_reference,
};

use super::constants::{
    AstFunction, ClassMemberKind, ClassMemberTemplate, ClassNameTemplate, ClassTemplate,
    Constant, PropEntry, PropKind, SpreadEntry, class_key_name,
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
    match compiler.compile_top(stmts) {
        Ok(main) => Ok(BytecodeModule { main: Rc::new(main) }),
        Err(decline) => Err(Unsupported { reason: decline.reason() }),
    }
}

/// How `this` resolves in the unit being compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThisMode {
    /// Non-arrow function bodies: the frame's `this` value.
    Frame,
    /// Top level and top-level arrows: the global environment's `this`.
    Global,
    /// Arrows nested in functions: `this` would capture (Phase G).
    Reject,
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

/// A function definition to compile, borrowed from the parsed unit.
#[derive(Clone)]
struct FuncDef<'a> {
    name: Option<String>,
    params: &'a [String],
    body: FuncBody<'a>,
    is_arrow: bool,
    is_async: bool,
    is_generator: bool,
    is_constructor: bool,
}

#[derive(Clone)]
enum FuncBody<'a> {
    Stmts(&'a [Statement]),
    /// Arrow expression bodies run as `return <expr>`, like the evaluator.
    Expr(&'a Expr),
}

/// A nested-function compilation result: bytecode, or an AST fallback.
enum FuncOutcome {
    Bytecode(Rc<BytecodeFunction>),
    Ast(Rc<AstFunction>),
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
    Computed(usize),
}

/// One compile-time lexical scope: slot bindings, or the global marker.
struct Scope {
    bindings: HashMap<String, Slot>,
    /// The top-level outermost scope: names resolve to the global
    /// environment instead of slots.
    global: bool,
}

/// Break/continue patch lists for one breakable context under compilation
/// (a loop, a switch body, or a labeled non-loop statement).
#[derive(Default)]
struct LoopCtx {
    breaks: Vec<usize>,
    continues: Vec<usize>,
    labels: Vec<String>,
    kind: CtxKind,
    /// Unwind-stack length when this context opened: a break/continue
    /// targeting it duplicates the cleanups above this depth.
    try_depth: usize,
}

/// One protected region an abrupt exit must unwind: a `try` with a
/// `finally` body to inline, and/or a `for-of` iterator to close. Both
/// own a VM handler the duplication also pops.
struct UnwindCtx<'a> {
    finally: Option<&'a [Statement]>,
    close_iter: Option<Reg>,
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
    /// Whether the enclosing scope is the top level (for arrow `this`).
    enclosing_is_top: bool,
    top_level: bool,
    this_mode: ThisMode,
    is_arrow: bool,
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
            scopes: vec![Scope { bindings: HashMap::new(), global: true }],
            loops: Vec::new(),
            pending_labels: Vec::new(),
            unwind: Vec::new(),
            deferred: Vec::new(),
            next_reg: 0,
            max_reg: 0,
            outer_block: HashSet::new(),
            captured: HashSet::new(),
            enclosing_is_top: true,
            top_level: true,
            this_mode: ThisMode::Global,
            is_arrow: false,
        }
    }

    fn for_function(
        outer_block: HashSet<String>,
        enclosing_is_top: bool,
        this_mode: ThisMode,
        is_arrow: bool,
    ) -> Self {
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
            scopes: vec![Scope { bindings: HashMap::new(), global: false }],
            loops: Vec::new(),
            pending_labels: Vec::new(),
            unwind: Vec::new(),
            deferred: Vec::new(),
            next_reg: 0,
            max_reg: 0,
            outer_block,
            captured: HashSet::new(),
            enclosing_is_top,
            top_level: false,
            this_mode,
            is_arrow,
        }
    }

    fn is_arrow(&self) -> bool {
        self.is_arrow
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
    }

    fn alloc_slot(&mut self, name: &str, kind: SlotKind) -> Result<Slot, Decline> {
        let slot = u16::try_from(self.slots.len()).map_err(|_| Decline::Func("too many locals"))?;
        // Only function-root slots box: a block slot sharing a captured
        // name shadows it and stays direct. Single-scope allocation means
        // the root scope — blocks always push first.
        let captured = self.scopes.len() == 1 && self.captured.contains(name);
        self.slots.push(SlotInfo { name: name.to_string(), kind, captured });
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
        let index = self.push_const(Constant::String(value.to_string()))?;
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

    fn push_scope(&mut self) {
        self.scopes.push(Scope { bindings: HashMap::new(), global: false });
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Bind `name` in the innermost scope, allocating a fresh slot. Same-name
    /// duplicates within one scope share the slot the first declaration
    /// allocated, mirroring `declare`'s replace-in-place; the caller passes
    /// the final kind after hoist-order merging.
    fn declare_slot(&mut self, name: &str, kind: SlotKind) -> Result<Slot, Decline> {
        if let Some(slot) = self.scopes.last().and_then(|scope| scope.bindings.get(name)) {
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
    /// serves nested readers. Shadowing block slots stay direct, and names
    /// bound to slots in enclosing blocks decline the whole unit.
    fn resolve(&self, name: &str) -> Result<Binding, Decline> {
        for (index, scope) in self.scopes.iter().enumerate().rev() {
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
            Statement::VarDecl { kind: VarKind::Let, destructuring: None, name, .. } => {
                out.push((name.clone(), SlotKind::Let));
            }
            Statement::VarDecl { kind: VarKind::Const, destructuring: None, name, .. } => {
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

/// `for-in`/`for-of` head names bound directly in one block: the head
/// assigns in the enclosing scope, so a function-level head is visible to
/// (and boxable for) nested closures exactly like a `var`.
fn block_loop_heads(stmts: &[Statement], out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Statement::ForIn { name, .. } => out.push(name.clone()),
            Statement::ForOf { name, pattern, .. } => match pattern {
                Some(pattern) => out.extend(pattern_names(pattern)),
                None => out.push(name.clone()),
            },
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
    name: &'a str,
    params: &'a [String],
    body: &'a [Statement],
    is_async: bool,
    is_generator: bool,
}

fn block_fn_decls<'a>(stmts: &'a [Statement], out: &mut Vec<FnDeclRef<'a>>) {
    for stmt in stmts {
        match stmt {
            Statement::FnDecl { name, params, body, is_async, is_generator } => {
                out.push(FnDeclRef { name, params, body, is_async: *is_async, is_generator: *is_generator });
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
        if let Some(slot) = self.scopes.last().and_then(|scope| scope.bindings.get(name)) {
            return Ok(*slot);
        }
        self.declare_slot(name, kind)
    }

    /// Top-level hoisting in AST order: conditional `var` defines (a
    /// previous `eval` may own the name), lexical declares, eager functions.
    fn hoist_top(&mut self, stmts: &'a [Statement]) -> Result<(), Decline> {
        let mut vars = Vec::new();
        collect_var_names(stmts, &mut vars);
        let mut seen = HashSet::new();
        for name in vars {
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
                self.emit(Instr::DefineGlobal { name: index, src: undef, kind, initialized: false });
            }
        }
        let mut fns = Vec::new();
        block_fn_decls(stmts, &mut fns);
        for decl in fns {
            let value = self.defer_function(FuncDef {
                name: Some(decl.name.to_string()),
                params: decl.params,
                body: FuncBody::Stmts(decl.body),
                is_arrow: false,
                is_async: decl.is_async,
                is_generator: decl.is_generator,
                is_constructor: !decl.is_async && !decl.is_generator,
            })?;
            let index = self.intern_string(decl.name)?;
            self.emit(Instr::InitGlobal { name: index, src: value });
        }
        Ok(())
    }

    /// Function-level hoisting. Slots pre-initialize from their merged kind
    /// (`var` starts defined, lexicals dead), which is exactly what
    /// `hoist_vars` + `hoist_lexical` produce on a fresh frame — so only the
    /// eager function instantiations need emission.
    fn hoist_function(&mut self, params: &'a [String], body: &'a [Statement]) -> Result<(), Decline> {
        for param in params {
            self.declare_slot(param, SlotKind::Var)?;
        }
        let mut vars = Vec::new();
        collect_var_names(body, &mut vars);
        let mut seen = HashSet::new();
        for name in vars {
            if seen.insert(name.clone()) {
                // A `var` sharing a parameter's slot keeps the argument:
                // bind only when absent.
                self.declare_slot_if_absent(&name, SlotKind::Var)?;
            }
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
                name: Some(decl.name.to_string()),
                params: decl.params,
                body: FuncBody::Stmts(decl.body),
                is_arrow: false,
                is_async: decl.is_async,
                is_generator: decl.is_generator,
                is_constructor: !decl.is_async && !decl.is_generator,
            })?;
            // Captured names live in the frame environment; the slot stays
            // an untouched placeholder.
            if self.captured.contains(decl.name) {
                let index = self.intern_string(decl.name)?;
                self.emit(Instr::InitGlobal { name: index, src: value });
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
                let slot = self.declare_slot(&name, kind)?;
                self.emit(Instr::DeclareLocal { slot, kind, initialized: false });
            }
        }
        let mut fns = Vec::new();
        block_fn_decls(stmts, &mut fns);
        for decl in fns {
            let slot = self.declare_slot_if_absent(decl.name, SlotKind::Var)?;
            let value = self.defer_function(FuncDef {
                name: Some(decl.name.to_string()),
                params: decl.params,
                body: FuncBody::Stmts(decl.body),
                is_arrow: false,
                is_async: decl.is_async,
                is_generator: decl.is_generator,
                is_constructor: !decl.is_async && !decl.is_generator,
            })?;
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
        Ok(BytecodeFunction {
            name,
            code: std::mem::take(&mut self.code),
            constants: std::mem::take(&mut self.constants),
            register_count: self.max_reg,
            local_count,
            parameter_count,
            upvalue_count: 0,
            slots: std::mem::take(&mut self.slots),
            is_arrow,
            is_constructor,
        })
    }

    /// Record a nested function for compilation after this unit's own body
    /// succeeds, emitting a placeholder the post-pass overwrites.
    fn defer_function(&mut self, def: FuncDef<'a>) -> Result<Reg, Decline> {
        let dst = self.alloc_reg()?;
        let addr = self.here();
        self.emit(Instr::MakeFunction { dst, func: u16::MAX });
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
            let outcome = compile_function(item.def, outer, self.top_level)?;
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
                PatchTarget::ClassMember { tmpl, index: member } => {
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

    fn compile_top(&mut self, stmts: &'a [Statement]) -> Result<BytecodeFunction, Decline> {
        // Register zero is the program completion value.
        let completion = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov { dst: completion, src: undef });
        self.hoist_top(stmts)?;
        let checkpoint = self.checkpoint();
        self.restore(checkpoint);
        for stmt in stmts {
            let value = self.compile_stmt(stmt)?;
            if produces_completion_value(stmt) {
                self.emit(Instr::Mov { dst: completion, src: value });
            }
            self.restore(checkpoint);
        }
        self.finish_functions()?;
        self.build_function(None, 0, false, false)
    }

    fn compile_unit_function(&mut self, def: FuncDef<'a>) -> Result<BytecodeFunction, Decline> {
        let FuncDef { name, params, body, is_arrow, is_constructor, .. } = def;
        // Box before hoisting: `resolve` and slot allocation both consult
        // this set while the body compiles.
        self.captured = find_captured(params, &body);
        let parameter_count = params.len();
        match body {
            FuncBody::Stmts(stmts) => {
                self.hoist_function(params, stmts)?;
                let checkpoint = self.checkpoint();
                for stmt in stmts {
                    let _ = self.compile_stmt(stmt)?;
                    self.restore(checkpoint);
                }
            }
            FuncBody::Expr(expr) => {
                for param in params {
                    self.declare_slot(param, SlotKind::Var)?;
                }
                let value = self.compile_expr(expr)?;
                self.emit(Instr::Return { src: value });
            }
        }
        self.finish_functions()?;
        self.build_function(name, parameter_count, is_arrow, is_constructor)
    }

    // -- blocks and statements ---------------------------------------------

    /// Compile a statement list to its completion value. Every statement
    /// yields a register; only completion-producing ones update the block
    /// value, mirroring `run`.
    fn compile_block(&mut self, stmts: &'a [Statement]) -> Result<Reg, Decline> {
        let block_value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov { dst: block_value, src: undef });
        let checkpoint = self.checkpoint();
        for stmt in stmts {
            let value = self.compile_stmt(stmt)?;
            if produces_completion_value(stmt) {
                self.emit(Instr::Mov { dst: block_value, src: value });
            }
            self.restore(checkpoint);
        }
        Ok(block_value)
    }

    /// Compile statements as a block: a fresh scope with hoisting only
    /// when the block has direct lexicals — otherwise the statements run
    /// in the enclosing scope, exactly like the evaluator.
    fn compile_scoped_block(&mut self, stmts: &'a [Statement]) -> Result<Reg, Decline> {
        if !block_needs_lexical_scope(stmts) {
            return self.compile_block(stmts);
        }
        self.push_scope();
        self.hoist_block(stmts)?;
        let value = self.compile_block(stmts)?;
        self.pop_scope();
        Ok(value)
    }

    fn compile_stmt(&mut self, stmt: &'a Statement) -> Result<Reg, Decline> {
        match stmt {
            Statement::Expr(expr) => self.compile_expr(expr),
            Statement::VarDecl { kind, name, init, destructuring } => {
                self.compile_var_decl(kind.clone(), name, init.as_deref(), destructuring.as_deref())
            }
            // Hoisted functions instantiate eagerly at scope entry AND again
            // here in order: the evaluator runs `eval_stmt` on the
            // declaration both during `hoist_lexical` and when `run` reaches
            // it, so two distinct function objects exist when code between
            // the hoist and the statement captured the first.
            Statement::FnDecl { name, params, body, is_async, is_generator } => {
                let value = self.defer_function(FuncDef {
                    name: Some(name.clone()),
                    params,
                    body: FuncBody::Stmts(body),
                    is_arrow: false,
                    is_async: *is_async,
                    is_generator: *is_generator,
                    is_constructor: !is_async && !is_generator,
                })?;
                match self.resolve(name)? {
                    Binding::Slot(slot) => self.emit(Instr::InitLocal { slot, src: value }),
                    Binding::Global => {
                        let index = self.intern_string(name)?;
                        self.emit(Instr::InitGlobal { name: index, src: value });
                    }
                }
                self.load_undefined()
            }
            Statement::Declarations(inner) => {
                for stmt in inner {
                    let _ = self.compile_stmt(stmt)?;
                }
                self.load_undefined()
            }
            Statement::Block(stmts) => self.compile_scoped_block(stmts),
            Statement::If { test, then, else_ } => self.compile_if(test, then, else_.as_deref()),
            Statement::While { test, body } => self.compile_while(test, body),
            Statement::DoWhile { test, body } => self.compile_do_while(test, body),
            Statement::For { init, test, update, body } => {
                self.compile_for(init.as_deref(), test.as_deref(), update.as_deref(), body)
            }
            Statement::Break => {
                // Plain `break` targets the innermost breakable context: a
                // loop, a switch, or a labeled block.
                let depth = match self.loops.iter().next_back() {
                    Some(ctx) => ctx.try_depth,
                    None => return Err(Decline::Func("break outside loop")),
                };
                self.duplicate_unwind(depth)?;
                let addr = self.emit_jump(|target| Instr::Jump { target });
                match self.loops.iter_mut().next_back() {
                    Some(ctx) => ctx.breaks.push(addr),
                    None => return Err(Decline::Func("break outside loop")),
                }
                self.load_undefined()
            }
            Statement::Continue => {
                // Plain `continue` targets the innermost loop, skipping over
                // any switch or labeled-block contexts (`continue` inside a
                // switch applies to the enclosing loop).
                let depth = match self
                    .loops
                    .iter()
                    .rev()
                    .find(|ctx| ctx.kind == CtxKind::Loop)
                {
                    Some(ctx) => ctx.try_depth,
                    None => return Err(Decline::Func("continue outside loop")),
                };
                self.duplicate_unwind(depth)?;
                let addr = self.emit_jump(|target| Instr::Jump { target });
                match self
                    .loops
                    .iter_mut()
                    .rev()
                    .find(|ctx| ctx.kind == CtxKind::Loop)
                {
                    Some(ctx) => ctx.continues.push(addr),
                    None => return Err(Decline::Func("continue outside loop")),
                }
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
            Statement::Try { body, catch, finally } => self.compile_try(body, catch, finally),
            Statement::Switch { disc, cases } => self.compile_switch(disc, cases),
            Statement::ClassDecl { name, superclass, body } => {
                let value = self.compile_class(name, None, superclass.as_deref(), body)?;
                match self.resolve(name)? {
                    Binding::Slot(slot) => self.emit(Instr::InitLocal { slot, src: value }),
                    Binding::Global => {
                        let index = self.intern_string(name)?;
                        self.emit(Instr::InitGlobal { name: index, src: value });
                    }
                }
                self.load_undefined()
            }
            Statement::ForIn { name, obj, body } => self.compile_for_in(name, obj, body),
            Statement::ForOf { name, pattern, iter, body, is_await } => {
                self.compile_for_of(name, pattern, iter, body, *is_await)
            }
            Statement::Labeled { label, body } => self.compile_labeled(label, body),
            Statement::LabeledBreak(label) => {
                let depth = match self
                    .loops
                    .iter()
                    .rev()
                    .find(|ctx| ctx.labels.iter().any(|known| known == label))
                {
                    Some(ctx) => ctx.try_depth,
                    None => return Err(Decline::Func("unresolved break label")),
                };
                self.duplicate_unwind(depth)?;
                let addr = self.emit_jump(|target| Instr::Jump { target });
                match self
                    .loops
                    .iter_mut()
                    .rev()
                    .find(|ctx| ctx.labels.iter().any(|known| known == label))
                {
                    Some(ctx) => ctx.breaks.push(addr),
                    None => return Err(Decline::Func("unresolved break label")),
                }
                self.load_undefined()
            }
            Statement::LabeledContinue(label) => {
                // `continue label` targets the innermost context carrying
                // that label, which must be a loop; anything else (or no
                // match at all) is a compile error the parser normally
                // rejects ahead of us.
                let mut depth = None;
                for ctx in self.loops.iter().rev() {
                    if ctx.labels.iter().any(|known| known == label) {
                        if ctx.kind == CtxKind::Loop {
                            depth = Some(ctx.try_depth);
                        }
                        break;
                    }
                }
                let Some(depth) = depth else {
                    return Err(Decline::Func("unresolved continue label"));
                };
                self.duplicate_unwind(depth)?;
                let addr = self.emit_jump(|target| Instr::Jump { target });
                let mut target = None;
                for ctx in self.loops.iter_mut().rev() {
                    if ctx.labels.iter().any(|known| known == label) {
                        if ctx.kind == CtxKind::Loop {
                            target = Some(ctx);
                        }
                        break;
                    }
                }
                match target {
                    Some(ctx) => ctx.continues.push(addr),
                    None => return Err(Decline::Func("unresolved continue label")),
                }
                self.load_undefined()
            }
            Statement::Import { .. } => Err(Decline::Func("modules need Phase G")),
            Statement::ExportDefault(_)
            | Statement::ExportNamed { .. }
            | Statement::ExportAll { .. } => Err(Decline::Func("modules need Phase G")),
        }
    }
}

/// Whether a pattern binds fresh declaration names or assigns through
/// existing bindings. `is_var` selects store-vs-initialize for declarations.
#[derive(Clone, Copy)]
enum DestructureMode {
    Decl { is_var: bool },
    Assign,
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
            (DestructureMode::Assign, Binding::Slot(slot))
            | (DestructureMode::Decl { is_var: true }, Binding::Slot(slot)) => {
                self.emit(Instr::StoreLocal { slot, src });
            }
            (DestructureMode::Assign, Binding::Global)
            | (DestructureMode::Decl { is_var: true }, Binding::Global) => {
                let index = self.intern_string(name)?;
                self.emit(Instr::StoreGlobal { name: index, src });
            }
            (DestructureMode::Decl { .. }, Binding::Slot(slot)) => {
                self.emit(Instr::InitLocal { slot, src });
            }
            (DestructureMode::Decl { .. }, Binding::Global) => {
                let index = self.intern_string(name)?;
                self.emit(Instr::InitGlobal { name: index, src });
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
        match pat {
            Pattern::Ident(name) => self.compile_pattern_ident(name, val, mode),
            Pattern::Member { object, property } => {
                if matches!(object.as_ref(), Expr::Super) {
                    // The reference fails before the property evaluates.
                    return self.raise_bare_super().map(|_| ());
                }
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                self.emit(Instr::SetProp { obj, key, val });
                Ok(())
            }
            Pattern::Array(elements) => {
                let arr = self.alloc_reg()?;
                self.emit(Instr::ToDestructArray { dst: arr, src: val });
                for (index, elem) in elements.iter().enumerate() {
                    if let Pattern::Rest(inner) = elem {
                        let from = u16::try_from(index)
                            .map_err(|_| Decline::Func("code too large"))?;
                        let rest = self.alloc_reg()?;
                        self.emit(Instr::RestArray { dst: rest, src: arr, from });
                        return self.compile_destructure(inner, rest, mode);
                    }
                    let position = self.intern_number(index as f64)?;
                    let key = self.load_const(position)?;
                    let found = self.alloc_reg()?;
                    self.emit(Instr::GetProp { dst: found, obj: arr, key });
                    self.compile_destructure(elem, found, mode)?;
                }
                Ok(())
            }
            Pattern::Object(props) => {
                let keys = self.alloc_reg()?;
                self.emit(Instr::CheckDestructObject { dst: keys, src: val });
                let mut taken = Vec::new();
                for (key, sub) in props {
                    if let PatternKey::Name(name) = key
                        && name == "..."
                        && let Some(Pattern::Rest(target)) = sub
                    {
                        let template = taken
                            .iter()
                            .map(|reg| SpreadEntry { spread: false, reg: *reg })
                            .collect();
                        let tmpl = self.push_const(Constant::SpreadTemplate(template))?;
                        let taken_array = self.alloc_reg()?;
                        self.emit(Instr::BuildArray { dst: taken_array, tmpl });
                        let rest = self.alloc_reg()?;
                        self.emit(Instr::RestObject {
                            dst: rest,
                            src: val,
                            keys,
                            taken: taken_array,
                        });
                        self.compile_destructure(target, rest, mode)?;
                        continue;
                    }
                    let key_reg = match key {
                        PatternKey::Name(name) => {
                            let index = self.intern_string(name)?;
                            self.load_const(index)?
                        }
                        PatternKey::Computed(expr) => self.compile_expr(expr)?,
                    };
                    taken.push(key_reg);
                    let found = self.alloc_reg()?;
                    self.emit(Instr::GetProp { dst: found, obj: val, key: key_reg });
                    match sub {
                        Some(next) => self.compile_destructure(next, found, mode)?,
                        None => match key {
                            PatternKey::Name(name) => {
                                self.compile_pattern_ident(name, found, mode)?;
                            }
                            PatternKey::Computed(_) => {
                                return Err(Decline::Func("invalid destructuring target"));
                            }
                        },
                    }
                }
                Ok(())
            }
            // A rest element only binds at the top of an array pattern; a
            // bare one anywhere else falls through, like the evaluator.
            Pattern::Rest(_) => Ok(()),
            Pattern::Default(inner, default) => {
                let value = self.alloc_reg()?;
                self.emit(Instr::Mov { dst: value, src: val });
                let has = self.emit_jump(|target| Instr::JumpIfNotNullish { src: val, target });
                let fallback = self.compile_expr(default)?;
                self.emit(Instr::Mov { dst: value, src: fallback });
                self.patch_jump(has, self.here())?;
                self.compile_destructure(inner, value, mode)
            }
        }
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
    ) -> Result<Reg, Decline> {
        let snapshot = self.live_block_names();
        let superclass = superclass.map(|expr| self.compile_expr(expr)).transpose()?;

        let mut members = Vec::new();
        let mut blocks = Vec::new();
        let mut ctor_computed_keys = Vec::new();
        let mut deferred = Vec::new();
        // Instance field keys in field order: a static name, or the index
        // into `ctor_computed_keys` holding the evaluated key.
        let mut instance_fields: Vec<(FieldKey, Option<&'a Expr>)> = Vec::new();
        let mut ctor: Option<OwnedCtor<'a>> = None;

        for member in body {
            match member {
                ClassMember::Method {
                    name: member_name,
                    is_static: st,
                    params,
                    body: method_body,
                    is_async,
                    is_generator,
                } => {
                    let template = self.compile_member_name(member_name)?;
                    // Only a written-out `constructor` is the constructor,
                    // like the evaluator; a computed "constructor" stays a
                    // plain method.
                    let is_ctor = !st
                        && matches!(member_name, MemberName::Static(n) if n == "constructor");
                    if is_ctor {
                        ctor = Some(OwnedCtor { params, body: method_body });
                        continue;
                    }
                    let display = match &template {
                        ClassNameTemplate::Static(key) => Some(key.clone()),
                        // The builder names computed members once the key
                        // value is known.
                        ClassNameTemplate::Computed(_) => None,
                    };
                    deferred.push((
                        PatchTarget::ClassMember { tmpl: u16::MAX, index: members.len() },
                        FuncDef {
                            name: display,
                            params,
                            body: FuncBody::Stmts(method_body),
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
                        value: None,
                    });
                }
                ClassMember::Field { name: field_name, is_static: st, init } => {
                    let template = self.compile_member_name(field_name)?;
                    if *st {
                        let value = match init {
                            Some(expr) => self.compile_expr(expr)?,
                            None => self.load_undefined()?,
                        };
                        members.push(ClassMemberTemplate {
                            kind: ClassMemberKind::Field,
                            is_static: true,
                            name: template,
                            func: None,
                            value: Some(value),
                        });
                    } else {
                        let key = match template {
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
                ClassMember::Getter { name: member_name, is_static: st, body: getter_body } => {
                    let template = self.compile_member_name(member_name)?;
                    let display = match &template {
                        ClassNameTemplate::Static(key) => Some(format!("get {key}")),
                        ClassNameTemplate::Computed(_) => None,
                    };
                    deferred.push((
                        PatchTarget::ClassMember { tmpl: u16::MAX, index: members.len() },
                        FuncDef {
                            name: display,
                            params: &[],
                            body: FuncBody::Stmts(getter_body),
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
                        value: None,
                    });
                }
                ClassMember::Setter {
                    name: member_name,
                    param,
                    is_static: st,
                    body: setter_body,
                } => {
                    let template = self.compile_member_name(member_name)?;
                    let display = match &template {
                        ClassNameTemplate::Static(key) => Some(format!("set {key}")),
                        ClassNameTemplate::Computed(_) => None,
                    };
                    deferred.push((
                        PatchTarget::ClassMember { tmpl: u16::MAX, index: members.len() },
                        FuncDef {
                            name: display,
                            params: std::slice::from_ref(param),
                            body: FuncBody::Stmts(setter_body),
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
                        value: None,
                    });
                }
                ClassMember::StaticBlock { body: block_body } => {
                    // Static blocks always run on the AST tier: the builder
                    // hands their bodies to assembly, like the evaluator.
                    let ast = build_ast_function(&FuncDef {
                        name: None,
                        params: &[],
                        body: FuncBody::Stmts(block_body),
                        is_arrow: false,
                        is_async: false,
                        is_generator: false,
                        is_constructor: false,
                    });
                    blocks.push(self.push_const(Constant::AstFunction(ast))?);
                }
            }
        }

        let (ctor_params, ctor_body) =
            Self::class_ctor_body(superclass.is_some(), ctor, &instance_fields);
        let ctor_length =
            ctor_params.iter().take_while(|param| !param.starts_with("...")).count();
        deferred.push((
            PatchTarget::ClassCtor { tmpl: u16::MAX },
            FuncDef {
                // The evaluator names the constructor after the class.
                name: Some(name.to_string()),
                params: ctor_params,
                body: FuncBody::Stmts(ctor_body),
                is_arrow: false,
                is_async: false,
                is_generator: false,
                is_constructor: false,
            },
        ));

        let tmpl = self.push_const(Constant::ClassTemplate(ClassTemplate {
            name: name.to_string(),
            expr_name,
            superclass,
            ctor_func: u16::MAX,
            ctor_length,
            ctor_computed_keys,
            members,
            blocks,
        }))?;
        let dst = self.alloc_reg()?;
        self.emit(Instr::BuildClass { dst, tmpl });
        for (mut patch, def) in deferred {
            match &mut patch {
                PatchTarget::ClassCtor { tmpl: slot }
                | PatchTarget::ClassMember { tmpl: slot, .. } => *slot = tmpl,
                PatchTarget::MakeFunction { .. } => unreachable!("class defers class patches"),
            }
            self.deferred.push(Deferred { patch, def, snapshot: snapshot.clone() });
        }
        Ok(dst)
    }

    /// A member name as written, or compiled and coerced when computed.
    /// Coercing now matches the evaluator, which keys members at class
    /// definition time rather than at first use.
    fn compile_member_name(&mut self, name: &'a MemberName) -> Result<ClassNameTemplate, Decline> {
        match name {
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
    /// `constructor(...args) { super(...args); }`, or the empty default.
    /// Owned bodies promote to the compilation lifetime, like converted
    /// destructuring patterns.
    fn class_ctor_body(
        is_derived: bool,
        ctor: Option<OwnedCtor<'a>>,
        instance_fields: &[(FieldKey, Option<&'a Expr>)],
    ) -> (&'a [String], &'a [Statement]) {
        let (params, body): (&'a [String], &'a [Statement]) = match ctor {
            Some(own) => (own.params, own.body),
            None if is_derived => {
                let params: &'a [String] =
                    Box::leak(Box::new(vec!["...args".to_string()]));
                let body: &'a [Statement] = Box::leak(Box::new(vec![Statement::Expr(
                    Expr::Call {
                        callee: Box::new(Expr::Super),
                        args: vec![Expr::Spread(Box::new(Expr::Identifier(
                            "args".to_string(),
                        )))],
                    },
                )]));
                (params, body)
            }
            None => (&[], &[]),
        };
        if instance_fields.is_empty() {
            return (params, body);
        }
        let mut full = Vec::with_capacity(instance_fields.len() + body.len());
        for (key, init) in instance_fields {
            let (property, computed) = match key {
                FieldKey::Static(field) => (Expr::String(field.clone()), false),
                // Evaluated at class definition time; the builder binds the
                // value into the constructor's scope under this key.
                FieldKey::Computed(index) => {
                    (Expr::Identifier(class_key_name(*index)), true)
                }
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
        full.extend(body.iter().cloned());
        (params, Box::leak(full.into_boxed_slice()))
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
                DestructureMode::Decl { is_var: matches!(kind, VarKind::Var) },
            )?;
            return self.load_undefined();
        }
        // `var` declarators assign through the scope chain (bare ones only
        // touch dead-zone-merged bindings); `let`/`const` declarators
        // initialize in place, defaulting to `undefined`.
        match (kind, self.resolve(name)?) {
            (VarKind::Var, Binding::Slot(slot)) => match init {
                Some(value) => {
                    let src = self.compile_expr(value)?;
                    self.emit(Instr::StoreLocal { slot, src });
                }
                None => self.emit(Instr::BareVarLocal { slot }),
            },
            (VarKind::Var, Binding::Global) => match init {
                Some(value) => {
                    let src = self.compile_expr(value)?;
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
                    Some(value) => self.compile_expr(value)?,
                    None => self.load_undefined()?,
                };
                self.emit(Instr::InitLocal { slot, src });
            }
            (VarKind::Let | VarKind::Const, Binding::Global) => {
                let src = match init {
                    Some(value) => self.compile_expr(value)?,
                    None => self.load_undefined()?,
                };
                let index = self.intern_string(name)?;
                self.emit(Instr::InitGlobal { name: index, src });
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
        let join = self.alloc_reg()?;
        let else_jump = self.emit_jump(|target| Instr::JumpIfFalse { src: cond, target });
        let then_value = self.compile_scoped_block(then)?;
        self.emit(Instr::Mov { dst: join, src: then_value });
        let end_jump = self.emit_jump(|target| Instr::Jump { target });
        self.patch_jump(else_jump, self.here())?;
        match else_ {
            Some(stmts) => {
                let else_value = self.compile_scoped_block(stmts)?;
                self.emit(Instr::Mov { dst: join, src: else_value });
            }
            None => {
                let undef = self.load_undefined()?;
                self.emit(Instr::Mov { dst: join, src: undef });
            }
        }
        self.patch_jump(end_jump, self.here())?;
        Ok(join)
    }

    /// Patch a finished loop's break/continue lists to their targets.
    fn finish_loop(&mut self, ctx: LoopCtx, continue_target: usize, end: usize) -> Result<(), Decline> {
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
    /// bodies innermost-first, then one handler pop per crossed region.
    /// `return` needs none of this: it unwinds as an error the VM handlers
    /// intercept.
    fn duplicate_unwind(&mut self, depth: usize) -> Result<(), Decline> {
        let mut index = self.unwind.len();
        while index > depth {
            index -= 1;
            let (finally, close_iter) = {
                let ctx = &self.unwind[index];
                (ctx.finally, ctx.close_iter)
            };
            if let Some(iter) = close_iter {
                self.emit(Instr::CloseIterator { src: iter });
            }
            if let Some(body) = finally {
                // The inline copy runs outside its own region: compile it
                // with the crossed regions (including this one) truncated
                // away, so abrupt exits inside it do not unwind twice.
                let mut crossed = self.unwind.split_off(index);
                self.compile_finally_body(body)?;
                self.unwind.append(&mut crossed);
            }
            self.emit(Instr::PopHandler);
        }
        Ok(())
    }

    /// One `finally` copy as a block; the block value is discarded, like
    /// the evaluator's, while abrupt exits propagate.
    fn compile_finally_body(&mut self, body: &'a [Statement]) -> Result<(), Decline> {
        let _ = self.compile_scoped_block(body)?;
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
        catch: &'a Option<(String, Vec<Statement>)>,
        finally: &'a Option<Vec<Statement>>,
    ) -> Result<Reg, Decline> {
        let value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov { dst: value, src: undef });
        if catch.is_none() && finally.is_none() {
            let body_value = self.compile_scoped_block(body)?;
            self.emit(Instr::Mov { dst: value, src: body_value });
            return Ok(value);
        }
        let err = self.alloc_reg()?;
        let mut finally_addr = None;
        let mut catch_addr = None;
        if finally.is_some() {
            finally_addr =
                Some(self.emit_jump(|target| Instr::PushFinally { target, dst: err }));
            self.unwind.push(UnwindCtx { finally: finally.as_deref(), close_iter: None });
        }
        if catch.is_some() {
            catch_addr = Some(self.emit_jump(|target| Instr::PushCatch { target, dst: err }));
            self.unwind.push(UnwindCtx { finally: None, close_iter: None });
        }
        let body_value = self.compile_scoped_block(body)?;
        self.emit(Instr::Mov { dst: value, src: body_value });
        if catch.is_some() {
            self.emit(Instr::PopHandler);
            self.unwind.pop().ok_or(Decline::Func("unwind stack underflow"))?;
        }
        let normal = self.emit_jump(|target| Instr::Jump { target });
        let catch_pad = self.here();
        if let Some((param, catch_body)) = catch {
            self.push_scope();
            let slot = self.declare_slot(param, SlotKind::Var)?;
            self.emit(Instr::InitLocal { slot, src: err });
            self.hoist_block(catch_body)?;
            let catch_value = self.compile_block(catch_body)?;
            self.emit(Instr::Mov { dst: value, src: catch_value });
            self.pop_scope();
        }
        let finally_run = self.here();
        if finally.is_some() {
            self.emit(Instr::PopHandler);
            self.unwind.pop().ok_or(Decline::Func("unwind stack underflow"))?;
            if let Some(body) = finally.as_deref() {
                self.compile_finally_body(body)?;
            }
        }
        let end = self.emit_jump(|target| Instr::Jump { target });
        let finally_pad = self.here();
        if let Some(body) = finally.as_deref() {
            self.compile_finally_body(body)?;
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
        Ok(value)
    }

    /// `label: body`. A labeled loop hands the label to the loop's
    /// context (so `break label`/`continue label` reach it); any other
    /// labeled statement runs inside a `LabelBlock` context whose breaks
    /// land just past it with the statement value reset to `undefined`,
    /// matching the evaluator's `LabeledBreak` propagation.
    fn compile_labeled(
        &mut self,
        label: &'a str,
        body: &'a Statement,
    ) -> Result<Reg, Decline> {
        // Only a directly-wrapped loop takes the label (the evaluator's
        // loops likewise take a single pending label on entry); an outer
        // label of a nested chain stays a `LabelBlock`, so breaking to it
        // discards the loop value exactly like the evaluator.
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
        self.loops.push(LoopCtx {
            labels: vec![label.to_string()],
            kind: CtxKind::LabelBlock,
            try_depth: self.unwind.len(),
            ..Default::default()
        });
        let value = self.compile_stmt(body)?;
        let ctx = self
            .loops
            .pop()
            .ok_or(Decline::Func("loop stack underflow"))?;
        debug_assert!(ctx.continues.is_empty());
        if !ctx.breaks.is_empty() {
            let over = self.emit_jump(|target| Instr::Jump { target });
            let taken = self.here();
            let undef = self.load_undefined()?;
            self.emit(Instr::Mov { dst: value, src: undef });
            let end = self.here();
            for addr in ctx.breaks {
                self.patch_jump(addr, taken)?;
            }
            self.patch_jump(over, end)?;
        }
        Ok(value)
    }

    /// Prepare a `for-in`/`for-of` head name. The parser erases the head
    /// kind, and the evaluator assigns (never re-declares) per iteration:
    /// top level writes the global binding, function bodies reuse or
    /// create the function-scope slot, nested blocks shadow it fresh.
    fn prepare_for_head(&mut self, name: &str) -> Result<ForHead, Decline> {
        if self.top_level {
            return Ok(ForHead::Global(self.intern_string(name)?));
        }
        if self.scopes.len() == 1 {
            let slot = self.declare_slot_if_absent(name, SlotKind::Var)?;
            // A captured head lives in the frame environment, like any
            // other captured function-scope name.
            if self.captured.contains(name) {
                return Ok(ForHead::Global(self.intern_string(name)?));
            }
            return Ok(ForHead::Slot(slot));
        }
        Ok(ForHead::Slot(self.declare_slot(name, SlotKind::Var)?))
    }

    /// One head write per iteration: an unchecked initialize, matching the
    /// evaluator's kind- and zone-ignoring head assignment.
    fn bind_for_head(&mut self, head: &ForHead, src: Reg) {
        match head {
            ForHead::Slot(slot) => self.emit(Instr::InitLocal { slot: *slot, src }),
            ForHead::Global(name) => self.emit(Instr::InitGlobal { name: *name, src }),
        }
    }

    /// `for (name in obj)`. Keys snapshot once up front; each iteration
    /// binds the key and runs the body, like the evaluator.
    fn compile_for_in(
        &mut self,
        name: &'a str,
        obj: &'a Expr,
        body: &'a [Statement],
    ) -> Result<Reg, Decline> {
        let head = self.prepare_for_head(name)?;
        let source = self.compile_expr(obj)?;
        let keys = self.alloc_reg()?;
        self.emit(Instr::EnumKeys { dst: keys, src: source });
        let length_key = self.intern_string("length")?;
        let length_key = self.load_const(length_key)?;
        let len = self.alloc_reg()?;
        self.emit(Instr::GetProp { dst: len, obj: keys, key: length_key });
        let zero = self.intern_number(0.0)?;
        let idx = self.load_const(zero)?;
        let one = self.intern_number(1.0)?;
        let one = self.load_const(one)?;
        let loop_value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov { dst: loop_value, src: undef });
        let top = self.here();
        self.emit(Instr::LoopHead);
        let cond = self.alloc_reg()?;
        self.emit(Instr::Binary { dst: cond, op: BinOp::Lt, lhs: idx, rhs: len });
        let end_jump = self.emit_jump(|target| Instr::JumpIfFalse { src: cond, target });
        self.loops.push(LoopCtx {
            labels: std::mem::take(&mut self.pending_labels),
            try_depth: self.unwind.len(),
            ..Default::default()
        });
        let key = self.alloc_reg()?;
        self.emit(Instr::GetProp { dst: key, obj: keys, key: idx });
        self.bind_for_head(&head, key);
        let body_value = self.compile_scoped_block(body)?;
        self.emit(Instr::Mov { dst: loop_value, src: body_value });
        let increment = self.here();
        let next = self.alloc_reg()?;
        self.emit(Instr::Binary { dst: next, op: BinOp::Add, lhs: idx, rhs: one });
        self.emit(Instr::Mov { dst: idx, src: next });
        self.emit(Instr::Jump { target: addr_target(top)? });
        let ctx = self.loops.pop().ok_or(Decline::Func("loop stack underflow"))?;
        let end = self.here();
        self.patch_jump(end_jump, end)?;
        self.finish_loop(ctx, increment, end)?;
        Ok(loop_value)
    }

    /// `for (name of iter)` (and pattern heads, destructured per
    /// iteration). Early exits close the iterator: `break` through a close
    /// pad, unwinding through a finally handler; exhaustion and `continue`
    /// do not close.
    fn compile_for_of(
        &mut self,
        name: &'a str,
        pattern: &'a Option<Box<Pattern>>,
        iter: &'a Expr,
        body: &'a [Statement],
        is_await: bool,
    ) -> Result<Reg, Decline> {
        if is_await {
            return Err(Decline::Func("async needs Phase G"));
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
        self.emit(Instr::ForOfInit { iter: iterator, next, src: source });
        let loop_value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov { dst: loop_value, src: undef });
        let scratch = self.alloc_reg()?;
        let unwind_pad = self.emit_jump(|target| Instr::PushFinally { target, dst: scratch });
        self.unwind.push(UnwindCtx { finally: None, close_iter: Some(iterator) });
        self.loops.push(LoopCtx {
            labels: std::mem::take(&mut self.pending_labels),
            try_depth: self.unwind.len(),
            ..Default::default()
        });
        let top = self.here();
        self.emit(Instr::LoopHead);
        let done = self.alloc_reg()?;
        let yielded = self.alloc_reg()?;
        self.emit(Instr::IterNext { done, value: yielded, iter: iterator, next });
        let exhausted = self.emit_jump(|target| Instr::JumpIfTrue { src: done, target });
        match (&head, pattern) {
            (Some(head), None) => self.bind_for_head(head, yielded),
            (None, Some(pattern)) => {
                self.compile_destructure(
                    pattern,
                    yielded,
                    DestructureMode::Decl { is_var: false },
                )?;
            }
            _ => return Err(Decline::Func("bad for-of head")),
        }
        let body_value = self.compile_scoped_block(body)?;
        self.emit(Instr::Mov { dst: loop_value, src: body_value });
        self.emit(Instr::Jump { target: addr_target(top)? });
        let ctx = self.loops.pop().ok_or(Decline::Func("loop stack underflow"))?;
        self.unwind.pop().ok_or(Decline::Func("unwind stack underflow"))?;
        let drained = self.here();
        self.emit(Instr::PopHandler);
        let end_jump = self.emit_jump(|target| Instr::Jump { target });
        let close_pad = self.here();
        self.emit(Instr::CloseIterator { src: iterator });
        self.emit(Instr::PopHandler);
        let over_pad = self.emit_jump(|target| Instr::Jump { target });
        self.patch_jump(unwind_pad, self.here())?;
        self.emit(Instr::CloseIterator { src: iterator });
        self.emit(Instr::Rethrow);
        let end = self.here();
        self.patch_jump(exhausted, drained)?;
        self.patch_jump(end_jump, end)?;
        self.patch_jump(over_pad, end)?;
        for addr in ctx.continues {
            self.patch_jump(addr, top)?;
        }
        for addr in ctx.breaks {
            self.patch_jump(addr, close_pad)?;
        }
        Ok(loop_value)
    }

    /// `switch (disc) { ... }`. One scope hosts every case's lexical hoists
    /// (shared switch scope, matching the evaluator); strict-equality tests
    /// dispatch to case bodies in order with fallthrough, `break` exits the
    /// switch, and a completed switch evaluates to `undefined`.
    fn compile_switch(
        &mut self,
        disc: &'a Expr,
        cases: &'a [SwitchCase],
    ) -> Result<Reg, Decline> {
        let scrutinee = self.compile_expr(disc)?;
        self.push_scope();
        for case in cases {
            self.hoist_block(&case.body)?;
        }
        let value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov { dst: value, src: undef });
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
            try_depth: self.unwind.len(),
            ..Default::default()
        });
        let mut bodies = Vec::with_capacity(cases.len());
        for case in cases {
            bodies.push(self.here());
            let case_value = self.compile_block(&case.body)?;
            self.emit(Instr::Mov { dst: value, src: case_value });
        }
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
        self.pop_scope();
        Ok(value)
    }

    fn compile_while(&mut self, test: &'a Expr, body: &'a [Statement]) -> Result<Reg, Decline> {
        let loop_value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov { dst: loop_value, src: undef });
        // Budget first, then the test — the evaluator's order — so a
        // `continue` re-entering here consumes exactly once per iteration.
        let test_addr = self.here();
        self.emit(Instr::LoopHead);
        let cond = self.compile_expr(test)?;
        let end_jump = self.emit_jump(|target| Instr::JumpIfFalse { src: cond, target });
        self.loops.push(LoopCtx {
            labels: std::mem::take(&mut self.pending_labels),
            try_depth: self.unwind.len(),
            ..Default::default()
        });
        let body_value = self.compile_scoped_block(body)?;
        self.emit(Instr::Mov { dst: loop_value, src: body_value });
        self.emit(Instr::Jump { target: addr_target(test_addr)? });
        let ctx = self.loops.pop().ok_or(Decline::Func("loop stack underflow"))?;
        let end = self.here();
        self.patch_jump(end_jump, end)?;
        // `break`/`continue` jump over the value move, so neither updates
        // the loop's completion value — matching the evaluator.
        self.finish_loop(ctx, test_addr, end)?;
        Ok(loop_value)
    }

    fn compile_do_while(&mut self, test: &'a Expr, body: &'a [Statement]) -> Result<Reg, Decline> {
        let loop_value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov { dst: loop_value, src: undef });
        let body_addr = self.here();
        self.emit(Instr::LoopHead);
        self.loops.push(LoopCtx {
            labels: std::mem::take(&mut self.pending_labels),
            try_depth: self.unwind.len(),
            ..Default::default()
        });
        let body_value = self.compile_scoped_block(body)?;
        self.emit(Instr::Mov { dst: loop_value, src: body_value });
        // A `continue` lands here, after the body: the test runs without
        // consuming the budget a second time, like the evaluator.
        let test_addr = self.here();
        let cond = self.compile_expr(test)?;
        self.emit(Instr::JumpIfTrue { src: cond, target: addr_target(body_addr)? });
        let ctx = self.loops.pop().ok_or(Decline::Func("loop stack underflow"))?;
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
        self.push_scope();
        if let Some(init) = init {
            self.compile_for_init(init)?;
        }
        let loop_value = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov { dst: loop_value, src: undef });
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
            try_depth: self.unwind.len(),
            ..Default::default()
        });
        let body_value = self.compile_scoped_block(body)?;
        self.emit(Instr::Mov { dst: loop_value, src: body_value });
        // A `continue` runs the update, then the budgeted test.
        let update_addr = self.here();
        if let Some(update) = update {
            let _ = self.compile_expr(update)?;
        }
        self.emit(Instr::Jump { target: addr_target(test_addr)? });
        let ctx = self.loops.pop().ok_or(Decline::Func("loop stack underflow"))?;
        let end = self.here();
        if let Some(addr) = end_jump {
            self.patch_jump(addr, end)?;
        }
        self.finish_loop(ctx, update_addr, end)?;
        self.pop_scope();
        Ok(loop_value)
    }

    /// Plain identifier declarators in a `for` head, shared by `Var`
    /// heads and the trailing declarators of pattern heads.
    fn compile_for_decls(
        &mut self,
        kind: &VarKind,
        decls: &'a [(String, Option<Expr>)],
    ) -> Result<(), Decline> {
        for (name, init) in decls {
            match kind {
                // Head `var`s live in the function scope (hoisted);
                // the head only assigns, defaulting to `undefined`
                // even when the declarator has no initializer.
                VarKind::Var => {
                    let src = match init {
                        Some(value) => self.compile_expr(value)?,
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
                    let slot_kind =
                        if matches!(kind, VarKind::Const) { SlotKind::Const } else { SlotKind::Let };
                    let slot = self.declare_slot(name, slot_kind)?;
                    self.emit(Instr::DeclareLocal { slot, kind: slot_kind, initialized: false });
                    let src = match init {
                        Some(value) => self.compile_expr(value)?,
                        None => self.load_undefined()?,
                    };
                    self.emit(Instr::InitLocal { slot, src });
                }
            }
        }
        Ok(())
    }

    fn compile_for_init(&mut self, init: &'a ForInit) -> Result<(), Decline> {
        match init {
            ForInit::Var { kind, decls } => self.compile_for_decls(kind, decls),
            ForInit::Pattern { kind, pattern, init, trailing } => {
                let src = self.compile_expr(init)?;
                if !matches!(kind, VarKind::Var) {
                    let slot_kind = if matches!(kind, VarKind::Const) {
                        SlotKind::Const
                    } else {
                        SlotKind::Let
                    };
                    for bound in pattern_names(pattern) {
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
                    DestructureMode::Decl { is_var: matches!(kind, VarKind::Var) },
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
        match expr {
            Expr::Number(value) => {
                let index = self.intern_number(*value)?;
                self.load_const(index)
            }
            Expr::String(value) => {
                let index = self.intern_string(value)?;
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
            Expr::Undefined => self.load_undefined(),
            Expr::Identifier(name) => self.compile_identifier(name),
            Expr::Array(items) => self.compile_array(items),
            Expr::Binary { op, left, right } => self.compile_binary(*op, left, right),
            Expr::Unary { op, operand, prefix } => self.compile_unary(*op, operand, *prefix),
            Expr::Call { callee, args } => self.compile_call(callee, args),
            Expr::Member { object, property, .. } => {
                if matches!(object.as_ref(), Expr::Super) {
                    let key = self.compile_expr(property)?;
                    let dst = self.alloc_reg()?;
                    self.emit(Instr::SuperMember { dst, key });
                    return Ok(dst);
                }
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::GetProp { dst, obj, key });
                Ok(dst)
            }
            Expr::Assignment { target, op, value } => self.compile_assignment(target, *op, value),
            Expr::LogicalAssignment { target, op, value } => {
                self.compile_logical_assignment(target, *op, value)
            }
            Expr::Conditional { test, consequent, alternate } => {
                let cond = self.compile_expr(test)?;
                let join = self.alloc_reg()?;
                let else_jump =
                    self.emit_jump(|target| Instr::JumpIfFalse { src: cond, target });
                let then_value = self.compile_expr(consequent)?;
                self.emit(Instr::Mov { dst: join, src: then_value });
                let end_jump = self.emit_jump(|target| Instr::Jump { target });
                self.patch_jump(else_jump, self.here())?;
                let else_value = self.compile_expr(alternate)?;
                self.emit(Instr::Mov { dst: join, src: else_value });
                self.patch_jump(end_jump, self.here())?;
                Ok(join)
            }
            Expr::ArrowFn { params, body, is_async } => {
                let body = match body.as_ref() {
                    ExprOrBlock::Block(stmts) => FuncBody::Stmts(stmts),
                    ExprOrBlock::Expr(expr) => FuncBody::Expr(expr),
                };
                self.defer_function(FuncDef {
                    name: None,
                    params,
                    body,
                    is_arrow: true,
                    is_async: *is_async,
                    is_generator: false,
                    is_constructor: false,
                })
            }
            Expr::FnExpr { name, params, body, is_async, is_generator } => {
                self.defer_function(FuncDef {
                    name: name.clone(),
                    params,
                    body: FuncBody::Stmts(body),
                    is_arrow: false,
                    is_async: *is_async,
                    is_generator: *is_generator,
                    is_constructor: !is_async && !is_generator,
                })
            }
            Expr::New { callee, args } => {
                // Spread in `new` is transparent (evaluates to one argument),
                // like the evaluator — unlike call spread, which declines.
                let mut compiled = Vec::with_capacity(args.len());
                for arg in args {
                    match arg {
                        Expr::Spread(inner) => compiled.push(self.compile_expr(inner)?),
                        _ => compiled.push(self.compile_expr(arg)?),
                    }
                }
                let start = self.alloc_regs(compiled.len())?;
                for (i, reg) in compiled.iter().enumerate() {
                    self.emit(Instr::Mov { dst: start + i as u16, src: *reg });
                }
                let callee = self.compile_expr(callee)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::Construct { dst, callee, args: start, argc: compiled.len() as u16 });
                Ok(dst)
            }
            Expr::Template { quasis, exprs } => self.compile_template(quasis, exprs),
            Expr::This => self.compile_this(),
            Expr::Object(props) => self.compile_object(props),
            Expr::ClassExpr { name, superclass, body } => self.compile_class(
                name.as_deref().unwrap_or(""),
                name.clone(),
                superclass.as_deref(),
                body,
            ),
            Expr::TaggedTemplate { tag, cooked, exprs, .. } => {
                self.compile_tagged(tag, cooked, exprs)
            }
            Expr::Super => self.raise_bare_super(),
            Expr::Spread(inner) => self.compile_expr(inner),
            Expr::ImportMeta | Expr::DynamicImport(_) => {
                Err(Decline::Func("modules need Phase G"))
            }
            Expr::Await(_) => Err(Decline::Func("async needs Phase G")),
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
            Expr::OptionalChain { object, property, .. } => {
                self.compile_optional_chain(object, property)
            }
        }
    }

    /// Identifier reads. `undefined` is always the value, even when shadowed
    /// — the evaluator special-cases it before any lookup. `arguments` in a
    /// real function needs the arguments object (per-function fallback); in
    /// a nested arrow it would capture the enclosing one (whole unit).
    fn compile_identifier(&mut self, name: &str) -> Result<Reg, Decline> {
        if name == "undefined" {
            return self.load_undefined();
        }
        if name == "arguments" && !self.top_level {
            if self.is_arrow() {
                if !self.enclosing_is_top {
                    return Err(Decline::Unit("arrow `arguments` capture needs Phase G"));
                }
            } else {
                return Err(Decline::Func("arguments object needs fallback"));
            }
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
                // Shorthand reads tolerate missing and dead bindings.
                ObjectProp::Shorthand(name) => {
                    let val = self.alloc_reg()?;
                    match self.resolve(name)? {
                        Binding::Slot(slot) => {
                            self.emit(Instr::LoadLocalSoft { dst: val, slot });
                        }
                        Binding::Global => {
                            let index = self.intern_string(name)?;
                            self.emit(Instr::LoadGlobalSoft { dst: val, name: index });
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
                    let val = self.compile_expr(expression)?;
                    let key = self.intern_string(key)?;
                    template.push(PropEntry {
                        key: Some(KeySrc::Const(key)),
                        val,
                        kind: PropKind::Data,
                    });
                }
                ObjectProp::Computed(key_expression, value_expression) => {
                    match key_expression {
                        // Statically known keys skip the normalization check.
                        Expr::String(key) => {
                            let val = self.compile_expr(value_expression)?;
                            let key = self.intern_string(key)?;
                            template.push(PropEntry {
                                key: Some(KeySrc::Const(key)),
                                val,
                                kind: PropKind::Data,
                            });
                        }
                        Expr::Number(key) => {
                            let val = self.compile_expr(value_expression)?;
                            let key = self.intern_string(&key.to_string())?;
                            template.push(PropEntry {
                                key: Some(KeySrc::Const(key)),
                                val,
                                kind: PropKind::Data,
                            });
                        }
                        _ => {
                            let key = self.compile_expr(key_expression)?;
                            let normalized = self.alloc_reg()?;
                            self.emit(Instr::NormalKey { dst: normalized, src: key });
                            // Bad keys skip the value evaluation entirely.
                            let end = self.emit_jump(|target| Instr::JumpIfNullish {
                                src: normalized,
                                target,
                            });
                            let val = self.compile_expr(value_expression)?;
                            template.push(PropEntry {
                                key: Some(KeySrc::Reg(key)),
                                val,
                                kind: PropKind::Data,
                            });
                            self.patch_jump(end, self.here())?;
                        }
                    }
                }
                ObjectProp::Method { name, params, body, is_async, is_generator } => {
                    let val = self.defer_function(FuncDef {
                        name: Some(name.clone()),
                        params,
                        body: FuncBody::Stmts(body),
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
                        kind: PropKind::Data,
                    });
                }
                ObjectProp::Getter { name, body } => {
                    let val = self.defer_function(FuncDef {
                        name: Some(format!("get {name}")),
                        params: &[],
                        body: FuncBody::Stmts(body),
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
                ObjectProp::Setter { name, param, body } => {
                    let val = self.defer_function(FuncDef {
                        name: Some(format!("set {name}")),
                        params: std::slice::from_ref(param),
                        body: FuncBody::Stmts(body),
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
                    template.push(PropEntry { key: None, val: src, kind: PropKind::Spread });
                }
            }
        }
        let tmpl = self.push_const(Constant::ObjectTemplate(template))?;
        let dst = self.alloc_reg()?;
        self.emit(Instr::BuildObject { dst, tmpl });
        Ok(dst)
    }

    fn compile_array(&mut self, items: &'a [Expr]) -> Result<Reg, Decline> {
        if items.iter().any(|item| matches!(item, Expr::Spread(_))) {
            let mut template = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Expr::Spread(inner) => {
                        let reg = self.compile_expr(inner)?;
                        template.push(SpreadEntry { spread: true, reg });
                    }
                    _ => {
                        let reg = self.compile_expr(item)?;
                        template.push(SpreadEntry { spread: false, reg });
                    }
                }
            }
            let tmpl = self.push_const(Constant::SpreadTemplate(template))?;
            let dst = self.alloc_reg()?;
            self.emit(Instr::BuildArray { dst, tmpl });
            return Ok(dst);
        }
        // Holes arrive as `Undefined` from the parser — no special case.
        let start = self.alloc_regs(items.len())?;
        for (i, item) in items.iter().enumerate() {
            let reg = self.compile_expr(item)?;
            self.emit(Instr::Mov { dst: start + i as u16, src: reg });
        }
        let dst = self.alloc_reg()?;
        self.emit(Instr::NewArray { dst, args: start, argc: items.len() as u16 });
        Ok(dst)
    }

    fn compile_binary(&mut self, op: BinOp, left: &'a Expr, right: &'a Expr) -> Result<Reg, Decline> {
        match op {
            BinOp::And | BinOp::Or | BinOp::Nullish => {
                let lhs = self.compile_expr(left)?;
                let join = self.alloc_reg()?;
                self.emit(Instr::Mov { dst: join, src: lhs });
                let end_jump = match op {
                    BinOp::And => self.emit_jump(|target| Instr::JumpIfFalse { src: lhs, target }),
                    BinOp::Or => self.emit_jump(|target| Instr::JumpIfTrue { src: lhs, target }),
                    _ => self.emit_jump(|target| Instr::JumpIfNotNullish { src: lhs, target }),
                };
                let rhs = self.compile_expr(right)?;
                self.emit(Instr::Mov { dst: join, src: rhs });
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

    fn compile_inc_dec(&mut self, op: UnOp, operand: &'a Expr, prefix: bool) -> Result<Reg, Decline> {
        let delta = if op == UnOp::Inc { 1 } else { -1 };
        match operand {
            Expr::Identifier(name) => {
                let dst = self.alloc_reg()?;
                match self.resolve(name)? {
                    Binding::Slot(slot) => {
                        self.emit(Instr::IncLocal { dst, slot, delta, prefix });
                    }
                    Binding::Global => {
                        let index = self.intern_string(name)?;
                        self.emit(Instr::IncGlobal { dst, name: index, delta, prefix });
                    }
                }
                Ok(dst)
            }
            Expr::Member { object, property, .. } => {
                if matches!(object.as_ref(), Expr::Super) {
                    return self.raise_bare_super();
                }
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::IncProp { dst, obj, key, delta, prefix });
                Ok(dst)
            }
            // Anything else evaluates, then converts without storing.
            _ => {
                let src = self.compile_expr(operand)?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::Unary { dst, op, src });
                Ok(dst)
            }
        }
    }

    fn compile_delete(&mut self, operand: &'a Expr) -> Result<Reg, Decline> {
        match operand {
            Expr::Member { object, property, .. } => {
                if matches!(object.as_ref(), Expr::Super) {
                    return self.raise_bare_super();
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
            Expr::OptionalChain { object, property, .. } => {
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

    /// `typeof` never throws on undeclared names — but a dead-zone slot
    /// still reads as `undefined` rather than throwing, like the evaluator.
    fn compile_typeof(&mut self, operand: &'a Expr) -> Result<Reg, Decline> {
        if let Expr::Identifier(name) = operand {
            if name == "undefined" {
                let src = self.load_undefined()?;
                let dst = self.alloc_reg()?;
                self.emit(Instr::Unary { dst, op: UnOp::Typeof, src });
                return Ok(dst);
            }
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
        self.emit(Instr::Unary { dst, op: UnOp::Typeof, src });
        Ok(dst)
    }

    /// `` tag`a${x}b` `` desugars to `tag(parts, x)` where `parts` is a
    /// fresh cooked-strings array per evaluation. Substitution values
    /// evaluate before the tag, like the evaluator; `raw` is not modeled
    /// there either.
    fn compile_tagged(
        &mut self,
        tag: &'a Expr,
        cooked: &'a [String],
        exprs: &'a [Expr],
    ) -> Result<Reg, Decline> {
        let mut template = Vec::with_capacity(cooked.len());
        for part in cooked {
            let index = self.intern_string(part)?;
            let reg = self.load_const(index)?;
            template.push(SpreadEntry { spread: false, reg });
        }
        let mut values = Vec::with_capacity(exprs.len());
        for expr in exprs {
            values.push(self.compile_expr(expr)?);
        }
        let tmpl = self.push_const(Constant::SpreadTemplate(template))?;
        let parts = self.alloc_reg()?;
        self.emit(Instr::BuildArray { dst: parts, tmpl });
        let start = self.alloc_regs(1 + values.len())?;
        self.emit(Instr::Mov { dst: start, src: parts });
        for (i, value) in values.iter().enumerate() {
            self.emit(Instr::Mov {
                dst: start + 1 + i as u16,
                src: *value,
            });
        }
        let argc = 1 + values.len() as u16;
        let dst = self.alloc_reg()?;
        match tag {
            Expr::Member { object, property, .. } if matches!(object.as_ref(), Expr::Super) => {
                let key = self.compile_expr(property)?;
                let callee = self.alloc_reg()?;
                self.emit(Instr::SuperMember { dst: callee, key });
                let this = self.compile_this()?;
                self.emit(Instr::CallMethod {
                    dst,
                    callee,
                    this,
                    args: start,
                    argc,
                });
            }
            Expr::Member { object, property, .. } => {
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                let callee = self.alloc_reg()?;
                self.emit(Instr::GetProp { dst: callee, obj, key });
                self.emit(Instr::CallMethod {
                    dst,
                    callee,
                    this: obj,
                    args: start,
                    argc,
                });
            }
            Expr::Super => {
                return self.raise_bare_super();
            }
            _ => {
                let callee = self.compile_expr(tag)?;
                self.emit(Instr::Call {
                    dst,
                    callee,
                    args: start,
                    argc,
                });
            }
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
            ThisMode::Reject => Err(Decline::Unit("arrow `this` capture needs Phase G")),
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

    fn compile_call(&mut self, callee: &'a Expr, args: &'a [Expr]) -> Result<Reg, Decline> {
        // The callee shape is syntactic, so classify before emitting: method
        // calls keep their receiver, chains join on nullish, `super`
        // declines.
        enum Callee<'x> {
            Method,
            Plain,
            Chain { object: &'x Expr, property: &'x Expr },
            Super,
            SuperMember { property: &'x Expr },
        }
        let kind = match callee {
            Expr::Member { object, property, .. } if matches!(object.as_ref(), Expr::Super) => {
                Callee::SuperMember { property }
            }
            Expr::Member { .. } => Callee::Method,
            Expr::OptionalChain { object, property, .. } => Callee::Chain { object, property },
            Expr::Super => Callee::Super,
            _ => Callee::Plain,
        };
        // Arguments evaluate before the callee, like the evaluator — even
        // for chains, whose short-circuit skips only the property and the
        // call itself.
        enum CallArgs {
            Range { start: Reg, argc: u16 },
            Spread { tmpl: u16 },
        }
        let call_args = if args.iter().any(|arg| matches!(arg, Expr::Spread(_))) {
            let mut template = Vec::with_capacity(args.len());
            for arg in args {
                match arg {
                    Expr::Spread(inner) => {
                        let reg = self.compile_expr(inner)?;
                        template.push(SpreadEntry { spread: true, reg });
                    }
                    _ => {
                        let reg = self.compile_expr(arg)?;
                        template.push(SpreadEntry { spread: false, reg });
                    }
                }
            }
            let tmpl = self.push_const(Constant::SpreadTemplate(template))?;
            CallArgs::Spread { tmpl }
        } else {
            let start = self.alloc_regs(args.len())?;
            for (i, arg) in args.iter().enumerate() {
                let reg = self.compile_expr(arg)?;
                self.emit(Instr::Mov { dst: start + i as u16, src: reg });
            }
            CallArgs::Range { start, argc: args.len() as u16 }
        };
        let dst = self.alloc_reg()?;
        match kind {
            Callee::Method => {
                let Expr::Member { object, property, .. } = callee else {
                    return Err(Decline::Func("callee shape changed"));
                };
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                let callee = self.alloc_reg()?;
                self.emit(Instr::GetProp { dst: callee, obj, key });
                match call_args {
                    CallArgs::Range { start, argc } => {
                        self.emit(Instr::CallMethod { dst, callee, this: obj, args: start, argc });
                    }
                    CallArgs::Spread { tmpl } => {
                        self.emit(Instr::MethodSpread { dst, callee, this: obj, tmpl });
                    }
                }
            }
            Callee::Plain => {
                let callee = self.compile_expr(callee)?;
                match call_args {
                    CallArgs::Range { start, argc } => {
                        self.emit(Instr::Call { dst, callee, args: start, argc });
                    }
                    CallArgs::Spread { tmpl } => {
                        self.emit(Instr::CallSpread { dst, callee, tmpl });
                    }
                }
            }
            Callee::Super => match call_args {
                CallArgs::Range { start, argc } => {
                    self.emit(Instr::SuperCall { dst, args: start, argc });
                }
                CallArgs::Spread { tmpl } => {
                    self.emit(Instr::SuperCallSpread { dst, tmpl });
                }
            },
            Callee::SuperMember { property } => {
                let key = self.compile_expr(property)?;
                let callee = self.alloc_reg()?;
                self.emit(Instr::SuperMember { dst: callee, key });
                let this = self.compile_this()?;
                match call_args {
                    CallArgs::Range { start, argc } => {
                        self.emit(Instr::CallMethod { dst, callee, this, args: start, argc });
                    }
                    CallArgs::Spread { tmpl } => {
                        self.emit(Instr::MethodSpread { dst, callee, this, tmpl });
                    }
                }
            }
            Callee::Chain { object, property } => {
                let obj = self.compile_expr(object)?;
                let undef = self.load_undefined()?;
                self.emit(Instr::Mov { dst, src: undef });
                let end = self.emit_jump(|target| Instr::JumpIfNullish { src: obj, target });
                // An `Undefined` property marks an optional call `obj?.(args)`.
                let callee = if matches!(property, Expr::Undefined) {
                    obj
                } else {
                    let key = self.compile_expr(property)?;
                    let callee = self.alloc_reg()?;
                    self.emit(Instr::GetProp { dst: callee, obj, key });
                    callee
                };
                match call_args {
                    CallArgs::Range { start, argc } => {
                        self.emit(Instr::CallMethod { dst, callee, this: obj, args: start, argc });
                    }
                    CallArgs::Spread { tmpl } => {
                        self.emit(Instr::MethodSpread { dst, callee, this: obj, tmpl });
                    }
                }
                self.patch_jump(end, self.here())?;
            }
        }
        Ok(dst)
    }

    fn compile_optional_chain(
        &mut self,
        object: &'a Expr,
        property: &'a Expr,
    ) -> Result<Reg, Decline> {
        let obj = self.compile_expr(object)?;
        let join = self.alloc_reg()?;
        let undef = self.load_undefined()?;
        self.emit(Instr::Mov { dst: join, src: undef });
        let end = self.emit_jump(|target| Instr::JumpIfNullish { src: obj, target });
        let key = self.compile_expr(property)?;
        self.emit(Instr::GetProp { dst: join, obj, key });
        self.patch_jump(end, self.here())?;
        Ok(join)
    }

    /// Value first, then the target reference — the evaluator's order.
    fn compile_assignment(&mut self, target: &'a Expr, op: AssignOp, value: &'a Expr) -> Result<Reg, Decline> {
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
                        self.emit(Instr::StoreGlobal { name: index, src: rhs });
                        Ok(rhs)
                    }
                    (Binding::Slot(slot), false) => {
                        self.emit(Instr::CompoundLocal { dst, slot, op, rhs });
                        Ok(dst)
                    }
                    (Binding::Global, false) => {
                        let index = self.intern_string(name)?;
                        self.emit(Instr::CompoundGlobal { dst, name: index, op, rhs });
                        Ok(dst)
                    }
                }
            }
            Expr::Member { object, property, .. } => {
                if matches!(object.as_ref(), Expr::Super) {
                    return self.raise_bare_super();
                }
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                if op == AssignOp::Assign {
                    self.emit(Instr::SetProp { obj, key, val: rhs });
                    Ok(rhs)
                } else {
                    let dst = self.alloc_reg()?;
                    self.emit(Instr::CompoundProp { dst, obj, key, op, rhs });
                    Ok(dst)
                }
            }
            Expr::Array(_) | Expr::Object(_) => {
                // Only plain `=` destructures; anything else (or an
                // unconvertible target) fails at runtime on the AST tier.
                if op != AssignOp::Assign {
                    return Err(Decline::Func("invalid assignment target"));
                }
                let Some(pattern) = expr_to_pattern(target) else {
                    return Err(Decline::Func("invalid assignment target"));
                };
                // The converted pattern is owned, but nested function
                // expressions borrow it for the rest of compilation: promote
                // it to the compilation lifetime.
                let pattern: &'a Pattern = Box::leak(Box::new(pattern));
                self.compile_destructure(pattern, rhs, DestructureMode::Assign)?;
                Ok(rhs)
            }
            _ => Err(Decline::Func("invalid assignment target")),
        }
    }

    fn compile_logical_assignment(
        &mut self,
        target: &'a Expr,
        op: LogicalAssignOp,
        value: &'a Expr,
    ) -> Result<Reg, Decline> {
        // Read the current value, skip the write when it already decides.
        let join = self.alloc_reg()?;
        let end_jump = match target {
            Expr::Identifier(name) => {
                let current = self.compile_identifier(name)?;
                self.emit(Instr::Mov { dst: join, src: current });
                let jump = self.logical_skip_jump(op, current)?;
                let rhs = self.compile_expr(value)?;
                match self.resolve(name)? {
                    Binding::Slot(slot) => self.emit(Instr::StoreLocal { slot, src: rhs }),
                    Binding::Global => {
                        let index = self.intern_string(name)?;
                        self.emit(Instr::StoreGlobal { name: index, src: rhs });
                    }
                }
                self.emit(Instr::Mov { dst: join, src: rhs });
                jump
            }
            Expr::Member { object, property, .. } => {
                if matches!(object.as_ref(), Expr::Super) {
                    return self.raise_bare_super();
                }
                let obj = self.compile_expr(object)?;
                let key = self.compile_expr(property)?;
                let current = self.alloc_reg()?;
                self.emit(Instr::GetProp { dst: current, obj, key });
                self.emit(Instr::Mov { dst: join, src: current });
                let jump = self.logical_skip_jump(op, current)?;
                let rhs = self.compile_expr(value)?;
                self.emit(Instr::SetProp { obj, key, val: rhs });
                self.emit(Instr::Mov { dst: join, src: rhs });
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
            LogicalAssignOp::And => Ok(self.emit_jump(|target| Instr::JumpIfFalse { src: current, target })),
            LogicalAssignOp::Or => Ok(self.emit_jump(|target| Instr::JumpIfTrue { src: current, target })),
            LogicalAssignOp::Nullish => {
                Ok(self.emit_jump(|target| Instr::JumpIfNotNullish { src: current, target }))
            }
        }
    }

    fn compile_template(&mut self, quasis: &[String], exprs: &'a [Expr]) -> Result<Reg, Decline> {
        let start = self.alloc_regs(exprs.len())?;
        for (i, expr) in exprs.iter().enumerate() {
            let reg = self.compile_expr(expr)?;
            self.emit(Instr::Mov { dst: start + i as u16, src: reg });
        }
        let quasis = self.push_const(Constant::StringList(quasis.to_vec()))?;
        let dst = self.alloc_reg()?;
        self.emit(Instr::Template { dst, quasis, args: start, argc: exprs.len() as u16 });
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
/// AST-backed constant. Captures and `super` decline the whole unit instead:
/// slot bindings are invisible to environment chains, so neither bytecode
/// nor a fallback closed over the defining frame could honor them.
fn compile_function(
    def: FuncDef<'_>,
    outer_block: HashSet<String>,
    enclosing_is_top: bool,
) -> Result<FuncOutcome, Decline> {
    if def.is_async || def.is_generator || has_rest_param(def.params) || has_dup_params(def.params) {
        return Ok(FuncOutcome::Ast(build_ast_function(&def)));
    }
    let this_mode = if def.is_arrow {
        if enclosing_is_top {
            ThisMode::Global
        } else {
            ThisMode::Reject
        }
    } else {
        ThisMode::Frame
    };
    let mut compiler =
        Compiler::for_function(outer_block, enclosing_is_top, this_mode, def.is_arrow);
    match compiler.compile_unit_function(def.clone()) {
        Ok(bytecode) => Ok(FuncOutcome::Bytecode(Rc::new(bytecode))),
        Err(Decline::Func(_)) => Ok(FuncOutcome::Ast(build_ast_function(&def))),
        Err(unit @ Decline::Unit(_)) => Err(unit),
    }
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
            ("let [a] = b;", "compiled"),
            ("f(...args);", "compiled"),
            ("let a = [...b];", "compiled"),
            ("a?.b;", "compiled"),
            ("import x from 'm';", "modules need Phase G"),
            ("export default 1;", "modules need Phase G"),
            ("outer: for (;;) { break outer; }", "compiled"),
            ("async function f() {} f();", "compiled"), // per-function fallback
            ("function* g() {}", "compiled"),           // per-function fallback
            ("function f() { return arguments; }", "compiled"), // per-function fallback
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
        assert_eq!(reason("let x = 1; function f() { return x; } f();"), "compiled");
        // Nested declarations recurse through the chain, not slots.
        assert_eq!(
            reason("function g() { function f() { return f; } return f(); } g();"),
            "compiled"
        );
    }

    #[test]
    fn block_captures_decline_the_whole_unit() {
        // Block slots have no frame of their own for the chain to serve.
        assert_eq!(
            reason("{ let y = 1; function f() { return y; } }"),
            "block-scope capture needs Phase G"
        );
        assert_eq!(
            reason("function g() { { let y = 1; function f() { return y; } } }"),
            "block-scope capture needs Phase G"
        );
        assert_eq!(
            reason("function g() { for (let i = 0; i < 1; i++) { function f() { return i; } } }"),
            "block-scope capture needs Phase G"
        );
        // Arrows inheriting `this` or `arguments` from a function.
        assert_eq!(
            reason("function g() { const f = () => this; return f; }"),
            "arrow `this` capture needs Phase G"
        );
        assert_eq!(
            reason("function g() { const f = () => arguments; return f; }"),
            "arrow `arguments` capture needs Phase G"
        );
    }

    #[test]
    fn fibonacci_disassembly() {
        let module = compile("function fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); }").unwrap();
        crate::bytecode::verify::verify_module(&module).expect("compiler output verifies");
        // Undefined + "fib" + two instantiations (hoist-eager and in-order,
        // mirroring the evaluator's double evaluation of declarations).
        assert_eq!(module.main.constants.len(), 4);
        let mut functions = module.main.constants.iter().filter_map(|constant| match constant {
            Constant::Function(func) => Some(func),
            _ => None,
        });
        let (first, second) = (functions.next().unwrap(), functions.next().unwrap());
        assert!(functions.next().is_none());
        assert_eq!(format!("{:?}", first.code), format!("{:?}", second.code));
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
        // Hoisted declarations instantiate twice (hoist + statement).
        let consts = nested_functions("function g(){}");
        assert_eq!(consts.len(), 2);
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
        assert_eq!(consts.len(), 2);
        assert!(consts.iter().all(|c| matches!(c, Constant::AstFunction(_))));
    }

    #[test]
    fn recursive_and_self_referencing_functions_compile() {
        // Declarations recurse through the enclosing scope in both tiers.
        let module = compile(
            "function fib(n){ return n < 2 ? n : fib(n - 1) + fib(n - 2); } fib(10);",
        )
        .expect("recursive declaration must compile");
        assert!(
            module.main.constants.iter().any(|c| matches!(c, Constant::Function(_)))
        );
        // Named expressions resolve their own name outward (the evaluator
        // has no intermediate self-scope), so self-reference compiles too.
        compile("(function bar(){ return typeof bar; })();")
            .expect("self-referencing expression must compile");
    }
}

/// Build the AST fallback for one function, mirroring the evaluator's
/// function-creation arms. The VM closes it over the defining frame
/// environment; capture-free-ness holds because capturing functions decline
/// the whole unit (slot bindings are invisible to environment chains).
fn build_ast_function(def: &FuncDef<'_>) -> Rc<AstFunction> {
    let body = match def.body {
        FuncBody::Stmts(stmts) => stmts.to_vec(),
        FuncBody::Expr(expr) => vec![Statement::Return(Some(Box::new((*expr).clone())))],
    };
    let uses_arguments = if def.is_arrow {
        arrow_body_references(&ExprOrBlock::Block(body.clone()), "arguments")
    } else {
        stmts_reference(&body, "arguments")
    };
    Rc::new(AstFunction {
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
