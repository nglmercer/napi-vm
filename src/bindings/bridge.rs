//! Main-thread host dispatch for the N-API VM.
//!
//! The interpreter is single-threaded. `runAsync` therefore holds it behind
//! the VM runtime gate and the worker communicates with Node only through a
//! ThreadsafeFunction. No `Value`, `Interpreter`, `napi_value`, or `napi_ref`
//! is sent to that worker: values use the owned `WireValue` representation and
//! N-API handles stay in the main-thread bridge state.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::c_char;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use napi::sys;

use crate::Interpreter;
use crate::error::VmErr;
use crate::format::to_string;
use crate::host::HostBridge;
use crate::value::Value;

use super::marshal::{WireValue, chk, from_napi, get_named_str, make_str, to_napi};

/// Keep the host queue and the number of fire-and-forget guest promises
/// bounded. A guest can still perform legitimate sequential async work, but
/// cannot turn an exposed async function into an unbounded native queue.
const MAX_HOST_QUEUE: usize = 1024;
const MAX_PENDING_HOST_CALLS: usize = 1024;

/// Main-thread-owned N-API references shared with worker messages.
///
/// The fields are intentionally all thread-safe Rust data. The worker only
/// reads copied integer handles and updates counters; every N-API call
/// involving those handles is made by a TSFN callback on Node's main thread.
pub(super) struct BridgeState {
    env: usize,
    tsfn: AtomicUsize,
    shutting_down: AtomicBool,
    released: AtomicBool,
    wake: Arc<crate::host::WakeSlot>,
    refs: Mutex<HashMap<usize, RefEntry>>,
    promises: Mutex<HashMap<usize, (usize, std::sync::Weak<Settlement>)>>,
    next_promise: AtomicUsize,
}

struct RefEntry {
    func_ref: usize,
    in_flight: usize,
    retired: bool,
}

impl BridgeState {
    fn new(env: sys::napi_env) -> Self {
        Self {
            env: env as usize,
            tsfn: AtomicUsize::new(0),
            shutting_down: AtomicBool::new(false),
            released: AtomicBool::new(false),
            wake: Arc::new(crate::host::WakeSlot::new()),
            refs: Mutex::new(HashMap::new()),
            promises: Mutex::new(HashMap::new()),
            next_promise: AtomicUsize::new(0),
        }
    }

    fn release_promise_on_main(&self, id: usize) {
        let entry = self
            .promises
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id);
        if let Some((reference, _)) = entry {
            self.delete_reference(reference);
        }
    }
    pub(super) fn prune_abandoned_on_main(&self) {
        let abandoned: Vec<_> = self
            .promises
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(|(&id, (_, weak))| {
                weak.upgrade()
                    .filter(|s| !s.result_alive.load(Ordering::Acquire))
                    .map(|s| (id, s))
            })
            .collect();
        for (id, settlement) in abandoned {
            settlement.reject("async host result abandoned".into());
            self.release_promise_on_main(id);
        }
    }
    pub(super) fn cancel_pending_on_main(&self) {
        let entries: Vec<_> = self
            .promises
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, v)| v)
            .collect();
        for (reference, settlement) in entries {
            if let Some(settlement) = settlement.upgrade() {
                settlement.reject("async host call cancelled or disposed".into());
            }
            self.delete_reference(reference);
        }
        self.wake.fire();
    }

    pub(super) fn wake_owner(&self) {
        self.wake.fire();
    }

    fn lock_refs(&self) -> std::sync::MutexGuard<'_, HashMap<usize, RefEntry>> {
        self.refs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn insert_ref(&self, id: usize, func_ref: sys::napi_ref) {
        self.lock_refs().insert(
            id,
            RefEntry {
                func_ref: func_ref as usize,
                in_flight: 0,
                retired: false,
            },
        );
    }

    fn begin_call(&self, id: usize) -> Result<usize, VmErr> {
        let mut refs = self.lock_refs();
        let entry = refs
            .get_mut(&id)
            .ok_or_else(|| VmErr::Msg(format!("host function #{} is not registered", id)))?;
        entry.in_flight = entry.in_flight.saturating_add(1);
        Ok(entry.func_ref)
    }

    /// Drop a call that could not be queued. This runs on the worker, so it
    /// only changes Rust state; deferred N-API reference deletion is performed
    /// by a main-thread cleanup path.
    fn abort_call(&self, id: usize) {
        let mut refs = self.lock_refs();
        if let Some(entry) = refs.get_mut(&id) {
            entry.in_flight = entry.in_flight.saturating_sub(1);
        }
    }

    /// Finish a call on the Node thread. If the binding was removed while a
    /// call was queued, delete the persistent reference only now, after the
    /// last callback has stopped using it.
    fn finish_call_on_main(&self, id: usize) {
        let maybe_ref = {
            let mut refs = self.lock_refs();
            let Some(entry) = refs.get_mut(&id) else {
                return;
            };
            entry.in_flight = entry.in_flight.saturating_sub(1);
            if entry.retired && entry.in_flight == 0 {
                refs.remove(&id).map(|entry| entry.func_ref)
            } else {
                None
            }
        };
        if let Some(func_ref) = maybe_ref {
            self.delete_reference(func_ref);
        }
    }

    /// Finish a queued callback after Node has started tearing down the
    /// environment. N-API references must not be deleted through a null
    /// environment; the environment owns their final cleanup.
    fn finish_call_on_teardown(&self, id: usize) {
        let mut refs = self.lock_refs();
        let Some(entry) = refs.get_mut(&id) else {
            return;
        };
        entry.in_flight = entry.in_flight.saturating_sub(1);
        if entry.retired && entry.in_flight == 0 {
            refs.remove(&id);
        }
    }

    fn retire(&self, id: usize) {
        let maybe_ref = {
            let mut refs = self.lock_refs();
            let Some(entry) = refs.get_mut(&id) else {
                return;
            };
            entry.retired = true;
            if entry.in_flight == 0 {
                refs.remove(&id).map(|entry| entry.func_ref)
            } else {
                None
            }
        };
        if let Some(func_ref) = maybe_ref {
            self.delete_reference(func_ref);
        }
    }

    fn delete_reference(&self, func_ref: usize) {
        // Called only by main-thread API/finalization paths. There is no useful
        // error channel from Drop, but the status is explicitly checked.
        let status = unsafe {
            sys::napi_delete_reference(self.env as sys::napi_env, func_ref as sys::napi_ref)
        };
        if status != sys::Status::napi_ok {
            // The environment may already be tearing down. The reference is
            // then owned by Node's environment cleanup and must not be reused.
            self.shutting_down.store(true, Ordering::Release);
        }
    }

    fn tsfn_handle(&self) -> Result<sys::napi_threadsafe_function, VmErr> {
        let handle = self.tsfn.load(Ordering::Acquire);
        if handle == 0 {
            Err(VmErr::Msg(
                "async host dispatcher is not initialized".into(),
            ))
        } else {
            Ok(handle as sys::napi_threadsafe_function)
        }
    }

    fn ensure_tsfn(&self) -> Result<sys::napi_threadsafe_function, VmErr> {
        self.tsfn_handle()
    }

    fn set_tsfn(&self, tsfn: sys::napi_threadsafe_function) {
        self.tsfn.store(tsfn as usize, Ordering::Release);
    }

    fn acquire_worker(&self) -> Result<(), VmErr> {
        let tsfn = self.ensure_tsfn()?;
        chk(unsafe { sys::napi_acquire_threadsafe_function(tsfn) })
    }

    fn release_worker(&self) {
        if let Ok(tsfn) = self.tsfn_handle() {
            let status = unsafe {
                sys::napi_release_threadsafe_function(
                    tsfn,
                    sys::ThreadsafeFunctionReleaseMode::release,
                )
            };
            if status != sys::Status::napi_ok {
                self.shutting_down.store(true, Ordering::Release);
            }
        }
    }

    /// Release the bridge's initial TSFN acquisition and retire all references
    /// that are not currently in flight. Called by `VM::drop` on Node's main
    /// thread; it does not wait for the interpreter worker.
    pub(super) fn shutdown_on_main(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        self.shutting_down.store(true, Ordering::Release);
        self.cancel_pending_on_main();
        self.wake.fire();

        let immediate = {
            let mut refs = self.lock_refs();
            let ids: Vec<usize> = refs.keys().copied().collect();
            let mut immediate = Vec::new();
            for id in ids {
                if let Some(entry) = refs.get_mut(&id) {
                    entry.retired = true;
                    if entry.in_flight == 0
                        && let Some(entry) = refs.remove(&id)
                    {
                        immediate.push(entry.func_ref);
                    }
                }
            }
            immediate
        };
        for func_ref in immediate {
            self.delete_reference(func_ref);
        }

        if let Ok(tsfn) = self.tsfn_handle() {
            let status = unsafe {
                sys::napi_release_threadsafe_function(
                    tsfn,
                    sys::ThreadsafeFunctionReleaseMode::release,
                )
            };
            if status != sys::Status::napi_ok {
                self.shutting_down.store(true, Ordering::Release);
            }
        }
    }
}

/// Session-lifetime Node dependencies; execution retirement only changes activity.
struct NodeDependencies {
    active: AtomicBool,
    calls: AtomicUsize,
}
struct DependencyLease(Arc<NodeDependencies>);
impl Drop for DependencyLease {
    fn drop(&mut self) {
        self.0.calls.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Message sent from the VM worker to the Node main thread. It contains only
/// owned wire values and integer N-API handles; no `Rc<RefCell<Value>>` crosses
/// the thread boundary.
struct AsyncCallMsg {
    dependency: DependencyLease,
    state: Arc<BridgeState>,
    func_id: usize,
    func_ref: usize,
    args: Vec<WireValue>,
    reply_tx: mpsc::Sender<Result<WireValue, crate::JsString>>,
    cancellation: crate::CancellationToken,
    result_alive: Arc<AtomicBool>,
    reply_signal: Arc<crate::host::WakeSignal>,
}

/// Shared async state used by the interpreter while it waits for host calls.
pub(super) struct AsyncState {
    state: Arc<BridgeState>,
    next_pending: AtomicUsize,
    waiting_for_node: AtomicUsize,
    dependencies: Arc<NodeDependencies>,
    pending: Mutex<HashMap<usize, PendingResult>>,
    cancellation: Mutex<crate::CancellationToken>,
    deadline: Mutex<Option<std::time::Instant>>,
    reply_wake: Mutex<Option<Arc<crate::host::WakeSignal>>>,
}

struct PendingResult {
    receiver: Option<mpsc::Receiver<Result<WireValue, crate::JsString>>>,
    alive: Arc<AtomicBool>,
}
impl Drop for PendingResult {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

struct NodeWaitGuard<'a>(&'a AsyncState);
impl Drop for NodeWaitGuard<'_> {
    fn drop(&mut self) {
        self.0.waiting_for_node.fetch_sub(1, Ordering::AcqRel);
    }
}

impl AsyncState {
    fn wait_guard(&self) -> NodeWaitGuard<'_> {
        self.waiting_for_node.fetch_add(1, Ordering::AcqRel);
        NodeWaitGuard(self)
    }
    fn pending_len(&self) -> usize {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

/// Bridge stored inside the single-threaded interpreter. Its N-API maps are
/// only accessed while the VM runtime gate is held; the cross-thread part is
/// the separate, fully owned `BridgeState` above.
pub(super) struct NapiHostBridge {
    env: sys::napi_env,
    state: Arc<BridgeState>,
    funcs: RefCell<HashMap<usize, sys::napi_ref>>,
    async_funcs: RefCell<HashSet<usize>>,
    next_id: Cell<usize>,
    async_state: Mutex<Option<Arc<AsyncState>>>,
    /// Nonzero only while a `runAsync` worker owns the interpreter lock.
    pub(super) on_vm_thread: AtomicUsize,
}

impl NapiHostBridge {
    pub(super) fn new(env: sys::napi_env) -> Self {
        Self {
            env,
            state: Arc::new(BridgeState::new(env)),
            funcs: RefCell::new(HashMap::new()),
            async_funcs: RefCell::new(HashSet::new()),
            next_id: Cell::new(0),
            async_state: Mutex::new(None),
            on_vm_thread: AtomicUsize::new(0),
        }
    }

    pub(super) fn shared_state(&self) -> Arc<BridgeState> {
        self.state.clone()
    }

    pub(super) fn register(&self, func: sys::napi_value) -> Result<usize, VmErr> {
        let mut reference: sys::napi_ref = ptr::null_mut();
        chk(unsafe { sys::napi_create_reference(self.env, func, 1, &mut reference) })?;
        let id = self.next_id.get();
        self.next_id.set(id.saturating_add(1));
        self.state.insert_ref(id, reference);
        self.funcs.borrow_mut().insert(id, reference);
        Ok(id)
    }

    /// Register a host function the guest may `await`.
    ///
    /// The dispatcher threadsafe-function is deliberately *not* created here.
    /// Every consumer of it runs on the `runAsync` worker, and `run_async`
    /// calls `prepare_for_async` before spawning that worker, so creating it
    /// eagerly buys nothing -- and costs a great deal: a live TSFN keeps the
    /// Node environment alive, so merely calling `exposeAsyncFunction` would
    /// stop the host process from ever exiting on its own.
    pub(super) fn register_async(&self, func: sys::napi_value) -> Result<usize, VmErr> {
        let id = self.register(func)?;
        self.async_funcs.borrow_mut().insert(id);
        Ok(id)
    }

    pub(super) fn unregister(&self, id: usize) {
        self.funcs.borrow_mut().remove(&id);
        self.async_funcs.borrow_mut().remove(&id);
        self.state.retire(id);
    }

    fn ensure_dispatcher(&self) -> Result<Arc<AsyncState>, VmErr> {
        let mut guard = self
            .async_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(state) = guard.as_ref() {
            return Ok(state.clone());
        }

        let tsfn = self.create_tsfn()?;
        self.state.set_tsfn(tsfn);
        let state = Arc::new(AsyncState {
            state: self.state.clone(),
            next_pending: AtomicUsize::new(0),
            waiting_for_node: AtomicUsize::new(0),
            dependencies: Arc::new(NodeDependencies {
                active: AtomicBool::new(false),
                calls: AtomicUsize::new(0),
            }),
            pending: Mutex::new(HashMap::new()),
            cancellation: Mutex::new(crate::CancellationToken::default()),
            deadline: Mutex::new(None),
            reply_wake: Mutex::new(Some(Arc::new(crate::host::WakeSignal::default()))),
        });
        *guard = Some(state.clone());
        Ok(state)
    }

    pub(super) fn prepare_for_async(&self) -> Result<(), VmErr> {
        let state = self.ensure_dispatcher()?;
        state.state.acquire_worker()
    }

    /// Prepare Node-owned handles once, then construct a separate bridge on
    /// the persistent owner. No Rc, guest value, or coroutine crosses threads.
    pub(super) fn prune_owner_results(&self, vm: &Interpreter) {
        if let Some(state) = self.get_async_state() {
            if state.pending_len() == 0 {
                return;
            }
            if let Some(live) = crate::heap::reachable_host_calls(vm.gc_roots()) {
                state
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .retain(|id, _| live.contains(id));
            }
        }
    }
    pub(super) fn owner_waiting_for_node(&self, guest_admitted: bool) -> bool {
        self.get_async_state().is_some_and(|s| {
            let execution = &s.dependencies;
            s.waiting_for_node.load(Ordering::Acquire) > 0
                || ((guest_admitted || execution.active.load(Ordering::Acquire))
                    && execution.calls.load(Ordering::Acquire) > 0)
        })
    }
    pub(super) fn set_owner_execution_active(&self, active: bool) {
        if let Some(state) = self.get_async_state() {
            // Unsettled calls can be awaited by later executions. Retirement
            // changes activity, not the identity/lifetime of their leases.
            state.dependencies.active.store(active, Ordering::Release);
        }
    }
    pub(super) fn owner_seed(&self) -> Result<Arc<AsyncState>, VmErr> {
        self.prepare_for_async()?;
        self.get_async_state()
            .ok_or_else(|| VmErr::Msg("owner dispatcher is unavailable".into()))
    }
    pub(super) fn release_owner_seed(seed: &AsyncState) {
        seed.state.release_worker();
    }

    pub(super) fn from_owner_seed(seed: Arc<AsyncState>) -> Self {
        Self {
            env: seed.state.env as sys::napi_env,
            state: seed.state.clone(),
            funcs: RefCell::new(HashMap::new()),
            async_funcs: RefCell::new(HashSet::new()),
            next_id: Cell::new(0),
            async_state: Mutex::new(Some(seed)),
            on_vm_thread: AtomicUsize::new(1),
        }
    }
    pub(super) fn mark_owner_function(&self, id: usize, is_async: bool) {
        if is_async {
            self.async_funcs.borrow_mut().insert(id);
        } else {
            self.async_funcs.borrow_mut().remove(&id);
        }
    }

    pub(super) fn finish_async_worker(&self) {
        self.on_vm_thread.store(0, Ordering::Release);
        if let Some(state) = self
            .async_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
        {
            state.state.release_worker();
        }
    }

    fn create_tsfn(&self) -> Result<sys::napi_threadsafe_function, VmErr> {
        let mut tsfn: sys::napi_threadsafe_function = ptr::null_mut();
        let name = make_str(self.env, "vm-host-dispatch")?;
        chk(unsafe {
            sys::napi_create_threadsafe_function(
                self.env,
                ptr::null_mut(),
                ptr::null_mut(),
                name,
                MAX_HOST_QUEUE,
                1,
                ptr::null_mut(),
                None,
                ptr::null_mut(),
                Some(tsfn_callback),
                &mut tsfn,
            )
        })?;
        // Unref so the dispatcher does not hold the *event loop* open. This is
        // necessary but not sufficient: the TSFN also holds a reference on the
        // N-API environment, which keeps the process alive until the handle is
        // released outright. That release happens in `shutdown_on_main`, via
        // `Vm.dispose()` or when the `Vm` is dropped.
        chk(unsafe { sys::napi_unref_threadsafe_function(self.env, tsfn) })?;
        Ok(tsfn)
    }

    fn get_async_state(&self) -> Option<Arc<AsyncState>> {
        self.async_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn set_owner_cancellation(&self, token: crate::CancellationToken) {
        if let Some(state) = self.get_async_state() {
            *state.cancellation.lock().unwrap_or_else(|e| e.into_inner()) = token;
        }
    }
    pub(super) fn set_owner_timeout(&self, timeout: Option<std::time::Duration>) {
        if let Some(state) = self.get_async_state() {
            *state.deadline.lock().unwrap_or_else(|e| e.into_inner()) =
                timeout.and_then(|d| std::time::Instant::now().checked_add(d));
        }
    }
    pub(super) fn set_owner_wake(&self, signal: Arc<crate::host::WakeSignal>) {
        if let Some(state) = self.get_async_state() {
            *state.reply_wake.lock().unwrap_or_else(|e| e.into_inner()) = Some(signal.clone());
        }
        self.state.wake.set(Arc::new(move || signal.fire()));
    }
    fn receive_reply(
        state: &AsyncState,
        receiver: mpsc::Receiver<Result<WireValue, crate::JsString>>,
    ) -> Result<Value, VmErr> {
        loop {
            if state.state.shutting_down.load(Ordering::Acquire)
                || state
                    .cancellation
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_cancelled()
            {
                return Err(VmErr::Msg(
                    "Error: async host call cancelled or disposed".into(),
                ));
            }
            let deadline = *state.deadline.lock().unwrap_or_else(|e| e.into_inner());
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                return Err(VmErr::Msg(
                    "RangeError: Guest execution deadline exceeded".into(),
                ));
            }
            let signal = state
                .reply_wake
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let signal = signal.expect("dispatcher reply signal");
            state
                .cancellation
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .register_wake(&signal);
            let reply = match receiver.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {
                    signal.wait(
                        deadline.map(|d| d.saturating_duration_since(std::time::Instant::now())),
                    );
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(VmErr::Msg("async host call channel closed".into()));
                }
                Ok(reply) => reply,
            };
            return reply.map(|v| v.into_value()).map_err(|message| {
                VmErr::Throw(Value::Error(crate::value::ErrorData::new("Error", message)))
            });
        }
    }

    fn wire_args(args: &[Value]) -> Result<Vec<WireValue>, VmErr> {
        args.iter().map(WireValue::from_value).collect()
    }

    fn dispatch(
        &self,
        state: Arc<AsyncState>,
        id: usize,
        args: Vec<WireValue>,
        reply_tx: mpsc::Sender<Result<WireValue, crate::JsString>>,
        result_alive: Arc<AtomicBool>,
    ) -> Result<(), VmErr> {
        if state.state.shutting_down.load(Ordering::Acquire) {
            return Err(VmErr::Msg("async bridge is shutting down".into()));
        }
        let func_ref = state.state.begin_call(id)?;
        let dependency = {
            let execution = state.dependencies.clone();
            execution.calls.fetch_add(1, Ordering::AcqRel);
            DependencyLease(execution)
        };
        let msg = Box::new(AsyncCallMsg {
            dependency,
            state: state.state.clone(),
            func_id: id,
            func_ref,
            args,
            reply_tx,
            reply_signal: state
                .reply_wake
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .expect("reply signal"),
            result_alive,
            cancellation: state
                .cancellation
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        });
        let raw = Box::into_raw(msg) as *mut std::ffi::c_void;
        let tsfn = state.state.ensure_tsfn()?;
        let status = unsafe {
            sys::napi_call_threadsafe_function(
                tsfn,
                raw,
                sys::ThreadsafeFunctionCallMode::nonblocking,
            )
        };
        if status != sys::Status::napi_ok {
            // The callback did not take ownership when N-API rejected the
            // message, so reclaim the allocation and the reference lease.
            drop(unsafe { Box::from_raw(raw as *mut AsyncCallMsg) });
            state.state.abort_call(id);
            return Err(VmErr::Msg(format!(
                "failed to dispatch host call (status {})",
                status
            )));
        }
        Ok(())
    }

    fn call_via_tsfn(
        &self,
        state: Arc<AsyncState>,
        id: usize,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        let _waiting = state.wait_guard();
        let args = Self::wire_args(&args)?;
        let (reply_tx, reply_rx) = mpsc::channel::<Result<WireValue, crate::JsString>>();
        self.dispatch(
            state.clone(),
            id,
            args,
            reply_tx,
            Arc::new(AtomicBool::new(true)),
        )?;
        Self::receive_reply(&state, reply_rx)
    }
}

impl HostBridge for NapiHostBridge {
    fn event_wait_mode(&self) -> crate::host::HostWaitMode {
        crate::host::HostWaitMode::Notifications
    }

    fn set_wake_notifier(&self, notifier: crate::host::WakeNotifier) {
        self.state.wake.set(notifier);
    }

    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        if self.on_vm_thread.load(Ordering::Acquire) != 0 {
            let state = self
                .get_async_state()
                .ok_or_else(|| VmErr::Msg("host dispatcher unavailable on the VM worker".into()))?;
            return self.call_via_tsfn(state, id, args);
        }

        // Synchronous `run()` executes on Node's main thread, where direct
        // N-API calls are valid and preserve the existing synchronous API.
        let func_ref = *self
            .funcs
            .borrow()
            .get(&id)
            .ok_or_else(|| VmErr::Msg(format!("host function #{} is not registered", id)))?;
        let mut func = ptr::null_mut();
        chk(unsafe { sys::napi_get_reference_value(self.env, func_ref, &mut func) })?;

        let mut argv = Vec::with_capacity(args.len());
        for arg in &args {
            argv.push(to_napi(self.env, arg)?);
        }
        let mut receiver = ptr::null_mut();
        chk(unsafe { sys::napi_get_global(self.env, &mut receiver) })?;
        let mut result = ptr::null_mut();
        let status = unsafe {
            sys::napi_call_function(
                self.env,
                receiver,
                func,
                argv.len(),
                argv.as_ptr(),
                &mut result,
            )
        };
        if status != sys::Status::napi_ok {
            let mut exception = ptr::null_mut();
            let exception_status =
                unsafe { sys::napi_get_and_clear_last_exception(self.env, &mut exception) };
            if exception_status == sys::Status::napi_ok && !exception.is_null() {
                return Err(VmErr::Throw(from_napi(self.env, exception)?));
            }
            return Err(VmErr::Msg(format!(
                "host function call failed (status {})",
                status
            )));
        }
        from_napi(self.env, result)
    }

    fn construct_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        if self.on_vm_thread.load(Ordering::Acquire) != 0 {
            return Err(VmErr::Msg(
                "host constructors cannot run on the async VM worker".to_string(),
            ));
        }
        let func_ref = *self
            .funcs
            .borrow()
            .get(&id)
            .ok_or_else(|| VmErr::Msg(format!("host function #{} is not registered", id)))?;
        let mut constructor = ptr::null_mut();
        chk(unsafe { sys::napi_get_reference_value(self.env, func_ref, &mut constructor) })?;
        let mut argv = Vec::with_capacity(args.len());
        for arg in &args {
            argv.push(to_napi(self.env, arg)?);
        }
        let mut result = ptr::null_mut();
        let status = unsafe {
            sys::napi_new_instance(
                self.env,
                constructor,
                argv.len(),
                argv.as_ptr(),
                &mut result,
            )
        };
        if status != sys::Status::napi_ok {
            let mut exception = ptr::null_mut();
            let exception_status =
                unsafe { sys::napi_get_and_clear_last_exception(self.env, &mut exception) };
            if exception_status == sys::Status::napi_ok && !exception.is_null() {
                return Err(VmErr::Throw(from_napi(self.env, exception)?));
            }
            return Err(VmErr::Msg(format!(
                "host constructor failed (status {})",
                status
            )));
        }
        from_napi(self.env, result)
    }

    fn is_async_fn(&self, id: usize) -> bool {
        self.async_funcs.borrow().contains(&id)
    }

    fn call_host_async(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        let state = self
            .get_async_state()
            .ok_or_else(|| VmErr::Msg("async bridge not initialized".to_string()))?;
        if state.pending_len() >= MAX_PENDING_HOST_CALLS {
            return Err(VmErr::Msg(
                "RangeError: Too many pending host calls".to_string(),
            ));
        }

        let args = Self::wire_args(&args)?;
        let (reply_tx, reply_rx) = mpsc::channel::<Result<WireValue, crate::JsString>>();
        let alive = Arc::new(AtomicBool::new(true));
        let pending_id = state.next_pending.fetch_add(1, Ordering::Relaxed);
        state
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                pending_id,
                PendingResult {
                    receiver: Some(reply_rx),
                    alive: alive.clone(),
                },
            );

        if let Err(error) = self.dispatch(state.clone(), id, args, reply_tx, alive) {
            state
                .pending
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&pending_id);
            return Err(error);
        }
        Ok(Value::HostPending { id: pending_id })
    }

    fn await_host(&self, pending_id: usize) -> Result<Value, VmErr> {
        let state = self
            .get_async_state()
            .ok_or_else(|| VmErr::Msg("async bridge not initialized".to_string()))?;
        let _waiting = state.wait_guard();
        let mut pending = state
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&pending_id)
            .ok_or_else(|| VmErr::Msg(format!("no pending async call #{}", pending_id)))?;
        Self::receive_reply(&state, pending.receiver.take().expect("owned receiver"))
    }
}

/// A main-thread lease that retires a persisted reference exactly once on all
/// callback exits, including every N-API error path. During environment
/// teardown Node can invoke a TSFN callback with a null env; that path may
/// release Rust bookkeeping but must not call N-API.
struct MainCallLease {
    state: Arc<BridgeState>,
    id: usize,
    env_available: bool,
    reply_signal: Arc<crate::host::WakeSignal>,
}

impl Drop for MainCallLease {
    fn drop(&mut self) {
        if self.env_available {
            self.state.finish_call_on_main(self.id);
            self.state.prune_abandoned_on_main();
        } else {
            self.state.finish_call_on_teardown(self.id);
        }
        self.state.wake.fire();
        self.reply_signal.fire();
    }
}

/// TSFN callback — runs on Node's main thread. Node may pass a null env
/// while tearing down the environment, in which case queued data is freed
/// and the worker is released without making any N-API calls.
extern "C" fn tsfn_callback(
    env: sys::napi_env,
    _js_callback: sys::napi_value,
    _context: *mut std::ffi::c_void,
    data: *mut std::ffi::c_void,
) {
    if data.is_null() {
        return;
    }
    let msg = unsafe { Box::from_raw(data as *mut AsyncCallMsg) };
    let _lease = MainCallLease {
        state: msg.state.clone(),
        id: msg.func_id,
        env_available: !env.is_null(),
        reply_signal: msg.reply_signal.clone(),
    };
    if env.is_null() {
        let _ = msg
            .reply_tx
            .send(Err("Node environment is tearing down".into()));
        return;
    }

    if msg.state.shutting_down.load(Ordering::Acquire) || msg.cancellation.is_cancelled() {
        let _ = msg
            .reply_tx
            .send(Err("async host call cancelled or disposed".into()));
        return;
    }

    let func_ref = msg.func_ref as sys::napi_ref;
    let mut func = ptr::null_mut();
    let status = unsafe { sys::napi_get_reference_value(env, func_ref, &mut func) };
    if status != sys::Status::napi_ok || func.is_null() {
        let _ = msg
            .reply_tx
            .send(Err("failed to resolve host function reference".into()));
        return;
    }

    let mut argv = Vec::with_capacity(msg.args.len());
    for wire in msg.args {
        match to_napi(env, &wire.into_value()) {
            Ok(value) => argv.push(value),
            Err(error) => {
                let _ = msg
                    .reply_tx
                    .send(Err(format!("marshal error: {}", error).into()));
                return;
            }
        }
    }

    let mut receiver = ptr::null_mut();
    let receiver_status = unsafe { sys::napi_get_global(env, &mut receiver) };
    if receiver_status != sys::Status::napi_ok {
        let _ = msg.reply_tx.send(Err(format!(
            "failed to get global receiver (status {})",
            receiver_status
        )
        .into()));
        return;
    }

    let mut result = ptr::null_mut();
    let call_status = unsafe {
        sys::napi_call_function(env, receiver, func, argv.len(), argv.as_ptr(), &mut result)
    };
    if call_status != sys::Status::napi_ok {
        let mut exception = ptr::null_mut();
        let exception_status =
            unsafe { sys::napi_get_and_clear_last_exception(env, &mut exception) };
        let message = if exception_status == sys::Status::napi_ok && !exception.is_null() {
            match from_napi(env, exception) {
                Ok(value) => match &value {
                    Value::String(value) => value.clone(),
                    Value::Error(error) => error
                        .name
                        .concat(&crate::JsString::from(": "))
                        .concat(&error.message),
                    _ => to_string(&value).into(),
                },
                Err(error) => error.to_string().into(),
            }
        } else {
            format!("host function call failed (status {})", call_status).into()
        };
        let _ = msg.reply_tx.send(Err(message));
        return;
    }

    // A synchronous exposed function returns immediately. An async exposed
    // function is allowed to return any thenable, so settlement is guarded by
    // shared once-only state rather than two one-shot Box pointers.
    let mut then_key = ptr::null_mut();
    let key_status =
        unsafe { sys::napi_create_string_utf8(env, c"then".as_ptr(), 4, &mut then_key) };
    if key_status != sys::Status::napi_ok {
        let _ = msg.reply_tx.send(Err(format!(
            "failed to create then key (status {})",
            key_status
        )
        .into()));
        return;
    }
    let mut then_value = ptr::null_mut();
    let get_then_status = unsafe { sys::napi_get_property(env, result, then_key, &mut then_value) };
    if get_then_status != sys::Status::napi_ok {
        let _ = msg.reply_tx.send(Err(format!(
            "failed to inspect thenable (status {})",
            get_then_status
        )
        .into()));
        return;
    }
    let mut then_type: sys::napi_valuetype = 0;
    let typeof_status = unsafe { sys::napi_typeof(env, then_value, &mut then_type) };
    if typeof_status != sys::Status::napi_ok {
        let _ = msg.reply_tx.send(Err(format!(
            "failed to inspect thenable type (status {})",
            typeof_status
        )
        .into()));
        return;
    }

    if then_type != sys::ValueType::napi_function {
        match WireValue::from_napi(env, result) {
            Ok(value) => {
                let _ = msg.reply_tx.send(Ok(value));
            }
            Err(error) => {
                let _ = msg.reply_tx.send(Err(error.to_string().into()));
            }
        }
        return;
    }

    let mut promise_ref = ptr::null_mut();
    if let Err(error) = chk(unsafe { sys::napi_create_reference(env, result, 1, &mut promise_ref) })
    {
        let _ = msg.reply_tx.send(Err(error.to_string().into()));
        return;
    }
    let promise_id = msg.state.next_promise.fetch_add(1, Ordering::Relaxed);
    let settlement = Arc::new(Settlement {
        dependency: Mutex::new(Some(msg.dependency)),
        reply_tx: Mutex::new(Some(msg.reply_tx)),
        wake: msg.state.wake.clone(),
        state: msg.state.clone(),
        promise_id,
        result_alive: msg.result_alive,
        reply_signal: msg.reply_signal,
    });
    msg.state
        .promises
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(
            promise_id,
            (promise_ref as usize, Arc::downgrade(&settlement)),
        );
    let resolve = match create_settlement_callback(env, settlement.clone(), false) {
        Ok(value) => value,
        Err(error) => {
            settlement.reject(error.to_string().into());
            return;
        }
    };
    let reject = match create_settlement_callback(env, settlement.clone(), true) {
        Ok(value) => value,
        Err(error) => {
            settlement.reject(error.to_string().into());
            return;
        }
    };
    let mut then_args = [resolve, reject];
    let mut ignored = ptr::null_mut();
    let then_status = unsafe {
        sys::napi_call_function(
            env,
            result,
            then_value,
            2,
            then_args.as_mut_ptr(),
            &mut ignored,
        )
    };
    if then_status != sys::Status::napi_ok {
        settlement.reject(format!("thenable callback failed (status {})", then_status).into());
    }
}

struct Settlement {
    dependency: Mutex<Option<DependencyLease>>,
    reply_signal: Arc<crate::host::WakeSignal>,
    result_alive: Arc<AtomicBool>,
    state: Arc<BridgeState>,
    promise_id: usize,
    wake: Arc<crate::host::WakeSlot>,
    reply_tx: Mutex<Option<mpsc::Sender<Result<WireValue, crate::JsString>>>>,
}

impl Settlement {
    fn resolve(&self, value: Result<WireValue, crate::JsString>) {
        let sender = self
            .reply_tx
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(sender) = sender {
            self.dependency
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            let _ = sender.send(value);
            self.state.release_promise_on_main(self.promise_id);
            self.wake.fire();
            self.reply_signal.fire();
        }
    }

    fn reject(&self, message: crate::JsString) {
        self.resolve(Err(message));
    }
}

struct SettlementContext {
    settlement: Arc<Settlement>,
}

extern "C" fn settlement_context_finalize(
    _env: sys::napi_env,
    data: *mut std::ffi::c_void,
    _hint: *mut std::ffi::c_void,
) {
    if !data.is_null() {
        drop(unsafe { Box::from_raw(data as *mut SettlementContext) });
    }
}

fn create_settlement_callback(
    env: sys::napi_env,
    settlement: Arc<Settlement>,
    reject: bool,
) -> Result<sys::napi_value, VmErr> {
    let context = Box::new(SettlementContext { settlement });
    let context_ptr = Box::into_raw(context);
    let callback = if reject {
        promise_reject_cb
    } else {
        promise_resolve_cb
    };
    let name = if reject { c"reject" } else { c"resolve" };
    let mut function = ptr::null_mut();
    let create_status = unsafe {
        sys::napi_create_function(
            env,
            name.as_ptr(),
            if reject { 6 } else { 7 },
            Some(callback),
            context_ptr as *mut std::ffi::c_void,
            &mut function,
        )
    };
    if create_status != sys::Status::napi_ok {
        drop(unsafe { Box::from_raw(context_ptr) });
        return Err(VmErr::Msg(format!(
            "failed to create promise callback (status {})",
            create_status
        )));
    }

    // The callback can be called zero, one, or many times by a hostile
    // thenable. N-API owns the function object, so attach a finalizer to free
    // its context when the function becomes unreachable. If an environment is
    // already tearing down and rejects this optional finalizer, keep the
    // context live; that is a bounded teardown leak, never a UAF/double free.
    let mut ignored = ptr::null_mut();
    let finalizer_status = unsafe {
        sys::napi_add_finalizer(
            env,
            function,
            context_ptr as *mut std::ffi::c_void,
            Some(settlement_context_finalize),
            ptr::null_mut(),
            &mut ignored,
        )
    };
    if finalizer_status != sys::Status::napi_ok {
        // The callback was never handed to the thenable, so its context is
        // still unreachable by JavaScript. Reclaim it before returning the
        // error rather than leaving a per-call leak.
        drop(unsafe { Box::from_raw(context_ptr) });
        return Err(VmErr::Msg(format!(
            "failed to register promise callback finalizer (status {})",
            finalizer_status
        )));
    }
    Ok(function)
}

extern "C" fn promise_resolve_cb(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let mut argc = 1usize;
    let mut argv = [ptr::null_mut(); 1];
    let mut data = ptr::null_mut();
    let status = unsafe {
        sys::napi_get_cb_info(
            env,
            info,
            &mut argc,
            argv.as_mut_ptr(),
            ptr::null_mut(),
            &mut data,
        )
    };
    if status != sys::Status::napi_ok || data.is_null() {
        return ptr::null_mut();
    }
    let context = unsafe { &*(data as *mut SettlementContext) };
    let value = if argc > 0 && !argv[0].is_null() {
        match WireValue::from_napi(env, argv[0]) {
            Ok(value) => value,
            Err(error) => {
                context
                    .settlement
                    .reject(format!("marshal error in resolve: {}", error).into());
                return ptr::null_mut();
            }
        }
    } else {
        WireValue::Undefined
    };
    context.settlement.resolve(Ok(value));
    ptr::null_mut()
}

extern "C" fn promise_reject_cb(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let mut argc = 1usize;
    let mut argv = [ptr::null_mut(); 1];
    let mut data = ptr::null_mut();
    let status = unsafe {
        sys::napi_get_cb_info(
            env,
            info,
            &mut argc,
            argv.as_mut_ptr(),
            ptr::null_mut(),
            &mut data,
        )
    };
    if status != sys::Status::napi_ok || data.is_null() {
        return ptr::null_mut();
    }
    let context = unsafe { &*(data as *mut SettlementContext) };
    let message = if argc > 0 && !argv[0].is_null() {
        let property = get_named_str(env, argv[0], "message").unwrap_or_default();
        if !property.is_empty() {
            property
        } else {
            match from_napi(env, argv[0]) {
                Ok(value) => match &value {
                    Value::String(text) => text.clone(),
                    Value::Error(error) => error
                        .name
                        .concat(&crate::JsString::from(": "))
                        .concat(&error.message),
                    _ => to_string(&value).into(),
                },
                Err(error) => error.to_string().into(),
            }
        }
    } else {
        "unknown rejection".into()
    };
    context.settlement.reject(message);
    ptr::null_mut()
}

/// Completion callback for `runAsync`, called on Node's main thread.
pub(super) extern "C" fn run_async_done_cb(
    env: sys::napi_env,
    _js_callback: sys::napi_value,
    context: *mut std::ffi::c_void,
    data: *mut std::ffi::c_void,
) {
    if data.is_null() {
        return;
    }
    let result = unsafe { *Box::from_raw(data as *mut Result<String, String>) };
    // Node can invoke a thread-safe-function callback with a null env during
    // teardown. The result has been reclaimed above; no deferred settlement
    // is safe or necessary once the environment is gone.
    if env.is_null() || context.is_null() {
        return;
    }
    let deferred = context as sys::napi_deferred;
    match result {
        Ok(value) => {
            let mut js_value = ptr::null_mut();
            let create_status = unsafe {
                sys::napi_create_string_utf8(
                    env,
                    value.as_ptr() as *const c_char,
                    value.len() as isize,
                    &mut js_value,
                )
            };
            if create_status == sys::Status::napi_ok {
                let resolve_status = unsafe { sys::napi_resolve_deferred(env, deferred, js_value) };
                if resolve_status != sys::Status::napi_ok {
                    // The environment may be closing; the promise cannot be
                    // settled after a failed deferred operation.
                }
            } else {
                reject_deferred_with_message(
                    env,
                    deferred,
                    format!(
                        "failed to create runAsync result (status {})",
                        create_status
                    ),
                );
            }
        }
        Err(message) => reject_deferred_with_message(env, deferred, message),
    }
}

fn reject_deferred_with_message(env: sys::napi_env, deferred: sys::napi_deferred, message: String) {
    let mut js_message = ptr::null_mut();
    let message_status = unsafe {
        sys::napi_create_string_utf8(
            env,
            message.as_ptr() as *const c_char,
            message.len() as isize,
            &mut js_message,
        )
    };
    if message_status != sys::Status::napi_ok {
        return;
    }
    let mut js_error = ptr::null_mut();
    let error_status =
        unsafe { sys::napi_create_error(env, ptr::null_mut(), js_message, &mut js_error) };
    if error_status != sys::Status::napi_ok {
        return;
    }
    let reject_status = unsafe { sys::napi_reject_deferred(env, deferred, js_error) };
    if reject_status != sys::Status::napi_ok {
        // The environment may be closing; there is no safe follow-up action.
    }
}

#[cfg(test)]
mod dependency_lifecycle_tests {
    use super::*;

    #[test]
    fn unresolved_call_survives_retirement_and_protects_the_next_execution() {
        // No Node handles are created: directly exercise the same admission
        // state used by the owner, with explicit execution transition barriers.
        let bridge = NapiHostBridge::new(ptr::null_mut());
        let state = Arc::new(AsyncState {
            state: bridge.shared_state(),
            next_pending: AtomicUsize::new(0),
            waiting_for_node: AtomicUsize::new(0),
            dependencies: Arc::new(NodeDependencies {
                active: AtomicBool::new(false),
                calls: AtomicUsize::new(0),
            }),
            pending: Mutex::new(HashMap::new()),
            cancellation: Mutex::new(crate::CancellationToken::default()),
            deadline: Mutex::new(None),
            reply_wake: Mutex::new(None),
        });
        *bridge.async_state.lock().unwrap() = Some(state.clone());
        bridge.set_owner_execution_active(true); // A starts.
        let execution = state.dependencies.clone();
        execution.calls.fetch_add(1, Ordering::AcqRel);
        let dispatched = DependencyLease(execution);
        bridge.set_owner_execution_active(false); // A returns saved, unawaited.
        assert!(!bridge.owner_waiting_for_node(false)); // Idle reentry stays usable.
        assert!(bridge.owner_waiting_for_node(true)); // B admitted, not started.
        bridge.set_owner_execution_active(true); // B starts before callback A.
        assert!(bridge.owner_waiting_for_node(false)); // B may later await saved.
        drop(dispatched); // Callback A settles exactly once.
        assert!(!bridge.owner_waiting_for_node(false));
        assert!(!bridge.owner_waiting_for_node(true)); // Stored settled results do not lock admission.
    }
}
