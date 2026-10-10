//! Async-generator requests and await reactions, all on the agent owner thread.
//! The body uses the existing generator coroutine; no worker enters the VM.
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use super::{Env, Interpreter};
use crate::error::{VmErr, error_value_from_msg};
use crate::value::{GenResume, GenSuspension, GeneratorInner, PromiseInner, Value};

struct Request {
    completion: GenResume,
    promise: Rc<RefCell<PromiseInner>>,
    realm: Env,
}

pub(crate) struct AsyncGeneratorState {
    requests: VecDeque<Request>,
    busy: bool,
    pub(crate) suspension: Rc<Cell<GenSuspension>>,
}

impl AsyncGeneratorState {
    pub(crate) fn new() -> Self {
        Self {
            requests: VecDeque::new(),
            busy: false,
            suspension: Rc::new(Cell::new(GenSuspension::Yield)),
        }
    }

    pub(crate) fn trace_values(&self) -> Vec<Value> {
        let mut values = Vec::new();
        for request in &self.requests {
            values.push(Value::Promise(request.promise.clone()));
            values.push(Value::RealmGlobal(request.realm.clone()));
            match &request.completion {
                GenResume::Next(value) => values.extend(value.clone()),
                GenResume::Throw(value) | GenResume::Return(value) => values.push(value.clone()),
                GenResume::Abandon => {}
            }
        }
        values
    }
}

pub(crate) fn next(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    enqueue(vm, this, GenResume::Next(args.first().cloned()))
}
pub(crate) fn return_(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    enqueue(
        vm,
        this,
        GenResume::Return(args.first().cloned().unwrap_or(Value::Undefined)),
    )
}
pub(crate) fn throw(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    enqueue(
        vm,
        this,
        GenResume::Throw(args.first().cloned().unwrap_or(Value::Undefined)),
    )
}

fn enqueue(vm: &mut Interpreter, this: Value, completion: GenResume) -> Result<Value, VmErr> {
    let promise = Value::pending_promise();
    let valid = matches!(&this, Value::Generator { inner } if inner.borrow().async_state.is_some());
    if !valid {
        vm.reject_promise(
            &promise,
            error_value_from_msg("TypeError: Method requires an async generator receiver"),
        );
        return Ok(Value::Promise(promise));
    }
    let Value::Generator { inner } = &this else {
        unreachable!()
    };
    {
        let mut generator = inner.borrow_mut();
        let state = generator.async_state.as_mut().expect("async generator");
        if state.requests.len() >= crate::value::MAX_ARRAY_LEN {
            vm.reject_promise(
                &promise,
                error_value_from_msg("RangeError: Async generator request capacity exceeded"),
            );
            return Ok(Value::Promise(promise));
        }
        state.requests.push_back(Request {
            completion,
            promise: promise.clone(),
            realm: vm.persistent_global.clone(),
        });
    }
    drive(vm, inner)?;
    Ok(Value::Promise(promise))
}

fn drive(vm: &mut Interpreter, inner: &Rc<RefCell<GeneratorInner>>) -> Result<(), VmErr> {
    loop {
        let (completion, was_complete) = {
            let mut generator = inner.borrow_mut();
            let done = generator.done;
            let started = generator.started;
            let state = generator.async_state.as_mut().expect("async generator");
            if state.busy {
                return Ok(());
            }
            let Some(request) = state.requests.front() else {
                return Ok(());
            };
            state.busy = true;
            let completion = request.completion.clone();
            let close_before_body = matches!(&completion, GenResume::Return(_)) && !started;
            if close_before_body {
                generator.done = true;
            }
            (completion, done || close_before_body)
        };
        if was_complete && let GenResume::Return(value) = &completion {
            await_value(vm, inner, value.clone(), Some(true))?;
            return Ok(());
        }
        let generator = Value::Generator {
            inner: inner.clone(),
        };
        let outcome = match completion {
            GenResume::Next(value) => {
                super::call::generator_next_driver(vm, generator, value.into_iter().collect())
            }
            GenResume::Throw(value) => {
                super::call::generator_throw_driver(vm, generator, vec![value])
            }
            GenResume::Return(value) => {
                super::call::generator_return_driver(vm, generator, vec![value])
            }
            GenResume::Abandon => unreachable!("not a guest request"),
        };
        match outcome {
            Ok(result) => {
                // These result objects are produced by the shared driver and
                // have not been exposed to guest code.
                let done = result
                    .get_prop("done")
                    .is_some_and(|value| value.is_truthy());
                let value = result.get_prop("value").unwrap_or(Value::Undefined);
                let awaiting = inner
                    .borrow()
                    .async_state
                    .as_ref()
                    .expect("async generator")
                    .suspension
                    .get()
                    == GenSuspension::Await;
                if !done && awaiting {
                    await_value(vm, inner, value, None)?;
                    return Ok(());
                }
                #[cfg(not(stackful_coroutines))]
                if !was_complete {
                    await_value(vm, inner, value, Some(done))?;
                    return Ok(());
                }
                settle(vm, inner, Ok((value, done)))?;
            }
            Err(error) => {
                let reason = match error {
                    VmErr::Throw(value) => value,
                    VmErr::RuntimeError(error) => error.guest_value(),
                    error => error_value_from_msg(&error.to_string()),
                };
                settle(vm, inner, Err(reason))?;
            }
        }
    }
}

const GENERATOR_SLOT: &str = "__symbol_async_generator__";
const FINISH_SLOT: &str = "__symbol_async_generator_finish__";

fn await_value(
    vm: &mut Interpreter,
    inner: &Rc<RefCell<GeneratorInner>>,
    value: Value,
    finish: Option<bool>,
) -> Result<(), VmErr> {
    // Body Await has already performed PromiseResolve before suspension.
    // A return request on a completed/unstarted generator has no body and
    // must perform the same operation here, rejecting the request on error.
    let value = if finish.is_some() {
        match vm.promise_resolve_intrinsic(value) {
            Ok(value) => value,
            Err(error) => {
                let reason = match error {
                    VmErr::Throw(value) => value,
                    VmErr::RuntimeError(error) => error.guest_value(),
                    error => error_value_from_msg(&error.to_string()),
                };
                settle(vm, inner, Err(reason))?;
                return drive(vm, inner);
            }
        }
    } else {
        value
    };
    let make = |callable: crate::builtins::NativeFn| {
        Value::object(vec![
            (
                GENERATOR_SLOT.into(),
                Value::Generator {
                    inner: inner.clone(),
                },
            ),
            (
                FINISH_SLOT.into(),
                finish.map(Value::Bool).unwrap_or(Value::Undefined),
            ),
            (
                super::call::CALL_SLOT.into(),
                Value::NativeFunction {
                    name: "".into(),
                    callable,
                },
            ),
        ])
    };
    let promise = inner
        .borrow()
        .async_state
        .as_ref()
        .expect("async generator")
        .requests
        .front()
        .expect("active request")
        .promise
        .clone();
    if let Some(awaited) = value.as_promise() {
        promise.borrow_mut().external_pending = awaited.borrow().external_pending;
    }
    vm.register(&value, make(resume_fulfilled), make(resume_rejected), None)?;
    Ok(())
}

fn settle(
    vm: &mut Interpreter,
    inner: &Rc<RefCell<GeneratorInner>>,
    completion: Result<(Value, bool), Value>,
) -> Result<(), VmErr> {
    let request = inner
        .borrow_mut()
        .async_state
        .as_mut()
        .expect("async generator")
        .requests
        .pop_front()
        .expect("active request");
    let _realm = super::realm::AllocationRealm::enter(Some(request.realm));
    match completion {
        Ok((value, done)) => {
            vm.resolve_promise(&request.promise, super::call::iter_result(value, done))?
        }
        Err(reason) => vm.reject_promise(&request.promise, reason),
    }
    inner
        .borrow_mut()
        .async_state
        .as_mut()
        .expect("async generator")
        .busy = false;
    Ok(())
}

fn resume_fulfilled(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    resume(
        vm,
        this,
        args.first().cloned().unwrap_or(Value::Undefined),
        false,
    )
}
fn resume_rejected(vm: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    resume(
        vm,
        this,
        args.first().cloned().unwrap_or(Value::Undefined),
        true,
    )
}

fn resume(vm: &mut Interpreter, this: Value, value: Value, rejected: bool) -> Result<Value, VmErr> {
    let generator = this.get_prop(GENERATOR_SLOT);
    let Some(Value::Generator { inner }) = generator.as_ref() else {
        return Ok(Value::Undefined);
    };
    let finish = this.get_prop(FINISH_SLOT);
    if let Some(Value::Bool(done)) = finish.as_ref() {
        settle(
            vm,
            inner,
            if rejected {
                Err(value)
            } else {
                Ok((value, *done))
            },
        )?;
    } else {
        // Resume the same active request, rather than enqueueing a new one.
        let mut generator = inner.borrow_mut();
        let state = generator.async_state.as_mut().expect("async generator");
        state
            .requests
            .front_mut()
            .expect("active request")
            .completion = if rejected {
            GenResume::Throw(value)
        } else {
            GenResume::Next(Some(value))
        };
        state.busy = false;
    }
    drive(vm, inner)?;
    Ok(Value::Undefined)
}
