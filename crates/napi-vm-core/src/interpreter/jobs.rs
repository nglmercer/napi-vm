//! The job queues: microtasks (promise reactions, `queueMicrotask`) and
//! macrotasks (`setTimeout`).
//!
//! The queue is shared, not owned: generator and async bodies run on their own
//! `Interpreter` (a separate stack), and a promise settled inside one must
//! schedule reactions the outer loop will run. Handing every interpreter an
//! `Rc` to the same queue is what keeps a single event loop across them.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
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
    ModuleEvaluation {
        realm: super::Env,
        id: String,
        target: Rc<RefCell<PromiseInner>>,
    },
    DynamicImport {
        realm: super::Env,
        target: Rc<RefCell<PromiseInner>>,
        specifier: String,
        referrer: Option<String>,
    },
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
    Callback {
        callback: Value,
        args: Vec<Value>,
    },
    Interval {
        id: Rc<std::cell::Cell<u64>>,
    },
    /// Callback queued by a host runtime after an external event. Unlike
    /// synchronous host calls, this runs at an event-loop checkpoint.
    HostCallback {
        callback: crate::host::HostCallback,
    },
    /// Settlement of a host promise received from the external event queue.
    HostPromiseSettled {
        promise: Rc<RefCell<PromiseInner>>,
        state: PromiseState,
        value: Value,
    },
    /// An exception reported asynchronously by a host runtime. It is offered
    /// to the guest process `uncaughtException` event before escaping to Rust.
    HostUncaughtException {
        exception: Value,
    },
    /// Timeout for a pending `Atomics.waitAsync` registration. The waiter is
    /// looked up by its shared-memory address and registration id so an
    /// earlier `Atomics.notify` makes this timeout a no-op.
    AtomicsWaitTimeout {
        key: (usize, usize),
        waiter_id: u64,
    },
}

impl Job {
    /// Values this queued job keeps alive, for the cycle collector.
    pub(crate) fn trace_values(&self, out: &mut Vec<Value>) {
        match self {
            Job::ModuleEvaluation { realm, target, .. }
            | Job::DynamicImport { realm, target, .. } => {
                out.push(Value::Promise(target.clone()));
                out.push(Value::RealmGlobal(realm.clone()));
            }
            Job::Reaction {
                value, reaction, ..
            } => out.extend([
                value.clone(),
                reaction.on_fulfilled.clone(),
                reaction.on_rejected.clone(),
                Value::Promise(reaction.derived.clone()),
            ]),
            Job::PromiseResolveThenable {
                target,
                thenable,
                then,
                resolution_guard,
            } => out.extend([
                Value::Promise(target.clone()),
                thenable.clone(),
                then.clone(),
                resolution_guard.clone(),
            ]),
            Job::Callback { callback, args } => {
                out.push(callback.clone());
                out.extend(args.iter().cloned());
            }
            Job::HostCallback { callback } => {
                out.extend([callback.callback.clone(), callback.this_value.clone()]);
                out.extend(callback.args.iter().cloned());
            }
            Job::HostPromiseSettled { promise, value, .. } => {
                out.extend([Value::Promise(promise.clone()), value.clone()]);
            }
            Job::HostUncaughtException { exception } => out.push(exception.clone()),
            Job::AtomicsWaitTimeout { .. } => {}
            Job::Interval { .. } => {}
        }
    }
}

type TimerKey = (u64, u128);
struct TimerEntry {
    key: TimerKey,
    id: u64,
    job: Job,
}
/// Sorted small queues avoid allocating a tree node and ID index per timer.
/// Promotion is sticky until empty so cancellation cannot cause mode churn.
struct TreeTimers {
    entries: BTreeMap<TimerKey, (u64, Job)>,
    ids: HashMap<u64, TimerKey, ahash::RandomState>,
}
impl Default for TreeTimers {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            ids: HashMap::with_hasher(super::env::randomized_hasher()),
        }
    }
}
enum TimerQueue {
    Small {
        entries: Vec<TimerEntry>,
        spare: Option<TreeTimers>,
    },
    Tree {
        entries: BTreeMap<TimerKey, (u64, Job)>,
        ids: HashMap<u64, TimerKey, ahash::RandomState>,
        small: Vec<TimerEntry>,
    },
}
impl Default for TimerQueue {
    fn default() -> Self {
        Self::Small {
            entries: Vec::new(),
            spare: None,
        }
    }
}
impl TimerQueue {
    const SMALL_LIMIT: usize = 128;
    const MAX_RETAINED_IDS: usize = 16_384;
    fn len(&self) -> usize {
        match self {
            Self::Small { entries: v, .. } => v.len(),
            Self::Tree { entries, .. } => entries.len(),
        }
    }
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn contains_id(&self, id: u64) -> bool {
        match self {
            Self::Small { entries: v, .. } => v.iter().any(|e| e.id == id),
            Self::Tree { ids, .. } => ids.contains_key(&id),
        }
    }
    fn insert(&mut self, key: TimerKey, id: u64, job: Job) {
        if let Self::Small { entries: v, spare } = self {
            let index = v.partition_point(|e| e.key < key);
            v.insert(index, TimerEntry { key, id, job });
            if v.len() <= Self::SMALL_LIMIT {
                return;
            }
            let TreeTimers {
                mut entries,
                mut ids,
            } = spare.take().unwrap_or_default();
            ids.reserve(v.len());
            for e in v.drain(..) {
                ids.insert(e.id, e.key);
                entries.insert(e.key, (e.id, e.job));
            }
            let small = std::mem::take(v);
            *self = Self::Tree {
                entries,
                ids,
                small,
            };
            return;
        }
        if let Self::Tree { entries, ids, .. } = self {
            entries.insert(key, (id, job));
            ids.insert(id, key);
        }
    }
    fn cancel(&mut self, id: u64) {
        match self {
            Self::Small { entries: v, .. } => {
                if let Some(i) = v.iter().position(|e| e.id == id) {
                    v.remove(i);
                }
            }
            Self::Tree { entries, ids, .. } => {
                if let Some(key) = ids.remove(&id) {
                    entries.remove(&key);
                }
            }
        }
        self.demote_empty();
    }
    fn demote_empty(&mut self) {
        let Self::Tree {
            entries,
            ids,
            small,
        } = self
        else {
            return;
        };
        if !entries.is_empty() {
            return;
        }
        debug_assert!(ids.is_empty());
        // Empty cached storage retains no jobs/guest roots. A small-only queue
        // never allocates an ID index. Cap reuse after unusually large queues.
        let empty_ids = HashMap::with_hasher(ids.hasher().clone());
        let spare = (ids.capacity() <= Self::MAX_RETAINED_IDS).then(|| TreeTimers {
            entries: std::mem::take(entries),
            ids: std::mem::replace(ids, empty_ids),
        });
        let entries = std::mem::take(small);
        *self = Self::Small { entries, spare };
    }
    fn first_key(&self) -> Option<TimerKey> {
        match self {
            Self::Small { entries: v, .. } => v.first().map(|e| e.key),
            Self::Tree { entries, .. } => entries.first_key_value().map(|(k, _)| *k),
        }
    }
    fn pop(&mut self) -> Option<Job> {
        let job = match self {
            Self::Small { entries: v, .. } => {
                if v.is_empty() {
                    return None;
                }
                v.remove(0).job
            }
            Self::Tree { entries, ids, .. } => {
                let (_, (id, job)) = entries.pop_first()?;
                ids.remove(&id);
                job
            }
        };
        self.demote_empty();
        Some(job)
    }
    fn trace_roots(&self, out: &mut Vec<Value>) {
        match self {
            Self::Small { entries: v, .. } => {
                for e in v {
                    e.job.trace_values(out);
                }
            }
            Self::Tree { entries, .. } => {
                for (_, job) in entries.values() {
                    job.trace_values(out);
                }
            }
        }
    }
}

struct AtomicsWaiter {
    id: u64,
    promise: Rc<RefCell<PromiseInner>>,
}

struct Interval {
    delay: f64,
    callback: Value,
    args: Vec<Value>,
    timer_id: u64,
}

#[derive(Default)]
pub struct JobQueue {
    kept_alive: Vec<Value>,
    kept_identities: HashSet<usize>,
    microtasks: VecDeque<Job>,
    external_events: VecDeque<Job>,
    // Retain oversized legacy ingress without silently discarding settlements.
    pub(crate) host_overflow: VecDeque<Job>,
    /// Timer callbacks, ordered by delay then by insertion. There is no real
    /// clock here: a timer runs after every microtask has, which preserves the
    /// ordering guarantees guest code depends on without a wall clock.
    // Nonnegative finite f64 bit patterns have the same order as their values.
    timers: TimerQueue,
    intervals: HashMap<u64, Interval>,
    pub max_timers: Option<usize>,
    next_timer_id: u64,
    timer_ids_wrapped: bool,
    next_sequence: u128,
    peak_depth: usize,
    clock: super::scheduler::ClockMode,
    pub checkpoint_pending: bool,
    pub(crate) dispatch_depth: usize,
    pub(crate) prefer_timer: bool,
    atomics_waiters: HashMap<(usize, usize), VecDeque<AtomicsWaiter>>,
    next_atomics_waiter_id: u64,
}

impl JobQueue {
    pub(crate) fn keep_alive(&mut self, value: Value) -> Result<(), crate::VmErr> {
        let Some(identity) = value.weak_identity() else {
            return Ok(());
        };
        if self.kept_identities.contains(&identity) {
            return Ok(());
        }
        if self.kept_alive.len() >= crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err(
                "Maximum kept-alive weak targets exceeded",
            ));
        }
        self.kept_identities.insert(identity);
        self.kept_alive.push(value);
        Ok(())
    }
    pub(crate) fn clear_kept_alive(&mut self) {
        self.kept_alive.clear();
        self.kept_identities.clear();
    }

    /// Every value the queued jobs and waiters keep alive, for the cycle
    /// collector's root set.
    pub(crate) fn trace_roots(&self, out: &mut Vec<Value>) {
        out.extend(self.kept_alive.iter().cloned());
        for job in self
            .microtasks
            .iter()
            .chain(self.external_events.iter())
            .chain(self.host_overflow.iter())
        {
            job.trace_values(out);
        }
        self.timers.trace_roots(out);
        for interval in self.intervals.values() {
            out.push(interval.callback.clone());
            out.extend(interval.args.iter().cloned());
        }
        for waiters in self.atomics_waiters.values() {
            for waiter in waiters {
                out.push(Value::Promise(waiter.promise.clone()));
            }
        }
    }

    pub fn push_microtask(&mut self, job: Job) {
        self.microtasks.push_back(job);
        self.observe_depth();
    }

    pub fn take_microtask(&mut self) -> Option<Job> {
        self.microtasks.pop_front()
    }

    pub fn push_external_event(&mut self, job: Job) {
        self.external_events.push_back(job);
        self.observe_depth();
    }

    pub fn take_external_event(&mut self) -> Option<Job> {
        self.external_events.pop_front()
    }

    #[doc(hidden)]
    pub fn check_timer_capacity(&self) -> Result<(), crate::VmErr> {
        let executing_intervals = self
            .intervals
            .values()
            .filter(|interval| !self.timers.contains_id(interval.timer_id))
            .count();
        if self
            .max_timers
            .is_some_and(|max| self.timers.len().saturating_add(executing_intervals) >= max)
        {
            return Err(crate::value::limit_err("Maximum timer count exceeded"));
        }
        Ok(())
    }
    #[doc(hidden)]
    pub fn push_interval(&mut self, delay: f64, callback: Value, args: Vec<Value>) -> u64 {
        let id_cell = Rc::new(std::cell::Cell::new(0));
        let id = self.push_timer_job(
            delay,
            Job::Interval {
                id: id_cell.clone(),
            },
        );
        id_cell.set(id);
        self.intervals.insert(
            id,
            Interval {
                delay,
                callback,
                args,
                timer_id: id,
            },
        );
        id
    }
    pub(crate) fn interval_callback(&self, id: u64) -> Option<(Value, Vec<Value>)> {
        self.intervals
            .get(&id)
            .map(|i| (i.callback.clone(), i.args.clone()))
    }
    pub(crate) fn reschedule_interval(&mut self, id: u64) {
        if let Some(interval) = self.intervals.get(&id) {
            let delay = interval.delay;
            let timer_id = self.push_timer_job(
                delay,
                Job::Interval {
                    id: Rc::new(std::cell::Cell::new(id)),
                },
            );
            if let Some(interval) = self.intervals.get_mut(&id) {
                interval.timer_id = timer_id;
            }
        }
    }

    /// Schedule a timer callback, returning the id `clearTimeout` cancels.
    pub fn push_timer(&mut self, delay: f64, callback: Value, args: Vec<Value>) -> u64 {
        self.push_timer_job(delay, Job::Callback { callback, args })
    }

    /// Schedule an internal event on the same timer queue used by guest
    /// timers. Returns a timer sequence id, which is deliberately not exposed
    /// to guest code for runtime-owned jobs.
    pub fn push_timer_job(&mut self, delay: f64, job: Job) -> u64 {
        // IDs are exactly representable in guest JS and never alias live timers,
        // even after wraparound. The independent sequence preserves FIFO ties.
        const MAX_ID: u64 = (1 << 53) - 1;
        loop {
            self.next_timer_id = if self.next_timer_id >= MAX_ID {
                self.timer_ids_wrapped = true;
                1
            } else {
                self.next_timer_id + 1
            };
            // Before the first wrap, monotonically minted IDs cannot alias.
            // Once wrapped, keep checking against every live timer.
            if !self.timer_ids_wrapped {
                break;
            }
            let occupied = self.timers.contains_id(self.next_timer_id);
            let occupied = occupied || self.intervals.contains_key(&self.next_timer_id);
            if !occupied {
                break;
            }
        }
        let id = self.next_timer_id;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("timer sequence exhausted");
        let delay = normalize_delay(delay);
        let deadline = (self.clock.now_ms() + delay).min(f64::MAX);
        let key = (deadline.to_bits(), self.next_sequence);
        self.timers.insert(key, id, job);
        self.observe_depth();
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
        if let Some(interval) = self.intervals.remove(&id) {
            self.timers.cancel(interval.timer_id);
        }
        self.timers.cancel(id);
    }

    /// Remove the smallest deadline, breaking ties by scheduling order.
    pub fn take_timer(&mut self) -> Option<Job> {
        if !self.has_due_timer() {
            return None;
        }
        self.timers.pop()
    }

    pub fn has_microtasks(&self) -> bool {
        !self.microtasks.is_empty()
    }

    pub fn set_clock(
        &mut self,
        clock: super::scheduler::ClockMode,
    ) -> Result<(), crate::error::VmErr> {
        if !self.timers.is_empty() {
            return Err(crate::error::VmErr::Msg(
                "cannot change clocks with pending timers".into(),
            ));
        }
        self.clock = clock;
        Ok(())
    }
    pub fn next_deadline(&self) -> Option<f64> {
        self.timers.first_key().map(|key| f64::from_bits(key.0))
    }
    pub fn has_due_timer(&self) -> bool {
        self.next_deadline()
            .is_some_and(|deadline| self.clock.is_legacy() || deadline <= self.clock.now_ms())
    }
    pub fn has_external_events(&self) -> bool {
        !self.external_events.is_empty()
    }
    pub fn external_len(&self) -> usize {
        self.external_events.len()
    }
    pub fn is_runnable(&self) -> bool {
        self.has_microtasks() || self.has_external_events() || self.has_due_timer()
    }
    pub fn try_push_external_event(&mut self, job: Job, capacity: usize) -> Result<(), Job> {
        if self.external_len() >= capacity {
            Err(job)
        } else {
            self.push_external_event(job);
            Ok(())
        }
    }
    #[doc(hidden)]
    pub fn timer_wait(&self) -> Option<std::time::Duration> {
        if !self.clock.is_real_time() {
            return None;
        }
        self.next_deadline().map(|d| {
            std::time::Duration::try_from_secs_f64(((d - self.clock.now_ms()).max(0.0)) / 1000.0)
                .unwrap_or(std::time::Duration::MAX)
        })
    }
    pub fn len(&self) -> usize {
        self.microtasks.len()
            + self.external_events.len()
            + self.timers.len()
            + self.host_overflow.len()
    }
    pub fn peak_depth(&self) -> usize {
        self.peak_depth
    }
    fn observe_depth(&mut self) {
        self.peak_depth = self.peak_depth.max(self.len());
    }
    pub(crate) fn push_overflow(&mut self, job: Job) {
        self.host_overflow.push_back(job);
        self.observe_depth();
    }
    pub(crate) fn has_outstanding_work(&self) -> bool {
        !self.is_empty() || !self.atomics_waiters.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.microtasks.is_empty()
            && self.external_events.is_empty()
            && self.timers.is_empty()
            && self.host_overflow.is_empty()
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

fn normalize_delay(delay: f64) -> f64 {
    if delay.is_finite() && delay > 0.0 {
        delay
    } else {
        0.0
    }
}

#[cfg(test)]
mod scheduler_tests {
    use super::*;
    fn job(n: f64) -> Job {
        Job::Callback {
            callback: Value::Number(n),
            args: vec![],
        }
    }
    fn take(q: &mut JobQueue) -> f64 {
        match q.take_timer().unwrap() {
            Job::Callback {
                callback: Value::Number(n),
                ..
            } => n,
            _ => panic!(),
        }
    }
    #[test]
    fn wrapped_ids_skip_multiple_live_small_queue_entries() {
        let mut q = JobQueue::default();
        for expected in 1..=10 {
            assert_eq!(q.push_timer(0., Value::Undefined, vec![]), expected);
        }
        q.next_timer_id = (1 << 53) - 1;
        assert_eq!(q.push_timer(0., Value::Undefined, vec![]), 11);
        assert!(q.timer_ids_wrapped);
        assert_eq!(q.len(), 11);
    }
    #[test]
    fn empty_storage_is_reused_and_large_index_retention_is_capped() {
        let mut q = JobQueue::default();
        q.push_timer(0., Value::Undefined, vec![]);
        q.take_timer().unwrap();
        let TimerQueue::Small { entries, spare } = &q.timers else {
            panic!()
        };
        let capacity = entries.capacity();
        assert!(capacity > 0);
        assert!(spare.is_none());
        for _ in 0..20 {
            let id = q.push_timer(0., Value::Undefined, vec![]);
            q.cancel_timer(id);
        }
        let TimerQueue::Small { entries, spare } = &q.timers else {
            panic!()
        };
        assert_eq!(entries.capacity(), capacity);
        assert!(spare.is_none());
        for _ in 0..1000 {
            q.push_timer(0., Value::Undefined, vec![]);
        }
        while q.take_timer().is_some() {}
        let TimerQueue::Small {
            spare: Some(spare), ..
        } = &q.timers
        else {
            panic!()
        };
        assert!(spare.ids.capacity() > 0);
        assert!(spare.ids.is_empty() && spare.entries.is_empty());
        for _ in 0..20000 {
            q.push_timer(0., Value::Undefined, vec![]);
        }
        while q.take_timer().is_some() {}
        assert!(matches!(&q.timers, TimerQueue::Small { spare: None, .. }));
    }
    #[test]
    fn hybrid_promotes_once_traces_roots_and_resets_when_empty() {
        let mut q = JobQueue::default();
        for i in 0..TimerQueue::SMALL_LIMIT {
            q.push_timer(1., Value::Number(i as f64), vec![]);
        }
        assert!(matches!(q.timers, TimerQueue::Small { .. }));
        q.push_timer(1., Value::Number(TimerQueue::SMALL_LIMIT as f64), vec![]);
        assert!(matches!(q.timers, TimerQueue::Tree { .. }));
        let mut roots = Vec::new();
        q.trace_roots(&mut roots);
        assert_eq!(roots.len(), TimerQueue::SMALL_LIMIT + 1);
        for i in 0..=TimerQueue::SMALL_LIMIT {
            if i == TimerQueue::SMALL_LIMIT {
                assert!(matches!(q.timers, TimerQueue::Tree { .. }));
            }
            let Some(Job::Callback {
                callback: Value::Number(n),
                ..
            }) = q.take_timer()
            else {
                panic!("missing timer");
            };
            assert_eq!(n, i as f64);
        }
        assert!(matches!(q.timers, TimerQueue::Small { .. }));
    }

    #[test]
    fn timers_normalize_and_keep_fifo_ties() {
        let mut q = JobQueue::default();
        for (n, d) in [4.0, 0.0, f64::NAN, -1.0, f64::INFINITY, -0.0, 4.0]
            .into_iter()
            .enumerate()
        {
            q.push_timer_job(d, job(n as f64));
        }
        assert_eq!(
            (0..7).map(|_| take(&mut q)).collect::<Vec<_>>(),
            vec![1., 2., 3., 4., 5., 0., 6.]
        );
        assert!(q.timers.is_empty());
    }
    #[test]
    fn timer_ids_wrap_without_collisions_or_reordering() {
        let mut q = JobQueue::default();
        assert_eq!(q.push_timer_job(1., job(1.)), 1);
        q.next_timer_id = (1 << 53) - 1;
        assert_eq!(q.push_timer_job(1., job(2.)), 2);
        assert_eq!(take(&mut q), 1.);
        assert_eq!(take(&mut q), 2.);
    }
    #[test]
    fn cancellation_releases_roots_immediately() {
        let mut q = JobQueue::default();
        let object = Value::object(vec![]);
        let id = q.push_timer(1., object.clone(), vec![object.clone()]);
        let mut roots = vec![];
        q.trace_roots(&mut roots);
        assert_eq!(roots.len(), 2);
        roots.clear();
        q.cancel_timer(id);
        q.cancel_timer(id);
        q.trace_roots(&mut roots);
        assert!(roots.is_empty());
        assert!(q.is_empty());
    }
}
