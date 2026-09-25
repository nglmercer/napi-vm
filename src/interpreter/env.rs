use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use smallvec::SmallVec;

use crate::value::{MAX_GLOBAL_BINDINGS, Value, limit_err};

pub type Env = Rc<RefCell<Environment>>;

/// Frames with more bindings than this are promoted from a flat vector to a
/// hash map. Call frames (params + `this`) almost never reach the threshold,
/// so they pay no hashing and no hash-table allocation; the builtins frame
/// (dozens of names) promotes once and stays a map.
const PROMOTE_AT: usize = 16;

/// Inline capacity for small frames. Most function calls bind `this` + 1–4
/// params, so 8 slots cover the overwhelming majority without heap-allocating.
const INLINE_CAP: usize = 8;

/// Binding key: `Rc<str>` so parameter names shared across millions of calls
/// are cloned with a refcount bump instead of a heap allocation.
type Key = Rc<str>;

/// How a binding was declared. This drives assignment and redeclaration
/// rules, and whether the binding has a temporal dead zone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BindKind {
    /// `var`, function declarations, parameters, catch parameters, and
    /// bindings created by assigning to an undeclared name. Function-scoped,
    /// reassignable, hoisted already-initialized (as `undefined`).
    Var,
    /// `let`. Block-scoped, reassignable, dead until its declaration runs.
    Let,
    /// `const`. Block-scoped, not reassignable, dead until its declaration runs.
    Const,
}

/// One binding: its value plus the declaration facts needed to enforce
/// `const` and the temporal dead zone.
#[derive(Clone)]
struct Binding {
    value: Value,
    kind: BindKind,
    /// `false` while a `let`/`const` is hoisted but not yet initialized --
    /// the temporal dead zone. Reading such a binding is a `ReferenceError`
    /// distinct from "not defined".
    initialized: bool,
}

impl Binding {
    fn initialized(value: Value, kind: BindKind) -> Self {
        Self {
            value,
            kind,
            initialized: true,
        }
    }
}

/// Result of resolving a name through the scope chain.
pub enum Lookup {
    /// No binding of this name anywhere in the chain.
    Missing,
    /// Declared in an enclosing block but still in its temporal dead zone.
    Uninitialized,
    /// A readable binding.
    Value(Value),
}

/// Result of assigning to an existing binding.
#[derive(PartialEq, Eq, Debug)]
pub enum AssignOutcome {
    Assigned,
    /// No binding of this name; the caller decides whether to create one.
    Missing,
    /// Assignment to a `const`.
    Const,
    /// Assignment before the declaration ran.
    Uninitialized,
}

/// Result of a read-modify-write on an existing binding.
pub enum ModifyOutcome {
    Updated(Value),
    Missing,
    Const,
    Uninitialized,
}

// `Vars` only ever lives behind `Env = Rc<RefCell<_>>` and is never moved or
// cloned by value, so the inline `SmallVec` (much larger than the `HashMap`
// variant) costs nothing; the inlining is intentional for small frames.
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
enum Vars {
    Small(SmallVec<[(Key, Binding); INLINE_CAP]>),
    Large(HashMap<Key, Binding>),
}

impl Vars {
    fn len(&self) -> usize {
        match self {
            Vars::Small(v) => v.len(),
            Vars::Large(m) => m.len(),
        }
    }

    fn get(&self, n: &str) -> Option<&Binding> {
        match self {
            Vars::Small(v) => v.iter().find(|(k, _)| &**k == n).map(|(_, b)| b),
            Vars::Large(m) => m.get(n),
        }
    }

    fn get_mut(&mut self, n: &str) -> Option<&mut Binding> {
        match self {
            Vars::Small(v) => v.iter_mut().find(|(k, _)| &**k == n).map(|(_, b)| b),
            Vars::Large(m) => m.get_mut(n),
        }
    }

    /// Overwrite the value of `n` in this frame, keeping its declaration
    /// facts, if it is already bound. Returns the value back on a miss so the
    /// caller can insert or forward it without cloning.
    fn try_set(&mut self, n: &str, v: Value) -> Result<(), Value> {
        match self.get_mut(n) {
            Some(binding) => {
                binding.value = v;
                binding.initialized = true;
                Ok(())
            }
            None => Err(v),
        }
    }

    /// Move all bound values out of this frame (keys are dropped). Used by
    /// the iterative `Drop` of `Value` to tear down closure chains without
    /// recursing.
    fn drain_into(&mut self, work: &mut Vec<Value>) {
        match self {
            Vars::Small(vars) => work.extend(vars.drain(..).map(|(_, b)| b.value)),
            Vars::Large(map) => work.extend(map.drain().map(|(_, b)| b.value)),
        }
    }

    /// Drop every binding in this frame. Only the cycle collector calls
    /// this, and only for unmarked environments.
    fn clear(&mut self) {
        match self {
            Vars::Small(vars) => vars.clear(),
            Vars::Large(map) => map.clear(),
        }
    }

    /// Clone every bound value out of this frame, for the marker.
    fn values_cloned(&self) -> Vec<Value> {
        match self {
            Vars::Small(vars) => vars.iter().map(|(_, b)| b.value.clone()).collect(),
            Vars::Large(map) => map.values().map(|b| b.value.clone()).collect(),
        }
    }

    /// Bind `n` in this frame, assuming it is not already bound. Small frames
    /// are promoted to a hash map once they outgrow `PROMOTE_AT`.
    fn insert_new(&mut self, n: &str, b: Binding) {
        match self {
            Vars::Small(vars) => {
                if vars.len() >= PROMOTE_AT {
                    let mut map: HashMap<Key, Binding> = vars.drain(..).collect();
                    map.insert(Rc::from(n), b);
                    *self = Vars::Large(map);
                } else {
                    vars.push((Rc::from(n), b));
                }
            }
            Vars::Large(map) => {
                map.insert(Rc::from(n), b);
            }
        }
    }
}

#[derive(Clone)]
pub struct Environment {
    vars: Vars,
    parent: Option<Env>,
    /// Only the persistent user-global frame has a binding quota. Local
    /// function/catch frames and the trusted builtins frame leave this unset.
    global_limit: Option<usize>,
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Env({} vars)", self.vars.len())
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment {
    pub fn new() -> Self {
        Self {
            vars: Vars::Small(SmallVec::new()),
            parent: None,
            global_limit: None,
        }
    }

    pub fn child(p: Env) -> Self {
        Self {
            vars: Vars::Small(SmallVec::new()),
            parent: Some(p),
            global_limit: None,
        }
    }

    /// Create the persistent user-global frame. Its parent is normally the
    /// trusted builtins frame, and only this frame enforces the guest binding
    /// quota.
    pub fn global(parent: Option<Env>) -> Self {
        Self {
            vars: Vars::Small(SmallVec::new()),
            parent,
            global_limit: Some(MAX_GLOBAL_BINDINGS),
        }
    }

    /// Create a child frame from a pre-built binding list. The call fast
    /// path uses this to bind `this` + params in one shot, with no
    /// per-parameter `RefCell` borrows or insertion scans.
    pub fn with_bindings(p: Env, vars: SmallVec<[(Key, Value); INLINE_CAP]>) -> Self {
        // Parameters and `this` are `var`-like: reassignable and never in a
        // temporal dead zone.
        let vars = vars
            .into_iter()
            .map(|(k, v)| (k, Binding::initialized(v, BindKind::Var)));
        let vars = if vars.len() > PROMOTE_AT {
            Vars::Large(vars.collect())
        } else {
            Vars::Small(vars.collect())
        };
        Self {
            vars,
            parent: Some(p),
            global_limit: None,
        }
    }

    /// Read a binding, treating one still in its temporal dead zone as absent.
    ///
    /// Callers that must tell "not declared" from "declared but not yet
    /// initialized" -- identifier evaluation, which reports different errors
    /// for the two -- should use [`Environment::lookup`] instead.
    pub fn get(&self, n: &str) -> Option<Value> {
        match self.lookup(n) {
            Lookup::Value(v) => Some(v),
            Lookup::Missing | Lookup::Uninitialized => None,
        }
    }

    /// Resolve a name through the scope chain, distinguishing an undeclared
    /// name from one in its temporal dead zone.
    pub fn lookup(&self, n: &str) -> Lookup {
        if let Some(binding) = self.vars.get(n) {
            return if binding.initialized {
                // A module import is an indirection to the exporting binding,
                // so reading it must follow the link rather than hand back the
                // link itself.
                Lookup::Value(binding.value.deref_binding())
            } else {
                Lookup::Uninitialized
            };
        }
        match self.parent {
            Some(ref p) => p.borrow().lookup(n),
            None => Lookup::Missing,
        }
    }

    /// Declare `n` in *this* frame, replacing any binding of the same name.
    ///
    /// `initialized: false` puts a `let`/`const` into its temporal dead zone;
    /// the declaration statement later calls [`Environment::initialize`].
    pub fn declare(&mut self, n: &str, value: Value, kind: BindKind, initialized: bool) {
        let binding = Binding {
            value,
            kind,
            initialized,
        };
        match self.vars.get_mut(n) {
            Some(slot) => *slot = binding,
            None => self.vars.insert_new(n, binding),
        }
    }

    /// Like [`Environment::declare`], but enforces this frame's binding quota
    /// when creating a new name. Used for top-level declarations in the
    /// persistent global frame.
    pub fn declare_checked(
        &mut self,
        n: &str,
        value: Value,
        kind: BindKind,
        initialized: bool,
    ) -> Result<(), crate::error::VmErr> {
        if self.vars.get(n).is_none()
            && self
                .global_limit
                .is_some_and(|limit| self.vars.len() >= limit)
        {
            return Err(limit_err("Maximum global binding count exceeded"));
        }
        self.declare(n, value, kind, initialized);
        Ok(())
    }

    /// Give a hoisted `let`/`const` its value, leaving the dead zone. Returns
    /// `false` if the name is not bound in this frame.
    pub fn initialize(&mut self, n: &str, value: Value) -> bool {
        match self.vars.get_mut(n) {
            Some(binding) => {
                binding.value = value;
                binding.initialized = true;
                true
            }
            None => false,
        }
    }

    /// Turn the binding named `n` into a *live* one and hand back the cell it
    /// now reads and writes through, so an importer can share it.
    ///
    /// Idempotent: a binding that is already live returns its existing cell,
    /// which is what makes re-exporting the same name from several modules
    /// converge on one storage location rather than a chain of copies.
    pub fn export_cell(&mut self, n: &str) -> Option<Rc<RefCell<Value>>> {
        let binding = self.vars.get_mut(n)?;
        if let Value::Binding(cell) = &binding.value {
            return Some(cell.clone());
        }
        let cell = crate::heap::tracked(Rc::new(RefCell::new(std::mem::replace(
            &mut binding.value,
            Value::Undefined,
        ))));
        binding.value = Value::Binding(cell.clone());
        binding.initialized = true;
        Some(cell)
    }

    /// The raw value bound to `n` in *this* frame, without following a live
    /// binding. Used to recognize a re-import of a name already linked to the
    /// same cell.
    pub fn own_binding(&self, n: &str) -> Option<Value> {
        self.vars.get(n).map(|b| b.value.clone())
    }

    /// Move the binding named `n` into an existing cell, so a name already
    /// promised to an importer becomes the one this scope reads and writes.
    ///
    /// This is how a cyclic import resolves: the importer bound a cell before
    /// the exporting module had run, and when the export finally executes the
    /// value must land in *that* cell rather than a fresh one.
    pub fn adopt_cell(&mut self, n: &str, cell: Rc<RefCell<Value>>) {
        let current = self
            .vars
            .get(n)
            .map(|binding| binding.value.deref_binding())
            .unwrap_or(Value::Undefined);
        *cell.borrow_mut() = current;
        match self.vars.get_mut(n) {
            Some(binding) => {
                binding.value = Value::Binding(cell);
                binding.initialized = true;
            }
            None => self.declare(n, Value::Binding(cell), BindKind::Var, true),
        }
    }

    /// Bind `n` to an existing live cell — how `import` links a name to the
    /// exporting module's binding instead of copying its current value.
    pub fn bind_cell(&mut self, n: &str, cell: Rc<RefCell<Value>>, kind: BindKind) {
        self.declare(n, Value::Binding(cell), kind, true);
    }

    /// The declaration kind of `n` in this frame only, if bound.
    pub fn kind_of(&self, n: &str) -> Option<BindKind> {
        self.vars.get(n).map(|b| b.kind)
    }

    pub fn set(&mut self, n: &str, v: Value) {
        // Reuse the existing key allocation when the variable is already bound
        // (the common case in loops); only allocate on first insertion. A name
        // created this way is `var`-like, matching an assignment to an
        // undeclared identifier.
        if let Err(v) = self.vars.try_set(n, v) {
            self.vars
                .insert_new(n, Binding::initialized(v, BindKind::Var));
        }
    }

    /// Insert or replace a binding in this frame, enforcing the frame's
    /// optional quota before allocating a new key/value slot.
    pub fn try_set(&mut self, n: &str, v: Value) -> Result<(), crate::error::VmErr> {
        if let Err(v) = self.vars.try_set(n, v) {
            if self
                .global_limit
                .is_some_and(|limit| self.vars.len() >= limit)
            {
                return Err(limit_err("Maximum global binding count exceeded"));
            }
            self.vars
                .insert_new(n, Binding::initialized(v, BindKind::Var));
        }
        Ok(())
    }

    /// Assign to an existing binding somewhere in the scope chain.
    ///
    /// Reports `const` reassignment and writes into the temporal dead zone
    /// separately from a plain miss, so the caller can raise the right error
    /// instead of silently creating an implicit global.
    pub fn assign(&mut self, n: &str, v: Value) -> AssignOutcome {
        if let Some(binding) = self.vars.get_mut(n) {
            if binding.kind == BindKind::Const {
                // A `const` in its dead zone is still a `const`: JavaScript
                // reports the TDZ first, since the declaration has not run.
                return if binding.initialized {
                    AssignOutcome::Const
                } else {
                    AssignOutcome::Uninitialized
                };
            }
            if !binding.initialized {
                return AssignOutcome::Uninitialized;
            }
            // Writing through a live binding updates the cell the exporting
            // module and every importer share.
            match &binding.value {
                Value::Binding(cell) => *cell.borrow_mut() = v,
                _ => binding.value = v,
            }
            return AssignOutcome::Assigned;
        }
        match self.parent {
            Some(ref p) => p.borrow_mut().assign(n, v),
            None => AssignOutcome::Missing,
        }
    }

    /// Read-modify-write a bound variable in a single borrow and a single
    /// scan. Locates `n` in the scope chain, applies `f` to its current
    /// value, stores the result back into the *same* slot, and returns the
    /// new value.
    ///
    /// This fuses what would otherwise be a read (`borrow` + scan + clone)
    /// followed by a write (`borrow_mut` + scan + set) -- the pattern behind
    /// `x++` and compound assignment (`x += …`) -- into one `borrow_mut` and
    /// one scan, which is the hot path in tight arithmetic loops.
    ///
    /// `const` and dead-zone bindings are refused without calling `f`, so a
    /// rejected `x += 1` has no side effects.
    pub fn modify<F>(&mut self, n: &str, mut f: F) -> ModifyOutcome
    where
        F: FnMut(Value) -> Value,
    {
        if let Some(binding) = self.vars.get_mut(n) {
            if binding.kind == BindKind::Const {
                return if binding.initialized {
                    ModifyOutcome::Const
                } else {
                    ModifyOutcome::Uninitialized
                };
            }
            if !binding.initialized {
                return ModifyOutcome::Uninitialized;
            }
            binding.value = f(binding.value.clone());
            return ModifyOutcome::Updated(binding.value.clone());
        }
        match self.parent {
            Some(ref p) => p.borrow_mut().modify(n, f),
            None => ModifyOutcome::Missing,
        }
    }

    /// Remove a binding from this frame only (does not walk the parent chain).
    /// Returns `true` if the binding existed and was removed.
    pub fn remove(&mut self, n: &str) -> bool {
        match &mut self.vars {
            Vars::Small(vars) => {
                if let Some(pos) = vars.iter().position(|(k, _)| &**k == n) {
                    vars.remove(pos);
                    true
                } else {
                    false
                }
            }
            Vars::Large(map) => map.remove(n).is_some(),
        }
    }

    /// Check whether a binding exists in this frame only (no parent walk).
    pub fn has(&self, n: &str) -> bool {
        self.vars.get(n).is_some()
    }

    /// Return the parent environment, if any. Used by a generator body
    /// spawner to find the builtins frame.
    pub fn parent_env(&self) -> Option<Env> {
        self.parent.clone()
    }

    /// Whether this frame is the persistent user-global scope.
    pub fn is_global_scope(&self) -> bool {
        self.global_limit.is_some()
    }

    /// Find the persistent global frame in an environment chain. This is used
    /// by generator bodies, whose active frame is detached from the normal
    /// interpreter `global` field while the body runs.
    pub fn find_global(env: &Env) -> Option<Env> {
        let mut current = env.clone();
        loop {
            let (is_global, parent) = {
                let borrowed = current.borrow();
                (borrowed.is_global_scope(), borrowed.parent_env())
            };
            if is_global {
                return Some(current);
            }
            current = parent?;
        }
    }

    /// Iteratively drain a scope chain into `work` for the iterative `Drop`
    /// of `Value`. Walks parent frames one Rc at a time; stops at the first
    /// shared frame (shared scopes stay alive and drop themselves later).
    /// Bound values for the cycle collector's marker.
    pub(crate) fn trace_values(&self) -> Vec<Value> {
        self.vars.values_cloned()
    }

    /// Parent link for the cycle collector's marker.
    pub(crate) fn trace_parent(&self) -> Option<Env> {
        self.parent.clone()
    }

    /// Drop this frame's bindings and parent link so an unreachable cycle
    /// can free. Only the collector calls this, on unmarked environments.
    pub(crate) fn clear_edges(&mut self) {
        self.vars.clear();
        self.parent = None;
    }

    pub(crate) fn drain_chain(env: Env, work: &mut Vec<Value>) {
        let mut cur = Some(env);
        while let Some(e) = cur {
            match Rc::try_unwrap(e) {
                Ok(cell) => {
                    let mut env = cell.into_inner();
                    env.vars.drain_into(work);
                    cur = env.parent.take();
                }
                Err(_) => break,
            }
        }
    }

    /// Return all variable names bound in this frame (not walking the parent
    /// chain). Used by `Object.getOwnPropertyNames(window)` to enumerate
    /// globals.
    pub fn own_keys(&self) -> Vec<String> {
        match &self.vars {
            Vars::Small(v) => v.iter().map(|(k, _)| k.to_string()).collect(),
            Vars::Large(m) => m.keys().map(|k| k.to_string()).collect(),
        }
    }

    /// Return all variable names reachable from this scope, walking the parent
    /// chain. Duplicates across frames are preserved (last write wins at
    /// lookup time, but the name list is a union).
    pub fn all_keys(&self) -> Vec<String> {
        let mut names = self.own_keys();
        if let Some(ref p) = self.parent {
            let parent_keys = p.borrow().all_keys();
            for k in parent_keys {
                if !names.contains(&k) {
                    names.push(k);
                }
            }
        }
        names
    }
}

#[derive(Clone)]
pub struct Module {
    pub exports: HashMap<String, Value>,
    pub default: Option<Value>,
    /// The module's own top-level scope.
    ///
    /// A module body is not a script: its declarations belong to the module,
    /// not to the global object, so two modules can each declare `helper`
    /// without colliding. Kept here so a re-entered module (a cycle, or a
    /// second `import`) continues in the scope it started in.
    pub scope: Option<Env>,
}
