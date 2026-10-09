//! Promise resolution and the event loop drain.
//!
//! The model is the specification's: a promise starts pending, settles once,
//! and every reaction runs as a *microtask* rather than inline. That is what
//! makes `Promise.resolve().then(f); g();` run `g` before `f`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use super::jobs::{Job, settle};
use super::{Clock, Fairness, Interpreter, RealTimeClock, TurnBudget, TurnOutcome, YieldReason};
use crate::error::VmErr;
use crate::value::{PromiseInner, PromiseState, Reaction, Value};

fn is_callable(value: &Value) -> bool {
    super::call::is_callable_value(value)
}

fn promise_error_reason(error: VmErr) -> Value {
    match error {
        VmErr::Throw(reason) => reason,
        VmErr::RuntimeError(data) => data.guest_value(),
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
    /// PromiseResolve(%Promise%, value), used by Await. Constructor access on
    /// an existing promise is observable and may throw before suspension.
    pub(crate) fn promise_resolve_intrinsic(&mut self, value: Value) -> Result<Value, VmErr> {
        if value.as_promise().is_some() {
            let constructor = self.get_prop_value_str(&value, "constructor")?;
            let intrinsic = self.persistent_global.borrow().intrinsic("Promise");
            if intrinsic.is_some_and(|intrinsic| super::strict_equals(&constructor, &intrinsic)) {
                return Ok(value);
            }
        }
        let promise = Value::pending_promise();
        self.resolve_promise(&promise, value)?;
        Ok(Value::Promise(promise))
    }

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

        // Thenable assimilation: any object with a callable `then` is treated
        // as a promise, which is how promises from other implementations
        // interoperate.
        if super::call::is_js_object(&value) {
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
                        realm: super::realm::value_realm(&then)
                            .unwrap_or_else(|| self.persistent_global.clone()),
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

    pub fn reject_promise(&mut self, promise: &Rc<RefCell<PromiseInner>>, reason: Value) {
        {
            let mut inner = promise.borrow_mut();
            if inner.resolution_locked || inner.state != PromiseState::Pending {
                return;
            }
            inner.resolution_locked = true;
        }
        settle(&self.jobs, promise, PromiseState::Rejected, reason);
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
        (
            resolving_function(&carrier, None, executor_resolve),
            resolving_function(&carrier, None, executor_reject),
        )
    }

    fn thenable_settle_functions(
        promise: Rc<RefCell<PromiseInner>>,
        resolution_guard: Value,
    ) -> (Value, Value) {
        let carrier = Value::Promise(promise);
        (
            resolving_function(&carrier, Some(&resolution_guard), thenable_resolve),
            resolving_function(&carrier, Some(&resolution_guard), thenable_reject),
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
                let reason = data.guest_value();
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
        self.drain_queued_jobs(false).map(|_| ())
    }

    fn dispatch_job(&mut self, job: Job) -> Result<(), VmErr> {
        struct DispatchGuard(super::Jobs);
        impl Drop for DispatchGuard {
            fn drop(&mut self) {
                self.0.borrow_mut().dispatch_depth -= 1;
            }
        }
        self.jobs.borrow_mut().dispatch_depth += 1;
        let _guard = DispatchGuard(self.jobs.clone());
        match job {
            Job::ModuleEvaluation { realm, id, target } => {
                self.with_global_storage(realm, |vm| vm.run_module_evaluation_job(&id, &target))
            }
            Job::DynamicImport {
                realm,
                target,
                specifier,
                referrer,
            } => {
                let outer = std::mem::replace(&mut self.cur_mod, referrer);
                let result = self.with_global_storage(realm, |vm| vm.import_module(&specifier));
                self.cur_mod = outer;
                match result {
                    Ok(promise) => self.resolve_promise(&target, promise),
                    Err(error) => {
                        self.reject_promise(&target, promise_error_reason(error));
                        Ok(())
                    }
                }
            }

            Job::Reaction {
                state,
                value,
                reaction,
            } => self.run_reaction(state, value, reaction),
            Job::PromiseResolveThenable {
                realm,
                target,
                thenable,
                then,
                resolution_guard,
            } => self.with_global_storage(realm.clone(), |vm| {
                let _allocation = super::realm::AllocationRealm::enter(Some(realm));
                vm.run_thenable_job(target, &thenable, &then, resolution_guard)
            }),
            Job::Callback { callback, args } => self
                .call_this(&callback, Value::Undefined, args)
                .map(|_| ()),
            Job::HostCallback { callback } => self.run_host_callback(callback).map(|_| ()),
            Job::HostPromiseSettled {
                promise,
                state,
                value,
            } => self.settle_host_promise(promise, state, value),
            Job::HostUncaughtException { exception } => self.run_host_uncaught_exception(exception),
            Job::Interval { id } => {
                let id = id.get();
                let callback = self.jobs.borrow().interval_callback(id);
                if let Some((callback, args)) = callback {
                    let result = self
                        .call_this(&callback, Value::Undefined, args)
                        .map(|_| ());
                    if result.is_ok() {
                        self.jobs.borrow_mut().reschedule_interval(id);
                    } else {
                        self.jobs.borrow_mut().cancel_timer(id);
                    }
                    result
                } else {
                    Ok(())
                }
            }
            Job::AtomicsWaitTimeout { key, waiter_id } => {
                super::jobs::settle_atomics_wait_timeout(&self.jobs, key, waiter_id);
                Ok(())
            }
        }
    }

    fn drain_queued_jobs(&mut self, microtasks_only: bool) -> Result<usize, VmErr> {
        self.poll_queued_jobs(TurnBudget::jobs(usize::MAX), microtasks_only)
            .map(|o| o.executed_jobs)
    }

    /// Run nonblocking work within a soft scheduling budget. Remaining hard
    /// guest fuel/loop/job limits survive soft yields and nested checkpoints.
    pub fn poll_event_loop(&mut self, budget: TurnBudget) -> Result<TurnOutcome, VmErr> {
        self.poll_queued_jobs(budget, false)
    }

    fn outcome(&mut self, executed_jobs: usize, mut yield_reason: YieldReason) -> TurnOutcome {
        let mut queue = self.jobs.borrow_mut();
        queue.checkpoint_pending = queue.has_microtasks();
        if !queue.host_overflow.is_empty() {
            yield_reason = YieldReason::Backpressure;
        }
        let outcome = TurnOutcome {
            executed_jobs,
            runnable: queue.is_runnable() || !queue.host_overflow.is_empty(),
            yield_reason,
            next_deadline: queue.next_deadline(),
            checkpoint_pending: queue.checkpoint_pending,
        };
        drop(queue);
        if yield_reason == YieldReason::Idle && self.execution.drain_depth.get() == 1 {
            self.retire_completed_execution();
        }
        outcome
    }

    fn poll_queued_jobs(
        &mut self,
        budget: TurnBudget,
        microtasks_only: bool,
    ) -> Result<TurnOutcome, VmErr> {
        if !self.execution.active.get()
            && self.execution.drain_depth.get() == 0
            && self.jobs.borrow().is_runnable()
        {
            self.begin_execution();
        }
        struct DrainGuard(Rc<super::scheduler::ExecutionState>, super::Jobs);
        impl Drop for DrainGuard {
            fn drop(&mut self) {
                let mut queue = self.1.borrow_mut();
                queue.checkpoint_pending = queue.has_microtasks();
                self.0.drain_depth.set(self.0.drain_depth.get() - 1);
            }
        }
        self.execution
            .drain_depth
            .set(self.execution.drain_depth.get() + 1);
        let _guard = DrainGuard(self.execution.clone(), self.jobs.clone());
        let clock = RealTimeClock::default();
        let mut initial_remaining = self.execution.jobs.get();
        let mut executed = 0;
        loop {
            self.check_execution_interrupt()?;
            if executed >= budget.max_jobs {
                return Ok(self.outcome(executed, YieldReason::JobBudget));
            }
            if budget
                .max_duration
                .is_some_and(|d| clock.now_ms() >= d.as_secs_f64() * 1000.0)
            {
                return Ok(self.outcome(executed, YieldReason::TimeBudget));
            }
            super::jobs::settle_notified_atomics_waiters(&self.jobs);
            let micro = self.jobs.borrow().has_microtasks();
            if !micro {
                self.jobs.borrow_mut().checkpoint_pending = false;
                self.jobs.borrow_mut().clear_kept_alive();
                if !microtasks_only {
                    self.enqueue_host_events(Duration::ZERO)?;
                }
            }
            let ready = if microtasks_only {
                self.jobs.borrow().has_microtasks()
            } else {
                self.jobs.borrow().is_runnable()
            };
            if ready && !self.execution.active.get() {
                // External ingress starts a fresh context only when work exists.
                self.execution.active.set(true);
                self.execution.loops.set(self.loop_budget);
                self.execution.fuel.set(self.fuel_budget);
                self.execution.jobs.set(self.max_jobs_per_drain);
                initial_remaining = self.execution.jobs.get();
            }
            if !ready {
                return Ok(self.outcome(executed, YieldReason::Idle));
            }
            if self.execution.jobs.get() == 0 {
                return Err(crate::value::limit_err("Maximum job count exceeded"));
            }
            let job = {
                let mut queue = self.jobs.borrow_mut();
                if let Some(job) = queue.take_microtask() {
                    queue.checkpoint_pending = true;
                    Some(job)
                } else if microtasks_only {
                    None
                } else {
                    let prefer_timer = self.event_loop_options.fairness == Fairness::Alternate
                        && queue.prefer_timer;
                    if prefer_timer && queue.has_due_timer() {
                        queue.prefer_timer = false;
                        queue.take_timer()
                    } else if queue.has_external_events() {
                        queue.prefer_timer = true;
                        queue.take_external_event()
                    } else {
                        queue.prefer_timer = false;
                        queue.take_timer()
                    }
                }
            };
            let Some(job) = job else {
                return Ok(self.outcome(executed, YieldReason::Idle));
            };
            self.execution.jobs.set(self.execution.jobs.get() - 1);
            self.dispatch_job(job)?;
            executed = initial_remaining.saturating_sub(self.execution.jobs.get());
        }
    }

    /// Wait only when no work was executed, with real-time waits capped at
    /// the next deadline. Virtual clocks never advance implicitly. WASM never waits.
    pub fn run_event_loop_once(&mut self, timeout: Duration) -> Result<bool, VmErr> {
        self.run_event_loop_turn(timeout, TurnBudget::jobs(usize::MAX))
    }

    pub(crate) fn run_await_event(&mut self, timeout: Duration) -> Result<bool, VmErr> {
        self.run_event_loop_turn(timeout, TurnBudget::jobs(1))
    }

    fn run_event_loop_turn(
        &mut self,
        timeout: Duration,
        budget: TurnBudget,
    ) -> Result<bool, VmErr> {
        if self.poll_event_loop(budget)?.executed_jobs > 0 {
            return Ok(true);
        }
        #[cfg(target_arch = "wasm32")]
        let timeout = {
            let _ = timeout;
            Duration::ZERO
        };
        let clock = RealTimeClock::default();
        loop {
            self.check_execution_interrupt()?;
            let elapsed = Duration::from_secs_f64(clock.now_ms() / 1000.0);
            let remaining = timeout.saturating_sub(elapsed);
            if remaining.is_zero() {
                return Ok(false);
            }
            let timer_wait = self.jobs.borrow().timer_wait();
            let mut wait = timer_wait.map_or(remaining, |d| d.min(remaining));
            if let Some(deadline) = self.execution.deadline.get() {
                wait = wait.min(Duration::from_secs_f64(
                    ((deadline - self.execution.clock.now_ms()).max(0.0)) / 1000.0,
                ));
            }
            #[cfg(not(target_arch = "wasm32"))]
            if !wait.is_zero() {
                self.execution
                    .cancellation
                    .borrow()
                    .register_wake(&self.execution.wake);
                if self
                    .host
                    .as_ref()
                    .is_some_and(|h| h.event_wait_mode() == crate::host::HostWaitMode::BlockingPoll)
                {
                    // Legacy polls cannot be interrupted by our signal. Bound
                    // cancellation latency, and avoid spinning on early returns.
                    let slice = wait.min(Duration::from_millis(10));
                    let started = std::time::Instant::now();
                    let count = self.enqueue_host_events(slice)?;
                    self.check_execution_interrupt()?;
                    if count == 0 {
                        self.execution
                            .wake
                            .wait(Some(slice.saturating_sub(started.elapsed())));
                    }
                } else {
                    self.execution.wake.wait(Some(wait));
                }
            }
            self.check_execution_interrupt()?;
            if self.poll_event_loop(budget)?.executed_jobs > 0 {
                return Ok(true);
            }
        }
    }

    fn enqueue_host_events(&mut self, timeout: Duration) -> Result<usize, VmErr> {
        let capacity = self.event_loop_options.external_capacity;
        {
            let mut queue = self.jobs.borrow_mut();
            while queue.external_len() < capacity {
                let Some(job) = queue.host_overflow.pop_front() else {
                    break;
                };
                queue.push_external_event(job);
            }
        }
        let room = capacity.saturating_sub(self.jobs.borrow().external_len());
        if room == 0 || !self.jobs.borrow().host_overflow.is_empty() {
            return Ok(0);
        }
        let Some(bridge) = self.host.clone() else {
            return Ok(0);
        };
        let events = bridge
            .poll_host_events_bounded(timeout, room.min(self.event_loop_options.host_batch_size))?;
        let count = events.len();
        self.push_host_events(events);
        Ok(count)
    }

    fn push_host_events(&mut self, events: Vec<crate::host::HostEvent>) {
        let mut queue = self.jobs.borrow_mut();
        for event in events {
            let job = match event {
                crate::host::HostEvent::Callback(callback) => Job::HostCallback { callback },
                crate::host::HostEvent::PromiseSettled {
                    promise,
                    state,
                    value,
                } => Job::HostPromiseSettled {
                    promise,
                    state,
                    value,
                },
                crate::host::HostEvent::UncaughtException(exception) => {
                    Job::HostUncaughtException { exception }
                }
            };
            if let Err(job) =
                queue.try_push_external_event(job, self.event_loop_options.external_capacity)
            {
                queue.push_overflow(job);
            }
        }
    }

    /// Dispatch an internal timer during top-level await under the same hard
    /// accounting as every drain. Limits are checked before removing its roots.
    pub(crate) fn run_await_timer(&mut self) -> Result<bool, VmErr> {
        self.check_execution_interrupt()?;
        if !self.jobs.borrow().has_due_timer() {
            return Ok(false);
        }
        if self.execution.jobs.get() == 0 {
            return Err(crate::value::limit_err("Maximum job count exceeded"));
        }
        let job = self.jobs.borrow_mut().take_timer();
        if let Some(job) = job {
            self.execution.jobs.set(self.execution.jobs.get() - 1);
            self.dispatch_job(job)?;
            Ok(true)
        } else {
            Ok(false)
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
        self.drain_queued_jobs(true).map(|_| ())
    }
}

/// Hidden slot carrying the promise a `resolve`/`reject` function settles.
const TARGET_SLOT: &str = "__symbol_promise_target__";
const RESOLUTION_GUARD_SLOT: &str = "__symbol_promise_resolution_guard__";

fn resolving_function(
    carrier: &Value,
    guard: Option<&Value>,
    callable: crate::builtins::NativeFn,
) -> Value {
    let mut slots = vec![(TARGET_SLOT.into(), carrier.clone())];
    if let Some(guard) = guard {
        slots.push((RESOLUTION_GUARD_SLOT.into(), guard.clone()));
    }
    let state = Value::object(slots);
    let prototype = super::realm::allocation_global()
        .and_then(|realm| crate::value::FunctionData::default_function_prototype(&realm));
    let target = crate::builtins::native_method("", 1, callable, prototype.clone());
    let properties = Value::object_with_proto(
        vec![
            ("name".into(), Value::String("".into())),
            ("length".into(), Value::Number(1.0)),
        ],
        prototype.map(Rc::new),
    )
    .property_cell()
    .expect("function properties");
    for name in ["name", "length"] {
        properties.meta.borrow_mut().set_attrs(
            name,
            crate::value::PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        );
    }
    // Reuse the existing bound-call machinery: state is an internal receiver,
    // never an observable property and never replaced by the caller's this.
    Value::Function(Rc::new(crate::value::FunctionData {
        strict: true,
        native: None,
        identity: Rc::new(0),
        name: Some("".into()),
        properties,
        standard_properties_initialized: Rc::new(std::cell::Cell::new(true)),
        params: Rc::new(Vec::new()),
        body: Rc::new(Vec::new()),
        closure: None,
        is_arrow: false,
        is_constructor: false,
        is_async: false,
        is_generator: false,
        uses_arguments: false,
        needs_hoisting: false,
        bytecode: None,
        bound: Some(Rc::new(crate::value::BoundFunctionData {
            target,
            this_value: state,
            arguments: Rc::new(Vec::new()),
        })),
    }))
}

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
