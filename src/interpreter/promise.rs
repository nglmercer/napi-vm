//! Promise resolution and the event loop drain.
//!
//! The model is the specification's: a promise starts pending, settles once,
//! and every reaction runs as a *microtask* rather than inline. That is what
//! makes `Promise.resolve().then(f); g();` run `g` before `f`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use super::Interpreter;
use super::jobs::{Job, MAX_JOBS_PER_DRAIN, settle};
use crate::error::VmErr;
use crate::value::{PromiseInner, PromiseState, Reaction, Value};

fn is_callable(value: &Value) -> bool {
    matches!(
        value,
        Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
    ) || crate::interpreter::call::callable_slot(value, crate::interpreter::call::CALL_SLOT)
        .is_some()
}

fn promise_error_reason(error: VmErr) -> Value {
    match error {
        VmErr::Throw(reason) => reason,
        VmErr::RuntimeError(data) => crate::error::error_value_from_msg(&data.message),
        other => crate::error::error_value_from_msg(&other.to_string()),
    }
}

fn claim_resolution(guard: &Value) -> bool {
    if guard
        .get_prop("resolved")
        .is_some_and(|resolved| resolved.is_truthy())
    {
        return false;
    }
    guard
        .set_prop("resolved".to_owned(), Value::Bool(true))
        .is_ok()
}

impl Interpreter {
    /// Settle `promise` with `value` as its *resolution*, which is not the
    /// same as fulfilling it: resolving with a promise or a thenable adopts
    /// that object's eventual state instead of fulfilling with the object.
    pub(crate) fn resolve_promise(
        &mut self,
        promise: &Rc<RefCell<PromiseInner>>,
        value: Value,
    ) -> Result<(), VmErr> {
        {
            let mut inner = promise.borrow_mut();
            if inner.resolution_locked || inner.state != PromiseState::Pending {
                return Ok(());
            }
            inner.resolution_locked = true;
        }
        self.resolve_promise_inner(promise, value)
    }

    /// Continue a resolution after the promise's public resolver has already
    /// been used. This path is reserved for the one-shot resolve function
    /// supplied to a thenable job.
    fn resolve_promise_inner(
        &mut self,
        promise: &Rc<RefCell<PromiseInner>>,
        value: Value,
    ) -> Result<(), VmErr> {
        // Resolving a promise with itself is a cycle the specification rejects
        // rather than deadlocks on.
        if let Value::Promise(inner) = &value
            && Rc::ptr_eq(inner, promise)
        {
            settle(
                &self.jobs,
                promise,
                PromiseState::Rejected,
                Value::Error(crate::value::ErrorData::new(
                    "TypeError",
                    "Chaining cycle detected for promise".to_string(),
                )),
            );
            return Ok(());
        }

        if let Value::Promise(_) = &value {
            let target = promise.clone();
            self.adopt(&value, target)?;
            return Ok(());
        }

        // Thenable assimilation: any object with a callable `then` is treated
        // as a promise, which is how promises from other implementations
        // interoperate.
        if matches!(value, Value::Object { .. }) {
            let then = match self.member(&value, "then") {
                Ok(then) => then,
                Err(error) => {
                    settle(
                        &self.jobs,
                        promise,
                        PromiseState::Rejected,
                        promise_error_reason(error),
                    );
                    return Ok(());
                }
            };
            if is_callable(&then) {
                self.jobs
                    .borrow_mut()
                    .push_microtask(Job::PromiseResolveThenable {
                        target: promise.clone(),
                        thenable: value,
                        then,
                        resolution_guard: Value::object(vec![(
                            "resolved".to_owned(),
                            Value::Bool(false),
                        )]),
                    });
                return Ok(());
            }
        }

        settle(&self.jobs, promise, PromiseState::Fulfilled, value);
        Ok(())
    }

    pub(crate) fn reject_promise(&mut self, promise: &Rc<RefCell<PromiseInner>>, reason: Value) {
        {
            let mut inner = promise.borrow_mut();
            if inner.resolution_locked || inner.state != PromiseState::Pending {
                return;
            }
            inner.resolution_locked = true;
        }
        settle(&self.jobs, promise, PromiseState::Rejected, reason);
    }

    /// Make `target` follow `source`'s eventual state.
    fn adopt(&mut self, source: &Value, target: Rc<RefCell<PromiseInner>>) -> Result<(), VmErr> {
        self.register(source, Value::Undefined, Value::Undefined, Some(target))?;
        Ok(())
    }

    fn run_thenable_job(
        &mut self,
        target: Rc<RefCell<PromiseInner>>,
        thenable: &Value,
        then: &Value,
        resolution_guard: Value,
    ) -> Result<(), VmErr> {
        let (resolve, reject) =
            Self::thenable_settle_functions(target.clone(), resolution_guard.clone());
        match self.call_this(then, thenable.clone(), vec![resolve, reject]) {
            Ok(_) => Ok(()),
            Err(error) if claim_resolution(&resolution_guard) => {
                settle(
                    &self.jobs,
                    &target,
                    PromiseState::Rejected,
                    promise_error_reason(error),
                );
                Ok(())
            }
            // A thenable that throws after using either resolver cannot change
            // the promise's already chosen state.
            Err(_) => Ok(()),
        }
    }

    /// The `(resolve, reject)` pair handed to a `new Promise` executor. They
    /// carry the promise in a hidden property, since a native function is a
    /// bare pointer with nowhere else to keep state.
    pub(crate) fn settle_functions(
        &mut self,
        promise: Rc<RefCell<PromiseInner>>,
    ) -> (Value, Value) {
        let carrier = Value::Promise(promise);
        let resolve = Value::object(vec![
            (TARGET_SLOT.to_string(), carrier.clone()),
            (
                crate::interpreter::call::CALL_SLOT.to_string(),
                Value::NativeFunction {
                    name: "resolve".into(),
                    callable: executor_resolve,
                },
            ),
        ]);
        let reject = Value::object(vec![
            (TARGET_SLOT.to_string(), carrier),
            (
                crate::interpreter::call::CALL_SLOT.to_string(),
                Value::NativeFunction {
                    name: "reject".into(),
                    callable: executor_reject,
                },
            ),
        ]);
        (resolve, reject)
    }

    fn thenable_settle_functions(
        promise: Rc<RefCell<PromiseInner>>,
        resolution_guard: Value,
    ) -> (Value, Value) {
        let carrier = Value::Promise(promise);
        let make_resolver = |name: &str, callable| {
            Value::object(vec![
                (TARGET_SLOT.to_owned(), carrier.clone()),
                (RESOLUTION_GUARD_SLOT.to_owned(), resolution_guard.clone()),
                (
                    crate::interpreter::call::CALL_SLOT.to_owned(),
                    Value::NativeFunction {
                        name: name.into(),
                        callable,
                    },
                ),
            ])
        };
        (
            make_resolver("resolve", thenable_resolve),
            make_resolver("reject", thenable_reject),
        )
    }

    /// `p.then(onFulfilled, onRejected)`.
    ///
    /// Returns the derived promise. When `p` has already settled the reaction
    /// is queued immediately rather than run inline — the deferral is the
    /// observable part of the semantics.
    pub(crate) fn register(
        &mut self,
        promise: &Value,
        on_fulfilled: Value,
        on_rejected: Value,
        derived: Option<Rc<RefCell<PromiseInner>>>,
    ) -> Result<Value, VmErr> {
        let adopted = derived.is_some();
        let derived = derived.unwrap_or_else(Value::pending_promise);
        // A non-promise is registered on through `Promise.resolve(v)`, so its
        // handler still runs — as a microtask — rather than being skipped.
        // The combinators rely on this for their plain-value inputs.
        let wrapped;
        let inner = match promise.as_promise() {
            Some(inner) => inner,
            None => {
                let bridge = Value::pending_promise();
                self.resolve_promise(&bridge, promise.clone())?;
                wrapped = bridge;
                wrapped.clone()
            }
        };
        let inner = &inner;

        if inner.borrow().state == PromiseState::Pending && inner.borrow().external_pending {
            derived.borrow_mut().external_pending = true;
        }

        let reaction = Reaction {
            on_fulfilled,
            on_rejected,
            derived: derived.clone(),
            adopted,
        };
        let settled = {
            let mut state = inner.borrow_mut();
            state.handled = true;
            match state.state {
                PromiseState::Pending => {
                    state.reactions.push(reaction);
                    None
                }
                other => Some((other, state.value.clone(), reaction)),
            }
        };
        if let Some((state, value, reaction)) = settled {
            self.jobs.borrow_mut().push_microtask(Job::Reaction {
                state,
                value,
                reaction,
            });
        }
        Ok(Value::Promise(derived))
    }

    /// Run one reaction: call the handler for the settled state, then settle
    /// the derived promise with what it produced.
    fn run_reaction(
        &mut self,
        state: PromiseState,
        value: Value,
        reaction: Reaction,
    ) -> Result<(), VmErr> {
        if reaction.adopted {
            settle(&self.jobs, &reaction.derived, state, value);
            return Ok(());
        }
        let handler = match state {
            PromiseState::Fulfilled => &reaction.on_fulfilled,
            _ => &reaction.on_rejected,
        };
        if !is_callable(handler) {
            // No handler for this state: the settlement passes straight
            // through, which is what makes `p.then(f)` forward a rejection and
            // `p.catch(g)` forward a fulfilment.
            match state {
                PromiseState::Fulfilled => self.resolve_promise(&reaction.derived, value)?,
                _ => self.reject_promise(&reaction.derived, value),
            }
            return Ok(());
        }
        let handler = handler.clone();
        match self.call_this(&handler, Value::Undefined, vec![value]) {
            Ok(result) => self.resolve_promise(&reaction.derived, result)?,
            Err(VmErr::Throw(reason)) => self.reject_promise(&reaction.derived, reason),
            // A thrown host/runtime error becomes a rejection too, so one bad
            // handler cannot abort the whole drain.
            Err(VmErr::Msg(message)) => {
                let reason = crate::error::error_value_from_msg(&message);
                self.reject_promise(&reaction.derived, reason);
            }
            Err(VmErr::RuntimeError(data)) => {
                let reason = crate::error::error_value_from_msg(&data.message);
                self.reject_promise(&reaction.derived, reason);
            }
            Err(other) => return Err(other),
        }
        Ok(())
    }

    /// Run every queued microtask, then the earliest timer, until nothing is
    /// left — the event loop this VM runs at the end of each entry point.
    ///
    /// Bounded by [`MAX_JOBS_PER_DRAIN`] so a self-rescheduling chain raises a
    /// catchable `RangeError` instead of hanging the host.
    pub fn drain_jobs(&mut self) -> Result<(), VmErr> {
        let mut executed = 0usize;
        loop {
            self.enqueue_host_events(Duration::ZERO)?;
            let job = {
                let mut queue = self.jobs.borrow_mut();
                match queue.take_microtask() {
                    Some(job) => Some(job),
                    // External events and timers are macrotasks. Run one
                    // between microtask checkpoints.
                    None => match queue.take_external_event() {
                        Some(job) => Some(job),
                        None => queue.take_timer(),
                    },
                }
            };
            let Some(job) = job else { return Ok(()) };
            executed += 1;
            if executed > MAX_JOBS_PER_DRAIN {
                return Err(crate::value::limit_err("Maximum job count exceeded"));
            }
            match job {
                Job::Reaction {
                    state,
                    value,
                    reaction,
                } => self.run_reaction(state, value, reaction)?,
                Job::PromiseResolveThenable {
                    target,
                    thenable,
                    then,
                    resolution_guard,
                } => self.run_thenable_job(target, &thenable, &then, resolution_guard)?,
                Job::Callback { callback, args } => {
                    match self.call_this(&callback, Value::Undefined, args) {
                        Ok(_) => {}
                        // An uncaught error in a queued callback is reported
                        // like an uncaught exception on the event loop: it
                        // stops the drain rather than being swallowed.
                        Err(error) => return Err(error),
                    }
                }
                Job::HostCallback { callback } => {
                    self.run_host_callback(callback)?;
                }
                Job::HostPromiseSettled {
                    promise,
                    state,
                    value,
                } => self.settle_host_promise(promise, state, value)?,
                Job::HostUncaughtException { exception } => {
                    self.run_host_uncaught_exception(exception)?;
                }
            }
        }
    }

    /// Wait for one host-originated event, then run it on the interpreter
    /// thread and drain its microtasks. Desktop runtimes can call this from
    /// their own event loop; guest callbacks are never invoked by the
    /// sidecar/network thread.
    pub fn run_event_loop_once(&mut self, timeout: Duration) -> Result<bool, VmErr> {
        let ready = self.enqueue_host_events(Duration::ZERO)?;
        self.drain_jobs()?;
        if ready > 0 {
            return Ok(true);
        }
        let Some(bridge) = self.host.clone() else {
            return Ok(false);
        };
        let events = bridge.poll_host_events(timeout)?;
        if events.is_empty() {
            return Ok(false);
        }
        self.push_host_events(events);
        self.drain_jobs()?;
        Ok(true)
    }

    fn enqueue_host_events(&mut self, timeout: Duration) -> Result<usize, VmErr> {
        let Some(bridge) = self.host.clone() else {
            return Ok(0);
        };
        let events = bridge.poll_host_events(timeout)?;
        let count = events.len();
        self.push_host_events(events);
        Ok(count)
    }

    fn push_host_events(&mut self, events: Vec<crate::host::HostEvent>) {
        let mut queue = self.jobs.borrow_mut();
        for event in events {
            match event {
                crate::host::HostEvent::Callback(callback) => {
                    queue.push_external_event(Job::HostCallback { callback });
                }
                crate::host::HostEvent::PromiseSettled {
                    promise,
                    state,
                    value,
                } => queue.push_external_event(Job::HostPromiseSettled {
                    promise,
                    state,
                    value,
                }),
                crate::host::HostEvent::UncaughtException(exception) => {
                    queue.push_external_event(Job::HostUncaughtException { exception });
                }
            }
        }
    }

    pub(crate) fn run_host_uncaught_exception(&mut self, exception: Value) -> Result<(), VmErr> {
        let process = self.persistent_global.borrow().get("process");
        let Some(process) = process else {
            return Err(VmErr::Throw(exception));
        };
        let Some(emit) = process.get_prop("emit") else {
            return Err(VmErr::Throw(exception));
        };
        let callable = matches!(
            emit,
            Value::Function(_)
                | Value::NativeFunction { .. }
                | Value::HostFunction { .. }
                | Value::Class(_)
        ) || crate::interpreter::call::callable_slot(
            &emit,
            crate::interpreter::call::CALL_SLOT,
        )
        .is_some();
        if !callable {
            return Err(VmErr::Throw(exception));
        }
        let handled = self.call_this(
            &emit,
            process,
            vec![Value::String("uncaughtException".into()), exception.clone()],
        )?;
        if handled.is_truthy() {
            Ok(())
        } else {
            Err(VmErr::Throw(exception))
        }
    }

    pub(crate) fn settle_host_promise(
        &mut self,
        promise: Rc<RefCell<PromiseInner>>,
        state: PromiseState,
        value: Value,
    ) -> Result<(), VmErr> {
        match state {
            PromiseState::Fulfilled => self.resolve_promise(&promise, value),
            PromiseState::Rejected => {
                self.reject_promise(&promise, value);
                Ok(())
            }
            PromiseState::Pending => Err(VmErr::Msg(
                "host promise event cannot settle to pending".into(),
            )),
        }
    }

    /// Drain only the microtask queue, leaving timers pending. `await` uses
    /// this so a promise chain settles without letting a `setTimeout`
    /// callback jump ahead of the code that is still running.
    pub(crate) fn drain_microtasks(&mut self) -> Result<(), VmErr> {
        let mut executed = 0usize;
        loop {
            let Some(job) = self.jobs.borrow_mut().take_microtask() else {
                return Ok(());
            };
            executed += 1;
            if executed > MAX_JOBS_PER_DRAIN {
                return Err(crate::value::limit_err("Maximum job count exceeded"));
            }
            match job {
                Job::Reaction {
                    state,
                    value,
                    reaction,
                } => self.run_reaction(state, value, reaction)?,
                Job::PromiseResolveThenable {
                    target,
                    thenable,
                    then,
                    resolution_guard,
                } => self.run_thenable_job(target, &thenable, &then, resolution_guard)?,
                Job::Callback { callback, args } => {
                    self.call_this(&callback, Value::Undefined, args)?;
                }
                Job::HostCallback { callback } => {
                    self.run_host_callback(callback)?;
                }
                Job::HostPromiseSettled {
                    promise,
                    state,
                    value,
                } => self.settle_host_promise(promise, state, value)?,
                Job::HostUncaughtException { exception } => {
                    self.run_host_uncaught_exception(exception)?;
                }
            }
        }
    }
}

/// Hidden slot carrying the promise a `resolve`/`reject` function settles.
const TARGET_SLOT: &str = "__symbol_promise_target__";
const RESOLUTION_GUARD_SLOT: &str = "__symbol_promise_resolution_guard__";

fn target_of(this: &Value) -> Option<Rc<RefCell<PromiseInner>>> {
    this.get_prop(TARGET_SLOT)?.as_promise()
}

fn executor_resolve(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    if let Some(target) = target_of(&this) {
        let value = args.into_iter().next().unwrap_or(Value::Undefined);
        interp.resolve_promise(&target, value)?;
    }
    Ok(Value::Undefined)
}

fn executor_reject(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    if let Some(target) = target_of(&this) {
        interp.reject_promise(&target, args.into_iter().next().unwrap_or(Value::Undefined));
    }
    Ok(Value::Undefined)
}

fn thenable_resolve(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    if let (Some(target), Some(guard)) = (target_of(&this), this.get_prop(RESOLUTION_GUARD_SLOT))
        && claim_resolution(&guard)
    {
        let value = args.into_iter().next().unwrap_or(Value::Undefined);
        interp.resolve_promise_inner(&target, value)?;
    }
    Ok(Value::Undefined)
}

fn thenable_reject(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    if let (Some(target), Some(guard)) = (target_of(&this), this.get_prop(RESOLUTION_GUARD_SLOT))
        && claim_resolution(&guard)
    {
        let reason = args.into_iter().next().unwrap_or(Value::Undefined);
        settle(&interp.jobs, &target, PromiseState::Rejected, reason);
    }
    Ok(Value::Undefined)
}
