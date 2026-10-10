//! Opt-in scheduling policy and clocks. All deadline values are milliseconds.
use crate::error::VmErr;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Injectable monotonic millisecond clock. Values must be finite, nonnegative,
/// and nondecreasing. A clock is local to the interpreter owner thread.
pub trait Clock {
    fn now_ms(&self) -> f64;
}

#[derive(Clone, Default)]
pub struct VirtualClock(Rc<Cell<f64>>);
impl VirtualClock {
    pub fn advance(&self, milliseconds: f64) -> Result<(), VmErr> {
        let next = self.now_ms() + milliseconds;
        if !milliseconds.is_finite() || milliseconds < 0.0 || !next.is_finite() {
            return Err(VmErr::Msg(
                "virtual clock advance must be finite and nonnegative".into(),
            ));
        }
        self.0.set(next);
        Ok(())
    }
}
impl Clock for VirtualClock {
    fn now_ms(&self) -> f64 {
        self.0.get()
    }
}

pub struct RealTimeClock {
    #[cfg(not(target_arch = "wasm32"))]
    origin: std::time::Instant,
    #[cfg(target_arch = "wasm32")]
    origin: f64,
    #[cfg(target_arch = "wasm32")]
    last: Cell<f64>,
}
impl Default for RealTimeClock {
    fn default() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            origin: std::time::Instant::now(),
            #[cfg(target_arch = "wasm32")]
            origin: js_sys::Date::now(),
            #[cfg(target_arch = "wasm32")]
            last: Cell::new(0.0),
        }
    }
}
impl Clock for RealTimeClock {
    fn now_ms(&self) -> f64 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.origin.elapsed().as_secs_f64() * 1000.0
        }
        #[cfg(target_arch = "wasm32")]
        {
            let now = (js_sys::Date::now() - self.origin).max(self.last.get());
            self.last.set(now);
            now
        }
    }
}

#[derive(Clone, Default)]
pub enum ClockMode {
    /// Historical delay ordering: every timer is eligible, no waiting.
    #[default]
    Legacy,
    Virtual(VirtualClock),
    RealTime(Rc<dyn Clock>),
}
impl ClockMode {
    pub(crate) fn now_ms(&self) -> f64 {
        match self {
            Self::Legacy => 0.,
            Self::Virtual(c) => c.now_ms(),
            Self::RealTime(c) => c.now_ms(),
        }
    }
    pub(crate) fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy)
    }
    pub(crate) fn is_real_time(&self) -> bool {
        matches!(self, Self::RealTime(_))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fairness {
    #[default]
    ExternalFirst,
    /// Alternate eligible external events and timers, starting with external.
    Alternate,
}
#[derive(Clone, Copy, Debug)]
pub struct EventLoopOptions {
    pub fairness: Fairness,
    pub host_batch_size: usize,
    pub external_capacity: usize,
}
impl Default for EventLoopOptions {
    fn default() -> Self {
        Self {
            fairness: Fairness::ExternalFirst,
            host_batch_size: 64,
            external_capacity: 1024,
        }
    }
}
/// Soft scheduler budget; a yield never refills hard guest limits.
#[derive(Clone, Copy, Debug)]
pub struct TurnBudget {
    pub max_jobs: usize,
    pub max_duration: Option<Duration>,
}
impl TurnBudget {
    pub fn jobs(max_jobs: usize) -> Self {
        Self {
            max_jobs,
            max_duration: None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YieldReason {
    Idle,
    JobBudget,
    TimeBudget,
    Backpressure,
}
#[derive(Clone, Copy, Debug)]
pub struct TurnOutcome {
    pub executed_jobs: usize,
    pub runnable: bool,
    pub yield_reason: YieldReason,
    /// Absolute milliseconds in the configured clock; legacy reports delays.
    pub next_deadline: Option<f64>,
    pub checkpoint_pending: bool,
}
/// A thread-safe cancellation signal. Cancels active guest execution and waits;
/// never transfers guest values between threads.
#[derive(Clone, Default)]
pub struct CancellationToken(
    Arc<AtomicBool>,
    Arc<std::sync::Mutex<Vec<std::sync::Weak<crate::host::WakeSignal>>>>,
);
impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
        for wake in self
            .1
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(std::sync::Weak::upgrade)
        {
            wake.fire();
        }
    }
    #[doc(hidden)]
    pub fn register_wake(&self, wake: &Arc<crate::host::WakeSignal>) {
        let mut wakes = self.1.lock().unwrap_or_else(|e| e.into_inner());
        wakes.retain(|w| w.strong_count() > 0);
        if !wakes.iter().any(|w| w.ptr_eq(&Arc::downgrade(wake))) {
            wakes.push(Arc::downgrade(wake));
        }
        if self.is_cancelled() {
            wake.fire();
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub(super) struct ExecutionState {
    pub can_block: Cell<bool>,
    pub wake: Arc<crate::host::WakeSignal>,
    pub fuel: Cell<u64>,
    pub poll_remaining: Cell<u64>,
    pub loops: Cell<u64>,
    pub jobs: Cell<usize>,
    pub cancellation: std::cell::RefCell<CancellationToken>,
    pub deadline: Cell<Option<f64>>,
    pub clock: Rc<dyn Clock>,
    pub drain_depth: Cell<usize>,
    pub active: Cell<bool>,
    pub continuations: Cell<usize>,
}
impl ExecutionState {
    pub fn new() -> Self {
        Self {
            wake: Arc::new(crate::host::WakeSignal::default()),
            fuel: Cell::new(super::DEFAULT_FUEL_BUDGET),
            poll_remaining: Cell::new(0),
            loops: Cell::new(super::DEFAULT_LOOP_BUDGET),
            jobs: Cell::new(super::jobs::MAX_JOBS_PER_DRAIN),
            can_block: Cell::new(false),
            cancellation: std::cell::RefCell::new(CancellationToken::default()),
            deadline: Cell::new(None),
            clock: Rc::new(RealTimeClock::default()),
            drain_depth: Cell::new(0),
            active: Cell::new(false),
            continuations: Cell::new(0),
        }
    }
    pub fn check(&self) -> Result<(), VmErr> {
        if self.cancellation.borrow().is_cancelled() {
            return Err(VmErr::Msg("Error: Guest execution cancelled".into()));
        }
        if self
            .deadline
            .get()
            .is_some_and(|d| self.clock.now_ms() >= d)
        {
            return Err(VmErr::Msg(
                "RangeError: Guest execution deadline exceeded".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::Interpreter;

    fn controlled() -> (Interpreter, VirtualClock) {
        let mut vm = Interpreter::with_builtins();
        let clock = VirtualClock::default();
        Rc::get_mut(&mut vm.execution)
            .expect("fresh execution")
            .clock = Rc::new(clock.clone());
        (vm, clock)
    }

    #[test]
    fn batched_polling_preserves_exact_fuel_and_bounds_zero_cost_instructions() {
        let (mut vm, _) = controlled();
        vm.set_fuel_budget(7);
        vm.begin_execution();
        vm.consume_fuel(3).unwrap();
        vm.consume_fuel(4).unwrap();
        assert_eq!(vm.execution.fuel.get(), 0);
        assert!(vm.consume_fuel(1).unwrap_err().to_string().contains("fuel"));
        vm.set_fuel_budget(1000);
        vm.execution.poll_remaining.set(0);
        vm.consume_fuel(0).unwrap();
        vm.execution.cancellation.borrow().cancel();
        let mut observed = false;
        for _ in 0..64 {
            if vm.consume_fuel(0).is_err() {
                observed = true;
                break;
            }
        }
        assert!(
            observed,
            "zero-cost dispatch must observe cancellation within 64 instructions"
        );
    }
    #[test]
    fn batched_deadline_polling_has_the_same_bound() {
        let (mut vm, clock) = controlled();
        vm.begin_execution();
        vm.execution.deadline.set(Some(1.));
        vm.consume_fuel(1).unwrap();
        clock.advance(2.).unwrap();
        let mut observed = false;
        for _ in 0..64 {
            if vm.consume_fuel(1).is_err() {
                observed = true;
                break;
            }
        }
        assert!(observed);
    }

    #[test]
    fn completed_execution_retires_deadline_without_idle_reactivation() {
        let (mut vm, clock) = controlled();
        vm.set_execution_timeout(Some(Duration::from_millis(50)));
        vm.eval_source("var answer = 42;").unwrap();
        assert!(!vm.has_active_execution());
        assert!(vm.execution.deadline.get().is_none());
        clock.advance(100.).unwrap();
        vm.poll_event_loop(TurnBudget::jobs(10)).unwrap();
        assert!(!vm.has_active_execution());
        vm.set_execution_timeout(Some(Duration::from_millis(50)));
        assert!(matches!(
            vm.eval_source("2 + 2;").unwrap(),
            crate::Value::Number(4.0)
        ));
    }

    #[test]
    fn timers_checkpoints_and_suspended_bodies_keep_the_original_deadline() {
        for source in [
            "setTimeout(() => 1, 1000);",
            "queueMicrotask(() => 1);",
            #[cfg(stackful_coroutines)]
            "var gate=new Promise(() => {}); async function f(){ await gate; } var suspended=f();",
        ] {
            let (mut vm, clock) = controlled();
            crate::test_support::install_timers(&mut vm.global.borrow_mut());
            vm.jobs
                .borrow_mut()
                .set_clock(ClockMode::Virtual(VirtualClock::default()))
                .unwrap();
            vm.set_execution_timeout(Some(Duration::from_millis(50)));
            vm.begin_execution();
            vm.run_program_body(&crate::parser::parse_cached(source).unwrap())
                .unwrap();
            vm.poll_event_loop(TurnBudget::jobs(0)).unwrap();
            vm.retire_completed_execution();
            assert!(vm.has_active_execution(), "{source}");
            let deadline = vm.execution.deadline.get();
            vm.begin_execution();
            assert_eq!(vm.execution.deadline.get(), deadline);
            clock.advance(100.).unwrap();
            assert!(
                vm.poll_event_loop(TurnBudget::jobs(10))
                    .unwrap_err()
                    .to_string()
                    .contains("deadline")
            );
        }
    }

    #[test]
    #[cfg(not(stackful_coroutines))]
    fn buffered_async_bodies_reject_unsettleable_await_without_retaining_a_deadline() {
        let (mut vm, _) = controlled();
        vm.set_execution_timeout(Some(Duration::from_millis(50)));
        let error = vm
            .eval_source("var gate=new Promise(() => {}); async function f(){ await gate; } f();")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot synchronously await a pending Promise")
        );
        assert!(!vm.has_active_execution());
        assert!(vm.execution.deadline.get().is_none());
    }
}
