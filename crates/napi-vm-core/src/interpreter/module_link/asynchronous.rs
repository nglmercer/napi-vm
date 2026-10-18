//! Dependency promises schedule module bodies without blocking the owner. Back
//! edges within a DFS use the already-instantiated cells rather than awaiting
//! themselves. Completion/failure is shared by all concurrent importers.
use super::*;
use crate::value::{PromiseInner, PromiseState};
const DRIVER: &str = "__symbol_module_driver__";
impl Interpreter {
    fn gather_async_dependencies(
        &self,
        id: &str,
        seen: &mut HashSet<String>,
        out: &mut Vec<String>,
    ) {
        if !seen.insert(id.into()) {
            return;
        }
        let record = self.module_graph.borrow().records.get(id).map(|record| {
            (
                record.state.clone(),
                record.has_tla,
                record.cycle_root.clone(),
                record.requests.clone(),
            )
        });
        let Some((state, has_tla, cycle_root, requests)) = record else {
            return;
        };
        if matches!(state, ModuleState::Failed(_)) {
            return;
        }
        if matches!(state, ModuleState::Evaluated)
            && let Some(root) = cycle_root
            && self
                .module_graph
                .borrow()
                .records
                .get(&root)
                .is_some_and(|record| matches!(record.state, ModuleState::Evaluating))
        {
            out.push(root);
            return;
        }
        if has_tla && matches!(state, ModuleState::Linked | ModuleState::Evaluating) {
            out.push(id.into());
            return;
        }
        for (_, dependency, _) in requests {
            self.gather_async_dependencies(&dependency, seen, out);
        }
    }

    /// Start an import without blocking on top-level await. Poll the existing
    /// event loop to progress it; the returned Promise resolves to a namespace.
    pub fn import_module(&mut self, specifier: &str) -> Result<Value, VmErr> {
        let result = (|| {
            let id = if self.cur_mod.is_none() && self.has_module(specifier) {
                specifier.to_string()
            } else {
                self.resolve_module_request(specifier)?.ok_or_else(|| {
                    VmErr::Msg(format!("TypeError: Module not found: {specifier}"))
                })?
            };
            if !self.link_module(&id)? {
                return Err(VmErr::Msg(format!("TypeError: Module not found: {id}")));
            }
            self.start_async_module(&id, &mut HashSet::new())
        })();
        Ok(match result {
            Ok(promise) => promise,
            Err(error) => Value::settled_promise(PromiseState::Rejected, error_value(&error)),
        })
    }
    fn start_async_module(
        &mut self,
        id: &str,
        ancestors: &mut HashSet<String>,
    ) -> Result<Value, VmErr> {
        let record = self.module_graph.borrow().records.get(id).map(|r| {
            (
                r.state.clone(),
                r.evaluation.clone(),
                r.cycle_root.clone(),
                r.requests.clone(),
            )
        });
        let Some((state, evaluation, cycle_root, requests)) = record else {
            return Ok(Value::settled_promise(
                PromiseState::Fulfilled,
                Self::namespace_object(&self.module(id).expect("linked module"))?,
            ));
        };
        if let Some(promise) = evaluation {
            if let Some(root) = cycle_root
                && !ancestors.contains(&root)
                && let Some(root_promise) = self
                    .module_graph
                    .borrow()
                    .records
                    .get(&root)
                    .and_then(|record| record.evaluation.clone())
                && root_promise.borrow().state == PromiseState::Pending
            {
                return Ok(Value::Promise(root_promise));
            }
            return Ok(Value::Promise(promise));
        }
        match state {
            ModuleState::Evaluated => {
                return Ok(Value::settled_promise(
                    PromiseState::Fulfilled,
                    Self::namespace_object(&self.module(id).unwrap())?,
                ));
            }
            ModuleState::Failed(value) => {
                return Ok(Value::settled_promise(PromiseState::Rejected, value));
            }
            _ => {}
        }
        if ancestors.len() >= MAX_CALL_DEPTH {
            return Err(crate::value::limit_err(
                "Maximum module evaluation depth exceeded",
            ));
        }
        let promise = Value::pending_promise();
        {
            let mut graph = self.module_graph.borrow_mut();
            let record = graph.records.get_mut(id).unwrap();
            record.evaluation = Some(promise.clone());
            record.state = ModuleState::Evaluating;
        }
        ancestors.insert(id.into());
        let mut dependencies = Vec::new();
        let mut scheduled = HashSet::new();
        for (_, dependency, phase) in requests {
            if phase == crate::parser::ImportPhase::Deferred {
                let mut asynchronous = Vec::new();
                self.gather_async_dependencies(&dependency, &mut HashSet::new(), &mut asynchronous);
                for asynchronous in asynchronous {
                    if ancestors.contains(&asynchronous) || !scheduled.insert(asynchronous.clone())
                    {
                        continue;
                    }
                    match self.start_async_module(&asynchronous, ancestors) {
                        Ok(dependency) => dependencies.push(dependency),
                        Err(error) => {
                            self.finish_async_module(id, &promise, Err(error_value(&error)))?;
                            ancestors.remove(id);
                            return Ok(Value::Promise(promise));
                        }
                    }
                }
                continue;
            }
            if !ancestors.contains(&dependency) && scheduled.insert(dependency.clone()) {
                match self.start_async_module(&dependency, ancestors) {
                    Ok(dependency) => dependencies.push(dependency),
                    Err(error) => {
                        self.finish_async_module(id, &promise, Err(error_value(&error)))?;
                        ancestors.remove(id);
                        return Ok(Value::Promise(promise));
                    }
                }
            } else if ancestors.contains(&dependency) {
                let mut graph = self.module_graph.borrow_mut();
                let record = graph.records.get_mut(id).expect("evaluating module");
                if record.cycle_root.is_none() {
                    record.cycle_root = Some(dependency);
                }
            }
        }
        ancestors.remove(id);
        let guard = Value::object(vec![
            ("id".into(), Value::String(id.into())),
            ("target".into(), Value::Promise(promise.clone())),
            ("remaining".into(), Value::Number(dependencies.len() as f64)),
        ]);
        let mut pending = false;
        for dependency in &dependencies {
            if let Some(inner) = dependency.as_promise() {
                let (state, value, external_pending) = {
                    let inner = inner.borrow();
                    (inner.state, inner.value.clone(), inner.external_pending)
                };
                if external_pending {
                    promise.borrow_mut().external_pending = true;
                }
                match state {
                    PromiseState::Pending => pending = true,
                    PromiseState::Rejected => {
                        self.finish_async_module(id, &promise, Err(value))?;
                        return Ok(Value::Promise(promise));
                    }
                    PromiseState::Fulfilled => {}
                }
            }
        }
        if !pending {
            self.run_module_evaluation_job(id, &promise)?;
        } else {
            for dependency in dependencies {
                self.register(
                    &dependency,
                    handler(&guard, dependency_fulfilled, &self.persistent_global),
                    handler(&guard, dependency_rejected, &self.persistent_global),
                    None,
                )?;
            }
        }
        Ok(Value::Promise(promise))
    }
    pub(crate) fn run_module_evaluation_job(
        &mut self,
        id: &str,
        target: &Rc<RefCell<PromiseInner>>,
    ) -> Result<(), VmErr> {
        if target.borrow().state != PromiseState::Pending {
            return Ok(());
        }
        let program = self
            .module_graph
            .borrow()
            .records
            .get(id)
            .filter(|r| r.evaluation.as_ref().is_some_and(|p| Rc::ptr_eq(p, target)))
            .map(|r| r.program.clone());
        let Some(program) = program else {
            self.reject_promise(
                target,
                Value::Error(crate::value::ErrorData::new(
                    "TypeError",
                    "Module was removed or replaced before evaluation",
                )),
            );
            return Ok(());
        };
        let outer = self.cur_mod.replace(id.into());
        let scope = self.module_scope(id);
        let saved = std::mem::replace(&mut self.global, scope.clone());
        self.evaluating.borrow_mut().insert(id.into());
        #[cfg(stackful_coroutines)]
        let result = super::super::async_fn::spawn_module(
            self,
            Rc::new(linked_body(&program.statements)),
            scope,
        );
        #[cfg(not(stackful_coroutines))]
        let result = self
            .execute_prepared_raw(&program)
            .map(|_| Value::settled_promise(PromiseState::Fulfilled, Value::Undefined));
        self.global = saved;
        self.cur_mod = outer;
        match result {
            Ok(completion) => {
                if let Some(inner) = completion.as_promise() {
                    let (state, value, external_pending) = {
                        let inner = inner.borrow();
                        (inner.state, inner.value.clone(), inner.external_pending)
                    };
                    if external_pending {
                        target.borrow_mut().external_pending = true;
                    }
                    match state {
                        PromiseState::Fulfilled => self.finish_async_module(id, target, Ok(()))?,
                        PromiseState::Rejected => {
                            self.finish_async_module(id, target, Err(value))?
                        }
                        PromiseState::Pending => {
                            let guard = Value::object(vec![
                                ("id".into(), Value::String(id.into())),
                                ("target".into(), Value::Promise(target.clone())),
                            ]);
                            self.register(
                                &completion,
                                handler(&guard, body_fulfilled, &self.persistent_global),
                                handler(&guard, dependency_rejected, &self.persistent_global),
                                None,
                            )?;
                        }
                    }
                } else {
                    self.finish_async_module(id, target, Ok(()))?;
                }
            }
            Err(error) => self.finish_async_module(id, target, Err(error_value(&error)))?,
        }
        Ok(())
    }
    fn finish_async_module(
        &mut self,
        id: &str,
        target: &Rc<RefCell<PromiseInner>>,
        result: Result<(), Value>,
    ) -> Result<(), VmErr> {
        if target.borrow().state != PromiseState::Pending {
            return Ok(());
        }
        let current = self
            .module_graph
            .borrow()
            .records
            .get(id)
            .is_some_and(|r| r.evaluation.as_ref().is_some_and(|p| Rc::ptr_eq(p, target)));
        if !current {
            self.reject_promise(
                target,
                Value::Error(crate::value::ErrorData::new(
                    "TypeError",
                    "Module was removed or replaced during evaluation",
                )),
            );
            return Ok(());
        }
        self.evaluating.borrow_mut().remove(id);
        match result {
            Ok(()) => {
                self.module_graph
                    .borrow_mut()
                    .records
                    .get_mut(id)
                    .unwrap()
                    .state = ModuleState::Evaluated;
                let namespace =
                    Self::namespace_object(&self.module(id).expect("evaluated module"))?;
                self.resolve_promise(target, namespace)?;
            }
            Err(value) => {
                let mut graph = self.module_graph.borrow_mut();
                let record = graph.records.get_mut(id).unwrap();
                record.state = ModuleState::Failed(value.clone());
                drop(graph);
                self.reject_promise(target, value);
            }
        }
        Ok(())
    }
}
fn handler(
    guard: &Value,
    callable: fn(&mut Interpreter, Value, Vec<Value>) -> Result<Value, VmErr>,
    realm: &Env,
) -> Value {
    let value = Value::object(vec![
        (DRIVER.into(), guard.clone()),
        (
            super::super::call::CALL_SLOT.into(),
            Value::NativeFunction {
                name: "".into(),
                callable,
            },
        ),
    ]);
    if let Value::Object { props } = &value {
        props.meta.borrow_mut().realm_global = Some(realm.clone());
    }
    value
}
fn driver(this: &Value) -> Result<(Value, String, Rc<RefCell<PromiseInner>>), VmErr> {
    let guard = this
        .get_prop(DRIVER)
        .ok_or_else(|| VmErr::Msg("internal error: module driver missing".into()))?;
    let Some(Value::String(ref id)) = guard.get_prop("id") else {
        return Err(VmErr::Msg("internal error: module identity missing".into()));
    };
    let target = guard
        .get_prop("target")
        .and_then(|v| v.as_promise())
        .ok_or_else(|| VmErr::Msg("internal error: module promise missing".into()))?;
    Ok((guard, id.to_string(), target))
}
fn dependency_fulfilled(
    interp: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    let (guard, id, target) = driver(&this)?;
    if target.borrow().state == PromiseState::Pending {
        let remaining = guard
            .get_prop("remaining")
            .unwrap_or(Value::Number(1.))
            .to_number()
            - 1.;
        guard.set_prop("remaining".into(), Value::Number(remaining))?;
        if remaining == 0. {
            interp
                .jobs
                .borrow_mut()
                .push_microtask(Job::ModuleEvaluation {
                    realm: interp.persistent_global.clone(),
                    id,
                    target,
                });
        }
    }
    Ok(Value::Undefined)
}
fn dependency_rejected(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let (_, id, target) = driver(&this)?;
    interp.finish_async_module(
        &id,
        &target,
        Err(args.into_iter().next().unwrap_or(Value::Undefined)),
    )?;
    Ok(Value::Undefined)
}
fn body_fulfilled(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let (_, id, target) = driver(&this)?;
    interp.finish_async_module(&id, &target, Ok(()))?;
    Ok(Value::Undefined)
}
