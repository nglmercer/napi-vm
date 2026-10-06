//! Dependency promises schedule module bodies without blocking the owner. Back
//! edges within a DFS use the already-instantiated cells rather than awaiting
//! themselves. Completion/failure is shared by all concurrent importers.
use super::*;
use crate::value::{PromiseInner, PromiseState};
const DRIVER: &str = "__symbol_module_driver__";
impl Interpreter {
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
        let record = self
            .module_graph
            .borrow()
            .records
            .get(id)
            .map(|r| (r.state.clone(), r.evaluation.clone(), r.requests.clone()));
        let Some((state, evaluation, requests)) = record else {
            return Ok(Value::settled_promise(
                PromiseState::Fulfilled,
                Self::namespace_object(&self.module(id).expect("linked module"))?,
            ));
        };
        if let Some(promise) = evaluation {
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
            record.evaluation_pin =
                Some(crate::heap::RootPin::new(Value::Promise(promise.clone())));
            record.state = ModuleState::Evaluating;
        }
        ancestors.insert(id.into());
        let mut dependencies = Vec::new();
        for (_, dependency) in requests {
            if !ancestors.contains(&dependency) {
                match self.start_async_module(&dependency, ancestors) {
                    Ok(dependency) => dependencies.push(dependency),
                    Err(error) => {
                        self.finish_async_module(id, &promise, Err(error_value(&error)))?;
                        ancestors.remove(id);
                        return Ok(Value::Promise(promise));
                    }
                }
            }
        }
        ancestors.remove(id);
        let guard = Value::object(vec![
            ("id".into(), Value::String(id.into())),
            ("target".into(), Value::Promise(promise.clone())),
            ("remaining".into(), Value::Number(dependencies.len() as f64)),
        ]);
        if dependencies.is_empty() {
            self.jobs
                .borrow_mut()
                .push_microtask(Job::ModuleEvaluation {
                    id: id.into(),
                    target: promise.clone(),
                });
        } else {
            for dependency in dependencies {
                if let Some(inner) = dependency.as_promise()
                    && inner.borrow().external_pending
                {
                    promise.borrow_mut().external_pending = true;
                }
                self.register(
                    &dependency,
                    handler(&guard, dependency_fulfilled),
                    handler(&guard, dependency_rejected),
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
                if let Some(inner) = completion.as_promise()
                    && inner.borrow().external_pending
                {
                    target.borrow_mut().external_pending = true;
                }
                let guard = Value::object(vec![
                    ("id".into(), Value::String(id.into())),
                    ("target".into(), Value::Promise(target.clone())),
                ]);
                self.register(
                    &completion,
                    handler(&guard, body_fulfilled),
                    handler(&guard, dependency_rejected),
                    None,
                )?;
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
                record.failure_pin = Some(crate::heap::RootPin::new(value.clone()));
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
) -> Value {
    Value::object(vec![
        (DRIVER.into(), guard.clone()),
        (
            super::super::call::CALL_SLOT.into(),
            Value::NativeFunction {
                name: "".into(),
                callable,
            },
        ),
    ])
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
                .push_microtask(Job::ModuleEvaluation { id, target });
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
