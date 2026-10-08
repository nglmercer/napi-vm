//! Realm-owned globals and intrinsic allocation context.
use super::{Env, Interpreter};
use crate::{Value, error::VmErr};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

thread_local! {
    static ALLOCATION_REALM: RefCell<Option<Env>> = const { RefCell::new(None) };
}

pub(crate) struct AllocationRealm(Option<Env>);
impl AllocationRealm {
    pub(crate) fn enter(global: Option<Env>) -> Self {
        Self(ALLOCATION_REALM.with(|slot| slot.replace(global)))
    }
}
impl Drop for AllocationRealm {
    fn drop(&mut self) {
        ALLOCATION_REALM.with(|slot| slot.replace(self.0.take()));
    }
}
pub(crate) fn allocation_global() -> Option<Env> {
    ALLOCATION_REALM.with(|slot| slot.borrow().clone())
}
pub(crate) fn allocation_prototype(name: &str) -> Option<Rc<Value>> {
    allocation_global()?
        .borrow()
        .intrinsic(name)?
        .get_prop("prototype")
        .map(Rc::new)
}

pub(crate) fn value_realm(value: &Value) -> Option<Env> {
    match value {
        Value::RealmGlobal(global) => Some(global.clone()),
        Value::Function(function) if function.bound.is_some() => {
            value_realm(&function.bound.as_ref().expect("bound function").target)
        }
        Value::Function(function) => function
            .closure
            .as_ref()
            .and_then(super::Environment::find_global)
            .or_else(|| function.properties.meta.borrow().realm_global.clone()),
        Value::Object { props } => props.meta.borrow().realm_global.clone(),
        Value::Array(array) => array.meta.borrow().realm_global.clone(),
        Value::Class(class) => class
            .statics
            .meta
            .borrow()
            .realm_global
            .clone()
            .or_else(|| value_realm(&class.constructor)),
        Value::Proxy(proxy) => value_realm(&proxy.target),
        _ => value
            .exotic_properties()
            .and_then(|properties| properties.meta.borrow().realm_global.clone()),
    }
}

/// Associate bootstrap values with their owning realm, including native
/// constructors and methods which do not have lexical closure environments.
pub(crate) fn own_intrinsics(global: &Env) {
    let mut work = global.borrow().trace_values();
    if let Some(parent) = global.borrow().parent_env() {
        work.extend(parent.borrow().trace_values());
    }
    let mut seen = HashSet::new();
    let mut native_methods = HashMap::new();
    let function_prototype = crate::value::FunctionData::default_function_prototype(global);
    while let Some(value) = work.pop() {
        if let Value::Array(array) = &value {
            if seen.insert(Rc::as_ptr(array) as usize) {
                work.extend(array.trace_children());
                array.meta.borrow_mut().realm_global = Some(global.clone());
            }
            continue;
        }
        let cell = match &value {
            Value::Object { props } => props.clone(),
            Value::Function(function) => function.properties.clone(),
            Value::Class(class) => class.statics.clone(),
            _ => continue,
        };
        if !seen.insert(Rc::as_ptr(&cell) as usize) {
            continue;
        }
        // Bootstrap native slots become ordinary realm-owned function objects.
        // Preserve aliases through the native value's shared identity token.
        for (_, slot) in cell.borrow_mut().iter_mut() {
            if let Value::NativeFunction { name, callable } = slot {
                let id = Rc::as_ptr(name) as *const () as usize;
                let method = native_methods.entry(id).or_insert_with(|| {
                    let method = crate::builtins::native_method(
                        name,
                        0,
                        *callable,
                        function_prototype.clone(),
                    );
                    if let Value::Function(function) = &method {
                        function.properties.meta.borrow_mut().realm_global = Some(global.clone());
                    }
                    method
                });
                *slot = method.clone();
            }
        }
        // Read children before introducing the realm back-edge.
        work.extend(cell.trace_children());
        cell.meta.borrow_mut().realm_global = Some(global.clone());
    }
}

impl Interpreter {
    /// Create an isolated ECMAScript realm with fresh globals and intrinsics.
    /// The realm shares this agent's scheduler and execution limits. Runtime
    /// globals are not installed into the new realm.
    pub fn create_realm(&self) -> Self {
        let mut child = Self::with_builtins();
        let modules = super::ModuleRealm::of(&child);
        super::Realm::of(self).install(&mut child);
        modules.install(&mut child);
        super::ModuleRealm::fork_sources(self, &mut child);
        child.republish_roots();
        child
    }

    pub(crate) fn with_global_storage<R>(
        &mut self,
        global: Env,
        operation: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let saved_modules = self.enter_module_realm(&global);
        let saved = std::mem::replace(&mut self.persistent_global, global);
        let result = operation(self);
        self.persistent_global = saved;
        saved_modules.install(self);
        result
    }

    pub(crate) fn constructor_prototype(
        &mut self,
        target: &Value,
        builtin: &str,
    ) -> Result<Option<Rc<Value>>, VmErr> {
        let prototype = self.get_prop_value_str(target, "prototype")?;
        if super::call::is_js_object(&prototype) {
            return Ok(Some(Rc::new(prototype)));
        }
        let owner = value_realm(target).unwrap_or_else(|| self.persistent_global.clone());
        Ok(owner
            .borrow()
            .intrinsic(builtin)
            .and_then(|constructor| constructor.get_prop("prototype"))
            .map(Rc::new))
    }

    /// Create a host function retaining this realm's identity and intrinsics.
    pub fn native_function_in_realm(
        &self,
        name: &str,
        callable: crate::builtins::NativeFn,
    ) -> Value {
        let global = &self.persistent_global;
        Value::Function(Rc::new(crate::value::FunctionData {
            strict: true,
            native: Some(callable),
            identity: Rc::new(0),
            name: Some(name.into()),
            properties: crate::value::FunctionData::properties_with_default_prototype(global),
            standard_properties_initialized: Rc::new(std::cell::Cell::new(false)),
            params: Rc::new(Vec::new()),
            body: Rc::new(Vec::new()),
            closure: Some(crate::heap::capture_env(global)),
            is_arrow: false,
            is_constructor: false,
            is_async: false,
            is_generator: false,
            uses_arguments: false,
            needs_hoisting: false,
            bound: None,
            bytecode: None,
        }))
    }

    pub fn realm_global_object(&self) -> Value {
        Value::RealmGlobal(self.persistent_global.clone())
    }

    pub(crate) fn global_scope_of(&self, value: &Value) -> Option<Env> {
        match value {
            Value::GlobalObject => Some(self.persistent_global.clone()),
            Value::RealmGlobal(global) => Some(global.clone()),
            _ => None,
        }
    }

    /// Import native shared backing memory as a fresh SAB in this realm.
    /// The data block is transferable; its guest wrapper and realm are not.
    pub fn shared_array_buffer_from_memory(&self, memory: crate::value::SharedMemory) -> Value {
        let _allocation_realm = AllocationRealm::enter(Some(self.persistent_global.clone()));
        Value::SharedArrayBuffer(crate::value::SharedBuffer::from_shared_memory(memory))
    }

    /// Evaluate a script in a realm while keeping the shared agent scheduler.
    pub fn eval_in_realm(&mut self, global: &Value, source: &str) -> Result<Value, VmErr> {
        self.eval_in_realm_utf16(global, &crate::JsString::from(source))
    }

    /// Evaluate realm script source without replacing unpaired surrogates.
    pub fn eval_in_realm_utf16(
        &mut self,
        global: &Value,
        source: &crate::JsString,
    ) -> Result<Value, VmErr> {
        let target = self
            .global_scope_of(global)
            .ok_or_else(|| VmErr::Msg("TypeError: expected a realm global".into()))?;
        let saved_modules = self.enter_module_realm(&target);
        let saved_strict = target.borrow_mut().replace_strict(Some(false));
        let target_context = target.clone();
        let saved_persistent = std::mem::replace(&mut self.persistent_global, target.clone());
        let saved_scope = std::mem::replace(&mut self.global, target);
        let saved_module = self.cur_mod.take();
        let result = Self::compile_utf16_with_goal(source, crate::parser::ParseGoal::Script)
            .and_then(|program| {
                self.execute_with_options(
                    &program,
                    super::EvaluationOptions {
                        drain: super::DrainPolicy::None,
                        ..Default::default()
                    },
                )
            });
        target_context.borrow_mut().replace_strict(saved_strict);
        self.cur_mod = saved_module;
        self.global = saved_scope;
        self.persistent_global = saved_persistent;
        saved_modules.install(self);
        self.republish_roots();
        result
    }
}

#[cfg(all(test, stackful_coroutines))]
mod tests {
    use super::*;

    #[test]
    fn abandoning_a_suspended_stack_restores_the_host_allocation_context() {
        use crate::value::{GenOutcome, GenResume};
        let vm = Interpreter::with_builtins();
        let owner = vm.persistent_global.clone();
        let mut coroutine = corosensei::Coroutine::new(move |yielder, _| {
            let _guard = AllocationRealm::enter(Some(owner));
            yielder.suspend(Value::Undefined);
            GenOutcome::Abandon
        });
        {
            let _caller = AllocationRealm::enter(Some(vm.persistent_global.clone()));
            assert!(matches!(
                coroutine.resume(GenResume::Next(None)),
                corosensei::CoroutineResult::Yield(_)
            ));
        }
        assert!(allocation_global().is_none());
        crate::value::force_abandon(coroutine);
        assert!(allocation_global().is_none());
        assert!(Value::array(Vec::new()).proto_of().is_none());
    }
}
