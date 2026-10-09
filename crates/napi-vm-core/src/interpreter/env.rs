use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::rc::Rc;

use smallvec::SmallVec;

use crate::value::{MAX_GLOBAL_BINDINGS, ObjectCell, PropAttrs, Value, limit_err};

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

thread_local! {
    // Retain randomized, secret keys while avoiding SipHash on every global
    // name lookup. This holds no guest data and is independent of owner TLS.
    static BINDING_HASHER: ahash::RandomState = {
        let seed = std::collections::hash_map::RandomState::new();
        ahash::RandomState::with_seeds(
            seed.hash_one(0_u64), seed.hash_one(1_u64),
            seed.hash_one(2_u64), seed.hash_one(3_u64),
        )
    };
}

/// Keyed runtime-map hashing without retaining any owner or guest data.
pub(super) fn randomized_hasher() -> ahash::RandomState {
    BINDING_HASHER.with(Clone::clone)
}

/// How a binding was declared. This drives assignment and redeclaration
/// rules, and whether the binding has a temporal dead zone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BindKind {
    /// `var`, function declarations, parameters, and
    /// bindings created by assigning to an undeclared name. Function-scoped,
    /// reassignable, hoisted already-initialized (as `undefined`).
    Var,
    /// An identifier catch parameter: mutable and local to the catch scope.
    /// Annex B permits sloppy direct-eval var declarations across this binding.
    Catch,
    /// `let`. Block-scoped, reassignable, dead until its declaration runs.
    Let,
    /// `const`. Block-scoped, not reassignable, dead until its declaration runs.
    Const,
}

/// One binding: its value plus the declaration facts needed to enforce
/// `const` and the temporal dead zone.
#[derive(Clone)]
struct Binding {
    silent_immutable: bool,
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
            silent_immutable: false,
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
    /// A non-writable global data property.
    ReadOnly,
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
    ReadOnly(Value),
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
    Large(HashMap<Key, Binding, ahash::RandomState>),
}

impl Vars {
    fn len(&self) -> usize {
        match self {
            Vars::Small(v) => v.len(),
            Vars::Large(m) => m.len(),
        }
    }

    #[inline(always)]
    fn get(&self, n: &str) -> Option<&Binding> {
        match self {
            Vars::Small(v) => v.iter().find(|(k, _)| &**k == n).map(|(_, b)| b),
            Vars::Large(m) => m.get(n),
        }
    }

    #[inline(always)]
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
                match &binding.value {
                    Value::Binding(cell) if !matches!(v, Value::Binding(_)) => {
                        cell.borrow_mut().assign_for_execution(v)
                    }
                    _ => binding.value.assign_for_execution(v),
                }
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
                    let mut map =
                        HashMap::with_capacity_and_hasher(vars.len() + 1, randomized_hasher());
                    map.extend(vars.drain(..));
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

/// The object side of a realm's GlobalEnvironment. Lexical declarations stay
/// in Environment::vars; properties, descriptors, prototype and extensibility
/// use the same storage as ordinary objects.
#[derive(Clone)]
struct GlobalEnvironment {
    object: Rc<ObjectCell>,
    var_names: HashSet<String>,
    user_names: HashSet<String>,
}

impl GlobalEnvironment {
    fn new(parent: Option<&Env>) -> Self {
        let _realm = super::realm::AllocationRealm::enter(None);
        let mut entries = Vec::new();
        let mut attributes = Vec::new();
        if let Some(parent) = parent {
            let parent = parent.borrow();
            for name in parent.global_property_keys() {
                if let Some((value, attrs)) = parent.global_property(&name) {
                    entries.push((name.clone(), value));
                    attributes.push((name, attrs));
                }
            }
        }
        let value = Value::object(entries);
        let Value::Object { props } = &value else {
            unreachable!()
        };
        for (name, attrs) in attributes {
            props.meta.borrow_mut().set_attrs(&name, attrs);
        }
        Self {
            object: props.clone(),
            var_names: HashSet::new(),
            user_names: HashSet::new(),
        }
    }

    fn value(&self) -> Value {
        Value::Object {
            props: self.object.clone(),
        }
    }
}

#[derive(Clone)]
pub struct Environment {
    pub(crate) class_initializer: bool,
    pub(crate) with_object: Option<Value>,
    vars: Vars,
    global_environment: Option<GlobalEnvironment>,
    parent: Option<Env>,
    /// Only the persistent user-global frame has a binding quota. Local
    /// function/catch frames and the trusted builtins frame leave this unset.
    global_limit: Option<usize>,
    module_context: Option<String>,
    pub(crate) module_realm: Option<super::ModuleRealm>,
    new_target: Option<Value>,
    /// None outside a constructor; Some(None) is an uninitialized derived this.
    constructor_this: Option<Option<Value>>,
    constructor_fields: Option<Rc<Vec<crate::parser::Statement>>>,
    private_names: HashMap<String, u64>,
    private_declarations: HashSet<String>,
    strict: Option<bool>,
    variable_scope: bool,
    pub(crate) async_generator_body: bool,
    eval_scope: bool,
    parameter_scope: bool,
    property_attributes: HashMap<String, crate::value::PropAttrs>,
    intrinsics: HashMap<String, Value>,
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
    pub(crate) fn eval_context(scope: &Env) -> crate::parser::EvalContext {
        let mut context = crate::parser::EvalContext {
            new_target: scope.borrow().new_target().is_some(),
            strict: scope.borrow().strict(),
            ..Default::default()
        };
        let mut current = Some(scope.clone());
        let mut found_this = false;
        while let Some(frame) = current {
            let environment = frame.borrow();
            if !found_this {
                context.forbid_arguments |= environment.class_initializer;
            }
            context
                .private_names
                .extend(environment.private_names.keys().cloned());
            context
                .private_names
                .extend(environment.private_declarations.iter().cloned());
            if !found_this && environment.own_binding("this").is_some() {
                found_this = true;
                let mut closure = environment.parent.clone();
                let mut derived = false;
                while let Some(parent) = closure {
                    let parent = parent.borrow();
                    if parent.own_binding("this").is_some() {
                        break;
                    }
                    derived |= parent.own_binding("__super_ctor").is_some();
                    context.super_property |=
                        parent.own_binding(super::eval::SUPER_PROTO).is_some();
                    closure = parent.parent.clone();
                }
                context.super_call = environment.constructor_this.is_some() && derived;
            }
            current = environment.parent.clone();
        }
        context
    }

    pub(crate) fn declare_private_declarations(&mut self, names: impl IntoIterator<Item = String>) {
        self.private_declarations.extend(names);
    }

    pub(crate) fn declare_private_field(&mut self, name: &str) {
        if !self.private_names.contains_key(name) {
            let Value::Symbol(ref symbol) = crate::builtins::new_symbol(None) else {
                unreachable!()
            };
            self.private_names.insert(name.into(), symbol.id);
        }
    }

    pub(crate) fn private_name(&self, name: &str) -> Option<u64> {
        self.private_names.get(name).copied().or_else(|| {
            self.parent
                .as_ref()
                .and_then(|parent| parent.borrow().private_name(name))
        })
    }
    pub(crate) fn enter_constructor(&mut self, derived: bool, fields: &[crate::parser::Statement]) {
        self.constructor_this = Some(if derived { None } else { self.get("this") });
        self.constructor_fields = Some(Rc::new(fields.to_vec()));
    }

    /// Arrows and lexical/eval scopes inherit constructor state. An ordinary
    /// function's own this binding stops the search.
    pub(crate) fn constructor_environment(scope: &Env) -> Option<Env> {
        let mut current = scope.clone();
        loop {
            let parent = {
                let env = current.borrow();
                if env.constructor_this.is_some() {
                    return Some(current.clone());
                }
                if env.own_binding("this").is_some() {
                    return None;
                }
                env.parent.clone()
            };
            current = parent?;
        }
    }

    pub(crate) fn bind_constructor_this(
        &mut self,
        value: Value,
    ) -> Result<(), crate::error::VmErr> {
        if !matches!(self.constructor_this, Some(None)) {
            return Err(crate::error::VmErr::Msg(
                "ReferenceError: super() has already initialized this".into(),
            ));
        }
        self.constructor_this = Some(Some(value));
        Ok(())
    }

    pub(crate) fn constructor_fields(&self) -> Rc<Vec<crate::parser::Statement>> {
        self.constructor_fields.clone().unwrap_or_default()
    }

    pub(crate) fn snapshot_intrinsics(&mut self) {
        let globals = self
            .own_keys()
            .into_iter()
            .filter_map(|name| self.own_binding(&name).map(|value| (name, value)))
            .collect::<HashMap<_, _>>();
        self.intrinsics.extend(globals);
    }

    pub(crate) fn install_intrinsic(&mut self, name: &str, value: Value) {
        self.intrinsics.insert(name.into(), value);
    }

    pub(crate) fn intrinsic(&self, name: &str) -> Option<Value> {
        self.intrinsics
            .get(name)
            .cloned()
            .or_else(|| {
                self.parent
                    .as_ref()
                    .and_then(|parent| parent.borrow().intrinsic(name))
            })
            .or_else(|| self.get(name))
    }

    pub(crate) fn intrinsic_name(&self, value: &Value) -> Option<String> {
        self.intrinsics
            .iter()
            .find_map(|(name, constructor)| {
                super::strict_equals(constructor, value).then(|| name.clone())
            })
            .or_else(|| {
                self.parent
                    .as_ref()
                    .and_then(|parent| parent.borrow().intrinsic_name(value))
            })
    }

    pub(crate) fn global_object(&self) -> Option<Value> {
        self.global_environment
            .as_ref()
            .map(GlobalEnvironment::value)
    }

    pub(crate) fn create_global_var_binding(
        &mut self,
        name: &str,
        value: Value,
        deletable: bool,
    ) -> Result<(), crate::error::VmErr> {
        self.check_global_quota(name)?;
        if self.vars.get(name).is_some() {
            return Err(crate::error::VmErr::Msg(format!(
                "SyntaxError: Identifier '{name}' has already been declared"
            )));
        }
        let Some(record) = &mut self.global_environment else {
            unreachable!("global environment");
        };
        if record.object.own_value(name).is_none() {
            if record.object.meta.borrow().non_extensible {
                return Err(crate::error::VmErr::Msg(format!(
                    "TypeError: Cannot declare global variable '{name}'"
                )));
            }
            record.value().set_prop(name.into(), value)?;
            record.object.meta.borrow_mut().set_attrs(
                name,
                PropAttrs {
                    writable: true,
                    enumerable: true,
                    configurable: deletable,
                },
            );
        }
        record.var_names.insert(name.into());
        record.user_names.insert(name.into());
        Ok(())
    }

    pub(crate) fn own_lexical_binding(&self, name: &str) -> bool {
        self.vars.get(name).is_some()
    }

    pub(crate) fn has_var_declaration(&self, name: &str) -> bool {
        self.global_environment
            .as_ref()
            .is_some_and(|record| record.var_names.contains(name))
    }

    pub(crate) fn check_global_quota(&self, name: &str) -> Result<(), crate::error::VmErr> {
        if let Some(record) = &self.global_environment
            && self.vars.get(name).is_none()
            && !record.user_names.contains(name)
            && self
                .global_limit
                .is_some_and(|limit| self.vars.len() + record.user_names.len() >= limit)
        {
            return Err(limit_err("Maximum global binding count exceeded"));
        }
        Ok(())
    }

    pub(crate) fn note_global_property(&mut self, name: &str) {
        if let Some(record) = &mut self.global_environment {
            record.user_names.insert(name.into());
        }
    }

    pub(crate) fn remove_global_var_name(&mut self, name: &str) {
        if let Some(record) = &mut self.global_environment {
            record.var_names.remove(name);
            if record.object.own_value(name).is_none() {
                record.user_names.remove(name);
            }
        }
    }

    pub(crate) fn set_property_attributes(&mut self, name: &str, attrs: PropAttrs) {
        if let Some(record) = &self.global_environment {
            record.object.meta.borrow_mut().set_attrs(name, attrs);
        } else {
            self.property_attributes.insert(name.into(), attrs);
        }
    }

    pub(crate) fn global_property(&self, name: &str) -> Option<(Value, PropAttrs)> {
        if let Some(record) = &self.global_environment {
            return record.object.own_value(name).map(|value| {
                (
                    value.deref_binding(),
                    record.object.meta.borrow().attrs_of(name),
                )
            });
        }
        if let Some(binding) = self.vars.get(name)
            && (binding.kind == BindKind::Var || self.property_attributes.contains_key(name))
        {
            return Some((
                binding.value.deref_binding(),
                self.property_attributes
                    .get(name)
                    .copied()
                    .unwrap_or_default(),
            ));
        }
        self.parent
            .as_ref()
            .and_then(|p| p.borrow().global_property(name))
    }

    pub(crate) fn delete_global_property(&mut self, name: &str) -> bool {
        let Some((_, attrs)) = self.global_property(name) else {
            return true;
        };
        if !attrs.configurable {
            return false;
        }
        if let Some(record) = &mut self.global_environment {
            let companion = format!("__setter:{name}__");
            record
                .object
                .borrow_mut()
                .retain(|(key, _)| key != name && key != &companion);
            record.object.meta.borrow_mut().forget(name);
            record.object.meta.borrow_mut().forget(&companion);
            record.object.note_mutated();
            if !record.var_names.contains(name) {
                record.user_names.remove(name);
            }
            return true;
        }
        self.remove(name);
        self.property_attributes.remove(name);
        if let Some(parent) = &self.parent {
            parent.borrow_mut().delete_global_property(name);
        }
        true
    }

    pub(crate) fn global_property_keys(&self) -> Vec<String> {
        if let Some(record) = &self.global_environment {
            let meta = record.object.meta.borrow();
            return record
                .object
                .borrow()
                .iter()
                .filter(|(name, _)| {
                    meta.symbol_key(name).is_none() && !name.starts_with("__setter:")
                })
                .map(|(name, _)| name.clone())
                .collect();
        }
        self.all_keys()
            .into_iter()
            .filter(|name| self.global_property(name).is_some())
            .collect()
    }

    pub(crate) fn strict(&self) -> bool {
        self.strict
            .unwrap_or_else(|| self.parent.as_ref().is_some_and(|p| p.borrow().strict()))
    }

    pub(crate) fn replace_strict(&mut self, strict: Option<bool>) -> Option<bool> {
        std::mem::replace(&mut self.strict, strict)
    }

    pub(crate) fn set_new_target(&mut self, target: Value) {
        self.new_target = Some(target);
    }

    pub(crate) fn new_target(&self) -> Option<Value> {
        self.new_target
            .clone()
            .or_else(|| self.parent.as_ref().and_then(|p| p.borrow().new_target()))
    }

    pub(crate) fn set_module_context(&mut self, name: &str) {
        self.variable_scope = true;
        self.module_context = Some(name.into());
    }
    pub(crate) fn module_context(&self) -> Option<String> {
        self.module_context.clone().or_else(|| {
            self.parent
                .as_ref()
                .and_then(|p| p.borrow().module_context())
        })
    }
    pub fn new() -> Self {
        Self {
            vars: Vars::Small(SmallVec::new()),
            global_environment: None,
            parent: None,
            global_limit: None,
            module_context: None,
            module_realm: None,
            new_target: None,
            class_initializer: false,
            constructor_this: None,
            constructor_fields: None,
            private_names: HashMap::new(),
            private_declarations: HashSet::new(),
            strict: None,
            variable_scope: false,
            async_generator_body: false,
            eval_scope: false,
            parameter_scope: false,
            with_object: None,
            property_attributes: HashMap::new(),
            intrinsics: HashMap::new(),
        }
    }

    pub fn child(p: Env) -> Self {
        Self {
            vars: Vars::Small(SmallVec::new()),
            global_environment: None,
            parent: Some(p),
            global_limit: None,
            module_context: None,
            module_realm: None,
            new_target: None,
            class_initializer: false,
            constructor_this: None,
            constructor_fields: None,
            private_names: HashMap::new(),
            private_declarations: HashSet::new(),
            strict: None,
            variable_scope: false,
            async_generator_body: false,
            eval_scope: false,
            parameter_scope: false,
            with_object: None,
            property_attributes: HashMap::new(),
            intrinsics: HashMap::new(),
        }
    }

    /// Create the persistent user-global frame. Its parent is normally the
    /// trusted builtins frame, and only this frame enforces the guest binding
    /// quota.
    pub fn global(parent: Option<Env>) -> Self {
        let record = GlobalEnvironment::new(parent.as_ref());
        Self {
            vars: Vars::Small(SmallVec::new()),
            global_environment: Some(record),
            parent,
            global_limit: Some(MAX_GLOBAL_BINDINGS),
            module_context: None,
            module_realm: None,
            new_target: None,
            class_initializer: false,
            constructor_this: None,
            constructor_fields: None,
            private_names: HashMap::new(),
            private_declarations: HashSet::new(),
            strict: None,
            variable_scope: false,
            async_generator_body: false,
            eval_scope: false,
            parameter_scope: false,
            with_object: None,
            property_attributes: HashMap::new(),
            intrinsics: HashMap::new(),
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
            let mut map = HashMap::with_hasher(randomized_hasher());
            map.extend(vars);
            Vars::Large(map)
        } else {
            Vars::Small(vars.collect())
        };
        Self {
            vars,
            global_environment: None,
            parent: Some(p),
            global_limit: None,
            module_context: None,
            module_realm: None,
            new_target: None,
            class_initializer: false,
            constructor_this: None,
            constructor_fields: None,
            private_names: HashMap::new(),
            private_declarations: HashSet::new(),
            strict: None,
            variable_scope: true,
            async_generator_body: false,
            eval_scope: false,
            parameter_scope: false,
            with_object: None,
            property_attributes: HashMap::new(),
            intrinsics: HashMap::new(),
        }
    }

    /// Function bodies own a variable environment; lexical blocks do not.
    pub(crate) fn awaits_return_value(&self) -> bool {
        if self.variable_scope {
            return self.async_generator_body;
        }
        self.parent
            .as_ref()
            .is_some_and(|parent| parent.borrow().awaits_return_value())
    }

    pub(crate) fn function_child(parent: Env) -> Self {
        let mut frame = Self::child(parent);
        frame.variable_scope = true;
        frame
    }

    pub(crate) fn mark_eval_scope(&mut self, strict: bool) {
        self.eval_scope = true;
        self.variable_scope = strict;
    }

    pub(crate) fn is_eval_scope(&self) -> bool {
        self.eval_scope
    }

    pub(crate) fn has_eval_var_conflict(&self, name: &str) -> bool {
        self.vars.get(name).is_some_and(|binding| {
            self.parameter_scope || !matches!(binding.kind, BindKind::Var | BindKind::Catch)
        })
    }

    /// Named function expressions own a lexical scope, distinct from declarations
    /// and inferred display names. Assignment is ignored in sloppy code.
    pub(crate) fn named_function_scope(parent: Env, name: Option<&str>) -> Env {
        let Some(name) = name else {
            return parent;
        };
        let mut scope = Self::child(parent);
        scope.declare(name, Value::Undefined, BindKind::Let, false);
        std::rc::Rc::new(std::cell::RefCell::new(scope))
    }

    pub(crate) fn initialize_function_name(scope: &Env, name: Option<&str>, value: &Value) {
        if let Some(name) = name {
            let mut scope = scope.borrow_mut();
            scope.initialize(name, value.clone());
            if let Some(binding) = scope.vars.get_mut(name) {
                binding.silent_immutable = true;
            }
        }
    }

    pub(crate) fn parameter_child(parent: Env) -> Self {
        let mut scope = Self::child(parent);
        scope.parameter_scope = true;
        scope
    }

    pub(crate) fn variable_environment(scope: &Env) -> Env {
        let mut frame = scope.clone();
        loop {
            let next = {
                let environment = frame.borrow();
                if environment.variable_scope || environment.is_global_scope() {
                    return frame.clone();
                }
                environment.parent.clone()
            };
            match next {
                Some(parent) => frame = parent,
                None => return frame,
            }
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
        if n == "this"
            && let Some(value) = &self.constructor_this
        {
            return match value {
                Some(value) => Lookup::Value(value.clone()),
                None => Lookup::Uninitialized,
            };
        }
        if let Some(binding) = self.vars.get(n) {
            return if binding.initialized {
                // A module import is an indirection to the exporting binding,
                // so reading it must follow the link rather than hand back the
                // link itself.
                let value = binding.value.deref_binding();
                if matches!(value, Value::Uninitialized) {
                    Lookup::Uninitialized
                } else {
                    Lookup::Value(value)
                }
            } else {
                Lookup::Uninitialized
            };
        }
        if let Some(record) = &self.global_environment {
            return record
                .object
                .own_value(n)
                .map(|value| Lookup::Value(value.deref_binding()))
                .unwrap_or(Lookup::Missing);
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
        if kind == BindKind::Var && self.global_environment.is_some() {
            let _ = self.create_global_var_binding(n, value, false);
            return;
        }
        let binding = Binding {
            silent_immutable: false,
            value,
            kind,
            initialized,
        };
        match self.vars.get_mut(n) {
            Some(slot) => {
                if let Value::Binding(cell) = &slot.value
                    && !matches!(binding.value, Value::Binding(_))
                {
                    *cell.borrow_mut() = if initialized {
                        binding.value
                    } else {
                        Value::Uninitialized
                    };
                    slot.kind = kind;
                    slot.initialized = initialized;
                } else {
                    *slot = binding;
                }
            }
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
        if kind == BindKind::Var && self.global_environment.is_some() {
            return self.create_global_var_binding(n, value, false);
        }
        self.check_global_quota(n)?;
        if self.global_environment.is_some() && kind != BindKind::Var {
            let conflict = self.vars.get(n).is_some_and(|binding| binding.initialized)
                || self.has_var_declaration(n)
                || self
                    .global_property(n)
                    .is_some_and(|(_, attrs)| !attrs.configurable);
            if conflict {
                return Err(crate::error::VmErr::Msg(format!(
                    "SyntaxError: Identifier '{n}' has already been declared"
                )));
            }
        }
        self.declare(n, value, kind, initialized);
        Ok(())
    }

    /// Give a hoisted `let`/`const` its value, leaving the dead zone. Returns
    /// `false` if the name is not bound in this frame.
    pub fn initialize(&mut self, n: &str, value: Value) -> bool {
        match self.vars.get_mut(n) {
            Some(binding) => {
                match &binding.value {
                    Value::Binding(cell) => *cell.borrow_mut() = value,
                    _ => binding.value = value,
                }
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
        let value = std::mem::replace(&mut binding.value, Value::Undefined);
        let cell = crate::heap::tracked(Rc::new(RefCell::new(if binding.initialized {
            value
        } else {
            Value::Uninitialized
        })));
        binding.value = Value::Binding(cell.clone());
        Some(cell)
    }

    /// The raw value bound to `n` in *this* frame, without following a live
    /// binding. Used to recognize a re-import of a name already linked to the
    /// same cell.
    pub fn own_binding(&self, n: &str) -> Option<Value> {
        self.vars.get(n).map(|b| b.value.clone()).or_else(|| {
            self.global_environment
                .as_ref()
                .and_then(|record| record.object.own_value(n))
        })
    }

    /// Move the binding named `n` into an existing cell, so a name already
    /// promised to an importer becomes the one this scope reads and writes.
    ///
    /// This is how a cyclic import resolves: the importer bound a cell before
    /// the exporting module had run, and when the export finally executes the
    /// value must land in *that* cell rather than a fresh one.
    pub fn adopt_cell(&mut self, n: &str, cell: Rc<RefCell<Value>>) {
        if let Some(Value::Binding(ref existing)) = self.own_binding(n)
            && Rc::ptr_eq(existing, &cell)
        {
            return;
        }
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
        self.vars.get(n).map(|b| b.kind).or_else(|| {
            self.global_environment
                .as_ref()
                .filter(|record| record.object.own_value(n).is_some())
                .map(|_| BindKind::Var)
        })
    }

    pub fn set(&mut self, n: &str, v: Value) {
        if self.global_environment.is_some() && self.vars.get(n).is_none() {
            let _ = self.try_set(n, v);
            return;
        }
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
        if self.vars.get(n).is_none() {
            self.check_global_quota(n)?;
            if let Some(record) = self.global_environment.as_mut() {
                record.user_names.insert(n.into());
                return record.value().set_prop(n.into(), v);
            }
        }
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
            if self
                .property_attributes
                .get(n)
                .is_some_and(|attrs| !attrs.writable)
            {
                return AssignOutcome::ReadOnly;
            }
            if binding.silent_immutable {
                return AssignOutcome::ReadOnly;
            }
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
        if let Some(record) = &self.global_environment {
            if record.object.own_value(n).is_none() {
                return AssignOutcome::Missing;
            }
            if !record.object.meta.borrow().attrs_of(n).writable {
                return AssignOutcome::ReadOnly;
            }
            record
                .value()
                .set_prop(n.into(), v)
                .expect("existing global property");
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
            let value = f(binding.value.deref_binding());
            if binding.silent_immutable {
                return ModifyOutcome::ReadOnly(value);
            }
            match &binding.value {
                Value::Binding(cell) => cell
                    .borrow_mut()
                    .assign_for_execution(value.clone_for_execution()),
                _ => binding
                    .value
                    .assign_for_execution(value.clone_for_execution()),
            }
            return ModifyOutcome::Updated(value);
        }
        if let Some(record) = &self.global_environment {
            let Some(current) = record.object.own_value(n) else {
                return ModifyOutcome::Missing;
            };
            let value = f(current.deref_binding());
            if !record.object.meta.borrow().attrs_of(n).writable {
                return ModifyOutcome::ReadOnly(value);
            }
            record
                .value()
                .set_prop(n.into(), value.clone())
                .expect("existing global property");
            return ModifyOutcome::Updated(value);
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
            || self
                .global_environment
                .as_ref()
                .is_some_and(|record| record.object.own_value(n).is_some())
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
        let mut values = self.vars.values_cloned();
        values.extend(
            self.global_environment
                .as_ref()
                .map(GlobalEnvironment::value),
        );
        values.extend(self.with_object.iter().cloned());
        values.extend(self.new_target.iter().cloned());
        values.extend(self.constructor_this.iter().flatten().cloned());
        values.extend(self.intrinsics.values().cloned());
        values
    }

    /// Parent link for the cycle collector's marker.
    pub(crate) fn trace_parent(&self) -> Option<Env> {
        self.parent.clone()
    }

    /// Drop this frame's bindings and parent link so an unreachable cycle
    /// can free. Only the collector calls this, on unmarked environments.
    #[doc(hidden)]
    pub fn clear_edges(&mut self) {
        self.vars.clear();
        self.global_environment = None;
        self.with_object = None;
        self.property_attributes.clear();
        self.intrinsics.clear();
        self.new_target = None;
        self.constructor_this = None;
        self.constructor_fields = None;
        self.parent = None;
        self.module_realm = None;
    }

    pub(crate) fn drain_chain(env: Env, work: &mut Vec<Value>) {
        let mut cur = Some(env);
        while let Some(e) = cur {
            match Rc::try_unwrap(e) {
                Ok(cell) => {
                    let mut env = cell.into_inner();
                    env.vars.drain_into(work);
                    work.extend(env.global_environment.take().map(|record| record.value()));
                    work.extend(env.with_object.take());
                    work.extend(env.new_target.take());
                    work.extend(env.constructor_this.take().flatten());
                    work.extend(env.intrinsics.drain().map(|(_, value)| value));
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
        let mut keys: Vec<String> = match &self.vars {
            Vars::Small(v) => v.iter().map(|(k, _)| k.to_string()).collect(),
            Vars::Large(m) => m.keys().map(|k| k.to_string()).collect(),
        };
        if let Some(record) = &self.global_environment {
            keys.extend(record.object.borrow().iter().map(|(name, _)| name.clone()));
        }
        keys
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
    /// Shared namespace identity, also shared by cloned export records.
    pub namespace: Rc<RefCell<Option<Value>>>,
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
