//! Async work pools and threadsafe function plumbing.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::error::VmErr;
use crate::host::{HostCallback, HostCallbackKind, HostEvent};
use crate::interpreter::Env;
use crate::interpreter::native_addon_binary::validate_native_addon_binary;
use crate::value::Value;

use super::api::threadsafe_function_registry;
use super::guest::create_native_callback_value_with_kind;
use super::lifecycle::napi_collect_weak_references;
use super::shim::NodeApiShim;
use super::state::{
    AsyncWorkCompletion, AsyncWorkPool, AsyncWorkTaskMessage, CallbackFrame,
    GuestCallbackDispatcher, GuestCallbackDispatcherScope, HostRuntimeNotification, HostState,
    NapiEnvironment, NapiThreadsafeFunctionShared, NativeCallback,
};
use super::{
    ASYNC_WORK_CANCELLED, ASYNC_WORK_FINISHED, ASYNC_WORK_QUEUE_CAPACITY, ASYNC_WORK_QUEUED,
    ASYNC_WORK_RUNNING, ASYNC_WORKER_COUNT, NAPI_CANCELLED, NAPI_GENERIC_FAILURE, NAPI_OK, NapiEnv,
    ReportedNodeVersion, RustNodeApiHost, call_guest_callback, dispatch_guest_callback,
    environment, napi_error,
};

pub(super) fn create_async_work_pool(
    runtime_notification_sender: Sender<HostRuntimeNotification>,
) -> Result<AsyncWorkPool, VmErr> {
    let (task_sender, task_receiver) = mpsc::sync_channel(ASYNC_WORK_QUEUE_CAPACITY);
    let task_receiver = Arc::new(Mutex::new(task_receiver));
    let mut workers: Vec<JoinHandle<()>> = Vec::with_capacity(ASYNC_WORKER_COUNT);

    for worker_id in 0..ASYNC_WORKER_COUNT {
        let task_receiver = task_receiver.clone();
        let runtime_notification_sender = runtime_notification_sender.clone();
        let worker = thread::Builder::new()
            .name(format!("napi-vm-addon-{worker_id}"))
            .spawn(move || {
                loop {
                    let message = match task_receiver.lock() {
                        Ok(receiver) => receiver.recv(),
                        Err(_) => return,
                    };
                    match message {
                        Ok(AsyncWorkTaskMessage::Run(task)) => {
                            let status = if task
                                .state
                                .compare_exchange(
                                    ASYNC_WORK_QUEUED,
                                    ASYNC_WORK_RUNNING,
                                    Ordering::AcqRel,
                                    Ordering::Acquire,
                                )
                                .is_ok()
                            {
                                // Node-API forbids using env from an execute callback.
                                // Preserve the ABI argument for addons that only inspect it.
                                unsafe {
                                    (task.execute)(
                                        task.environment as NapiEnv,
                                        task.data as *mut c_void,
                                    );
                                }
                                NAPI_OK
                            } else if task.state.load(Ordering::Acquire) == ASYNC_WORK_CANCELLED {
                                NAPI_CANCELLED
                            } else {
                                NAPI_GENERIC_FAILURE
                            };
                            task.state.store(ASYNC_WORK_FINISHED, Ordering::Release);
                            task.completion_status
                                .store(status as u8, Ordering::Release);
                            let _ = runtime_notification_sender.send(
                                HostRuntimeNotification::AsyncWorkCompletion(AsyncWorkCompletion {
                                    work_id: task.work_id,
                                    status,
                                }),
                            );
                        }
                        Ok(AsyncWorkTaskMessage::Stop) | Err(_) => return,
                    }
                }
            })
            .map_err(|error| {
                for _ in 0..workers.len() {
                    let _ = task_sender.send(AsyncWorkTaskMessage::Stop);
                }
                for worker in workers.drain(..) {
                    let _ = worker.join();
                }
                VmErr::Msg(format!("cannot start Node-API worker pool: {error}"))
            })?;
        workers.push(worker);
    }

    Ok((task_sender, workers))
}

impl RustNodeApiHost {
    pub(super) fn new(
        global: Env,
        reported_node_version: ReportedNodeVersion,
        max_napi_version: u32,
        allowed_roots: Vec<PathBuf>,
        allowed_addons: HashMap<PathBuf, [u8; 32]>,
    ) -> Result<Self, VmErr> {
        let object_prototype = global
            .borrow()
            .get("Object")
            .and_then(|object| object.get_prop("prototype"));
        let shim = NodeApiShim::load()?;
        let (runtime_notification_sender, runtime_notifications) = mpsc::channel();
        let (async_work_sender, async_workers) =
            create_async_work_pool(runtime_notification_sender.clone())?;
        Ok(Self {
            state: Rc::new(RefCell::new(HostState {
                global,
                object_prototype,
                reported_node_version,
                max_napi_version,
                next_callback_id: 1,
                callbacks: HashMap::new(),
                environments: Vec::new(),
                type_tags: HashMap::new(),
                libraries: HashMap::new(),
                async_work_sender,
                runtime_notifications,
                runtime_notification_sender,
                async_workers,
                _shim: shim.clone(),
            })),
            _shim: shim,
            allowed_roots,
            allowed_addons,
            shutdown_started: Cell::new(false),
        })
    }

    /// Stop native workers and run addon cleanup hooks and finalizers.
    /// Repeated calls are safe.
    pub fn shutdown(&self) -> Result<(), VmErr> {
        self.shutdown_inner()
    }

    /// Whether this host has completed shutdown.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown_started.get()
    }

    pub(super) fn ensure_running(&self) -> Result<(), VmErr> {
        if self.is_shutdown() {
            Err(VmErr::Msg("Rust Node-API host has been shut down".into()))
        } else {
            Ok(())
        }
    }

    pub(super) fn preflight_addon_path(&self, filename: &Path) -> Result<PathBuf, VmErr> {
        self.ensure_running()?;
        let filename = fs::canonicalize(filename).map_err(|error| {
            VmErr::Msg(format!(
                "cannot resolve native addon {}: {error}",
                filename.display()
            ))
        })?;
        if !self
            .allowed_roots
            .iter()
            .any(|root| filename.starts_with(root))
        {
            return Err(VmErr::Msg(format!(
                "native addon escapes configured roots: {}",
                filename.display()
            )));
        }
        if filename
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("node")
        {
            return Err(VmErr::Msg(format!(
                "native addon path must use the .node extension: {}",
                filename.display()
            )));
        }
        let expected_digest = self.allowed_addons.get(&filename).ok_or_else(|| {
            VmErr::Msg(format!(
                "native addon is not allowlisted: {}",
                filename.display()
            ))
        })?;
        let actual_digest =
            crate::interpreter::commonjs::sha256_file(&filename).map_err(|error| {
                VmErr::Msg(format!(
                    "cannot verify native addon {}: {error}",
                    filename.display()
                ))
            })?;
        if &actual_digest != expected_digest {
            return Err(VmErr::Msg(format!(
                "native addon integrity check failed before loading: {}",
                filename.display()
            )));
        }
        validate_native_addon_binary(&filename)?;
        Ok(filename)
    }

    pub(super) fn invoke_native(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        new_target: Option<Value>,
        mut callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        let callback = {
            let mut state = self.state.borrow_mut();
            let callback =
                state.callbacks.get(&id).cloned().ok_or_else(|| {
                    VmErr::Msg("native callback handle is no longer valid".into())
                })?;
            if callback.one_shot {
                state.callbacks.remove(&id);
            }
            callback
        };
        let scope = callback
            .env
            .handles
            .borrow_mut()
            .open_scope()
            .map_err(|status| napi_error("opening callback handle scope", status))?;
        let mut threadsafe_call = None;
        let result = (|| {
            let callback_handler_pointer: *mut &mut (
                     dyn FnMut(HostCallback) -> Result<Value, VmErr> + '_
                 ) = &mut callback_handler;
            let dispatcher = GuestCallbackDispatcher {
                context: callback_handler_pointer.cast(),
                invoke: dispatch_guest_callback,
            };
            let dispatcher_scope =
                GuestCallbackDispatcherScope::push(callback.env.clone(), dispatcher);
            let (returned, completion_work_id) = match callback.callback {
                NativeCallback::Function(callback_fn) => {
                    let this_arg = callback
                        .env
                        .handles
                        .borrow_mut()
                        .create(this_value)
                        .map_err(|status| {
                            napi_error("creating callback receiver handle", status)
                        })?;
                    let new_target = match new_target {
                        Some(new_target) => callback
                            .env
                            .handles
                            .borrow_mut()
                            .create(new_target)
                            .map_err(|status| napi_error("creating new.target handle", status))?,
                        None => std::ptr::null_mut(),
                    };
                    let arg_handles = args
                        .into_iter()
                        .map(|value| {
                            callback
                                .env
                                .handles
                                .borrow_mut()
                                .create(value)
                                .map_err(|status| {
                                    napi_error("creating callback argument handle", status)
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let frame = CallbackFrame {
                        args: arg_handles,
                        this_arg,
                        new_target,
                        data: callback.data,
                    };
                    let callback_info =
                        (&frame as *const CallbackFrame).cast_mut().cast::<c_void>();
                    let frame_key = callback_info as usize;
                    callback
                        .env
                        .active_callbacks
                        .borrow_mut()
                        .insert(frame_key, frame.clone());
                    let returned = unsafe { callback_fn(callback.env.raw(), callback_info) };
                    callback
                        .env
                        .active_callbacks
                        .borrow_mut()
                        .remove(&frame_key);
                    (returned, None)
                }
                NativeCallback::PostedFinalizer {
                    finalize,
                    data,
                    hint,
                } => {
                    unsafe { finalize(callback.env.raw(), data, hint) };
                    (std::ptr::null_mut(), None)
                }
                NativeCallback::AsyncComplete {
                    callback: callback_fn,
                    status,
                    work_id,
                } => {
                    if let Some(work) = callback.env.async_works.borrow_mut().get_mut(&work_id) {
                        work.completion_callback_active = true;
                    }
                    unsafe { callback_fn(callback.env.raw(), status, callback.data) };
                    if let Some(work) = callback.env.async_works.borrow_mut().get_mut(&work_id) {
                        work.completion_callback_active = false;
                    }
                    (std::ptr::null_mut(), Some(work_id))
                }
                NativeCallback::ThreadsafeFunctionCall {
                    callback: js_callback,
                    call_js,
                    context,
                    shared,
                } => {
                    threadsafe_call = Some(shared);
                    if let Some(call_js) = call_js {
                        let callback_handle = match js_callback {
                            Some(js_callback) => callback
                                .env
                                .handles
                                .borrow_mut()
                                .create(js_callback)
                                .map_err(|status| {
                                    napi_error("creating thread-safe callback handle", status)
                                })?,
                            None => std::ptr::null_mut(),
                        };
                        unsafe {
                            call_js(callback.env.raw(), callback_handle, context, callback.data);
                        }
                    } else if let Some(js_callback) = js_callback {
                        let _ = call_guest_callback(
                            &callback.env,
                            HostCallback {
                                callback: js_callback,
                                this_value: Value::Undefined,
                                args: Vec::new(),
                                kind: HostCallbackKind::Call,
                            },
                        );
                    }
                    (std::ptr::null_mut(), None)
                }
            };
            drop(dispatcher_scope);
            if let Some(work_id) = completion_work_id
                && let Some(work) = callback.env.async_works.borrow_mut().get_mut(&work_id)
            {
                work.callback_run = true;
            }
            if let Some(exception) = callback.env.pending_exception.borrow_mut().take() {
                Err(VmErr::Throw(exception))
            } else if returned.is_null() {
                Ok(Value::Undefined)
            } else {
                callback
                    .env
                    .handles
                    .borrow()
                    .get(returned)
                    .map_err(|status| napi_error("reading native callback result", status))
            }
        })();
        let close_result = callback
            .env
            .handles
            .borrow_mut()
            .close_scope(scope)
            .map_err(|status| napi_error("closing callback handle scope", status));
        napi_collect_weak_references(&callback.env);
        let threadsafe_result = if let Some(shared) = threadsafe_call {
            finish_threadsafe_call(&callback.env, &shared)
        } else {
            Ok(())
        };
        match (result, close_result, threadsafe_result) {
            (Ok(value), Ok(()), Ok(())) => Ok(value),
            (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
        }
    }
}

pub(super) fn finish_threadsafe_call(
    environment: &Rc<NapiEnvironment>,
    shared: &Arc<NapiThreadsafeFunctionShared>,
) -> Result<(), VmErr> {
    {
        let mut state = shared
            .state
            .lock()
            .map_err(|_| VmErr::Msg("Node-API thread-safe function state is poisoned".into()))?;
        state.in_flight = state.in_flight.saturating_sub(1);
        shared.queue_space.notify_all();
    }
    finalize_threadsafe_function(environment, shared)
}

pub(super) fn finalize_threadsafe_function(
    environment: &Rc<NapiEnvironment>,
    shared: &Arc<NapiThreadsafeFunctionShared>,
) -> Result<(), VmErr> {
    let ready = {
        let mut state = shared
            .state
            .lock()
            .map_err(|_| VmErr::Msg("Node-API thread-safe function state is poisoned".into()))?;
        if state.finalized
            || state.thread_count != 0
            || !state.values.is_empty()
            || state.in_flight != 0
        {
            false
        } else {
            state.finalized = true;
            true
        }
    };
    if !ready {
        return Ok(());
    }

    let function = environment
        .threadsafe_functions
        .borrow_mut()
        .remove(&shared.id);
    if let Ok(mut registry) = threadsafe_function_registry().lock() {
        registry.remove(&shared.id);
    }
    if let Some(function) = function
        && let Some(finalize) = function.finalize
    {
        let scope = environment
            .handles
            .borrow_mut()
            .open_scope()
            .map_err(|status| napi_error("opening thread-safe finalizer scope", status))?;
        unsafe {
            finalize(environment.raw(), function.finalize_data, function.context);
        }
        environment.pending_exception.borrow_mut().take();
        environment
            .handles
            .borrow_mut()
            .close_scope(scope)
            .map_err(|status| napi_error("closing thread-safe finalizer scope", status))?;
    }
    Ok(())
}

pub(super) fn shutdown_threadsafe_functions(
    host_state: &Rc<RefCell<HostState>>,
    environments: &[Rc<NapiEnvironment>],
) -> bool {
    let mut has_active_native_threads = false;
    for environment in environments {
        let functions = environment
            .threadsafe_functions
            .borrow()
            .iter()
            .map(|(id, function)| {
                (
                    *id,
                    function.shared.clone(),
                    function.call_js,
                    function.context,
                )
            })
            .collect::<Vec<_>>();
        for (_, shared, call_js, context) in functions {
            let (queued, active) = match shared.state.lock() {
                Ok(mut queue) => {
                    queue.closing = true;
                    queue.orphaned = true;
                    let queued = queue.values.drain(..).collect::<Vec<_>>();
                    let active = queue.thread_count != 0;
                    queue.in_flight = 0;
                    shared.queue_space.notify_all();
                    (queued, active)
                }
                Err(_) => (Vec::new(), true),
            };

            let callbacks = {
                let mut state = host_state.borrow_mut();
                let ids = state
                    .callbacks
                    .iter()
                    .filter_map(|(callback_id, record)| {
                        matches!(
                            &record.callback,
                            NativeCallback::ThreadsafeFunctionCall {
                                shared: record_shared,
                                ..
                            } if Arc::ptr_eq(record_shared, &shared)
                        )
                        .then_some(*callback_id)
                    })
                    .collect::<Vec<_>>();
                ids.into_iter()
                    .filter_map(|callback_id| state.callbacks.remove(&callback_id))
                    .collect::<Vec<_>>()
            };

            if let Some(call_js) = call_js {
                for data in queued {
                    unsafe {
                        call_js(
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            context,
                            data as *mut c_void,
                        );
                    }
                }
                for callback in &callbacks {
                    unsafe {
                        call_js(
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            context,
                            callback.data,
                        );
                    }
                }
            }
            if active {
                has_active_native_threads = true;
            } else if let Err(error) = finalize_threadsafe_function(environment, &shared) {
                eprintln!("failed to finalize Node-API thread-safe function: {error}");
            }
        }
        environment.threadsafe_functions.borrow_mut().clear();
    }
    has_active_native_threads
}

pub(super) fn take_threadsafe_function_queue(
    shared: &Arc<NapiThreadsafeFunctionShared>,
) -> Result<Vec<usize>, i32> {
    let mut state = shared.state.lock().map_err(|_| NAPI_GENERIC_FAILURE)?;
    let values = state.values.drain(..).collect::<Vec<_>>();
    state.in_flight = state
        .in_flight
        .checked_add(values.len())
        .ok_or(NAPI_GENERIC_FAILURE)?;
    shared.queue_space.notify_all();
    Ok(values)
}

pub(super) fn thread_safe_function_events(function_id: usize) -> Result<Vec<HostEvent>, VmErr> {
    let Some(shared) = threadsafe_function_registry()
        .lock()
        .map_err(|_| VmErr::Msg("Node-API thread-safe function registry is poisoned".into()))?
        .get(&function_id)
        .cloned()
    else {
        return Ok(Vec::new());
    };
    let environment = environment(shared.environment as NapiEnv)
        .map_err(|status| napi_error("reading thread-safe function environment", status))?;
    let values = take_threadsafe_function_queue(&shared)
        .map_err(|status| napi_error("draining thread-safe function queue", status))?;
    let (js_callback, call_js, context) = environment
        .threadsafe_functions
        .borrow()
        .get(&function_id)
        .map(|function| {
            (
                function.callback.clone(),
                function.call_js,
                function.context,
            )
        })
        .ok_or_else(|| VmErr::Msg("Node-API thread-safe function was finalized early".into()))?;

    let mut events = Vec::with_capacity(values.len());
    let value_count = values.len();
    for data in values {
        let callback = match create_native_callback_value_with_kind(
            &environment,
            "napi_threadsafe_function_call",
            NativeCallback::ThreadsafeFunctionCall {
                callback: js_callback.clone(),
                call_js,
                context,
                shared: shared.clone(),
            },
            data as *mut c_void,
            true,
        ) {
            Ok(callback) => callback,
            Err(status) => {
                let mut state = shared.state.lock().map_err(|_| {
                    VmErr::Msg("Node-API thread-safe function state is poisoned".into())
                })?;
                state.in_flight = state.in_flight.saturating_sub(value_count - events.len());
                return Err(napi_error("creating thread-safe callback", status));
            }
        };
        events.push(HostEvent::Callback(HostCallback {
            callback,
            this_value: Value::Undefined,
            args: Vec::new(),
            kind: HostCallbackKind::Call,
        }));
    }
    if events.is_empty() {
        finalize_threadsafe_function(&environment, &shared)?;
    }
    Ok(events)
}
