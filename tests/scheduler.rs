#![cfg(feature = "runtime")]
use napi_vm::interpreter::{ExecutionBudget, Job};
use napi_vm::{HostBridge, HostEvent, Interpreter, Value, VmErr};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

struct ProbeBridge {
    polls: Cell<usize>,
    blocking: Cell<usize>,
    events: RefCell<Vec<HostEvent>>,
}
impl HostBridge for ProbeBridge {
    fn trace_roots(&self, values: &mut Vec<Value>, _: &mut Vec<napi_vm::interpreter::Env>) {
        for event in self.events.borrow().iter() {
            match event {
                HostEvent::Callback(callback) => {
                    values.extend([callback.callback.clone(), callback.this_value.clone()]);
                    values.extend(callback.args.iter().cloned());
                }
                HostEvent::UncaughtException(value) => values.push(value.clone()),
                HostEvent::PromiseSettled { promise, value, .. } => {
                    values.extend([Value::Promise(promise.clone()), value.clone()]);
                }
            }
        }
    }

    fn call_host(&self, _: usize, _: Vec<Value>) -> Result<Value, VmErr> {
        unreachable!()
    }
    fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        self.polls.set(self.polls.get() + 1);
        if !timeout.is_zero() {
            self.blocking.set(self.blocking.get() + 1);
        }
        Ok(std::mem::take(&mut *self.events.borrow_mut()))
    }
}
fn probe() -> Rc<ProbeBridge> {
    Rc::new(ProbeBridge {
        polls: Cell::new(0),
        blocking: Cell::new(0),
        events: RefCell::new(vec![]),
    })
}
fn callback(vm: &mut Interpreter, source: &str) -> Value {
    vm.eval_source(source).unwrap()
}
#[test]
fn hard_job_boundary_keeps_next_job() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let cb = callback(&mut vm, "var hits=0; ()=>hits++;");
    for _ in 0..2 {
        vm.jobs.borrow_mut().push_microtask(Job::Callback {
            callback: cb.clone(),
            args: vec![],
        });
    }
    vm.set_execution_budget(ExecutionBudget {
        max_jobs: 1,
        ..ExecutionBudget::default()
    });
    assert!(
        vm.drain_jobs()
            .unwrap_err()
            .to_string()
            .contains("Maximum job count")
    );
    assert!(vm.jobs.borrow().has_microtasks());
    assert!(matches!(
        vm.global.borrow().get("hits"),
        Some(Value::Number(1.0))
    ));
}
#[test]
fn queued_work_counts_as_progress_without_waiting() {
    for micro in [true, false] {
        let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
        let cb = callback(&mut vm, "()=>1;");
        let p = probe();
        vm.set_host_bridge(p.clone());
        let job = Job::Callback {
            callback: cb,
            args: vec![],
        };
        if micro {
            vm.jobs.borrow_mut().push_microtask(job);
        } else {
            vm.jobs.borrow_mut().push_timer_job(0., job);
        }
        assert!(vm.run_event_loop_once(Duration::from_secs(1)).unwrap());
        assert_eq!(p.blocking.get(), 0);
        assert!(!vm.run_event_loop_once(Duration::ZERO).unwrap());
    }
}
#[test]
fn host_is_sampled_at_checkpoints_not_per_microtask() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let cb = callback(&mut vm, "var hits=0; ()=>hits++;");
    let p = probe();
    vm.set_host_bridge(p.clone());
    for _ in 0..100 {
        vm.jobs.borrow_mut().push_microtask(Job::Callback {
            callback: cb.clone(),
            args: vec![],
        });
    }
    vm.drain_jobs().unwrap();
    assert_eq!(p.polls.get(), 1);
    assert!(matches!(
        vm.global.borrow().get("hits"),
        Some(Value::Number(100.0))
    ));
}
#[test]
fn nested_timers_and_recursive_microtasks_keep_order() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    vm.eval_source("var seen=[]; setTimeout(()=>{seen.push('a'); queueMicrotask(()=>{seen.push('micro');queueMicrotask(()=>seen.push('nested'));});setTimeout(()=>seen.push('inner'),0);},1); setTimeout(()=>seen.push('b'),1);").unwrap();
    assert!(
        matches!(vm.eval_source("seen.join(',')").unwrap(),Value::String(ref s) if s=="a,micro,nested,inner,b")
    );
}

use napi_vm::{
    CancellationToken, Clock, ClockMode, EventLoopOptions, Fairness, RealTimeClock, TurnBudget,
    VirtualClock, YieldReason,
};
#[test]
fn virtual_deadlines_and_nested_timers_are_absolute() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let clock = VirtualClock::default();
    vm.jobs
        .borrow_mut()
        .set_clock(ClockMode::Virtual(clock.clone()))
        .unwrap();
    vm.eval_source(
        "var seen=[];setTimeout(()=>{seen.push('a');setTimeout(()=>seen.push('b'),5);},10);",
    )
    .unwrap();
    assert_eq!(
        vm.poll_event_loop(TurnBudget::jobs(10))
            .unwrap()
            .next_deadline,
        Some(10.)
    );
    clock.advance(9.).unwrap();
    assert_eq!(
        vm.poll_event_loop(TurnBudget::jobs(10))
            .unwrap()
            .executed_jobs,
        0
    );
    clock.advance(1.).unwrap();
    let turn = vm.poll_event_loop(TurnBudget::jobs(10)).unwrap();
    assert_eq!(turn.executed_jobs, 1);
    assert_eq!(turn.next_deadline, Some(15.));
    assert!(!turn.runnable);
    clock.advance(5.).unwrap();
    assert_eq!(
        vm.poll_event_loop(TurnBudget::jobs(10))
            .unwrap()
            .executed_jobs,
        1
    );
    assert!(matches!(vm.eval_source("seen.join(',')").unwrap(),Value::String(ref s) if s=="a,b"));
    for invalid in [-1., f64::NAN, f64::INFINITY] {
        assert!(clock.advance(invalid).is_err());
    }
    assert_eq!(clock.now_ms(), 15.);
}
#[test]
fn soft_yield_resumes_microtasks_before_timers_or_new_evaluation() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let cb = callback(
        &mut vm,
        "var seen=[];()=>{seen.push('a');queueMicrotask(()=>{seen.push('m');queueMicrotask(()=>seen.push('n'));});}",
    );
    vm.jobs.borrow_mut().push_timer(0., cb, vec![]);
    let cb = callback(&mut vm, "()=>seen.push('b')"); // previous timer completes before installing this one
    // Clear prior observations and queue a fresh macro that creates two microtasks.
    vm.eval_source("seen=[]").unwrap();
    let cb_a = callback(
        &mut vm,
        "()=>{seen.push('a');queueMicrotask(()=>{seen.push('m');queueMicrotask(()=>seen.push('n'));});}",
    );
    vm.jobs.borrow_mut().push_timer(0., cb_a, vec![]);
    vm.jobs.borrow_mut().push_timer(0., cb, vec![]);
    let first = vm.poll_event_loop(TurnBudget::jobs(1)).unwrap();
    assert!(first.checkpoint_pending);
    assert!(
        vm.eval_source("seen.push('intruder')")
            .unwrap_err()
            .to_string()
            .contains("checkpoint")
    );
    for _ in 0..3 {
        vm.poll_event_loop(TurnBudget::jobs(1)).unwrap();
    }
    assert!(
        matches!(vm.eval_source("seen.join(',')").unwrap(),Value::String(ref s) if s=="a,m,n,b")
    );
}
#[test]
fn hard_limits_survive_soft_yields() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let cb = callback(&mut vm, "var hits=0;()=>hits++;");
    vm.set_execution_budget(ExecutionBudget {
        max_jobs: 2,
        ..ExecutionBudget::default()
    });
    for _ in 0..3 {
        vm.jobs.borrow_mut().push_microtask(Job::Callback {
            callback: cb.clone(),
            args: vec![],
        });
    }
    for _ in 0..2 {
        assert_eq!(
            vm.poll_event_loop(TurnBudget::jobs(1))
                .unwrap()
                .executed_jobs,
            1
        );
    }
    vm.begin_execution(); // a pending checkpoint must not refill the budget
    assert!(vm.poll_event_loop(TurnBudget::jobs(1)).is_err());
    assert!(vm.jobs.borrow().has_microtasks());
}
#[test]
fn zero_and_time_budgets_keep_the_next_job() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let cb = callback(&mut vm, "()=>1");
    vm.jobs.borrow_mut().push_timer(0., cb, vec![]);
    let outcome = vm.poll_event_loop(TurnBudget::jobs(0)).unwrap();
    assert_eq!(outcome.executed_jobs, 0);
    assert!(outcome.runnable);
    let outcome = vm
        .poll_event_loop(TurnBudget {
            max_jobs: 10,
            max_duration: Some(Duration::ZERO),
        })
        .unwrap();
    assert_eq!(outcome.yield_reason, YieldReason::TimeBudget);
    assert!(outcome.runnable);
    assert_eq!(
        vm.poll_event_loop(TurnBudget::jobs(1))
            .unwrap()
            .executed_jobs,
        1
    );
}
#[test]
fn real_time_waits_only_for_due_timers_and_reports_progress() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let cb = callback(&mut vm, "var hit=false;()=>{hit=true}");
    vm.jobs
        .borrow_mut()
        .set_clock(ClockMode::RealTime(Rc::new(RealTimeClock::default())))
        .unwrap();
    vm.jobs.borrow_mut().push_timer(20., cb, vec![]);
    assert_eq!(
        vm.poll_event_loop(TurnBudget::jobs(10))
            .unwrap()
            .executed_jobs,
        0
    );
    assert!(vm.run_event_loop_once(Duration::from_secs(1)).unwrap());
    assert!(matches!(
        vm.global.borrow().get("hit"),
        Some(Value::Bool(true))
    ));
}
struct FloodBridge {
    callback: Value,
}
impl HostBridge for FloodBridge {
    fn trace_roots(&self, values: &mut Vec<Value>, _: &mut Vec<napi_vm::interpreter::Env>) {
        values.push(self.callback.clone());
    }

    fn call_host(&self, _: usize, _: Vec<Value>) -> Result<Value, VmErr> {
        unreachable!()
    }
    fn poll_host_events_bounded(&self, _: Duration, limit: usize) -> Result<Vec<HostEvent>, VmErr> {
        assert_eq!(limit, 1);
        Ok(vec![HostEvent::Callback(napi_vm::HostCallback {
            callback: self.callback.clone(),
            this_value: Value::Undefined,
            args: vec![],
            kind: napi_vm::HostCallbackKind::Call,
        })])
    }
}
#[test]
fn opt_in_alternation_prevents_timer_starvation_under_host_flood() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let e = callback(&mut vm, "var seen=[];()=>seen.push('e')");
    let t = callback(&mut vm, "()=>seen.push('t')");
    vm.set_event_loop_options(EventLoopOptions {
        fairness: Fairness::Alternate,
        host_batch_size: 1,
        external_capacity: 2,
    })
    .unwrap();
    vm.set_host_bridge(Rc::new(FloodBridge { callback: e }));
    vm.jobs.borrow_mut().push_timer(0., t.clone(), vec![]);
    vm.jobs.borrow_mut().push_timer(0., t, vec![]);
    assert_eq!(
        vm.poll_event_loop(TurnBudget::jobs(4))
            .unwrap()
            .executed_jobs,
        4
    );
    vm.host = None;
    // Read without admitting another evaluation/macrotask.
    let seen = vm.global.borrow().get("seen").unwrap();
    assert_eq!(napi_vm::format::to_string(&seen), "[e, t, e, t]");
    assert!(vm.jobs.borrow().external_len() <= 2);
}
#[test]
fn oversized_legacy_ingress_reports_backpressure_without_losing_work() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let cb = callback(&mut vm, "var hits=0;()=>hits++");
    let p = probe();
    for _ in 0..7 {
        p.events
            .borrow_mut()
            .push(HostEvent::Callback(napi_vm::HostCallback {
                callback: cb.clone(),
                this_value: Value::Undefined,
                args: vec![],
                kind: napi_vm::HostCallbackKind::Call,
            }));
    }
    vm.set_event_loop_options(EventLoopOptions {
        external_capacity: 2,
        host_batch_size: 1,
        ..EventLoopOptions::default()
    })
    .unwrap();
    vm.set_host_bridge(p);
    let turn = vm.poll_event_loop(TurnBudget::jobs(1)).unwrap();
    assert_eq!(turn.yield_reason, YieldReason::Backpressure);
    assert!(vm.jobs.borrow().external_len() <= 2);
    vm.drain_jobs().unwrap();
    assert!(matches!(
        vm.global.borrow().get("hits"),
        Some(Value::Number(7.0))
    ));
}
#[test]
fn cancellation_and_deadlines_interrupt_long_callbacks_and_coroutine_bodies() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let cb = callback(&mut vm, "()=>{while(true){}};");
    vm.jobs.borrow_mut().push_timer(0., cb, vec![]);
    vm.set_execution_timeout(Some(Duration::from_millis(5)));
    assert!(
        vm.poll_event_loop(TurnBudget::jobs(1))
            .unwrap_err()
            .to_string()
            .contains("deadline")
    );
    vm.set_execution_timeout(None);
    let token = CancellationToken::default();
    vm.set_cancellation_token(token.clone());
    let producer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(5));
        token.cancel();
    });
    assert!(
        vm.eval_source("async function spin(){while(true){}} spin();")
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    producer.join().unwrap();
}

#[test]
fn published_gc_roots_do_not_retain_a_cancelled_timer() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let value = Value::object(vec![]);
    let weak = match &value {
        Value::Object { props, .. } => Rc::downgrade(props),
        _ => unreachable!(),
    };
    let id = vm.jobs.borrow_mut().push_timer(1., value, vec![]);
    vm.run_program_body(&[]).unwrap(); // publish interpreter roots with timer pending
    assert!(weak.upgrade().is_some());
    vm.jobs.borrow_mut().cancel_timer(id);
    assert!(
        weak.upgrade().is_none(),
        "published snapshots must not pin removed callbacks"
    );
}

#[test]
fn nested_await_checkpoints_share_the_hard_job_budget() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    vm.set_execution_budget(ExecutionBudget {
        max_jobs: 2,
        ..ExecutionBudget::default()
    });
    assert!(vm.eval_source("var hits=0;queueMicrotask(()=>hits++);await 0;queueMicrotask(()=>hits++);queueMicrotask(()=>hits++);").unwrap_err().to_string().contains("job count"));
    assert!(matches!(
        vm.global.borrow().get("hits"),
        Some(Value::Number(2.0))
    ));
    assert!(vm.jobs.borrow().has_microtasks());
}
#[test]
fn soft_yields_do_not_refill_guest_fuel() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let cb = callback(&mut vm, "()=>1+1");
    vm.set_fuel_budget(100);
    for _ in 0..3 {
        vm.jobs.borrow_mut().push_microtask(Job::Callback {
            callback: cb.clone(),
            args: vec![],
        });
    }
    let mut previous = vm.execution_budget().fuel;
    for _ in 0..3 {
        vm.poll_event_loop(TurnBudget::jobs(1)).unwrap();
        let fuel = vm.execution_budget().fuel;
        assert!(fuel < previous);
        previous = fuel;
    }
}
#[test]
fn collection_refuses_while_dequeued_native_job_values_are_on_the_stack() {
    fn probe(vm: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
        assert_eq!(
            vm.collect_cycles().skipped,
            Some(napi_vm::heap::SkipReason::Executing)
        );
        Ok(Value::Undefined)
    }
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    vm.jobs.borrow_mut().push_microtask(Job::Callback {
        callback: Value::NativeFunction {
            name: "gc_probe".into(),
            callable: probe,
        },
        args: vec![Value::object(vec![])],
    });
    vm.drain_jobs().unwrap();
    assert_eq!(vm.collect_cycles().skipped, None);
}

#[test]
fn throwing_checkpoint_reconciles_state_without_refilling_jobs() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    let bad = callback(&mut vm, "()=>{throw new Error('boom')}");
    let good = callback(&mut vm, "var hits=0;()=>hits++");
    vm.jobs.borrow_mut().push_microtask(Job::Callback {
        callback: bad,
        args: vec![],
    });
    vm.jobs.borrow_mut().push_microtask(Job::Callback {
        callback: good,
        args: vec![],
    });
    vm.set_execution_budget(ExecutionBudget {
        max_jobs: 1,
        ..ExecutionBudget::default()
    });
    assert!(vm.drain_jobs().unwrap_err().to_string().contains("boom"));
    assert!(vm.ensure_can_evaluate().is_err());
    vm.begin_execution();
    assert!(
        vm.drain_jobs()
            .unwrap_err()
            .to_string()
            .contains("Maximum job count")
    );
    assert!(vm.jobs.borrow().has_microtasks());
    assert!(matches!(
        vm.global.borrow().get("hits"),
        Some(Value::Number(0.0))
    ));
}

#[test]
fn last_throwing_microtask_clears_checkpoint() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    assert!(
        vm.eval_source("queueMicrotask(()=>{throw new Error('boom')});")
            .is_err()
    );
    assert!(vm.ensure_can_evaluate().is_ok());
    assert!(matches!(
        vm.eval_source("42;").unwrap(),
        Value::Number(42.0)
    ));
}

#[test]
fn future_timer_wait_is_capped_by_execution_deadline() {
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    vm.jobs
        .borrow_mut()
        .set_clock(ClockMode::RealTime(Rc::new(RealTimeClock::default())))
        .unwrap();
    vm.eval_source("setTimeout(()=>42,60000);").unwrap();
    vm.set_execution_timeout(Some(Duration::ZERO));
    assert!(
        vm.run_event_loop_once(Duration::from_secs(60))
            .map_err(|e| e.to_string())
            .unwrap_err()
            .to_string()
            .contains("deadline")
    );
    assert!(vm.jobs.borrow().next_deadline().is_some());
}

#[test]
fn cancellation_between_readiness_check_and_wait_is_latched() {
    struct BarrierBridge {
        ready: std::sync::mpsc::Sender<()>,
        release: RefCell<std::sync::mpsc::Receiver<()>>,
    }
    impl HostBridge for BarrierBridge {
        fn call_host(&self, _: usize, _: Vec<Value>) -> Result<Value, VmErr> {
            unreachable!()
        }
        fn poll_host_events(&self, _: Duration) -> Result<Vec<HostEvent>, VmErr> {
            self.ready.send(()).unwrap();
            self.release.borrow().recv().unwrap();
            Ok(vec![])
        }
    }
    let token = CancellationToken::default();
    let cancelled = token.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
        vm.set_cancellation_token(token);
        vm.set_host_bridge(Rc::new(BarrierBridge {
            ready: ready_tx,
            release: RefCell::new(release_rx),
        }));
        vm.run_event_loop_once(Duration::from_secs(60))
            .map_err(|e| e.to_string())
            .unwrap_err()
            .to_string()
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    cancelled.cancel();
    release_tx.send(()).unwrap();
    assert!(worker.join().unwrap().contains("cancelled"));
}

#[test]
fn future_await_resumes_before_another_due_timer() {
    // A controllable real-time clock makes both timers become due during
    // await, without relying on matching wall-clock registration timestamps.
    struct StepClock(Cell<usize>);
    impl Clock for StepClock {
        fn now_ms(&self) -> f64 {
            let call = self.0.get();
            self.0.set(call + 1);
            if call < 4 { 0.0 } else { 50.0 }
        }
    }
    let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
    vm.jobs
        .borrow_mut()
        .set_clock(ClockMode::RealTime(Rc::new(StepClock(Cell::new(0)))))
        .unwrap();
    let result = vm.eval_source("var seen=[];var p=new Promise(r=>setTimeout(()=>{seen.push('first');r(42);},50));setTimeout(()=>seen.push('second'),50);await p;seen.push('after');seen.join(',');").unwrap();
    assert!(matches!(result, Value::String(ref s) if s == "first,after"));
    assert!(
        matches!(vm.eval_source("seen.join(',');").unwrap(), Value::String(ref s) if s == "first,after,second")
    );
}

#[test]
fn blocking_only_bridge_is_polled_before_the_full_timeout() {
    struct BlockingBridge {
        entered: std::sync::mpsc::Sender<Duration>,
        input: std::sync::mpsc::Receiver<()>,
        promise: Rc<RefCell<napi_vm::value::PromiseInner>>,
    }
    impl HostBridge for BlockingBridge {
        fn trace_roots(&self, values: &mut Vec<Value>, _: &mut Vec<napi_vm::interpreter::Env>) {
            values.push(Value::Promise(self.promise.clone()));
        }

        fn call_host(&self, _: usize, _: Vec<Value>) -> Result<Value, VmErr> {
            unreachable!()
        }
        fn supports_blocking_event_wait(&self) -> bool {
            true
        }
        fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
            if timeout.is_zero() {
                return Ok(vec![]);
            }
            self.entered.send(timeout).unwrap();
            match self.input.recv_timeout(timeout) {
                Ok(()) => Ok(vec![HostEvent::PromiseSettled {
                    promise: self.promise.clone(),
                    state: napi_vm::value::PromiseState::Fulfilled,
                    value: Value::Number(42.0),
                }]),
                Err(_) => Ok(vec![]),
            }
        }
    }
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let token = CancellationToken::default();
    let cancel = token.clone();
    let worker = std::thread::spawn(move || {
        let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
        vm.set_cancellation_token(token);
        vm.set_host_bridge(Rc::new(BlockingBridge {
            entered: entered_tx,
            input: input_rx,
            promise: Value::pending_promise(),
        }));
        vm.run_event_loop_once(Duration::from_secs(60))
            .map_err(|e| e.to_string())
    });
    let entered = entered_rx.recv_timeout(Duration::from_secs(1));
    // Cleanly interrupt the old private wait before reporting the failed barrier.
    if entered.is_err() {
        cancel.cancel();
    } else {
        input_tx.send(()).unwrap();
    }
    let result = worker.join().unwrap();
    assert!(
        entered.is_ok(),
        "blocking-only bridge was never given a blocking poll"
    );
    assert!(result.unwrap());
    assert!(entered.unwrap() <= Duration::from_millis(10));
}

#[test]
fn blocking_only_waits_bound_cancellation_deadlines_timeouts_and_shutdown() {
    struct Bridge {
        entered: std::sync::mpsc::Sender<Duration>,
        input: std::sync::mpsc::Receiver<()>,
    }
    impl HostBridge for Bridge {
        fn call_host(&self, _: usize, _: Vec<Value>) -> Result<Value, VmErr> {
            unreachable!()
        }
        fn supports_blocking_event_wait(&self) -> bool {
            true
        }
        fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
            if timeout.is_zero() {
                return Ok(vec![]);
            }
            self.entered.send(timeout).unwrap();
            match self.input.recv_timeout(timeout) {
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    Err(VmErr::Msg("bridge shutdown".into()))
                }
                _ => Ok(vec![]),
            }
        }
    }
    for mode in ["cancel", "deadline", "timeout", "shutdown"] {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (input_tx, input_rx) = std::sync::mpsc::channel();
        let token = CancellationToken::default();
        let cancel = token.clone();
        let worker = std::thread::spawn(move || {
            let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
            vm.set_host_bridge(Rc::new(Bridge {
                entered: entered_tx,
                input: input_rx,
            }));
            vm.set_cancellation_token(token);
            if mode == "deadline" {
                vm.set_execution_timeout(Some(Duration::from_millis(25)));
            }
            vm.run_event_loop_once(if mode == "timeout" {
                Duration::from_millis(25)
            } else {
                Duration::from_secs(60)
            })
            .map_err(|e| e.to_string())
        });
        let slice = entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking poll barrier");
        assert!(slice <= Duration::from_millis(10));
        if mode == "cancel" {
            cancel.cancel();
        }
        let input = if mode == "shutdown" {
            drop(input_tx);
            None
        } else {
            Some(input_tx)
        };
        let result = worker.join().unwrap();
        match mode {
            "cancel" => assert!(result.unwrap_err().contains("cancelled")),
            "deadline" => assert!(result.unwrap_err().contains("deadline")),
            "shutdown" => assert!(result.unwrap_err().contains("shutdown")),
            _ => assert!(!result.unwrap()),
        }
        // The bounded blocking path sleeps rather than repeatedly polling.
        assert!(entered_rx.try_iter().count() < 10);
        drop(input);
    }
}

#[test]
fn notifier_delivery_between_poll_and_sleep_is_latched() {
    struct Bridge {
        ready: std::sync::mpsc::Sender<napi_vm::host::WakeNotifier>,
        release: RefCell<std::sync::mpsc::Receiver<()>>,
        wake: RefCell<Option<napi_vm::host::WakeNotifier>>,
        first: Cell<bool>,
        emitted: Cell<bool>,
        promise: Rc<RefCell<napi_vm::value::PromiseInner>>,
    }
    impl HostBridge for Bridge {
        fn trace_roots(&self, values: &mut Vec<Value>, _: &mut Vec<napi_vm::interpreter::Env>) {
            values.push(Value::Promise(self.promise.clone()));
        }

        fn call_host(&self, _: usize, _: Vec<Value>) -> Result<Value, VmErr> {
            unreachable!()
        }
        fn event_wait_mode(&self) -> napi_vm::host::HostWaitMode {
            napi_vm::host::HostWaitMode::Notifications
        }
        fn set_wake_notifier(&self, wake: napi_vm::host::WakeNotifier) {
            *self.wake.borrow_mut() = Some(wake);
        }
        fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
            assert!(
                timeout.is_zero(),
                "notifier bridges must use nonblocking ingress"
            );
            if self.first.replace(false) {
                self.ready
                    .send(self.wake.borrow().as_ref().unwrap().clone())
                    .unwrap();
                self.release.borrow().recv().unwrap();
                Ok(vec![])
            } else if !self.emitted.replace(true) {
                Ok(vec![HostEvent::PromiseSettled {
                    promise: self.promise.clone(),
                    state: napi_vm::value::PromiseState::Fulfilled,
                    value: Value::Number(42.),
                }])
            } else {
                Ok(vec![])
            }
        }
    }
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut vm = napi_vm_runtime::runtime::with_runtime_builtins();
        vm.set_host_bridge(Rc::new(Bridge {
            ready: ready_tx,
            release: RefCell::new(release_rx),
            wake: RefCell::new(None),
            first: Cell::new(true),
            emitted: Cell::new(false),
            promise: Value::pending_promise(),
        }));
        vm.run_event_loop_once(Duration::from_secs(60))
            .map_err(|e| e.to_string())
    });
    let wake = ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    wake(); // Notification arrives before the owner can enter its wait.
    release_tx.send(()).unwrap();
    assert!(worker.join().unwrap().unwrap());
}
