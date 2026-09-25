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

use crate::interpreter::produces_completion_value;
use crate::parser::{
    AssignOp, BinOp, Expr, ExprOrBlock, ForInit, LogicalAssignOp, Statement, UnOp, VarKind,
    arrow_body_references, collect_var_names, stmts_reference,
};

use super::constants::{AstFunction, Constant};
use super::function::{BytecodeFunction, SlotInfo, SlotKind};
use super::module::BytecodeModule;
use super::opcode::{Instr, Reg, Slot, Target};

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
    /// Arrows nested in functions: `this` would capture (Phase F).
    Reject,
}

/// One lexically nested function awaiting compilation. Functions compile
/// after their parent's own body succeeds (see the module docs); `snapshot`
/// freezes the live block bindings at the definition site so capture
/// detection sees the scope the source had, not the scope left when the
/// parent finishes.
struct Deferred<'a> {
    /// Address of the placeholder `MakeFunction` to overwrite.
    addr: usize,
    def: FuncDef<'a>,
    snapshot: HashSet<String>,
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

/// One compile-time lexical scope: slot bindings, or the global marker.
struct Scope {
    bindings: HashMap<String, Slot>,
    /// The top-level outermost scope: names resolve to the global
    /// environment instead of slots.
    global: bool,
}

/// Break/continue patch lists for one loop under compilation.
#[derive(Default)]
struct LoopCtx {
    breaks: Vec<usize>,
    continues: Vec<usize>,
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
    deferred: Vec<Deferred<'a>>,
    next_reg: u16,
    max_reg: u16,
    /// Names bound to slots anywhere in enclosing functions/blocks: a free
    /// variable landing here is a closure capture (Phase F work).
    outer: HashSet<String>,
    /// This function's function-level bindings (params, hoisted vars,
    /// top-level lexicals and functions), for children's capture checks.
    fn_level: HashSet<String>,
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
            deferred: Vec::new(),
            next_reg: 0,
            max_reg: 0,
            outer: HashSet::new(),
            fn_level: HashSet::new(),
            enclosing_is_top: true,
            top_level: true,
            this_mode: ThisMode::Global,
            is_arrow: false,
        }
    }

    fn for_function(
        outer: HashSet<String>,
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
            deferred: Vec::new(),
            next_reg: 0,
            max_reg: 0,
            outer,
            fn_level: HashSet::new(),
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
        self.slots.push(SlotInfo { name: name.to_string(), kind });
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
            | Some(Instr::JumpIfNotNullish { target: slot, .. }) => {
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
    /// reports `ReferenceError` for true misses, exactly like the AST);
    /// anything bound to a slot in an enclosing function is a capture.
    fn resolve(&self, name: &str) -> Result<Binding, Decline> {
        for scope in self.scopes.iter().rev() {
            if let Some(slot) = scope.bindings.get(name) {
                return Ok(Binding::Slot(*slot));
            }
            if scope.global {
                return Ok(Binding::Global);
            }
        }
        if self.outer.contains(name) {
            return Err(Decline::Unit("closure capture needs Phase F"));
        }
        Ok(Binding::Global)
    }

    /// Live slot-bound names in the current scope stack, for the capture
    /// snapshot of a nested function defined here.
    fn live_block_names(&self) -> HashSet<String> {
        let mut names = HashSet::new();
        for scope in &self.scopes {
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
            Statement::VarDecl { kind: VarKind::Let, name, destructuring: None, .. } => {
                out.push((name.clone(), SlotKind::Let));
            }
            Statement::VarDecl { kind: VarKind::Const, name, destructuring: None, .. } => {
                out.push((name.clone(), SlotKind::Const));
            }
            Statement::Declarations(inner) => block_lexicals(inner, out),
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
            self.fn_level.insert(param.clone());
        }
        let mut vars = Vec::new();
        collect_var_names(body, &mut vars);
        let mut seen = HashSet::new();
        for name in vars {
            if seen.insert(name.clone()) {
                // A `var` sharing a parameter's slot keeps the argument:
                // bind only when absent.
                self.declare_slot_if_absent(&name, SlotKind::Var)?;
                self.fn_level.insert(name);
            }
        }
        let mut lexicals = Vec::new();
        block_lexicals(body, &mut lexicals);
        for (name, kind) in lexicals {
            // The lexical pass replaces whatever the `var` pass declared.
            self.declare_slot(&name, kind)?;
            self.fn_level.insert(name);
        }
        let mut fns = Vec::new();
        block_fn_decls(body, &mut fns);
        for decl in fns {
            let slot = self.declare_slot_if_absent(decl.name, SlotKind::Var)?;
            self.fn_level.insert(decl.name.to_string());
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

    fn build_function(&mut self, name: Option<String>, parameter_count: usize) -> Result<BytecodeFunction, Decline> {
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
        })
    }

    /// Record a nested function for compilation after this unit's own body
    /// succeeds, emitting a placeholder the post-pass overwrites.
    fn defer_function(&mut self, def: FuncDef<'a>) -> Result<Reg, Decline> {
        let dst = self.alloc_reg()?;
        let addr = self.here();
        self.emit(Instr::MakeFunction { dst, func: u16::MAX });
        self.deferred.push(Deferred { addr, def, snapshot: self.live_block_names() });
        Ok(dst)
    }

    /// Compile the deferred nested functions and patch their placeholders.
    /// Runs only when the unit's own body compiled: an unsupported unit
    /// never wastes work on, or fails for, bodies it discards.
    fn finish_functions(&mut self) -> Result<(), Decline> {
        let deferred = std::mem::take(&mut self.deferred);
        for item in deferred {
            let mut outer = self.outer.clone();
            outer.extend(self.fn_level.iter().cloned());
            outer.extend(item.snapshot);
            let outcome = compile_function(item.def, outer, self.top_level)?;
            let (index, is_bytecode) = match outcome {
                FuncOutcome::Bytecode(code) => (self.push_const(Constant::Function(code))?, true),
                FuncOutcome::Ast(ast) => (self.push_const(Constant::AstFunction(ast))?, false),
            };
            match self.code.get_mut(item.addr) {
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
        self.build_function(None, 0)
    }

    fn compile_unit_function(&mut self, def: FuncDef<'a>) -> Result<BytecodeFunction, Decline> {
        let FuncDef { name, params, body, .. } = def;
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
                    self.fn_level.insert(param.clone());
                }
                let value = self.compile_expr(expr)?;
                self.emit(Instr::Return { src: value });
            }
        }
        self.finish_functions()?;
        self.build_function(name, parameter_count)
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

    /// Compile statements as a lexical block: fresh scope, hoisting, body.
    fn compile_scoped_block(&mut self, stmts: &'a [Statement]) -> Result<Reg, Decline> {
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
                self.compile_var_decl(kind.clone(), name, init.as_deref(), destructuring.is_some())
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
                if self.loops.is_empty() {
                    return Err(Decline::Func("break outside loop"));
                }
                let addr = self.emit_jump(|target| Instr::Jump { target });
                if let Some(ctx) = self.loops.last_mut() {
                    ctx.breaks.push(addr);
                }
                self.load_undefined()
            }
            Statement::Continue => {
                if self.loops.is_empty() {
                    return Err(Decline::Func("continue outside loop"));
                }
                let addr = self.emit_jump(|target| Instr::Jump { target });
                if let Some(ctx) = self.loops.last_mut() {
                    ctx.continues.push(addr);
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
            Statement::Try { .. } => Err(Decline::Func("try/catch needs Phase G")),
            Statement::Switch { .. } => Err(Decline::Func("switch needs Phase G")),
            Statement::ClassDecl { .. } => Err(Decline::Func("classes need Phase G")),
            Statement::ForIn { .. } | Statement::ForOf { .. } => {
                Err(Decline::Func("for-in/of needs Phase G"))
            }
            Statement::Labeled { .. } => Err(Decline::Func("labels need Phase G")),
            Statement::LabeledBreak(_) | Statement::LabeledContinue(_) => {
                Err(Decline::Func("labels need Phase G"))
            }
            Statement::Import { .. } => Err(Decline::Func("modules need Phase G")),
            Statement::ExportDefault(_)
            | Statement::ExportNamed { .. }
            | Statement::ExportAll { .. } => Err(Decline::Func("modules need Phase G")),
        }
    }
}

impl<'a> Compiler<'a> {
    // -- declarations ----------------------------------------------------

    fn compile_var_decl(
        &mut self,
        kind: VarKind,
        name: &str,
        init: Option<&'a Expr>,
        destructuring: bool,
    ) -> Result<Reg, Decline> {
        if destructuring {
            return Err(Decline::Func("destructuring needs Phase G"));
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
        self.loops.push(LoopCtx::default());
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
        self.loops.push(LoopCtx::default());
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
        self.loops.push(LoopCtx::default());
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

    fn compile_for_init(&mut self, init: &'a ForInit) -> Result<(), Decline> {
        match init {
            ForInit::Var { kind, decls } => {
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
            ForInit::Pattern { .. } => Err(Decline::Func("destructuring needs Phase G")),
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
                    return Err(Decline::Unit("super needs Phase G"));
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
            Expr::This => match self.this_mode {
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
                ThisMode::Reject => Err(Decline::Unit("arrow `this` capture needs Phase F")),
            },
            Expr::Object(_) => Err(Decline::Func("object literals need Phase G")),
            Expr::ClassExpr { .. } => Err(Decline::Func("classes need Phase G")),
            Expr::TaggedTemplate { .. } => Err(Decline::Func("tagged templates need Phase G")),
            Expr::Super => Err(Decline::Unit("super needs Phase G")),
            Expr::ImportMeta | Expr::DynamicImport(_) => {
                Err(Decline::Func("modules need Phase G"))
            }
            Expr::Await(_) => Err(Decline::Func("async needs Phase G")),
            Expr::Yield(_) | Expr::YieldFrom(_) => Err(Decline::Func("generators need Phase G")),
            Expr::Spread(_) => Err(Decline::Func("spread needs Phase G")),
            Expr::BigIntLiteral(_) => Err(Decline::Func("bigint literals need Phase G")),
            Expr::Regex(_, _) => Err(Decline::Func("regex literals need Phase G")),
            Expr::OptionalChain { .. } => Err(Decline::Func("optional chaining needs Phase G")),
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
                    return Err(Decline::Unit("arrow `arguments` capture needs Phase F"));
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

    fn compile_array(&mut self, items: &'a [Expr]) -> Result<Reg, Decline> {
        if items.iter().any(|item| matches!(item, Expr::Spread(_))) {
            return Err(Decline::Func("array spread needs Phase G"));
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
                    return Err(Decline::Unit("super needs Phase G"));
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
                    return Err(Decline::Unit("super needs Phase G"));
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
            Expr::OptionalChain { .. } => Err(Decline::Func("optional chaining needs Phase G")),
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

    /// Calls evaluate arguments before the callee, like the evaluator (and
    /// unlike the specification — this engine's order is load-bearing).
    fn compile_call(&mut self, callee: &'a Expr, args: &'a [Expr]) -> Result<Reg, Decline> {
        if args.iter().any(|arg| matches!(arg, Expr::Spread(_))) {
            return Err(Decline::Func("call spread needs Phase G"));
        }
        // The callee shape is syntactic, so classify before emitting: method
        // calls keep their receiver, `super`/optional chains decline.
        enum Callee {
            Method,
            Plain,
        }
        let kind = match callee {
            Expr::Member { object, .. } => {
                check_callable_spine(object)?;
                Callee::Method
            }
            Expr::Super | Expr::OptionalChain { .. } => {
                return Err(Decline::Unit("super needs Phase G"));
            }
            _ => Callee::Plain,
        };
        if matches!(kind, Callee::Plain) && contains_optional_chain(callee) {
            return Err(Decline::Func("optional chaining needs Phase G"));
        }
        let start = self.alloc_regs(args.len())?;
        for (i, arg) in args.iter().enumerate() {
            let reg = self.compile_expr(arg)?;
            self.emit(Instr::Mov { dst: start + i as u16, src: reg });
        }
        let argc = args.len() as u16;
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
                self.emit(Instr::CallMethod { dst, callee, this: obj, args: start, argc });
            }
            Callee::Plain => {
                let callee = self.compile_expr(callee)?;
                self.emit(Instr::Call { dst, callee, args: start, argc });
            }
        }
        Ok(dst)
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
                    return Err(Decline::Unit("super needs Phase G"));
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
                Err(Decline::Func("destructuring needs Phase G"))
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
                    return Err(Decline::Unit("super needs Phase G"));
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

/// Reject a method-call receiver spine containing `super` (whole unit) or an
/// optional chain (this function). Plain member spines are fine.
fn check_callable_spine(mut object: &Expr) -> Result<(), Decline> {
    loop {
        match object {
            Expr::Super => return Err(Decline::Unit("super needs Phase G")),
            Expr::OptionalChain { .. } => {
                return Err(Decline::Func("optional chaining needs Phase G"));
            }
            Expr::Member { object: inner, .. } => object = inner,
            _ => return Ok(()),
        }
    }
}

/// Whether an expression contains an optional chain anywhere. Used for plain
/// callees, whose evaluation the `Call` instruction cannot short-circuit.
fn contains_optional_chain(expr: &Expr) -> bool {
    match expr {
        Expr::OptionalChain { .. } => true,
        Expr::Member { object, property, .. } => {
            contains_optional_chain(object) || contains_optional_chain(property)
        }
        Expr::Call { callee, args } => {
            contains_optional_chain(callee) || args.iter().any(contains_optional_chain)
        }
        _ => false,
    }
}

fn has_rest_param(params: &[String]) -> bool {
    params.iter().any(|param| param.starts_with("..."))
}

fn has_dup_params(params: &[String]) -> bool {
    let mut seen = HashSet::new();
    params.iter().any(|param| !seen.insert(param))
}

/// Compile one nested function: bytecode when supported, otherwise an
/// AST-backed constant. Captures and `super` decline the whole unit instead:
/// an AST fallback with no closure environment could not honor them.
fn compile_function(
    def: FuncDef<'_>,
    outer: HashSet<String>,
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
    let mut compiler = Compiler::for_function(outer, enclosing_is_top, this_mode, def.is_arrow);
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
            ("try { f(); } catch (e) { g(); }", "try/catch needs Phase G"),
            ("switch (x) { case 1: y(); }", "switch needs Phase G"),
            ("class C {}", "classes need Phase G"),
            ("for (let k in o) { f(k); }", "for-in/of needs Phase G"),
            ("for (const v of a) { f(v); }", "for-in/of needs Phase G"),
            ("let o = { a: 1 };", "object literals need Phase G"),
            ("let [a] = b;", "destructuring needs Phase G"),
            ("f(...args);", "call spread needs Phase G"),
            ("let a = [...b];", "array spread needs Phase G"),
            ("a?.b;", "optional chaining needs Phase G"),
            ("import x from 'm';", "modules need Phase G"),
            ("export default 1;", "modules need Phase G"),
            ("outer: for (;;) { break outer; }", "labels need Phase G"),
            ("async function f() {} f();", "compiled"), // per-function fallback
            ("function* g() {}", "compiled"),           // per-function fallback
            ("function f() { return arguments; }", "compiled"), // per-function fallback
            ("let x = 10n;", "bigint literals need Phase G"),
            ("let r = /ab+c/;", "regex literals need Phase G"),
        ] {
            assert_eq!(reason(source), expected, "source: {source}");
        }
    }

    #[test]
    fn captures_decline_the_whole_unit() {
        assert_eq!(
            reason("function g() { let x = 1; function f() { return x; } return f(); } g();"),
            "closure capture needs Phase F"
        );
        // Top-level names are globals, not captures.
        assert_eq!(reason("let x = 1; function f() { return x; } f();"), "compiled");
        // Arrows inheriting `this` from a function capture it.
        assert_eq!(
            reason("function g() { const f = () => this; return f; }"),
            "arrow `this` capture needs Phase F"
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
}

/// Build the AST fallback for one function, mirroring the evaluator's
/// function-creation arms (minus the closure: fallback functions are
/// capture-free by construction).
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
