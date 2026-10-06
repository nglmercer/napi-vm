//! Explicit persistent asynchronous ownership. The owner constructs and drops
//! the interpreter, bridge facade, guest values and coroutine stacks locally.
//! Only owned strings, wire values, tokens, and integer N-API handles cross.
use super::bridge::{AsyncState, BridgeState, NapiHostBridge};
use super::marshal::{WireValue, chk, make_str, to_napi};
use crate::host::WakeSignal;
use crate::{
    CancellationToken, ClockMode, EventLoopOptions, Fairness, Interpreter, RealTimeClock,
    TurnBudget, Value, VirtualClock,
};
use napi::bindgen_prelude::Unknown;
use napi::{Env, JsValue, sys};
use napi_derive::napi;
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

#[napi(object)]
#[derive(Default)]
pub struct AsyncSessionOptions {
    pub command_capacity: Option<u32>,
    /// Explicitly install guest timers; requires the runtime Cargo feature.
    pub timers: Option<bool>,
    /// legacy (default), virtual, or real-time.
    pub clock: Option<String>,
    /// external-first (default) or alternate.
    pub fairness: Option<String>,
    pub max_jobs_per_turn: Option<u32>,
    pub auto_poll: Option<bool>,
}

struct CompletionState {
    bridge: Arc<BridgeState>,
    tsfn: AtomicUsize,
    pending: AtomicUsize,
    guest_admissions: AtomicUsize,
    closed: AtomicBool,
    main_released: AtomicBool,
    wake: Arc<WakeSignal>,
    active_cancel: Mutex<CancellationToken>,
    cancellations: Mutex<std::collections::HashMap<usize, CancellationToken>>,
    capacity: usize,
    env: usize,
    cleanup: AtomicUsize,
    owner: Mutex<Option<std::thread::JoinHandle<()>>>,
}
struct Cleanup {
    state: Arc<CompletionState>,
    bridge: Arc<BridgeState>,
}
fn close_session(state: &CompletionState, bridge: &BridgeState) {
    state.closed.store(true, Ordering::Release);
    if !state.main_released.swap(true, Ordering::AcqRel) {
        state
            .active_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cancel();
        for token in state
            .cancellations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            token.cancel();
        }
        bridge.shutdown_on_main();
        state.wake.fire();
        let _status = unsafe {
            sys::napi_release_threadsafe_function(
                state.tsfn.load(Ordering::Acquire) as sys::napi_threadsafe_function,
                sys::ThreadsafeFunctionReleaseMode::release,
            )
        };
    }
}
unsafe extern "C" fn environment_cleanup(data: *mut std::ffi::c_void) {
    let cleanup = unsafe { Box::from_raw(data as *mut Cleanup) };
    cleanup.state.cleanup.store(0, Ordering::Release);
    close_session(&cleanup.state, &cleanup.bridge);
    // Node may destroy TSFNs as soon as this hook returns. The owner must
    // relinquish both acquisitions while the environment is still alive.
    if let Some(owner) = cleanup
        .state
        .owner
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        let _ = owner.join();
    }
}
fn remove_cleanup(state: &CompletionState) {
    let raw = state.cleanup.swap(0, Ordering::AcqRel);
    if raw != 0 {
        let status = unsafe {
            sys::napi_remove_env_cleanup_hook(
                state.env as sys::napi_env,
                Some(environment_cleanup),
                raw as *mut std::ffi::c_void,
            )
        };
        if status == sys::Status::napi_ok {
            drop(unsafe { Box::from_raw(raw as *mut Cleanup) });
        } // otherwise Node still owns the hook
    }
}

struct Reply {
    deferred: usize,
    state: Arc<CompletionState>,
    result: Result<WireValue, String>,
}
enum Operation {
    Diagnostics,
    Collect,
    Run(crate::JsString, bool),
    Expose {
        name: String,
        id: usize,
        is_async: bool,
    },
    GetGlobal(String),
    SetGlobal(String, WireValue),
    DefineModule(String, String),
    AdvanceClock(f64),
    Poll(usize),
    Limits {
        fuel: u64,
        timeout_ms: Option<u32>,
    },
}
// Covers the interval from admission through execution completion. Keeping
// this lease in the queued command closes the pre-start dependency window and
// releases reservations on send failure, cancellation, panic and shutdown.
struct GuestAdmission(Arc<CompletionState>);
impl Drop for GuestAdmission {
    fn drop(&mut self) {
        self.0.guest_admissions.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Command {
    admission: Option<GuestAdmission>,
    deferred: usize,
    operation: Operation,
    cancellation: CancellationToken,
}

/// Separate async-only API. Existing Vm synchronous/runAsync behavior is unchanged.
#[napi]
pub struct AsyncSession {
    main_bridge: Rc<NapiHostBridge>,
    sender: mpsc::SyncSender<Command>,
    state: Arc<CompletionState>,
}

fn turn_wire(o: crate::TurnOutcome) -> WireValue {
    WireValue::Object(vec![
        (
            "executedJobs".into(),
            WireValue::Number(o.executed_jobs as f64),
        ),
        ("runnable".into(), WireValue::Bool(o.runnable)),
        (
            "checkpointPending".into(),
            WireValue::Bool(o.checkpoint_pending),
        ),
        (
            "yieldReason".into(),
            WireValue::String(format!("{:?}", o.yield_reason).into()),
        ),
        (
            "nextDeadline".into(),
            o.next_deadline.map_or(WireValue::Null, WireValue::Number),
        ),
    ])
}

extern "C" fn complete_on_node(
    env: sys::napi_env,
    _callback: sys::napi_value,
    _context: *mut std::ffi::c_void,
    data: *mut std::ffi::c_void,
) {
    if data.is_null() {
        return;
    }
    let Reply {
        deferred,
        state,
        result,
    } = unsafe { *Box::from_raw(data as *mut Reply) };
    state
        .cancellations
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&deferred);
    if !env.is_null() {
        state.bridge.prune_abandoned_on_main();
        match result.and_then(|v| to_napi(env, &v.into_value()).map_err(|e| e.to_string())) {
            Ok(value) => {
                let _status = unsafe {
                    sys::napi_resolve_deferred(env, deferred as sys::napi_deferred, value)
                };
            }
            Err(message) => {
                super::vm::reject_deferred_now(env, deferred as sys::napi_deferred, message)
            }
        }
    }
    if state.pending.fetch_sub(1, Ordering::AcqRel) == 1 && !env.is_null() {
        let _status = unsafe {
            sys::napi_unref_threadsafe_function(
                env,
                state.tsfn.load(Ordering::Acquire) as sys::napi_threadsafe_function,
            )
        };
    }
}
fn complete(state: &Arc<CompletionState>, deferred: usize, result: Result<WireValue, String>) {
    let raw = Box::into_raw(Box::new(Reply {
        deferred,
        state: state.clone(),
        result,
    })) as *mut std::ffi::c_void;
    let status = unsafe {
        sys::napi_call_threadsafe_function(
            state.tsfn.load(Ordering::Acquire) as sys::napi_threadsafe_function,
            raw,
            sys::ThreadsafeFunctionCallMode::blocking,
        )
    };
    if status != sys::Status::napi_ok {
        drop(unsafe { Box::from_raw(raw as *mut Reply) });
        state
            .cancellations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&deferred);
        state.pending.fetch_sub(1, Ordering::AcqRel);
    }
}

struct OwnerLease {
    seed: Arc<AsyncState>,
    state: Arc<CompletionState>,
}
impl Drop for OwnerLease {
    fn drop(&mut self) {
        NapiHostBridge::release_owner_seed(&self.seed);
        let _status = unsafe {
            sys::napi_release_threadsafe_function(
                self.state.tsfn.load(Ordering::Acquire) as sys::napi_threadsafe_function,
                sys::ThreadsafeFunctionReleaseMode::release,
            )
        };
    }
}

fn owner(
    seed: Arc<AsyncState>,
    rx: &mpsc::Receiver<Command>,
    state: Arc<CompletionState>,
    options: AsyncSessionOptions,
) {
    let bridge = Rc::new(NapiHostBridge::from_owner_seed(seed));
    let mut vm = Interpreter::with_builtins();
    #[cfg(feature = "runtime")]
    if options.timers == Some(true) {
        crate::runtime::install_timers(&mut vm.global.borrow_mut());
    }
    vm.set_host_bridge(bridge.clone());
    bridge.set_owner_wake(state.wake.clone());
    let wake = state.wake.clone();
    vm.set_host_wake_notifier(Arc::new(move || wake.fire()));
    let virtual_clock = if options.clock.as_deref() == Some("virtual") {
        Some(VirtualClock::default())
    } else {
        None
    };
    let mode = match options.clock.as_deref() {
        Some("virtual") => ClockMode::Virtual(virtual_clock.as_ref().unwrap().clone()),
        Some("real-time") => ClockMode::RealTime(Rc::new(RealTimeClock::default())),
        _ => ClockMode::Legacy,
    };
    vm.jobs
        .borrow_mut()
        .set_clock(mode)
        .expect("fresh clock configuration");
    vm.set_event_loop_options(EventLoopOptions {
        fairness: if options.fairness.as_deref() == Some("alternate") {
            Fairness::Alternate
        } else {
            Fairness::ExternalFirst
        },
        ..EventLoopOptions::default()
    })
    .expect("valid defaults");
    let budget = TurnBudget::jobs(options.max_jobs_per_turn.unwrap_or(1024) as usize);
    let auto_poll = options.auto_poll.unwrap_or(true);
    let mut configured_timeout = None;
    let mut configured_fuel = None;
    let mut stopped_error = None;
    while !state.closed.load(Ordering::Acquire) {
        // An unfinished checkpoint precedes command admission, including new
        // evaluations. It keeps the current hard budget until completion.
        if auto_poll && vm.jobs.borrow().checkpoint_pending {
            match vm.poll_event_loop(budget) {
                Ok(o) if o.runnable => continue,
                Ok(_) => {}
                Err(e) => {
                    stopped_error = Some(e.to_string());
                }
            }
        }
        bridge.set_owner_execution_active(vm.has_active_execution());
        match rx.try_recv() {
            Ok(Command {
                admission,
                deferred,
                operation,
                cancellation: token,
            }) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> Result<WireValue, String> {
                        if token.is_cancelled() {
                            return Err("Error: Guest execution cancelled".into());
                        }
                        if let Some(error) = stopped_error.as_ref() {
                            return Err(error.clone());
                        }
                        match operation {
                            Operation::Diagnostics => {
                                Ok(WireValue::String(vm.evaluation_diagnostics().into()))
                            }
                            Operation::Collect => {
                                Ok(WireValue::Number(vm.collect_cycles().collected as f64))
                            }
                            Operation::Run(source, drain) => {
                                vm.ensure_can_evaluate().map_err(|e| e.to_string())?;
                                if !vm.has_active_execution() {
                                    *state
                                        .active_cancel
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner()) = token.clone();
                                    vm.set_cancellation_token(token.clone());
                                    bridge.set_owner_cancellation(token.clone());
                                    if let Some(fuel) = configured_fuel {
                                        vm.set_fuel_budget(fuel);
                                    }
                                    let timeout = configured_timeout
                                        .map(|ms: u32| std::time::Duration::from_millis(ms as u64));
                                    vm.set_execution_timeout(timeout);
                                    bridge.set_owner_timeout(timeout);
                                }
                                bridge.set_owner_execution_active(true);
                                let value = if drain {
                                    super::vm::execute_source_utf16(&mut vm, &source)
                                } else {
                                    vm.eval_utf16_with_options(
                                        &source,
                                        crate::interpreter::EvaluationOptions {
                                            drain: crate::interpreter::DrainPolicy::None,
                                            ..Default::default()
                                        },
                                    )
                                }
                                .map_err(|e| vm.enrich_error(e, None).to_string())?;
                                super::vm::async_result_string(value)
                                    .map(|v| WireValue::String(v.into()))
                            }
                            Operation::Expose { name, id, is_async } => {
                                bridge.mark_owner_function(id, is_async);
                                vm.global
                                    .borrow_mut()
                                    .set(&name, Value::host_function(name.clone(), id));
                                Ok(WireValue::Undefined)
                            }
                            Operation::GetGlobal(name) => Ok(WireValue::String(
                                vm.global
                                    .borrow()
                                    .get(&name)
                                    .map(|v| crate::format::try_to_string(&v))
                                    .transpose()
                                    .map_err(|e| e.to_string())?
                                    .unwrap_or_else(|| "undefined".into())
                                    .into(),
                            )),
                            Operation::SetGlobal(name, value) => {
                                vm.global.borrow_mut().set(&name, value.into_value());
                                Ok(WireValue::Undefined)
                            }
                            Operation::DefineModule(name, source) => {
                                vm.define_module(&name, source);
                                Ok(WireValue::Undefined)
                            }
                            Operation::AdvanceClock(ms) => {
                                virtual_clock
                                    .as_ref()
                                    .ok_or_else(|| "virtual clock is not enabled".to_string())?
                                    .advance(ms)
                                    .map_err(|e| e.to_string())?;
                                Ok(WireValue::Undefined)
                            }
                            Operation::Poll(jobs) => vm
                                .poll_event_loop(TurnBudget::jobs(jobs))
                                .map(turn_wire)
                                .map_err(|e| e.to_string()),
                            Operation::Limits { fuel, timeout_ms } => {
                                configured_fuel = Some(fuel);
                                configured_timeout = timeout_ms;
                                Ok(WireValue::Undefined)
                            }
                        }
                    },
                ))
                .unwrap_or_else(|_| {
                    state.closed.store(true, Ordering::Release);
                    Err("Error: async session owner panicked".into())
                });
                let result = if state.closed.load(Ordering::Acquire) {
                    Err("Error: async session disposed".into())
                } else {
                    result
                };
                vm.retire_completed_execution();
                vm.maybe_collect_cycles();
                bridge.set_owner_execution_active(vm.has_active_execution());
                bridge.prune_owner_results(&vm);
                drop(admission);
                complete(&state, deferred, result);
                if auto_poll {
                    match vm.poll_event_loop(budget) {
                        Ok(o) if o.runnable => continue,
                        Ok(_) => {}
                        Err(e) => {
                            stopped_error = Some(e.to_string());
                        }
                    }
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {
                if auto_poll && stopped_error.is_none() {
                    match vm.poll_event_loop(budget) {
                        Ok(o) if o.runnable => continue,
                        Ok(_) => {}
                        Err(e) => {
                            stopped_error = Some(e.to_string());
                        }
                    }
                }
                let timeout = if auto_poll && stopped_error.is_none() {
                    vm.jobs.borrow().timer_wait()
                } else {
                    None
                };
                bridge.set_owner_execution_active(vm.has_active_execution());
                let timeout = match (
                    timeout,
                    if auto_poll && stopped_error.is_none() {
                        vm.execution_wait_remaining()
                    } else {
                        None
                    },
                ) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                state.wake.wait(timeout);
            }
        }
    }
    for command in rx.try_iter() {
        complete(
            &state,
            command.deferred,
            Err("Error: async session disposed".into()),
        );
    }
    // Drop all guest values/stacks on the owner, then relinquish its handles.
    drop(vm);
    crate::heap::collect_after_interpreter_drop();
}

#[napi]
impl AsyncSession {
    #[napi(constructor)]
    pub fn new(env: Env, options: Option<AsyncSessionOptions>) -> napi::Result<Self> {
        let options = options.unwrap_or_default();
        #[cfg(not(feature = "runtime"))]
        if options.timers == Some(true) {
            return Err(napi::Error::from_reason(
                "timers require the runtime Cargo feature",
            ));
        }
        let capacity = options.command_capacity.unwrap_or(64) as usize;
        if !(1..=4096).contains(&capacity) || options.max_jobs_per_turn == Some(0) {
            return Err(napi::Error::from_reason(
                "command capacity must be 1..4096 and turn budget must be positive",
            ));
        }
        if options
            .clock
            .as_deref()
            .is_some_and(|v| !matches!(v, "legacy" | "virtual" | "real-time"))
            || options
                .fairness
                .as_deref()
                .is_some_and(|v| !matches!(v, "external-first" | "alternate"))
        {
            return Err(napi::Error::from_reason(
                "invalid session clock or fairness",
            ));
        }
        let raw_env = env.raw();
        let main_bridge = Rc::new(NapiHostBridge::new(raw_env));
        let seed = main_bridge
            .owner_seed()
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let name = make_str(raw_env, "vm-session-completions")
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let mut tsfn = ptr::null_mut();
        if let Err(e) = chk(unsafe {
            sys::napi_create_threadsafe_function(
                raw_env,
                ptr::null_mut(),
                ptr::null_mut(),
                name,
                capacity,
                2,
                ptr::null_mut(),
                None,
                ptr::null_mut(),
                Some(complete_on_node),
                &mut tsfn,
            )
        }) {
            main_bridge.finish_async_worker();
            main_bridge.shared_state().shutdown_on_main();
            return Err(napi::Error::from_reason(e.to_string()));
        }
        if let Err(error) = chk(unsafe { sys::napi_unref_threadsafe_function(raw_env, tsfn) }) {
            main_bridge.finish_async_worker();
            main_bridge.shared_state().shutdown_on_main();
            for _ in 0..2 {
                let _ = unsafe {
                    sys::napi_release_threadsafe_function(
                        tsfn,
                        sys::ThreadsafeFunctionReleaseMode::release,
                    )
                };
            }
            return Err(napi::Error::from_reason(error.to_string()));
        }
        let state = Arc::new(CompletionState {
            bridge: main_bridge.shared_state(),
            tsfn: AtomicUsize::new(tsfn as usize),
            pending: AtomicUsize::new(0),
            guest_admissions: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            main_released: AtomicBool::new(false),
            wake: Arc::new(WakeSignal::default()),
            active_cancel: Mutex::new(CancellationToken::default()),
            cancellations: Mutex::new(std::collections::HashMap::new()),
            capacity,
            env: raw_env as usize,
            cleanup: AtomicUsize::new(0),
            owner: Mutex::new(None),
        });
        let cleanup = Box::into_raw(Box::new(Cleanup {
            state: state.clone(),
            bridge: main_bridge.shared_state(),
        }));
        if let Err(error) = chk(unsafe {
            sys::napi_add_env_cleanup_hook(
                raw_env,
                Some(environment_cleanup),
                cleanup as *mut std::ffi::c_void,
            )
        }) {
            drop(unsafe { Box::from_raw(cleanup) });
            close_session(&state, &main_bridge.shared_state());
            main_bridge.finish_async_worker();
            let _ = unsafe {
                sys::napi_release_threadsafe_function(
                    tsfn,
                    sys::ThreadsafeFunctionReleaseMode::release,
                )
            };
            return Err(napi::Error::from_reason(error.to_string()));
        }
        state.cleanup.store(cleanup as usize, Ordering::Release);
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let owner_state = state.clone();
        let owner_thread = std::thread::Builder::new()
            .name("napi-vm-session".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let _lease = OwnerLease {
                    seed: seed.clone(),
                    state: owner_state.clone(),
                };
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    owner(seed, &receiver, owner_state.clone(), options)
                }))
                .is_err()
                {
                    owner_state.closed.store(true, Ordering::Release);
                    for command in receiver.try_iter() {
                        complete(
                            &owner_state,
                            command.deferred,
                            Err("Error: async session owner panicked".into()),
                        );
                    }
                }
            });
        match owner_thread {
            Ok(handle) => *state.owner.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle),
            Err(e) => {
                remove_cleanup(&state);
                main_bridge.finish_async_worker();
                main_bridge.shared_state().shutdown_on_main();
                for _ in 0..2 {
                    let _ = unsafe {
                        sys::napi_release_threadsafe_function(
                            tsfn,
                            sys::ThreadsafeFunctionReleaseMode::release,
                        )
                    };
                }
                return Err(napi::Error::from_reason(e.to_string()));
            }
        }
        Ok(Self {
            main_bridge,
            sender,
            state,
        })
    }
    #[napi(ts_return_type = "Promise<string>")]
    pub fn evaluation_stats(&self, env: Env) -> napi::Result<Unknown<'_>> {
        self.submit(env, Operation::Diagnostics)
    }
    #[napi(ts_return_type = "Promise<number>")]
    pub fn collect_cycles(&self, env: Env) -> napi::Result<Unknown<'_>> {
        self.submit(env, Operation::Collect)
    }
    #[napi(ts_return_type = "Promise<string>")]
    pub fn run(
        &self,
        env: Env,
        source: napi::bindgen_prelude::Utf16String,
    ) -> napi::Result<Unknown<'_>> {
        self.submit(
            env,
            Operation::Run(crate::JsString::from_units(source.to_vec()), true),
        )
    }
    #[napi(ts_return_type = "Promise<string>")]
    pub fn evaluate(
        &self,
        env: Env,
        source: napi::bindgen_prelude::Utf16String,
    ) -> napi::Result<Unknown<'_>> {
        self.submit(
            env,
            Operation::Run(crate::JsString::from_units(source.to_vec()), false),
        )
    }
    #[napi(ts_return_type = "Promise<void>")]
    pub fn expose_function(
        &self,
        env: Env,
        name: napi::bindgen_prelude::Utf16String,
        #[napi(ts_arg_type = "(...args: any[]) => any")] callback: Unknown,
        is_async: Option<bool>,
    ) -> napi::Result<Unknown<'_>> {
        let name = crate::JsString::from_units(name.to_vec()).to_key();
        let id = self
            .main_bridge
            .register(callback.raw())
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        match self.submit(
            env,
            Operation::Expose {
                name,
                id,
                is_async: is_async.unwrap_or(false),
            },
        ) {
            Ok(p) => Ok(p),
            Err(e) => {
                self.main_bridge.unregister(id);
                Err(e)
            }
        }
    }
    #[napi(ts_return_type = "Promise<string>")]
    pub fn get_global(
        &self,
        env: Env,
        name: napi::bindgen_prelude::Utf16String,
    ) -> napi::Result<Unknown<'_>> {
        let name = crate::JsString::from_units(name.to_vec()).to_key();
        self.submit(env, Operation::GetGlobal(name))
    }
    #[napi(ts_return_type = "Promise<void>")]
    pub fn set_global(
        &self,
        env: Env,
        name: napi::bindgen_prelude::Utf16String,
        value: Unknown,
    ) -> napi::Result<Unknown<'_>> {
        let name = crate::JsString::from_units(name.to_vec()).to_key();
        let value = WireValue::from_napi(env.raw(), value.raw())
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        self.submit(env, Operation::SetGlobal(name, value))
    }
    #[napi(ts_return_type = "Promise<void>")]
    pub fn define_module(
        &self,
        env: Env,
        name: napi::bindgen_prelude::Utf16String,
        source: napi::bindgen_prelude::Utf16String,
    ) -> napi::Result<Unknown<'_>> {
        let name = String::from_utf16(&name)
            .map_err(|_| napi::Error::from_reason("Module identifiers require valid Unicode"))?;
        let source = String::from_utf16(&source).map_err(|_| napi::Error::from_reason("Module source requires valid Unicode; use Unicode escapes for unpaired surrogate literals"))?;
        self.submit(env, Operation::DefineModule(name, source))
    }
    #[napi(ts_return_type = "Promise<void>")]
    pub fn advance_clock(&self, env: Env, milliseconds: f64) -> napi::Result<Unknown<'_>> {
        self.submit(env, Operation::AdvanceClock(milliseconds))
    }
    #[napi(
        ts_return_type = "Promise<{executedJobs: number, runnable: boolean, checkpointPending: boolean, yieldReason: string, nextDeadline: number | null}>"
    )]
    pub fn poll_event_loop(&self, env: Env, max_jobs: u32) -> napi::Result<Unknown<'_>> {
        self.submit(env, Operation::Poll(max_jobs as usize))
    }
    #[napi(ts_return_type = "Promise<void>")]
    pub fn set_execution_limits(
        &self,
        env: Env,
        fuel: u32,
        timeout_ms: Option<u32>,
    ) -> napi::Result<Unknown<'_>> {
        self.submit(
            env,
            Operation::Limits {
                fuel: fuel as u64,
                timeout_ms,
            },
        )
    }
    #[napi]
    pub fn cancel(&self) {
        self.state
            .active_cancel
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cancel();
        for token in self
            .state
            .cancellations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            token.cancel();
        }
        self.main_bridge.shared_state().cancel_pending_on_main();
        self.main_bridge.shared_state().wake_owner();
        self.state.wake.fire();
    }
    #[napi]
    pub fn dispose(&self) {
        close_session(&self.state, &self.main_bridge.shared_state());
        if let Some(owner) = self
            .state
            .owner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let _ = owner.join();
        }
        remove_cleanup(&self.state);
    }
    #[napi]
    pub fn wakeups(&self) -> f64 {
        self.state.wake.wakeups() as f64
    }
}
impl AsyncSession {
    fn release_slot(&self, env: sys::napi_env) {
        if self.state.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
            let _status = unsafe {
                sys::napi_unref_threadsafe_function(
                    env,
                    self.state.tsfn.load(Ordering::Acquire) as sys::napi_threadsafe_function,
                )
            };
        }
    }
    fn submit(&self, env: Env, operation: Operation) -> napi::Result<Unknown<'_>> {
        self.main_bridge.shared_state().prune_abandoned_on_main();
        if self
            .main_bridge
            .owner_waiting_for_node(self.state.guest_admissions.load(Ordering::Acquire) > 0)
        {
            return Err(napi::Error::from_reason(
                "async session is awaiting Node (active host-call dependency); reentrant commands must wait for the active execution to complete",
            ));
        }
        if self.state.closed.load(Ordering::Acquire) {
            return Err(napi::Error::from_reason("async session is disposed"));
        }
        let previous = self
            .state
            .pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                if n < self.state.capacity {
                    Some(n + 1)
                } else {
                    None
                }
            })
            .map_err(|_| {
                napi::Error::from_reason(
                    "async session command queue is full; await a completion before retrying",
                )
            })?;
        let admission = if matches!(&operation, Operation::Run(..)) {
            self.state.guest_admissions.fetch_add(1, Ordering::AcqRel);
            Some(GuestAdmission(self.state.clone()))
        } else {
            None
        };
        let raw_env = env.raw();
        if previous == 0
            && let Err(error) = chk(unsafe {
                sys::napi_ref_threadsafe_function(
                    raw_env,
                    self.state.tsfn.load(Ordering::Acquire) as sys::napi_threadsafe_function,
                )
            })
        {
            self.release_slot(raw_env);
            return Err(napi::Error::from_reason(error.to_string()));
        }
        let mut deferred = ptr::null_mut();
        let mut promise = ptr::null_mut();
        if let Err(error) =
            chk(unsafe { sys::napi_create_promise(raw_env, &mut deferred, &mut promise) })
        {
            self.release_slot(raw_env);
            return Err(napi::Error::from_reason(error.to_string()));
        }
        let cancellation = CancellationToken::default();
        self.state
            .cancellations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(deferred as usize, cancellation.clone());
        if let Err(error) = self.sender.try_send(Command {
            admission,
            deferred: deferred as usize,
            operation,
            cancellation,
        }) {
            self.state
                .cancellations
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&(deferred as usize));
            // Promise already exists: reject it on Node instead of abandoning it.
            super::vm::reject_deferred_now(raw_env, deferred, error.to_string());
            self.release_slot(raw_env);
        } else {
            self.state.wake.fire();
        }
        Ok(unsafe { Unknown::from_raw_unchecked(raw_env, promise) })
    }
}
impl Drop for AsyncSession {
    fn drop(&mut self) {
        self.dispose();
    }
}
