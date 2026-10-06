//! OS scheduling adapter. Threaded producers transfer JSON, never VM values.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use super::permissions::Permissions;
use crate::host::{HostBridge, HostEvent, HostWaitMode, WakeNotifier};
use crate::interpreter::{ClockMode, DrainPolicy, EvaluationOptions, ExecutionBudget};
use crate::value::{PromiseInner, PromiseState};
use crate::{Interpreter, RealTimeClock, TurnBudget, TurnOutcome, Value, VmErr};

type Completion = (u64, Result<serde_json::Value, String>);

/// Try-send provides backpressure without blocking producer threads.
#[derive(Clone)]
pub struct ExternalEventSender {
    sender: mpsc::SyncSender<Completion>,
    wake: Arc<Mutex<Option<WakeNotifier>>>,
}
impl ExternalEventSender {
    #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
    pub(super) fn complete_wait(&self, id: u64, result: Result<serde_json::Value, String>) {
        if self.sender.send((id, result)).is_ok()
            && let Some(wake) = self.wake.lock().unwrap_or_else(|e| e.into_inner()).clone()
        {
            wake();
        }
    }
    pub fn complete(
        &self,
        id: u64,
        result: Result<serde_json::Value, String>,
    ) -> Result<(), mpsc::TrySendError<Completion>> {
        self.sender.try_send((id, result))?;
        if let Some(wake) = self.wake.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            wake();
        }
        Ok(())
    }
}

pub struct ExternalEventQueue {
    receiver: mpsc::Receiver<Completion>,
    sender: ExternalEventSender,
    staged: RefCell<Option<Completion>>,
}
impl ExternalEventQueue {
    pub fn new(capacity: usize) -> Result<Self, VmErr> {
        if capacity == 0 {
            return Err(VmErr::Msg("event queue capacity must be positive".into()));
        }
        let (sender, receiver) = mpsc::sync_channel(capacity);
        Ok(Self {
            receiver,
            staged: RefCell::new(None),
            sender: ExternalEventSender {
                sender,
                wake: Arc::new(Mutex::new(None)),
            },
        })
    }
    pub fn sender(&self) -> ExternalEventSender {
        self.sender.clone()
    }
    fn next(&self) -> Option<Completion> {
        self.staged
            .borrow_mut()
            .take()
            .or_else(|| self.receiver.try_recv().ok())
    }
    fn has_ready(&self) -> bool {
        if self.staged.borrow().is_some() {
            return true;
        }
        if let Ok(completion) = self.receiver.try_recv() {
            *self.staged.borrow_mut() = Some(completion);
            return true;
        }
        false
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeLimits {
    pub fuel: u64,
    pub stack_depth: usize,
    pub jobs: usize,
    pub timers: usize,
    pub external_events: usize,
    pub io_resources: usize,
    pub file_bytes: usize,
    /// Elapsed execution deadline, not OS CPU accounting.
    pub timeout: Option<Duration>,
}
impl Default for RuntimeLimits {
    fn default() -> Self {
        let budget = ExecutionBudget::default();
        Self {
            fuel: budget.fuel,
            stack_depth: budget.max_call_depth,
            jobs: budget.max_jobs,
            timers: 1024,
            external_events: 1024,
            io_resources: 1024,
            file_bytes: 16 * 1024 * 1024,
            timeout: None,
        }
    }
}

struct RuntimeBridge {
    queue: ExternalEventQueue,
    pending: RefCell<HashMap<u64, Value>>,
    next_id: Cell<u64>,
    permissions: Permissions,
    limits: RuntimeLimits,
    #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
    workers: Arc<AtomicUsize>,
    #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
    cancellations: RefCell<HashMap<u64, Arc<AtomicBool>>>,
    #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
    sockets: RefCell<HashMap<u64, SocketHandle>>,
    #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
    binary_operations: RefCell<std::collections::HashSet<u64>>,
}
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
impl Drop for RuntimeBridge {
    fn drop(&mut self) {
        for cancelled in self.cancellations.get_mut().values() {
            cancelled.store(true, Ordering::Release);
        }
    }
}
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
struct SocketHandle {
    commands: mpsc::SyncSender<super::sockets::Command>,
    closed: Arc<AtomicBool>,
}
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
impl Drop for SocketHandle {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}
impl RuntimeBridge {
    #[cfg(any(feature = "runtime-web", feature = "runtime-net"))]
    fn buffer_bytes(&self, value: &Value) -> Result<Vec<u8>, VmErr> {
        let (backing, offset, length) = match value {
            Value::ArrayBuffer(buffer) => (
                crate::value::BufferBacking::Array(buffer.clone()),
                0,
                buffer.borrow().len(),
            ),
            Value::SharedArrayBuffer(buffer) => (
                crate::value::BufferBacking::Shared(buffer.clone()),
                0,
                buffer.len(),
            ),
            Value::TypedArray(view) => (
                view.buffer.clone(),
                view.byte_offset,
                view.effective_length().saturating_mul(view.kind.size()),
            ),
            Value::DataView(view) => (
                view.buffer.clone(),
                view.byte_offset,
                view.effective_length(),
            ),
            _ => return Err(VmErr::Msg("TypeError: expected BufferSource".into())),
        };
        if length > self.limits.file_bytes {
            return Err(VmErr::Msg(
                "ResourceLimit: binary byte count exceeded".into(),
            ));
        }
        if backing.is_detached() {
            return Err(VmErr::Msg("TypeError: detached buffer".into()));
        }
        backing
            .read(offset, length)
            .ok_or_else(|| VmErr::Msg("TypeError: detached or invalid buffer".into()))
    }
    fn external_promise(&self) -> Result<(u64, Value), VmErr> {
        if self.pending.borrow().len() >= self.limits.io_resources {
            return Err(VmErr::Msg("ResourceLimit: IO resources exceeded".into()));
        }
        let id = self.next_id.get();
        self.next_id.set(
            id.checked_add(1)
                .ok_or_else(|| VmErr::Msg("external operation IDs exhausted".into()))?,
        );
        let promise = Value::Promise(Value::pending_promise());
        self.pending.borrow_mut().insert(id, promise.clone());
        Ok((id, promise))
    }
    #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
    fn fetch(&self, input: &str, body: Option<&Value>) -> Result<Value, VmErr> {
        let mut request: super::network::FetchRequest =
            serde_json::from_str(input).map_err(|e| VmErr::Msg(e.to_string()))?;
        if let Some(body) = body.filter(|v| !matches!(v, Value::Undefined | Value::Null)) {
            request.body = Some(self.buffer_bytes(body)?);
        }
        super::network::validate_url(&request.url, &self.permissions).map_err(VmErr::Msg)?;
        if !matches!(request.redirect.as_str(), "follow" | "error" | "manual") {
            return Err(VmErr::Msg("TypeError: invalid redirect mode".into()));
        }
        if request
            .body
            .as_ref()
            .is_some_and(|b| b.len() > self.limits.file_bytes)
        {
            return Err(VmErr::Msg("ResourceLimit: request body exceeded".into()));
        }
        if self.workers.load(Ordering::Acquire) >= self.limits.io_resources {
            return Err(VmErr::Msg("ResourceLimit: network workers exceeded".into()));
        }
        let (id, promise) = self.external_promise()?;
        self.binary_operations.borrow_mut().insert(id);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.cancellations
            .borrow_mut()
            .insert(id, cancelled.clone());
        let sender = self.queue.sender();
        let permissions = self.permissions.clone();
        let max_bytes = self.limits.file_bytes;
        let timeout = self.limits.timeout.unwrap_or(Duration::from_secs(30));
        let workers = self.workers.clone();
        workers.fetch_add(1, Ordering::AcqRel);
        if let Err(error) = std::thread::Builder::new()
            .name("napi-vm-http".into())
            .spawn(move || {
                struct WorkerGuard(Arc<AtomicUsize>);
                impl Drop for WorkerGuard {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                let _guard = WorkerGuard(workers);
                let result =
                    super::network::fetch(request, permissions, max_bytes, timeout, cancelled);
                sender.complete_wait(id, result);
            })
        {
            self.workers.fetch_sub(1, Ordering::AcqRel);
            self.pending.borrow_mut().remove(&id);
            self.cancellations.borrow_mut().remove(&id);
            self.binary_operations.borrow_mut().remove(&id);
            return Err(VmErr::Msg(error.to_string()));
        }
        Ok(Value::object(vec![
            ("id".into(), Value::Number(id as f64)),
            ("promise".into(), promise),
        ]))
    }
    #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
    fn open_socket(&self, input: &str) -> Result<Value, VmErr> {
        let request: super::sockets::OpenRequest =
            serde_json::from_str(input).map_err(|e| VmErr::Msg(e.to_string()))?;
        super::sockets::authority(&request, &self.permissions).map_err(VmErr::Msg)?;
        if self.sockets.borrow().len() >= self.limits.io_resources
            || self.workers.load(Ordering::Acquire) >= self.limits.io_resources
        {
            return Err(VmErr::Msg(
                "ResourceLimit: socket resources exceeded".into(),
            ));
        }
        let (id, promise) = self.external_promise()?;
        let (commands, receiver) = mpsc::sync_channel(self.limits.external_events);
        let closed = Arc::new(AtomicBool::new(false));
        let events = self.queue.sender();
        let permissions = self.permissions.clone();
        let max_bytes = self.limits.file_bytes;
        let timeout = self.limits.timeout.unwrap_or(Duration::from_secs(30));
        let workers = self.workers.clone();
        workers.fetch_add(1, Ordering::AcqRel);
        self.sockets.borrow_mut().insert(
            id,
            SocketHandle {
                commands,
                closed: closed.clone(),
            },
        );
        if let Err(error) = std::thread::Builder::new()
            .name("napi-vm-socket".into())
            .spawn(move || {
                struct Guard(Arc<AtomicUsize>);
                impl Drop for Guard {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::AcqRel);
                    }
                }
                let _guard = Guard(workers);
                super::sockets::worker(
                    request,
                    super::sockets::WorkerConfig {
                        permissions,
                        max_bytes,
                        timeout,
                    },
                    id,
                    receiver,
                    events,
                    closed,
                );
            })
        {
            self.workers.fetch_sub(1, Ordering::AcqRel);
            self.sockets.borrow_mut().remove(&id);
            self.pending.borrow_mut().remove(&id);
            return Err(VmErr::Msg(error.to_string()));
        }
        Ok(Value::object(vec![
            ("id".into(), Value::Number(id as f64)),
            ("promise".into(), promise),
        ]))
    }
    #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
    fn socket_operation(
        &self,
        handle: u64,
        payload: Option<super::sockets::Payload>,
    ) -> Result<Value, VmErr> {
        let sockets = self.sockets.borrow();
        let socket = sockets
            .get(&handle)
            .ok_or_else(|| VmErr::Msg("Socket is closed".into()))?;
        let (id, promise) = self.external_promise()?;
        let command = if let Some(payload) = payload {
            super::sockets::Command::Send(id, payload)
        } else {
            super::sockets::Command::Next(id)
        };
        if socket.commands.try_send(command).is_err() {
            self.pending.borrow_mut().remove(&id);
            return Err(VmErr::Msg(
                "ResourceLimit: socket command queue unavailable".into(),
            ));
        }
        self.binary_operations.borrow_mut().insert(id);
        Ok(promise)
    }
}
impl HostBridge for RuntimeBridge {
    fn call_host_with_interp(
        &self,
        id: usize,
        _this: Value,
        args: Vec<Value>,
        _callback: &mut dyn FnMut(
            &mut Interpreter,
            crate::host::HostCallback,
        ) -> Result<Value, VmErr>,
        interpreter: &mut Interpreter,
    ) -> Result<Value, VmErr> {
        let result = self.call_host(id, args);
        interpreter.republish_roots();
        result
    }
    fn trace_roots(&self, values: &mut Vec<Value>, _: &mut Vec<crate::interpreter::Env>) {
        values.extend(self.pending.borrow().values().cloned());
    }
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        let string = |index| match args.get(index) {
            Some(Value::String(s)) => Ok(s.as_str()),
            _ => Err(VmErr::Msg("TypeError: expected string argument".into())),
        };
        match id {
            #[cfg(feature = "runtime-web")]
            32 => self
                .buffer_bytes(args.first().unwrap_or(&Value::Undefined))
                .map(|bytes| Value::ArrayBuffer(crate::value::Buffer::owned(bytes))),
            #[cfg(all(feature = "runtime-web", not(target_arch = "wasm32")))]
            30 => {
                let length = args.first().map(Value::to_number).unwrap_or(f64::NAN);
                if !length.is_finite()
                    || !(0.0..=65536.0).contains(&length)
                    || length.fract() != 0.0
                {
                    return Err(VmErr::Msg(
                        "QuotaExceededError: invalid random byte count".into(),
                    ));
                }
                let mut bytes = vec![0u8; length as usize];
                getrandom::fill(&mut bytes).map_err(|e| VmErr::Msg(e.to_string()))?;
                crate::value_from_json(&serde_json::json!(bytes))
            }
            #[cfg(feature = "runtime-web")]
            31 => {
                use sha2::Digest;
                let algorithm = string(0)?;
                let bytes = self.buffer_bytes(args.get(1).unwrap_or(&Value::Undefined))?;
                if bytes.len() > self.limits.file_bytes {
                    return Err(VmErr::Msg(
                        "ResourceLimit: digest byte count exceeded".into(),
                    ));
                }
                let digest = match algorithm {
                    "SHA-256" => sha2::Sha256::digest(bytes).to_vec(),
                    "SHA-384" => sha2::Sha384::digest(bytes).to_vec(),
                    "SHA-512" => sha2::Sha512::digest(bytes).to_vec(),
                    _ => {
                        return Err(VmErr::Msg(
                            "NotSupportedError: unsupported digest algorithm".into(),
                        ));
                    }
                };
                crate::value_from_json(&serde_json::json!(digest))
            }
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            20 => self.open_socket(string(0)?),
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            21 => self.socket_operation(
                args.first().map(Value::to_number).unwrap_or(0.0) as u64,
                None,
            ),
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            22 => {
                let payload: super::sockets::Payload = if let Some(buffer) = args
                    .get(2)
                    .filter(|v| !matches!(v, Value::Undefined | Value::Null))
                {
                    let bytes = self.buffer_bytes(buffer)?;
                    if let Some(Value::String(metadata)) = args.get(1) {
                        super::sockets::Payload::Http {
                            metadata: metadata.to_string(),
                            bytes,
                        }
                    } else {
                        super::sockets::Payload::Bytes(bytes)
                    }
                } else {
                    serde_json::from_str(string(1)?).map_err(|e| VmErr::Msg(e.to_string()))?
                };
                let size = match &payload {
                    super::sockets::Payload::Text(t) => t.len(),
                    super::sockets::Payload::Bytes(b) => b.len(),
                    super::sockets::Payload::Http { metadata, bytes } => {
                        metadata.len().saturating_add(bytes.len())
                    }
                };
                if size > self.limits.file_bytes {
                    return Err(VmErr::Msg(
                        "ResourceLimit: socket message size exceeded".into(),
                    ));
                }
                self.socket_operation(
                    args.first().map(Value::to_number).unwrap_or(0.0) as u64,
                    Some(payload),
                )
            }
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            23 => {
                let handle = args.first().map(Value::to_number).unwrap_or(0.0) as u64;
                let code = args.get(1).map(Value::to_number).unwrap_or(1000.0) as u16;
                if let Some(socket) = self.sockets.borrow().get(&handle) {
                    socket
                        .commands
                        .try_send(super::sockets::Command::Close(code, string(2)?.into()))
                        .map_err(|_| VmErr::Msg("Socket close queue unavailable".into()))?;
                }
                Ok(Value::Undefined)
            }
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            24 => {
                self.sockets
                    .borrow_mut()
                    .remove(&(args.first().map(Value::to_number).unwrap_or(0.0) as u64));
                Ok(Value::Undefined)
            }
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            10 => self.fetch(string(0)?, args.get(1)),
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            11 => {
                let id = args.first().map(Value::to_number).unwrap_or(0.0) as u64;
                if let Some(cancelled) = self.cancellations.borrow().get(&id) {
                    cancelled.store(true, Ordering::Release);
                }
                // Guest abort handling rejects immediately. Keep the host operation
                // rooted and counted until its worker actually exits.
                Ok(Value::Undefined)
            }
            #[cfg(feature = "runtime-fs")]
            1 => self
                .permissions
                .read_text(string(0)?, self.limits.file_bytes)
                .map(|value| Value::String(value.into())),
            #[cfg(feature = "runtime-fs")]
            2 => {
                self.permissions
                    .write_text(string(0)?, string(1)?, self.limits.file_bytes)?;
                Ok(Value::Undefined)
            }
            3 => Ok(self
                .permissions
                .environment(string(0)?)?
                .map(|value| Value::String(value.into()))
                .unwrap_or(Value::Undefined)),
            _ => Err(VmErr::Msg("runtime host function is not installed".into())),
        }
    }
    fn poll_host_events_bounded(&self, _: Duration, limit: usize) -> Result<Vec<HostEvent>, VmErr> {
        let mut events = Vec::new();
        // Bound even stale/duplicate completions so they cannot monopolize a turn.
        for _ in 0..limit {
            let Some((id, result)) = self.queue.next() else {
                break;
            };
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            self.cancellations.borrow_mut().remove(&id);
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            if result.is_err() {
                self.sockets.borrow_mut().remove(&id);
            }
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            let binary = self.binary_operations.borrow_mut().remove(&id);
            let promise = self.pending.borrow_mut().remove(&id);
            let Some(Value::Promise(ref promise)) = promise else {
                continue;
            };
            let (state, value) = match result {
                Ok(json) => {
                    let converted = {
                        #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
                        if binary && json.get("bytes_base64").is_some() {
                            super::network::response_bytes(&json, self.limits.file_bytes)
                                .map_err(VmErr::Msg)
                                .and_then(|bytes| {
                                    let mut json = json;
                                    if let Some(object) = json.as_object_mut() {
                                        object.remove("bytes_base64");
                                    }
                                    let value = crate::value_from_json(&json)?;
                                    value.set_prop(
                                        "bytes".into(),
                                        Value::ArrayBuffer(crate::value::Buffer::owned(bytes)),
                                    )?;
                                    Ok(value)
                                })
                        } else {
                            crate::value_from_json(&json)
                        }
                        #[cfg(not(all(feature = "runtime-net", not(target_arch = "wasm32"))))]
                        crate::value_from_json(&json)
                    };
                    match converted {
                        Ok(value) => (PromiseState::Fulfilled, value),
                        Err(error) => (
                            PromiseState::Rejected,
                            Value::String((error.to_string()).into()),
                        ),
                    }
                }
                Err(error) => (PromiseState::Rejected, Value::String((error).into())),
            };
            events.push(HostEvent::PromiseSettled {
                promise: promise.clone(),
                state,
                value,
            });
        }
        Ok(events)
    }
    fn event_wait_mode(&self) -> HostWaitMode {
        HostWaitMode::Notifications
    }
    fn set_wake_notifier(&self, notifier: WakeNotifier) {
        *self
            .queue
            .sender
            .wake
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(notifier);
    }
    fn has_pending_host_work(&self, promise: &Rc<RefCell<PromiseInner>>) -> bool {
        self.pending
            .borrow()
            .values()
            .any(|value| matches!(value, Value::Promise(p) if Rc::ptr_eq(p, promise)))
    }
}

/// Single-owner runtime. No interpreter state is shared with producer threads.
pub struct Runtime {
    interpreter: Interpreter,
    bridge: Rc<RuntimeBridge>,
    deadline: Option<std::time::Instant>,
}
impl Runtime {
    pub(super) fn from_interpreter(
        mut interpreter: Interpreter,
        permissions: Permissions,
        limits: RuntimeLimits,
        filesystem: bool,
        environment: bool,
    ) -> Result<Self, VmErr> {
        if interpreter.host.is_some() {
            return Err(VmErr::Msg(
                "Runtime owns the host bridge; use EngineBuilder for custom bridges".into(),
            ));
        }
        let bridge = Rc::new(RuntimeBridge {
            queue: ExternalEventQueue::new(limits.external_events)?,
            pending: RefCell::new(HashMap::new()),
            next_id: Cell::new(1),
            permissions,
            limits: limits.clone(),
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            workers: Arc::new(AtomicUsize::new(0)),
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            cancellations: RefCell::new(HashMap::new()),
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            sockets: RefCell::new(HashMap::new()),
            #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
            binary_operations: RefCell::new(std::collections::HashSet::new()),
        });
        interpreter.set_host_bridge(bridge.clone());
        interpreter
            .jobs
            .borrow_mut()
            .set_clock(ClockMode::RealTime(Rc::new(RealTimeClock::default())))?;
        interpreter.set_execution_budget(ExecutionBudget {
            fuel: limits.fuel,
            max_call_depth: limits.stack_depth,
            max_jobs: limits.jobs,
        });
        let mut properties = Vec::new();
        #[cfg(feature = "runtime-fs")]
        if filesystem {
            properties.push((
                "readTextFile".into(),
                Value::host_function("readTextFile", 1),
            ));
            properties.push((
                "writeTextFile".into(),
                Value::host_function("writeTextFile", 2),
            ));
        }
        #[cfg(not(feature = "runtime-fs"))]
        let _ = filesystem;
        if environment {
            properties.push(("env".into(), Value::host_function("env", 3)));
        }
        if !properties.is_empty() {
            interpreter
                .global
                .borrow_mut()
                .set("napiVm", Value::object(properties));
        }
        interpreter.jobs.borrow_mut().max_timers = Some(limits.timers);
        Ok(Self {
            interpreter,
            bridge,
            deadline: None,
        })
    }
    pub fn interpreter(&self) -> &Interpreter {
        &self.interpreter
    }
    pub fn permissions(&self) -> &Permissions {
        &self.bridge.permissions
    }
    pub fn limits(&self) -> &RuntimeLimits {
        &self.bridge.limits
    }
    #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
    pub(super) fn install_network(&mut self) {
        self.interpreter
            .global
            .borrow_mut()
            .set("__napiVmFetch", Value::host_function("fetch", 10));
        self.interpreter
            .global
            .borrow_mut()
            .set("__napiVmAbort", Value::host_function("abort", 11));
        for (name, id) in [
            ("__napiVmSocketOpen", 20),
            ("__napiVmSocketNext", 21),
            ("__napiVmSocketSend", 22),
            ("__napiVmSocketClose", 23),
            ("__napiVmSocketRelease", 24),
        ] {
            self.interpreter
                .global
                .borrow_mut()
                .set(name, Value::host_function(name, id));
        }
    }
    #[cfg(feature = "runtime-web")]
    pub(super) fn install_web_runtime(&mut self) -> Result<(), VmErr> {
        self.interpreter
            .global
            .borrow_mut()
            .set("__napiVmCopyBuffer", Value::host_function("copyBuffer", 32));
        self.interpreter
            .global
            .borrow_mut()
            .set("__napiVmRandom", Value::host_function("random", 30));
        self.interpreter
            .global
            .borrow_mut()
            .set("__napiVmDigest", Value::host_function("digest", 31));
        self.interpreter.eval_source(super::web::SOURCE)?;
        Ok(())
    }
    pub fn interpreter_mut(&mut self) -> &mut Interpreter {
        &mut self.interpreter
    }
    pub fn external_sender(&self) -> ExternalEventSender {
        self.bridge.queue.sender()
    }
    /// Register on the VM owner; complete by ID and JSON from any producer.
    pub fn external_promise(&mut self) -> Result<(u64, Value), VmErr> {
        let (id, promise) = self.bridge.external_promise()?;
        self.interpreter.republish_roots();
        Ok((id, promise))
    }
    /// Reject a cancelled operation on the owner thread; late completions are ignored.
    pub fn cancel_external(&mut self, id: u64) -> bool {
        #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
        if let Some(cancelled) = self.bridge.cancellations.borrow().get(&id) {
            cancelled.store(true, Ordering::Release);
        }
        #[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
        self.bridge.binary_operations.borrow_mut().remove(&id);
        let pending = self.bridge.pending.borrow_mut().remove(&id);
        if let Some(Value::Promise(ref promise)) = pending {
            self.interpreter
                .reject_promise(promise, Value::String("AbortError: cancelled".into()));
            self.interpreter.republish_roots();
            true
        } else {
            false
        }
    }
    pub fn eval(&mut self, source: &str) -> Result<Value, VmErr> {
        self.start_deadline()?;
        self.interpreter.eval_source_with_options(
            source,
            EvaluationOptions {
                drain: DrainPolicy::Microtasks,
                ..EvaluationOptions::default()
            },
        )
    }
    pub fn run_module(&mut self, specifier: &str) -> Result<String, VmErr> {
        self.start_deadline()?;
        self.interpreter.begin_execution();
        self.interpreter.load_module(specifier)
    }
    fn start_deadline(&mut self) -> Result<(), VmErr> {
        if self.deadline.is_none()
            && let Some(timeout) = self.bridge.limits.timeout
        {
            self.deadline = Some(
                std::time::Instant::now()
                    .checked_add(timeout)
                    .ok_or_else(|| VmErr::Msg("invalid execution timeout".into()))?,
            );
        }
        self.apply_deadline()
    }
    fn apply_deadline(&mut self) -> Result<(), VmErr> {
        if let Some(deadline) = self.deadline {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| VmErr::Msg("ResourceLimit: execution timeout exceeded".into()))?;
            self.interpreter.set_execution_timeout(Some(remaining));
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Result<TurnOutcome, VmErr> {
        self.apply_deadline()?;
        let mut outcome = self.interpreter.poll_event_loop(TurnBudget::jobs(1024))?;
        outcome.runnable |= self.bridge.queue.has_ready();
        Ok(outcome)
    }
    /// Nonblocking bounded turn; future timers and pending IO remain pending.
    pub fn run_event_loop_once(&mut self) -> Result<TurnOutcome, VmErr> {
        self.poll()
    }
    pub fn run_until_idle(&mut self) -> Result<(), VmErr> {
        loop {
            let outcome = self.poll()?;
            if !outcome.runnable {
                if outcome.next_deadline.is_none() && self.bridge.pending.borrow().is_empty() {
                    self.deadline = None;
                }
                return Ok(());
            }
        }
    }
    /// Wait for timers and registered IO. Producers never enter the VM.
    pub fn run_event_loop(&mut self) -> Result<(), VmErr> {
        loop {
            let outcome = self.poll()?;
            if !outcome.runnable
                && outcome.next_deadline.is_none()
                && self.bridge.pending.borrow().is_empty()
            {
                self.deadline = None;
                return Ok(());
            }
            if !outcome.runnable {
                #[cfg(target_arch = "wasm32")]
                return Err(VmErr::Msg(
                    "blocking runtime loop is unavailable on wasm; use poll()".into(),
                ));
                #[cfg(not(target_arch = "wasm32"))]
                self.interpreter
                    .run_event_loop_once(Duration::from_millis(50))?;
            }
        }
    }
}
