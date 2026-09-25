//! The job queues: microtasks (promise reactions, `queueMicrotask`) and
//! macrotasks (`setTimeout`).
//!
//! The queue is shared, not owned: generator and async bodies run on their own
//! `Interpreter` (a separate stack), and a promise settled inside one must
//! schedule reactions the outer loop will run. Handing every interpreter an
//! `Rc` to the same queue is what keeps a single event loop across them.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use crate::value::{PromiseInner, PromiseState, Reaction, Value};

/// Hard cap on how many jobs one drain will run.
///
/// A promise chain can schedule work forever (`function tick() {
/// Promise.resolve().then(tick); }`), which would hang the host inside a
/// single `run()` call. The cap turns that into a catchable `RangeError`, the
/// same treatment loops and recursion get.
pub const MAX_JOBS_PER_DRAIN: usize = 1_000_000;

/// A unit of deferred work.
pub enum Job {
    /// A promise reaction: run `reaction`'s handler for a promise that settled
    /// to `state` with `value`, then settle the derived promise.
    Reaction {
        state: PromiseState,
        value: Value,
        reaction: Reaction,
    },
    /// Invoke a thenable's `then` method in a PromiseResolveThenableJob, after
    /// the current JavaScript stack has finished.
    PromiseResolveThenable {
        target: Rc<RefCell<PromiseInner>>,
        thenable: Value,
        then: Value,
        resolution_guard: Value,
    },
    /// A plain callback: `queueMicrotask(fn)`, or a timer callback.
    Callback { callback: Value, args: Vec<Value> },
    /// Callback queued by a host runtime after an external event. Unlike
    /// synchronous host calls, this runs at an event-loop checkpoint.
    HostCallback { callback: crate::host::HostCallback },
    /// Settlement of a host promise received from the external event queue.
    HostPromiseSettled {
        promise: Rc<RefCell<PromiseInner>>,
        state: PromiseState,
        value: Value,
    },
    /// An exception reported asynchronously by a host runtime. It is offered
    /// to the guest process `uncaughtException` event before escaping to Rust.
    HostUncaughtException { exception: Value },
    /// Timeout for a pending `Atomics.waitAsync` registration. The waiter is
    /// looked up by its shared-memory address and registration id so an
    /// earlier `Atomics.notify` makes this timeout a no-op.
    AtomicsWaitTimeout { key: (usize, usize), waiter_id: u64 },
}

impl Job {
    /// Values this queued job keeps alive, for the cycle collector.
    pub(crate) fn trace_values(&self) -> Vec<Value> {
        match self {
            Job::Reaction {
                value, reaction, ..
            } => vec![
                value.clone(),
                reaction.on_fulfilled.clone(),
                reaction.on_rejected.clone(),
                Value::Promise(reaction.derived.clone()),
            ],
            Job::PromiseResolveThenable {
                target,
                thenable,
                then,
                resolution_guard,
            } => vec![
                Value::Promise(target.clone()),
                thenable.clone(),
                then.clone(),
                resolution_guard.clone(),
            ],
            Job::Callback { callback, args } => {
                let mut out = vec![callback.clone()];
                out.extend(args.iter().cloned());
                out
            }
            Job::HostCallback { callback } => {
                let mut out = vec![callback.callback.clone(), callback.this_value.clone()];
                out.extend(callback.args.iter().cloned());
                out
            }
            Job::HostPromiseSettled { promise, value, .. } => {
                vec![Value::Promise(promise.clone()), value.clone()]
            }
            Job::HostUncaughtException { exception } => vec![exception.clone()],
            Job::AtomicsWaitTimeout { .. } => Vec::new(),
        }
    }
}

struct AtomicsWaiter {
    id: u64,
    promise: Rc<RefCell<PromiseInner>>,
}

#[derive(Default)]
pub struct JobQueue {
    microtasks: VecDeque<Job>,
    external_events: VecDeque<Job>,
    /// Timer callbacks, ordered by delay then by insertion. There is no real
    /// clock here: a timer runs after every microtask has, which preserves the
    /// ordering guarantees guest code depends on without a wall clock.
    timers: Vec<(f64, u64, Job)>,
    next_timer_id: u64,
    cancelled: Vec<u64>,
    atomics_waiters: HashMap<(usize, usize), VecDeque<AtomicsWaiter>>,
    next_atomics_waiter_id: u64,
}

impl JobQueue {
    /// Every value the queued jobs and waiters keep alive, for the cycle
    /// collector's root set.
    pub(crate) fn trace_roots(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for job in self.microtasks.iter().chain(self.external_events.iter()) {
            out.extend(job.trace_values());
        }
        for (_, _, job) in &self.timers {
            out.extend(job.trace_values());
        }
        for waiters in self.atomics_waiters.values() {
            for waiter in waiters {
                out.push(Value::Promise(waiter.promise.clone()));
            }
        }
        out
    }

    pub fn push_microtask(&mut self, job: Job) {
        self.microtasks.push_back(job);
    }

    pub fn take_microtask(&mut self) -> Option<Job> {
        self.microtasks.pop_front()
    }

    pub fn push_external_event(&mut self, job: Job) {
        self.external_events.push_back(job);
    }

    pub fn take_external_event(&mut self) -> Option<Job> {
        self.external_events.pop_front()
    }

    /// Schedule a timer callback, returning the id `clearTimeout` cancels.
    pub fn push_timer(&mut self, delay: f64, callback: Value, args: Vec<Value>) -> u64 {
        self.push_timer_job(delay, Job::Callback { callback, args })
    }

    /// Schedule an internal event on the same timer queue used by guest
    /// timers. Returns a timer sequence id, which is deliberately not exposed
    /// to guest code for runtime-owned jobs.
    pub fn push_timer_job(&mut self, delay: f64, job: Job) -> u64 {
        let id = self.next_timer_id + 1;
        self.next_timer_id = id;
        let delay = if delay.is_finite() && delay > 0.0 {
            delay
        } else {
            0.0
        };
        self.timers.push((delay, id, job));
        id
    }

    /// Register a promise waiting on the shared memory word at `key`.
    pub fn register_atomics_waiter(
        &mut self,
        key: (usize, usize),
        promise: Rc<RefCell<PromiseInner>>,
    ) -> u64 {
        self.next_atomics_waiter_id = self.next_atomics_waiter_id.wrapping_add(1).max(1);
        let id = self.next_atomics_waiter_id;
        self.atomics_waiters
            .entry(key)
            .or_default()
            .push_back(AtomicsWaiter { id, promise });
        id
    }

    /// Remove a registered waiter, typically when its timeout fires.
    pub fn remove_atomics_waiter(
        &mut self,
        key: (usize, usize),
        waiter_id: u64,
    ) -> Option<Rc<RefCell<PromiseInner>>> {
        let waiters = self.atomics_waiters.get_mut(&key)?;
        let index = waiters.iter().position(|waiter| waiter.id == waiter_id)?;
        let waiter = waiters.remove(index)?;
        if waiters.is_empty() {
            self.atomics_waiters.remove(&key);
        }
        Some(waiter.promise)
    }

    /// Take up to `count` pending waiters in FIFO order. Settled entries are
    /// discarded so a timeout cannot make a later notify report a false hit.
    pub fn take_atomics_waiters(
        &mut self,
        key: (usize, usize),
        count: usize,
    ) -> Vec<Rc<RefCell<PromiseInner>>> {
        let Some(waiters) = self.atomics_waiters.get_mut(&key) else {
            return Vec::new();
        };
        let mut selected = Vec::new();
        let mut retained = VecDeque::new();
        while let Some(waiter) = waiters.pop_front() {
            if waiter.promise.borrow().state != PromiseState::Pending {
                continue;
            }
            if selected.len() < count {
                selected.push(waiter.promise);
            } else {
                retained.push_back(waiter);
            }
        }
        *waiters = retained;
        if waiters.is_empty() {
            self.atomics_waiters.remove(&key);
        }
        selected
    }

    pub fn cancel_timer(&mut self, id: u64) {
        self.cancelled.push(id);
        self.timers.retain(|(_, timer_id, _)| *timer_id != id);
    }

    /// Remove the timer that should fire next: the smallest delay, breaking
    /// ties by scheduling order.
    pub fn take_timer(&mut self) -> Option<Job> {
        let index = self
            .timers
            .iter()
            .enumerate()
            .min_by(|(_, (da, ia, _)), (_, (db, ib, _))| {
                da.partial_cmp(db)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(ia.cmp(ib))
            })
            .map(|(index, _)| index)?;
        Some(self.timers.remove(index).2)
    }

    pub fn is_empty(&self) -> bool {
        self.microtasks.is_empty() && self.external_events.is_empty() && self.timers.is_empty()
    }
}

/// Shared handle to the queue.
pub type Jobs = Rc<RefCell<JobQueue>>;

/// Settle `promise`, moving every registration it accumulated onto the
/// microtask queue. A promise that has already settled is left alone — the
/// specification's "resolve once" rule, and what makes a `resolve`/`reject`
/// pair handed to an executor safe to call twice.
pub fn settle(jobs: &Jobs, promise: &Rc<RefCell<PromiseInner>>, state: PromiseState, value: Value) {
    let reactions = {
        let mut inner = promise.borrow_mut();
        if inner.state != PromiseState::Pending {
            return;
        }
        inner.state = state;
        inner.resolution_locked = true;
        inner.external_pending = false;
        inner.value = value.clone();
        std::mem::take(&mut inner.reactions)
    };
    let mut queue = jobs.borrow_mut();
    for reaction in reactions {
        queue.push_microtask(Job::Reaction {
            state,
            value: value.clone(),
            reaction,
        });
    }
}

/// Complete a timed `Atomics.waitAsync` registration. If a notify already
/// removed it, the timeout is stale and does nothing.
pub fn settle_atomics_wait_timeout(jobs: &Jobs, key: (usize, usize), waiter_id: u64) {
    let promise = jobs.borrow_mut().remove_atomics_waiter(key, waiter_id);
    if let Some(promise) = promise {
        settle(
            jobs,
            &promise,
            PromiseState::Fulfilled,
            Value::String("timed-out".into()),
        );
    }
}
