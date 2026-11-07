//! Deterministic scheduler workloads; JSONL output for baseline comparisons.
use napi_vm::interpreter::jobs::{Job, JobQueue};
use napi_vm::{HostBridge, HostCallback, HostCallbackKind, HostEvent, Interpreter, Value, VmErr};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct CountingAllocator;
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
// SAFETY: allocation and deallocation are delegated unchanged to System.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct Events {
    events: RefCell<Vec<HostEvent>>,
    polls: Cell<u64>,
}
impl HostBridge for Events {
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
    fn poll_host_events(&self, _: Duration) -> Result<Vec<HostEvent>, VmErr> {
        self.polls.set(self.polls.get() + 1);
        Ok(std::mem::take(&mut *self.events.borrow_mut()))
    }
}
fn measure(name: &str, jobs: usize, mut run: impl FnMut() -> u64) {
    run(); // warm parse caches and native runtime
    let mut samples = Vec::with_capacity(25);
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let mut polls = 0;
    for _ in 0..25 {
        let start = Instant::now();
        polls += run();
        samples.push(start.elapsed().as_nanos() as u64);
    }
    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - allocations;
    let total: u64 = samples.iter().sum();
    samples.sort_unstable();
    println!(
        "{}",
        serde_json::json!({"workload":name,"jobs_per_sample":jobs,"samples":25,
        "throughput_jobs_s":jobs as f64 * 25.0 * 1e9 / total as f64,
        "batch_p50_us":samples[12] as f64/1000.0,"batch_p95_us":samples[23] as f64/1000.0,
        "batch_p99_us":samples[24] as f64/1000.0,"allocations_per_sample":allocations as f64/25.0,
        "host_polls_per_sample":polls as f64/25.0,"queue_depth_upper_bound":jobs,
        "timer_lateness_us":null,"wakeups":null,"idle_cpu":null})
    );
}
fn main() {
    for n in [100, 1_000, 10_000] {
        measure(&format!("timers/{n}"), n, || {
            let mut q = JobQueue::default();
            for i in 0..n {
                q.push_timer((i % 17) as f64, Value::Undefined, vec![]);
            }
            for _ in 0..n {
                assert!(q.take_timer().is_some());
            }
            assert!(q.is_empty());
            0
        });
        measure(&format!("cancel/{n}"), n, || {
            let mut q = JobQueue::default();
            let ids: Vec<_> = (0..n)
                .map(|i| q.push_timer(i as f64, Value::Undefined, vec![]))
                .collect();
            for id in ids {
                q.cancel_timer(id);
            }
            assert!(q.is_empty());
            0
        });
    }
    for (name, source, jobs, expected_hits) in [
        (
            "vm/timers",
            "var hits=0; for(var i=0;i<1000;i++) setTimeout(()=>hits++,i%17);",
            1000,
            1000,
        ),
        (
            "vm/cancel",
            "var hits=0; for(var i=0;i<1000;i++){ var id=setTimeout(()=>hits++,i); clearTimeout(id); }",
            1000,
            0,
        ),
        (
            "vm/promises",
            "var hits=0; var p=Promise.resolve(); for(var i=0;i<1000;i++) p=p.then(()=>hits++);",
            1000,
            1000,
        ),
    ] {
        measure(name, jobs, || {
            let mut vm = Interpreter::with_builtins();
            napi_vm::runtime::install_timers(&mut vm.global.borrow_mut());
            vm.eval_source(source).unwrap();
            vm.drain_jobs().unwrap();
            assert!(
                matches!(vm.global.borrow().get("hits"), Some(Value::Number(n)) if n == expected_hits as f64)
            );
            0
        });
    }
    measure("vm/mixed", 4000, || {
        let mut vm = Interpreter::with_builtins();
        napi_vm::runtime::install_timers(&mut vm.global.borrow_mut());
        let callback = vm
            .eval_source("var hits=0; ()=>{hits++; queueMicrotask(()=>hits++);}")
            .unwrap();
        let bridge = Rc::new(Events {
            events: RefCell::new(
                (0..1000)
                    .map(|_| {
                        HostEvent::Callback(HostCallback {
                            callback: callback.clone(),
                            this_value: Value::Undefined,
                            args: vec![],
                            kind: HostCallbackKind::Call,
                        })
                    })
                    .collect(),
            ),
            polls: Cell::new(0),
        });
        vm.set_host_bridge(bridge.clone());
        for i in 0..1000 {
            vm.jobs.borrow_mut().push_timer_job(
                (i % 17) as f64,
                Job::Callback {
                    callback: callback.clone(),
                    args: vec![],
                },
            );
        }
        vm.drain_jobs().unwrap();
        assert!(matches!(
            vm.global.borrow().get("hits"),
            Some(Value::Number(4000.0))
        ));
        bridge.polls.get()
    });
}
