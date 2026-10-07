//! Test262's host-agent adapter. Every worker constructs its VM on its own
//! owner thread. Channels transfer only source, primitive reports and SAB data
//! blocks; guest callbacks and realm-local values never leave that owner.
use napi_vm::interpreter::ExecutionBudget;
use napi_vm::value::SharedMemory;
use napi_vm::{CancellationToken, Interpreter, Value, VmErr};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAX_AGENTS: usize = 64;
const MAX_REPORTS: usize = 10_000;

#[derive(Clone)]
enum BroadcastId {
    Number(i32),
    BigInt(napi_vm::bigint::BigInt),
}
impl BroadcastId {
    fn value(self) -> Value {
        match self {
            Self::Number(number) => Value::Number(f64::from(number)),
            Self::BigInt(number) => Value::BigInt(std::rc::Rc::new(number)),
        }
    }
}

enum Command {
    Broadcast {
        memory: SharedMemory,
        id: BroadcastId,
        received: mpsc::Sender<()>,
    },
    Shutdown,
}

struct Worker {
    sender: mpsc::Sender<Command>,
    cancellation: CancellationToken,
    leaving: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

#[derive(Default)]
struct State {
    workers: Vec<Worker>,
    reports: VecDeque<napi_vm::JsString>,
    errors: Vec<String>,
    stopped: bool,
}

#[derive(Default)]
struct Cluster {
    state: Mutex<State>,
}

struct Context {
    cluster: Arc<Cluster>,
    receiver: Option<mpsc::Receiver<Command>>,
    callback: Option<Value>,
    callback_root: Option<napi_vm::heap::RootId>,
    cancellation: CancellationToken,
    leaving: Arc<AtomicBool>,
}

impl Drop for Context {
    fn drop(&mut self) {
        if let Some(root) = self.callback_root.take() {
            napi_vm::heap::remove_root(root);
        }
    }
}

thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

fn context<T>(f: impl FnOnce(&mut Context) -> Result<T, VmErr>) -> Result<T, VmErr> {
    CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut();
        f(slot
            .as_mut()
            .ok_or_else(|| VmErr::Msg("Error: Agent host unavailable".into()))?)
    })
}

fn cluster() -> Result<Arc<Cluster>, VmErr> {
    context(|context| Ok(context.cluster.clone()))
}

fn check_errors(cluster: &Cluster) -> Result<(), VmErr> {
    let state = cluster.state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(error) = state.errors.first() {
        return Err(VmErr::Msg(format!("Error: Agent failed: {error}")));
    }
    Ok(())
}

/// Scope all workers to this one isolated variant, including error paths.
pub(super) struct Session(Arc<Cluster>);
impl Session {
    pub(super) fn new() -> Self {
        let cluster = Arc::new(Cluster::default());
        CONTEXT.with(|slot| {
            *slot.borrow_mut() = Some(Context {
                cluster: cluster.clone(),
                receiver: None,
                callback: None,
                callback_root: None,
                cancellation: CancellationToken::default(),
                leaving: Arc::new(AtomicBool::new(false)),
            });
        });
        Self(cluster)
    }

    pub(super) fn finish(&self) -> Result<(), VmErr> {
        shutdown_cluster(&self.0);
        check_errors(&self.0)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        shutdown_cluster(&self.0);
        CONTEXT.with(|slot| *slot.borrow_mut() = None);
    }
}

fn shutdown_cluster(cluster: &Cluster) {
    let workers = {
        let mut state = cluster.state.lock().unwrap_or_else(|e| e.into_inner());
        state.stopped = true;
        std::mem::take(&mut state.workers)
    };
    for worker in &workers {
        worker.cancellation.cancel();
        let _ = worker.sender.send(Command::Shutdown);
    }
    for worker in workers {
        if worker.join.join().is_err() {
            cluster
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .errors
                .push("worker panicked".into());
        }
    }
}

pub(super) fn host(vm: &mut Interpreter) -> Value {
    Value::object(vec![
        ("start".into(), vm.native_function_in_realm("start", start)),
        (
            "broadcast".into(),
            vm.native_function_in_realm("broadcast", broadcast),
        ),
        (
            "receiveBroadcast".into(),
            vm.native_function_in_realm("receiveBroadcast", receive_broadcast),
        ),
        (
            "report".into(),
            vm.native_function_in_realm("report", report),
        ),
        (
            "getReport".into(),
            vm.native_function_in_realm("getReport", get_report),
        ),
        ("sleep".into(), vm.native_function_in_realm("sleep", sleep)),
        (
            "leaving".into(),
            vm.native_function_in_realm("leaving", leaving),
        ),
        (
            "shutdown".into(),
            vm.native_function_in_realm("shutdown", shutdown),
        ),
        (
            "monotonicNow".into(),
            vm.native_function_in_realm("monotonicNow", monotonic_now),
        ),
    ])
}

fn start(vm: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let source = vm.to_js_string(args.first().unwrap_or(&Value::Undefined))?;
    let cluster = cluster()?;
    check_errors(&cluster)?;
    let mut state = cluster.state.lock().unwrap_or_else(|e| e.into_inner());
    if state.stopped || state.workers.len() >= MAX_AGENTS {
        return Err(VmErr::Msg(
            "RangeError: Agent capacity exceeded or cluster shut down".into(),
        ));
    }
    let (sender, receiver) = mpsc::channel();
    let (started, ready) = mpsc::channel();
    let cancellation = CancellationToken::default();
    let leaving = Arc::new(AtomicBool::new(false));
    let worker_cluster = cluster.clone();
    let worker_cancellation = cancellation.clone();
    let worker_leaving = leaving.clone();
    let join = std::thread::Builder::new()
        .name("test262-agent".into())
        .spawn(move || {
            CONTEXT.with(|slot| {
                *slot.borrow_mut() = Some(Context {
                    cluster: worker_cluster.clone(),
                    receiver: Some(receiver),
                    callback: None,
                    callback_root: None,
                    cancellation: worker_cancellation.clone(),
                    leaving: worker_leaving.clone(),
                })
            });
            let result = run_worker(&source, worker_cancellation.clone(), started);
            if let Err(error) = result
                && !(worker_cancellation.is_cancelled()
                    && error
                        .to_string()
                        .starts_with("Error: Guest execution cancelled"))
            {
                worker_cluster
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .errors
                    .push(error.to_string());
            }
            worker_leaving.store(true, Ordering::Release);
            CONTEXT.with(|slot| *slot.borrow_mut() = None);
        })
        .map_err(|error| VmErr::Msg(format!("Error: Cannot start agent: {error}")))?;
    state.workers.push(Worker {
        sender,
        cancellation,
        leaving,
        join,
    });
    drop(state);
    ready
        .recv()
        .map_err(|_| VmErr::Msg("Error: Agent failed to initialize".into()))?;
    Ok(Value::Undefined)
}

fn run_worker(
    source: &napi_vm::JsString,
    cancellation: CancellationToken,
    started: mpsc::Sender<()>,
) -> Result<(), VmErr> {
    let mut vm = Interpreter::with_builtins();
    vm.set_can_block(true);
    vm.jobs
        .borrow_mut()
        .set_clock(napi_vm::ClockMode::RealTime(std::rc::Rc::new(
            napi_vm::RealTimeClock::default(),
        )))?;
    vm.set_execution_budget(ExecutionBudget {
        fuel: 1_000_000,
        max_call_depth: 128,
        max_jobs: 10_000,
    });
    vm.set_loop_budget(100_000);
    vm.set_cancellation_token(cancellation.clone());
    let host = super::realm_host(&mut vm);
    vm.global.borrow_mut().set("$262", host);
    let _ = started.send(());
    let program = Interpreter::compile_utf16_with_goal(source, napi_vm::parser::ParseGoal::Script)?;
    vm.execute(&program)?;
    super::collect_requested_gc(&mut vm);
    loop {
        if context(|context| Ok(context.leaving.load(Ordering::Acquire)))?
            || cancellation.is_cancelled()
        {
            return Ok(());
        }
        vm.set_cancellation_token(cancellation.clone());
        vm.poll_event_loop(napi_vm::TurnBudget::jobs(10_000))?;
        super::collect_requested_gc(&mut vm);
        let command = context(|context| {
            Ok(context
                .receiver
                .as_ref()
                .expect("worker receiver")
                .recv_timeout(Duration::from_millis(10)))
        })?;
        match command {
            Ok(Command::Broadcast {
                memory,
                id,
                received,
            }) => {
                let callback =
                    context(|context| Ok(context.callback.clone()))?.ok_or_else(|| {
                        VmErr::Msg("Error: Agent has no receiveBroadcast callback".into())
                    })?;
                let buffer = vm.shared_array_buffer_from_memory(memory);
                let _ = received.send(());
                // The interpreter and callback are used only on this worker's
                // owner thread. notify/report only signal native host state.
                vm.set_cancellation_token(cancellation.clone());
                vm.jobs
                    .borrow_mut()
                    .push_external_event(napi_vm::interpreter::Job::Callback {
                        callback,
                        args: vec![buffer, id.value()],
                    });
                vm.poll_event_loop(napi_vm::TurnBudget::jobs(10_000))?;
                super::collect_requested_gc(&mut vm);
            }
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn broadcast(vm: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let Some(Value::SharedArrayBuffer(buffer)) = args.first() else {
        return Err(VmErr::Msg(
            "TypeError: broadcast requires a SharedArrayBuffer".into(),
        ));
    };
    let memory = buffer
        .shared_memory()
        .ok_or_else(|| VmErr::Msg("TypeError: Cannot transfer external shared storage".into()))?;
    let id = match args.get(1) {
        Some(Value::BigInt(number)) => BroadcastId::BigInt((**number).clone()),
        value => {
            let number = vm
                .un_op(
                    napi_vm::parser::UnOp::Pos,
                    value.unwrap_or(&Value::Undefined),
                )?
                .to_number();
            let int = if !number.is_finite() {
                0
            } else {
                number.trunc().rem_euclid(4_294_967_296.0) as u32 as i32
            };
            BroadcastId::Number(int)
        }
    };
    let cluster = cluster()?;
    check_errors(&cluster)?;
    let acknowledgements = {
        let state = cluster.state.lock().unwrap_or_else(|e| e.into_inner());
        let mut acknowledgements = Vec::new();
        for worker in &state.workers {
            if worker.leaving.load(Ordering::Acquire) {
                continue;
            }
            let (received, acknowledgement) = mpsc::channel();
            if worker
                .sender
                .send(Command::Broadcast {
                    memory: memory.clone(),
                    id: id.clone(),
                    received,
                })
                .is_ok()
            {
                acknowledgements.push((acknowledgement, worker.leaving.clone()));
            }
        }
        acknowledgements
    };
    for (acknowledgement, leaving) in acknowledgements {
        loop {
            check_errors(&cluster)?;
            if leaving.load(Ordering::Acquire) {
                break;
            }
            match acknowledgement.recv_timeout(Duration::from_millis(10)) {
                Ok(()) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) if leaving.load(Ordering::Acquire) => {
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(VmErr::Msg(
                        "Error: Agent disconnected before receiving broadcast".into(),
                    ));
                }
            }
        }
    }
    Ok(Value::Undefined)
}

fn receive_broadcast(_: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let callback = args
        .first()
        .filter(|value| napi_vm::interpreter::is_callable_value(value))
        .cloned()
        .ok_or_else(|| VmErr::Msg("TypeError: receiveBroadcast requires a callback".into()))?;
    context(|context| {
        if context.receiver.is_none() {
            return Err(VmErr::Msg(
                "TypeError: receiveBroadcast is a worker operation".into(),
            ));
        }
        if let Some(root) = context.callback_root.take() {
            napi_vm::heap::remove_root(root);
        }
        context.callback_root = Some(napi_vm::heap::add_root(callback.clone()));
        context.callback = Some(callback);
        Ok(Value::Undefined)
    })
}

fn report(vm: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let message = vm.to_js_string(args.first().unwrap_or(&Value::Undefined))?;
    let cluster = cluster()?;
    let mut state = cluster.state.lock().unwrap_or_else(|e| e.into_inner());
    if state.reports.len() >= MAX_REPORTS {
        return Err(VmErr::Msg(
            "RangeError: Agent report capacity exceeded".into(),
        ));
    }
    state.reports.push_back(message);
    Ok(Value::Undefined)
}

fn get_report(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let cluster = cluster()?;
    check_errors(&cluster)?;
    let report = cluster
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .reports
        .pop_front();
    Ok(report.map_or(Value::Null, Value::String))
}

fn sleep(vm: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let duration = vm
        .un_op(
            napi_vm::parser::UnOp::Pos,
            args.first().unwrap_or(&Value::Undefined),
        )?
        .to_number();
    let duration = if duration.is_nan() || duration <= 0.0 {
        Duration::ZERO
    } else {
        Duration::try_from_secs_f64(duration / 1000.0)
            .map_err(|_| VmErr::Msg("RangeError: Invalid agent sleep duration".into()))?
    };
    let deadline = Instant::now()
        .checked_add(duration)
        .ok_or_else(|| VmErr::Msg("RangeError: Invalid agent sleep duration".into()))?;
    while Instant::now() < deadline {
        if context(|context| Ok(context.cancellation.is_cancelled()))? {
            return Err(VmErr::Msg("Error: Guest execution cancelled".into()));
        }
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(10)),
        );
    }
    Ok(Value::Undefined)
}

fn leaving(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    context(|context| {
        context.leaving.store(true, Ordering::Release);
        Ok(Value::Undefined)
    })
}

fn shutdown(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    context(|context| {
        if context.receiver.is_some() {
            return Err(VmErr::Msg(
                "TypeError: shutdown is a main agent operation".into(),
            ));
        }
        Ok(())
    })?;
    let cluster = cluster()?;
    shutdown_cluster(&cluster);
    check_errors(&cluster)?;
    Ok(Value::Undefined)
}

fn monotonic_now(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    Ok(Value::Number(
        START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0,
    ))
}
