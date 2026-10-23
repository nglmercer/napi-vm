//! Independently embeddable Rust plugin authoring SDK. Importing has no process effects.
#![forbid(unsafe_code)]
use napi_vm_plugin_protocol::*;
pub use napi_vm_plugin_protocol::{Contract, PluginFuture, PluginResult, RpcError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{net::TcpStream, sync::Notify, task::JoinHandle, time::Instant};

#[derive(Clone)]
pub struct InvokeOptions {
    pub timeout: Duration,
    pub call_chain: Vec<String>,
    pub cancellation: Cancellation,
}
pub trait ClientTransport: Send + Sync + 'static {
    fn invoke(
        &self,
        contract: Contract,
        method: String,
        input: Value,
        options: InvokeOptions,
    ) -> PluginFuture<'static, Value>;
    fn emit(
        &self,
        contract: Contract,
        event: String,
        payload: Value,
        related_request: Option<String>,
    ) -> PluginFuture<'static, ()>;
    fn subscribe(
        &self,
        _contract: &Contract,
        _event: &str,
        _capacity: usize,
    ) -> PluginResult<Subscription> {
        Err(RpcError::new(
            "INVALID_ARGUMENT",
            "this context has no incoming event source",
        ))
    }
}
#[derive(Clone)]
/// Invocation context carrying inherited deadline, call chain and cooperative cancellation.
/// Child calls propagate the context; timeout never guarantees rollback of side effects.
pub struct CallContext {
    transport: Arc<dyn ClientTransport>,
    pub request_id: String,
    pub cancellation: Cancellation,
    pub call_chain: Vec<String>,
    deadline: Option<Instant>,
    timeout: Duration,
    resources: Resources,
    application: Arc<RwLock<Value>>,
}
impl CallContext {
    pub fn new(transport: Arc<dyn ClientTransport>, timeout: Duration) -> Self {
        Self {
            transport,
            request_id: String::new(),
            cancellation: Cancellation::new(),
            call_chain: Vec::new(),
            deadline: None,
            timeout,
            resources: Resources::default(),
            application: Arc::new(RwLock::new(json!({}))),
        }
    }
    pub fn remaining(&self) -> Duration {
        self.deadline.map_or(self.timeout, |deadline| {
            deadline.saturating_duration_since(Instant::now())
        })
    }
    pub fn throw_if_cancelled(&self) -> PluginResult<()> {
        self.cancellation.check()?;
        if self.remaining().is_zero() {
            Err(RpcError::new(
                "DEADLINE_EXCEEDED",
                "handler deadline expired",
            ))
        } else {
            Ok(())
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        if let Some(deadline) = self.deadline {
            self.deadline = Some(deadline.min(Instant::now() + timeout));
        }
        self
    }
    pub fn with_cancellation(mut self, cancellation: Cancellation) -> Self {
        self.cancellation = cancellation;
        self
    }
    pub fn with_resources(mut self, resources: Resources) -> Self {
        self.resources = resources;
        self
    }
    pub fn resources(&self) -> Resources {
        self.resources.clone()
    }
    /// Connection descriptor for an injected HTTP/MCP transport; credentials remain runtime-only.
    pub fn service(&self, name: &str) -> PluginResult<Value> {
        self.application.read().unwrap()["services"]
            .get(name)
            .cloned()
            .ok_or_else(|| {
                RpcError::new("NOT_READY", "requested service connection was not injected")
            })
    }
    pub fn native_dependency(&self, name: &str) -> PluginResult<String> {
        self.application.read().unwrap()["nativeDependencies"][name]["path"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| RpcError::new("NOT_READY", "native dependency path was not prepared"))
    }
    pub fn has_capability(&self, name: &str) -> bool {
        self.application.read().unwrap()["capabilities"]
            .as_array()
            .is_some_and(|v| v.iter().any(|x| x == name))
    }
    pub async fn invoke(
        &self,
        contract: &Contract,
        method: &str,
        input: Value,
    ) -> PluginResult<Value> {
        self.throw_if_cancelled()?;
        contract.input(method, &input)?;
        let result = self
            .transport
            .invoke(
                contract.clone(),
                method.into(),
                input,
                InvokeOptions {
                    timeout: self.remaining(),
                    call_chain: self.call_chain.clone(),
                    cancellation: self.cancellation.clone(),
                },
            )
            .await;
        match &result {
            Ok(v) => contract.output(method, v)?,
            Err(e) => contract.error(method, e)?,
        };
        result
    }
    pub async fn emit(&self, contract: &Contract, event: &str, payload: Value) -> PluginResult<()> {
        self.throw_if_cancelled()?;
        contract.event(event, &payload)?;
        self.transport
            .emit(
                contract.clone(),
                event.into(),
                payload,
                if self.request_id.is_empty() {
                    None
                } else {
                    Some(self.request_id.clone())
                },
            )
            .await
    }
    pub fn subscribe(
        &self,
        contract: &Contract,
        event: &str,
        capacity: usize,
    ) -> PluginResult<Subscription> {
        self.transport.subscribe(contract, event, capacity)
    }
    pub fn log(&self, message: &str) {
        eprintln!("{message}");
    }
    fn for_request(
        &self,
        id: String,
        cancellation: Cancellation,
        chain: Vec<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            transport: self.transport.clone(),
            request_id: id,
            cancellation,
            call_chain: chain,
            deadline: Some(Instant::now() + timeout),
            timeout,
            resources: self.resources.clone(),
            application: self.application.clone(),
        }
    }
}
#[derive(Clone)]
pub struct PeerTransport {
    pub peer: Peer,
    pub remote_contracts: BTreeMap<String, InterfaceIdentity>,
    pub local_contracts: BTreeMap<String, Contract>,
    pub ready: Arc<AtomicBool>,
    sequence: Arc<AtomicU64>,
    events: EventHub,
}
impl PeerTransport {
    pub fn new(
        peer: Peer,
        remote_contracts: BTreeMap<String, InterfaceIdentity>,
        local_contracts: BTreeMap<String, Contract>,
        ready: Arc<AtomicBool>,
    ) -> Self {
        Self {
            peer: peer.clone(),
            remote_contracts,
            local_contracts,
            ready,
            sequence: Arc::new(AtomicU64::new(1)),
            events: EventHub::with_limits(
                peer.limits().max_event_queue,
                peer.limits().max_queued_bytes,
            ),
        }
    }
}
impl PeerTransport {
    pub fn with_events(mut self, events: EventHub) -> Self {
        self.events = events;
        self
    }
    pub fn with_sequence(mut self, sequence: Arc<AtomicU64>) -> Self {
        self.sequence = sequence;
        self
    }
    pub fn event_hub(&self) -> EventHub {
        self.events.clone()
    }
}
impl ClientTransport for PeerTransport {
    fn subscribe(
        &self,
        contract: &Contract,
        event: &str,
        capacity: usize,
    ) -> PluginResult<Subscription> {
        if self.remote_contracts.get(contract.id()) != Some(&contract.identity()) {
            return Err(RpcError::new(
                "CONTRACT_MISMATCH",
                "event interface was not negotiated",
            ));
        }
        self.events.subscribe(
            contract,
            event,
            capacity.min(self.peer.limits().max_event_queue),
        )
    }

    fn invoke(
        &self,
        c: Contract,
        method: String,
        input: Value,
        o: InvokeOptions,
    ) -> PluginFuture<'static, Value> {
        let p = self.peer.clone();
        let negotiated = self.remote_contracts.get(c.id()) == Some(&c.identity());
        Box::pin(async move {
            if !negotiated {
                return Err(RpcError::new(
                    "CONTRACT_MISMATCH",
                    "interface was not negotiated",
                ));
            }
            if o.call_chain.contains(&p.remote_endpoint_id()) {
                return Err(RpcError::new(
                    "REENTRANT_CALL",
                    "target already active in this call chain",
                ));
            }
            let timeout = o
                .timeout
                .min(Duration::from_millis(p.limits().call_timeout_ms));
            p.request("system.invoke",json!({"interface":c.id(),"version":c.version(),"method":method,"input":input,"context":{"timeoutMs":timeout.as_millis().max(1).min(u64::MAX as u128)as u64,"callChain":o.call_chain}}),timeout,Some(o.cancellation)).await
        })
    }
    fn emit(
        &self,
        c: Contract,
        event: String,
        payload: Value,
        related: Option<String>,
    ) -> PluginFuture<'static, ()> {
        let p = self.peer.clone();
        let ready = self.ready.load(Ordering::Acquire);
        let registered = self
            .local_contracts
            .get(c.id())
            .is_some_and(|x| x.digest == c.digest);
        let sequence = self
            .sequence
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1));
        Box::pin(async move {
            if !ready {
                return Err(RpcError::new("NOT_READY", "business events require READY"));
            }
            if !registered {
                return Err(RpcError::new(
                    "CONTRACT_MISMATCH",
                    "event contract not registered",
                ));
            }
            let n =
                sequence.map_err(|_| RpcError::new("OVERLOADED", "event sequence exhausted"))?;
            let mut v = json!({"interface":c.id(),"version":c.version(),"event":event,"sessionId":p.session_id(),"sequence":n.to_string(),"payload":payload});
            if let Some(id) = related {
                v["relatedRequestId"] = Value::String(id);
            }
            p.notify("system.event", v)
        })
    }
}
type Handler = Arc<dyn Fn(Value, CallContext) -> PluginFuture<'static, Value> + Send + Sync>;
struct RegistryInner {
    contracts: RwLock<BTreeMap<String, Contract>>,
    handlers: RwLock<BTreeMap<(String, String), Handler>>,
    busy: AtomicBool,
    draining: AtomicBool,
    changed: Notify,
}
#[derive(Clone)]
/// Contract-bound handler registry, shared by clones. Registration precedes endpoint startup.
pub struct Registry(Arc<RegistryInner>);
impl Default for Registry {
    fn default() -> Self {
        Self(Arc::new(RegistryInner {
            contracts: RwLock::new(BTreeMap::new()),
            handlers: RwLock::new(BTreeMap::new()),
            busy: AtomicBool::new(false),
            draining: AtomicBool::new(false),
            changed: Notify::new(),
        }))
    }
}
struct BusyGuard(Registry);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.0.busy.store(false, Ordering::Release);
        self.0.0.changed.notify_waiters();
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Invocation {
    interface: String,
    version: String,
    method: String,
    input: Value,
    context: WireContext,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireContext {
    timeout_ms: u64,
    call_chain: Vec<String>,
}
impl Registry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(
        &self,
        contract: Contract,
        method: &str,
        handler: impl Fn(Value, CallContext) -> PluginFuture<'static, Value> + Send + Sync + 'static,
    ) -> PluginResult<()> {
        contract.method(method)?;
        let mut cs = self.0.contracts.write().unwrap();
        if cs
            .get(contract.id())
            .is_some_and(|c| c.identity() != contract.identity())
        {
            return Err(RpcError::new(
                "CONTRACT_MISMATCH",
                "different contracts share an interface ID",
            ));
        }
        let mut hs = self.0.handlers.write().unwrap();
        let key = (contract.id().to_string(), method.to_string());
        if hs.contains_key(&key) {
            return Err(RpcError::new(
                "INVALID_ARGUMENT",
                "duplicate method registration",
            ));
        }
        cs.insert(contract.id().to_string(), contract);
        hs.insert(key, Arc::new(handler));
        Ok(())
    }
    pub fn contracts(&self) -> BTreeMap<String, Contract> {
        self.0.contracts.read().unwrap().clone()
    }
    pub fn identities(&self) -> BTreeMap<String, InterfaceIdentity> {
        self.contracts()
            .into_iter()
            .map(|(k, c)| (k, c.identity()))
            .collect()
    }
    pub fn check_complete(&self) -> PluginResult<()> {
        let cs = self.0.contracts.read().unwrap();
        let hs = self.0.handlers.read().unwrap();
        for (id, c) in cs.iter() {
            for m in c.descriptor["methods"].as_object().unwrap().keys() {
                if !hs.contains_key(&(id.clone(), m.clone())) {
                    return Err(RpcError::new(
                        "INVALID_ARGUMENT",
                        format!("missing handler {id}.{m}"),
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn is_busy(&self) -> bool {
        self.0.busy.load(Ordering::Acquire)
    }
    pub fn is_draining(&self) -> bool {
        self.0.draining.load(Ordering::Acquire)
    }
    pub async fn dispatch(
        &self,
        params: Value,
        request: RequestContext,
        base: CallContext,
        endpoint: String,
        limits: Limits,
    ) -> PluginResult<Value> {
        let v: Invocation = serde_json::from_value(params)?;
        if v.context.timeout_ms == 0
            || v.context.call_chain.len() >= limits.max_call_chain_length
            || v.context.call_chain.iter().any(|s| s.len() > 132)
        {
            return Err(RpcError::new(
                "INVALID_ARGUMENT",
                "invalid invocation context",
            ));
        }
        if self.is_draining() {
            return Err(RpcError::new("NOT_READY", "endpoint is draining"));
        }
        if v.context.call_chain.contains(&endpoint) {
            return Err(RpcError::new(
                "REENTRANT_CALL",
                "endpoint already active in call chain",
            ));
        }
        let contract = self
            .0
            .contracts
            .read()
            .unwrap()
            .get(&v.interface)
            .cloned()
            .ok_or_else(|| RpcError::new("CONTRACT_MISMATCH", "unknown interface"))?;
        if contract.version() != v.version {
            return Err(RpcError::new(
                "CONTRACT_MISMATCH",
                "interface version mismatch",
            ));
        }
        contract.input(&v.method, &v.input)?;
        let handler = self
            .0
            .handlers
            .read()
            .unwrap()
            .get(&(v.interface, v.method.clone()))
            .cloned()
            .ok_or_else(|| RpcError::new("METHOD_NOT_FOUND", "no registered method"))?;
        if self
            .0
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(RpcError::new(
                "REENTRANT_CALL",
                "endpoint has an active handler; concurrent admission is rejected to avoid callback cycles",
            ));
        }
        let _guard = BusyGuard(self.clone());
        if self.is_draining() {
            return Err(RpcError::new("NOT_READY", "endpoint began draining"));
        }
        let mut chain = v.context.call_chain;
        chain.push(endpoint);
        let timeout = Duration::from_millis(v.context.timeout_ms.min(limits.call_timeout_ms));
        let context = base.for_request(request.id, request.cancellation, chain, timeout);
        context.throw_if_cancelled()?;
        let cancel = context.cancellation.clone();
        let deadline_task = tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            cancel.cancel();
        });
        let result = handler(v.input, context).await;
        deadline_task.abort();
        match &result {
            Ok(x) => contract.output(&v.method, x)?,
            Err(e) => contract.error(&v.method, e)?,
        }
        result
    }
    pub async fn quiesce(&self, timeout: Duration) -> PluginResult<()> {
        self.0.draining.store(true, Ordering::Release);
        let wait = async {
            loop {
                let n = self.0.changed.notified();
                tokio::pin!(n);
                n.as_mut().enable();
                if !self.is_busy() {
                    break;
                }
                n.await;
            }
        };
        tokio::time::timeout(timeout, wait).await.map_err(|_| {
            RpcError::new(
                "DEADLINE_EXCEEDED",
                "quiescence failed; endpoint remains DRAINING",
            )
        })
    }
}

#[derive(Clone, Default)]
/// Tracked task and cleanup ownership for quiescence and explicit shutdown.
/// Unmanaged external threads are not tracked or assumed safe to snapshot.
pub struct Resources(Arc<ResourceInner>);
type Cleanup = Box<dyn FnOnce() -> PluginFuture<'static, ()> + Send>;
#[derive(Default)]
struct ResourceInner {
    tasks: Mutex<Vec<ManagedTask>>,
    cleanups: Mutex<Vec<Cleanup>>,
    draining: AtomicBool,
}
struct ManagedTask {
    cancel: Cancellation,
    task: JoinHandle<()>,
}
impl Resources {
    pub fn spawn(
        &self,
        work: impl FnOnce(Cancellation) -> PinTask + Send + 'static,
    ) -> PluginResult<Cancellation> {
        if self.0.draining.load(Ordering::Acquire) {
            return Err(RpcError::new(
                "NOT_READY",
                "managed resources are quiescing",
            ));
        }
        let cancel = Cancellation::new();
        let mut tasks = self.0.tasks.lock().unwrap();
        if self.0.draining.load(Ordering::Acquire) {
            return Err(RpcError::new(
                "NOT_READY",
                "managed resources began quiescing",
            ));
        }
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { work(task_cancel).await });
        tasks.push(ManagedTask {
            cancel: cancel.clone(),
            task,
        });
        Ok(cancel)
    }
    pub fn cleanup(
        &self,
        f: impl FnOnce() -> PluginFuture<'static, ()> + Send + 'static,
    ) -> PluginResult<()> {
        if self.0.draining.load(Ordering::Acquire) {
            return Err(RpcError::new("NOT_READY", "resources are draining"));
        }
        let mut cleanups = self.0.cleanups.lock().unwrap();
        if self.0.draining.load(Ordering::Acquire) {
            return Err(RpcError::new("NOT_READY", "resources began draining"));
        }
        cleanups.push(Box::new(f));
        Ok(())
    }
    pub async fn quiesce(&self, timeout: Duration) -> PluginResult<()> {
        self.0.draining.store(true, Ordering::Release);
        let deadline = Instant::now() + timeout;
        let mut tasks = std::mem::take(&mut *self.0.tasks.lock().unwrap());
        for task in &tasks {
            task.cancel.cancel();
        }
        while let Some(mut task) = tasks.pop() {
            match tokio::time::timeout_at(deadline, &mut task.task).await {
                Ok(_) => {}
                Err(_) => {
                    tasks.push(task);
                    self.0.tasks.lock().unwrap().extend(tasks);
                    return Err(RpcError::new(
                        "DEADLINE_EXCEEDED",
                        "managed task ignored quiescence cancellation; snapshot is unsafe",
                    ));
                }
            }
        }
        Ok(())
    }
    /// Defensive synchronous fallback; awaited shutdown remains required for cleanup hooks.
    pub fn abort(&self) {
        self.0.draining.store(true, Ordering::Release);
        for task in self.0.tasks.lock().unwrap().drain(..) {
            task.cancel.cancel();
            task.task.abort();
        }
    }
    pub async fn shutdown(&self, timeout: Duration) -> PluginResult<()> {
        self.0.draining.store(true, Ordering::Release);
        let tasks = std::mem::take(&mut *self.0.tasks.lock().unwrap());
        for task in &tasks {
            task.cancel.cancel();
            task.task.abort();
        }
        for task in tasks {
            let _ = task.task.await;
        }
        let cleanups = std::mem::take(&mut *self.0.cleanups.lock().unwrap());
        tokio::time::timeout(timeout, async move {
            for f in cleanups {
                f().await?;
            }
            Ok(())
        })
        .await
        .map_err(|_| RpcError::new("DEADLINE_EXCEEDED", "managed cleanup timed out"))?
    }
}
pub type PinTask = std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>;
pub trait Lifecycle: Send + Sync + 'static {
    fn initialize(
        &self,
        _configuration: Value,
        _context: Value,
        snapshot: Value,
        _call: CallContext,
    ) -> PluginFuture<'_, ()> {
        Box::pin(async move {
            if !snapshot.is_null() {
                return Err(RpcError::new(
                    "STATE_INCOMPATIBLE",
                    "plugin has no restore hook",
                ));
            }
            Ok(())
        })
    }
    fn quiesce(&self, _call: CallContext) -> PluginFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn snapshot(&self, _call: CallContext) -> PluginFuture<'_, Value> {
        Box::pin(async { Ok(Value::Null) })
    }
    fn shutdown(&self, _call: CallContext) -> PluginFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}
pub struct Stateless;
impl Lifecycle for Stateless {}
#[derive(Clone)]
pub struct Plugin {
    pub registry: Registry,
    pub lifecycle: Arc<dyn Lifecycle>,
}
impl Plugin {
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            lifecycle: Arc::new(Stateless),
        }
    }
    pub fn with_lifecycle(mut self, lifecycle: impl Lifecycle) -> Self {
        self.lifecycle = Arc::new(lifecycle);
        self
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginMetadata {
    pub id: String,
    pub version: String,
    #[serde(default, rename = "requiresHost")]
    pub requires_host: Vec<Contract>,
}

/// Shared lifecycle state machine used by process serving and the in-process harness.
pub struct PluginRuntime {
    plugin: Plugin,
    base: RwLock<Option<CallContext>>,
    endpoint: String,
    limits: RwLock<Limits>,
    state: std::sync::atomic::AtomicU8,
    lifecycle_busy: AtomicBool,
    quiesced: AtomicBool,
    pub ready: Arc<AtomicBool>,
    pub resources: Resources,
}
struct LifecycleGuard<'a>(&'a AtomicBool);
impl Drop for LifecycleGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl PluginRuntime {
    pub fn new(plugin: Plugin, endpoint: String, limits: Limits) -> PluginResult<Arc<Self>> {
        plugin.registry.check_complete()?;
        Ok(Arc::new(Self {
            plugin,
            base: RwLock::new(None),
            endpoint,
            limits: RwLock::new(limits),
            state: std::sync::atomic::AtomicU8::new(0),
            lifecycle_busy: AtomicBool::new(false),
            quiesced: AtomicBool::new(false),
            ready: Arc::new(AtomicBool::new(false)),
            resources: Resources::default(),
        }))
    }
    pub fn set_limits(&self, limits: Limits) {
        *self.limits.write().unwrap() = limits;
    }
    pub fn set_context(&self, mut context: CallContext) {
        context.resources = self.resources.clone();
        *self.base.write().unwrap() = Some(context);
    }
    fn base(&self) -> PluginResult<CallContext> {
        self.base
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| RpcError::new("NOT_READY", "handshake has not completed"))
    }
    fn lifecycle_context(
        &self,
        id: String,
        cancel: Cancellation,
        timeout: Duration,
    ) -> PluginResult<CallContext> {
        let b = self.base()?;
        Ok(b.for_request(id, cancel, vec![self.endpoint.clone()], timeout))
    }
    fn validate_snapshot(&self, snapshot: &Value) -> PluginResult<()> {
        if snapshot.is_null() {
            return Ok(());
        }
        let cs = self.plugin.registry.contracts();
        let contract = cs
            .values()
            .find(|c| c.descriptor["state"]["contract"] == snapshot["contract"])
            .ok_or_else(|| {
                RpcError::new("STATE_INCOMPATIBLE", "snapshot contract not registered")
            })?;
        contract.snapshot(snapshot)
    }
    pub async fn handle(
        self: Arc<Self>,
        method: String,
        params: Value,
        request: RequestContext,
    ) -> PluginResult<Value> {
        let limits = self.limits.read().unwrap().clone();
        if method == "system.ping" {
            if params != json!({}) {
                return Err(RpcError::new(
                    "INVALID_ARGUMENT",
                    "ping parameters must be empty",
                ));
            }
            return Ok(Value::Null);
        }
        if matches!(method.as_str(), "system.snapshot" | "system.shutdown") && params != json!({}) {
            return Err(RpcError::new(
                "INVALID_ARGUMENT",
                "control parameters must be empty",
            ));
        }
        if method == "system.quiesce" {
            let o = params.as_object().ok_or_else(|| {
                RpcError::new("INVALID_ARGUMENT", "quiesce params must be object")
            })?;
            if o.keys().any(|k| k != "timeoutMs")
                || o.get("timeoutMs")
                    .is_some_and(|v| v.as_u64().is_none_or(|n| n == 0 || n > 9007199254740991))
            {
                return Err(RpcError::new(
                    "INVALID_ARGUMENT",
                    "invalid quiesce timeout/fields",
                ));
            }
        }

        if method == "system.invoke" {
            if !self.ready.load(Ordering::Acquire) {
                return Err(RpcError::new("NOT_READY", "plugin is not READY"));
            }
            return self
                .plugin
                .registry
                .dispatch(
                    params,
                    request,
                    self.base()?,
                    self.endpoint.clone(),
                    limits.clone(),
                )
                .await;
        }
        if ![
            "system.initialize",
            "system.quiesce",
            "system.snapshot",
            "system.shutdown",
        ]
        .contains(&method.as_str())
        {
            return Err(RpcError::new("METHOD_NOT_FOUND", "unknown control method"));
        }
        if self
            .lifecycle_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(RpcError::new(
                "NOT_READY",
                "lifecycle operation already active",
            ));
        }
        let _guard = LifecycleGuard(&self.lifecycle_busy);
        match method.as_str() {
            "system.initialize" => {
                if self
                    .state
                    .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return Err(RpcError::new("NOT_READY", "initialize may run only once"));
                }
                let o = params.as_object().ok_or_else(|| {
                    RpcError::new("INVALID_ARGUMENT", "initialize params must be object")
                })?;
                if o.len() != 3
                    || !o.contains_key("configuration")
                    || !params["context"].is_object()
                    || !o.contains_key("snapshot")
                {
                    self.state.store(5, Ordering::Release);
                    return Err(RpcError::new(
                        "INVALID_ARGUMENT",
                        "initialize requires configuration, context and snapshot",
                    ));
                }
                self.validate_snapshot(&params["snapshot"])?;
                *self.base()?.application.write().unwrap() = params["context"].clone();
                let timeout = Duration::from_millis(limits.startup_timeout_ms);
                let ctx = self.lifecycle_context(request.id, request.cancellation, timeout)?;
                let result = tokio::time::timeout(
                    timeout,
                    self.plugin.lifecycle.initialize(
                        params["configuration"].clone(),
                        params["context"].clone(),
                        params["snapshot"].clone(),
                        ctx,
                    ),
                )
                .await
                .map_err(|_| RpcError::new("DEADLINE_EXCEEDED", "initialize hook timed out"))?;
                if let Err(e) = result {
                    self.state.store(5, Ordering::Release);
                    return Err(e);
                }
                self.state.store(2, Ordering::Release);
                self.ready.store(true, Ordering::Release);
                Ok(Value::Null)
            }
            "system.quiesce" => {
                let s = self.state.load(Ordering::Acquire);
                if s != 2 && s != 3 {
                    return Err(RpcError::new(
                        "NOT_READY",
                        "only READY or DRAINING plugins can quiesce",
                    ));
                }
                self.state.store(3, Ordering::Release);
                self.quiesced.store(false, Ordering::Release);
                let timeout = params["timeoutMs"]
                    .as_u64()
                    .filter(|&n| n > 0)
                    .unwrap_or(limits.shutdown_timeout_ms)
                    .min(limits.call_timeout_ms);
                let timeout = Duration::from_millis(timeout);
                let deadline = Instant::now() + timeout;
                self.plugin.registry.quiesce(timeout).await?;
                let ctx = self.lifecycle_context(
                    request.id,
                    request.cancellation,
                    deadline.saturating_duration_since(Instant::now()),
                )?;
                tokio::time::timeout_at(deadline, self.plugin.lifecycle.quiesce(ctx))
                    .await
                    .map_err(|_| {
                        RpcError::new(
                            "DEADLINE_EXCEEDED",
                            "quiesce hook timed out; remains DRAINING",
                        )
                    })??;
                self.resources
                    .quiesce(deadline.saturating_duration_since(Instant::now()))
                    .await?;
                self.ready.store(false, Ordering::Release);
                self.quiesced.store(true, Ordering::Release);
                Ok(Value::Null)
            }
            "system.snapshot" => {
                if self.state.load(Ordering::Acquire) != 3
                    || !self.quiesced.load(Ordering::Acquire)
                    || self.plugin.registry.is_busy()
                {
                    return Err(RpcError::new(
                        "NOT_READY",
                        "snapshot requires successful quiescence of handlers and managed tasks",
                    ));
                }
                if params != json!({}) {
                    return Err(RpcError::new(
                        "INVALID_ARGUMENT",
                        "snapshot params must be empty",
                    ));
                }
                let timeout = Duration::from_millis(limits.call_timeout_ms);
                let ctx = self.lifecycle_context(request.id, request.cancellation, timeout)?;
                let snapshot = tokio::time::timeout(timeout, self.plugin.lifecycle.snapshot(ctx))
                    .await
                    .map_err(|_| RpcError::new("DEADLINE_EXCEEDED", "snapshot hook timed out"))??;
                if snapshot.is_null()
                    && self
                        .plugin
                        .registry
                        .contracts()
                        .values()
                        .any(|c| !c.descriptor["state"].is_null())
                {
                    return Err(RpcError::new(
                        "STATE_INCOMPATIBLE",
                        "stateful plugin must provide a snapshot",
                    ));
                }
                self.validate_snapshot(&snapshot)?;
                if serde_json::to_vec(&snapshot)?.len() > limits.max_frame_bytes / 2 {
                    return Err(RpcError::new(
                        "OVERLOADED",
                        "snapshot exceeds half-frame budget",
                    ));
                }
                Ok(snapshot)
            }
            "system.shutdown" => {
                self.ready.store(false, Ordering::Release);
                if self.state.swap(4, Ordering::AcqRel) == 4 {
                    return Ok(Value::Null);
                }
                let timeout = Duration::from_millis(limits.shutdown_timeout_ms);
                self.plugin.registry.quiesce(timeout).await?;
                let ctx = self.lifecycle_context(request.id, request.cancellation, timeout)?;
                let resource = self.resources.shutdown(timeout).await;
                let cleanup = tokio::time::timeout(timeout, self.plugin.lifecycle.shutdown(ctx))
                    .await
                    .map_err(|_| RpcError::new("DEADLINE_EXCEEDED", "shutdown hook timed out"))?;
                resource?;
                cleanup?;
                Ok(Value::Null)
            }
            _ => unreachable!(),
        }
    }
    pub async fn disconnect_cleanup(&self) {
        let limits = self.limits.read().unwrap().clone();
        self.ready.store(false, Ordering::Release);
        let timeout = Duration::from_millis(limits.shutdown_timeout_ms);
        let _ = self.resources.shutdown(timeout).await;
        if self.state.swap(4, Ordering::AcqRel) != 4
            && let Ok(ctx) =
                self.lifecycle_context("disconnect".into(), Cancellation::new(), timeout)
        {
            let _ = tokio::time::timeout(timeout, self.plugin.lifecycle.shutdown(ctx)).await;
        }
        *self.base.write().unwrap() = None;
    }
}

/// Connect using the managed environment. No listening socket or fallback standalone mode is created.
pub async fn serve(plugin: Plugin, metadata: PluginMetadata) -> PluginResult<()> {
    if !identifier(&metadata.id) || !version(&metadata.version) {
        return Err(RpcError::new(
            "INVALID_ARGUMENT",
            "invalid generated plugin metadata",
        ));
    }
    let env = |k: &str| {
        std::env::var(k).map_err(|_| {
            RpcError::new(
                "NOT_READY",
                format!(
                    "missing {k}; start this plugin through the trusted host or napi-vm-plugin dev"
                ),
            )
        })
    };
    let endpoint = env("NAPI_VM_PLUGIN_ENDPOINT")?;
    let token = env("NAPI_VM_PLUGIN_TOKEN")?;
    let instance = env("NAPI_VM_PLUGIN_INSTANCE_ID")?;
    let session = env("NAPI_VM_PLUGIN_SESSION_ID")?;
    let address: std::net::SocketAddr = endpoint.parse().map_err(|_| {
        RpcError::new(
            "INVALID_ARGUMENT",
            "managed endpoint must be numeric 127.0.0.1:port",
        )
    })?;
    if !address.ip().is_loopback() || !address.is_ipv4() {
        return Err(RpcError::new(
            "INVALID_ARGUMENT",
            "managed endpoint must use IPv4 loopback",
        ));
    }
    let limits = Limits::default();
    let stream = tokio::time::timeout(
        Duration::from_millis(limits.startup_timeout_ms),
        TcpStream::connect(address),
    )
    .await
    .map_err(|_| RpcError::new("DEADLINE_EXCEEDED", "managed host connection timed out"))??;
    let local = plugin.registry.contracts();
    let interfaces = plugin.registry.identities();
    let required: BTreeMap<_, _> = metadata
        .requires_host
        .iter()
        .map(|c| (c.id().to_string(), c.identity()))
        .collect();
    let runtime = PluginRuntime::new(plugin, format!("p:{session}"), limits.clone())?;
    let rt = runtime.clone();
    let handler: RequestHandler = Arc::new(move |m, v, c| {
        let rt = rt.clone();
        Box::pin(async move { rt.handle(m, v, c).await })
    });
    let peer = Peer::new(stream, session.clone(), 'p', limits.clone(), handler)?;
    peer.hold_events_until_reply("system.initialize");
    // Install context before hello: initialization can arrive as soon as its response is written.
    let target = PeerTransport::new(peer.clone(), required.clone(), local, runtime.ready.clone());
    let incoming = target.event_hub();
    incoming.activate_session(&session);
    let incoming_limits = incoming.clone();
    let incoming_session = session.clone();
    let incoming_contracts: BTreeMap<_, _> = metadata
        .requires_host
        .iter()
        .map(|c| (c.id().to_string(), c.clone()))
        .collect();
    peer.set_event_handler(Arc::new(move |event| {
        incoming.accept(&incoming_session, &incoming_contracts, event)
    }));
    runtime.set_context(CallContext::new(
        Arc::new(target),
        Duration::from_millis(limits.call_timeout_ms),
    ));
    let reply=peer.request("system.hello",json!({"token":token,"instanceId":instance,"sessionId":session,"pluginId":metadata.id,"pluginVersion":metadata.version,"protocol":{"major":1,"minMinor":0,"maxMinor":0},"interfaces":interfaces,"requiresHost":required,"extensions":[]}),Duration::from_millis(limits.startup_timeout_ms),None).await;
    let result=async {let reply=reply?;if reply["protocol"]!=json!({"major":1,"minor":0}){return Err(RpcError::new("PROTOCOL_MISMATCH","host selected unsupported protocol"));}let granted:BTreeMap<String,InterfaceIdentity>=serde_json::from_value(reply["interfaces"].clone())?;for(k,v)in required{if granted.get(&k)!=Some(&v){return Err(RpcError::new("CONTRACT_MISMATCH",format!("host service {k} has incompatible identity")));}}peer.negotiate_limits(serde_json::from_value(reply["limits"].clone())?)?;runtime.set_limits(peer.limits());incoming_limits.tighten_limits(&peer.limits());
        tokio::select!{_=peer.closed()=>{},r=tokio::signal::ctrl_c()=>{r.map_err(RpcError::from)?;peer.close(RpcError::new("CONNECTION_CLOSED","plugin interrupted"));}}Ok(())}.await;
    peer.close(RpcError::new("CONNECTION_CLOSED", "plugin server stopping"));
    runtime.disconnect_cleanup().await;
    result
}

/// Bounded, isolated event delivery. Session rotation purges pending old events.
#[derive(Clone)]
pub struct EventHub(Arc<EventHubInner>);
struct EventHubInner {
    subscribers: Mutex<BTreeMap<u64, Subscriber>>,
    next: AtomicU64,
    last_sequence: Mutex<BTreeMap<String, u64>>,
    active_session: RwLock<Option<String>>,
    max_events: AtomicUsize,
    max_bytes: AtomicUsize,
}
struct Subscriber {
    interface: String,
    event: String,
    state: Arc<SubscriptionState>,
}
struct SubscriptionState {
    queue: Mutex<VecDeque<(Value, usize)>>,
    bytes: AtomicUsize,
    capacity: usize,
    overflow: AtomicBool,
    closed: AtomicBool,
    changed: Notify,
}
pub struct Subscription {
    pub id: u64,
    state: Arc<SubscriptionState>,
    hub: EventHub,
}
impl Default for EventHub {
    fn default() -> Self {
        Self::with_limits(256, 16 * 1024 * 1024)
    }
}
impl Subscription {
    pub async fn recv(&mut self) -> PluginResult<Option<Value>> {
        loop {
            let notified = self.state.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.state.overflow.load(Ordering::Acquire) {
                self.state.queue.lock().unwrap().clear();
                self.state.bytes.store(0, Ordering::Release);
                return Err(RpcError::new(
                    "OVERLOADED",
                    "event subscriber overflowed and was terminated",
                ));
            }
            if let Some((v, size)) = self.state.queue.lock().unwrap().pop_front() {
                self.state.bytes.fetch_sub(size, Ordering::AcqRel);
                return Ok(Some(v));
            }
            if self.state.closed.load(Ordering::Acquire) {
                return Ok(None);
            }
            notified.await;
        }
    }
    pub fn unsubscribe(&mut self) {
        self.hub.0.subscribers.lock().unwrap().remove(&self.id);
        self.state.closed.store(true, Ordering::Release);
        self.state.queue.lock().unwrap().clear();
        self.state.bytes.store(0, Ordering::Release);
        self.state.changed.notify_waiters();
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.unsubscribe();
    }
}
impl EventHub {
    pub fn with_limits(max_events: usize, max_bytes: usize) -> Self {
        Self(Arc::new(EventHubInner {
            subscribers: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(1),
            last_sequence: Mutex::new(BTreeMap::new()),
            active_session: RwLock::new(None),
            max_events: AtomicUsize::new(max_events.max(1)),
            max_bytes: AtomicUsize::new(max_bytes.max(1)),
        }))
    }
    pub fn tighten_limits(&self, limits: &Limits) {
        self.0
            .max_events
            .fetch_min(limits.max_event_queue, Ordering::AcqRel);
        self.0
            .max_bytes
            .fetch_min(limits.max_queued_bytes, Ordering::AcqRel);
    }
    pub fn activate_session(&self, session: &str) {
        *self.0.active_session.write().unwrap() = Some(session.to_string());
        self.0.last_sequence.lock().unwrap().clear();
        for sub in self.0.subscribers.lock().unwrap().values() {
            let mut queue = sub.state.queue.lock().unwrap();
            queue.clear();
            sub.state.bytes.store(0, Ordering::Release);
            sub.state.changed.notify_waiters();
        }
    }
    pub fn subscribe(
        &self,
        contract: &Contract,
        event: &str,
        capacity: usize,
    ) -> PluginResult<Subscription> {
        if contract.descriptor["events"].get(event).is_none() || capacity == 0 || capacity > 65536 {
            return Err(RpcError::new(
                "INVALID_ARGUMENT",
                "invalid event subscription",
            ));
        }
        let mut subscribers = self.0.subscribers.lock().unwrap();
        if subscribers.len() >= 256 {
            return Err(RpcError::new(
                "OVERLOADED",
                "event subscription limit exceeded",
            ));
        }
        let id = self.0.next.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(SubscriptionState {
            queue: Mutex::new(VecDeque::new()),
            bytes: AtomicUsize::new(0),
            capacity: capacity.min(self.0.max_events.load(Ordering::Acquire)),
            overflow: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            changed: Notify::new(),
        });
        subscribers.insert(
            id,
            Subscriber {
                interface: contract.id().into(),
                event: event.into(),
                state: state.clone(),
            },
        );
        Ok(Subscription {
            id,
            state,
            hub: self.clone(),
        })
    }
    pub fn accept(
        &self,
        session: &str,
        contracts: &BTreeMap<String, Contract>,
        event: Value,
    ) -> PluginResult<()> {
        let o = event
            .as_object()
            .ok_or_else(|| RpcError::new("INVALID_ARGUMENT", "event must be object"))?;
        if o.keys().any(|k| {
            ![
                "interface",
                "version",
                "event",
                "sessionId",
                "sequence",
                "payload",
                "relatedRequestId",
            ]
            .contains(&k.as_str())
        }) || !o.contains_key("payload")
        {
            return Err(RpcError::new("INVALID_ARGUMENT", "invalid event envelope"));
        }
        if event["sessionId"] != session
            || self
                .0
                .active_session
                .read()
                .unwrap()
                .as_ref()
                .is_some_and(|s| s != session)
        {
            return Err(RpcError::new("INVALID_REQUEST", "stale event session"));
        }
        let interface = event["interface"].as_str().unwrap_or("");
        let name = event["event"].as_str().unwrap_or("");
        let c = contracts
            .get(interface)
            .ok_or_else(|| RpcError::new("CONTRACT_MISMATCH", "event interface not negotiated"))?;
        if event["version"] != c.version() {
            return Err(RpcError::new("CONTRACT_MISMATCH", "event version mismatch"));
        }
        c.event(name, &event["payload"])?;
        let sequence = event["sequence"]
            .as_str()
            .filter(|s| *s != "0" && canonical_decimal(s, false))
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| {
                RpcError::new(
                    "INVALID_ARGUMENT",
                    "event sequence must be positive canonical u64 string",
                )
            })?;
        let mut last = self.0.last_sequence.lock().unwrap();
        if last.get(session).is_some_and(|&n| sequence <= n) {
            return Err(RpcError::new(
                "INVALID_REQUEST",
                "event sequence did not increase",
            ));
        }
        last.insert(session.into(), sequence);
        drop(last);
        let size = serde_json::to_vec(&event)?.len();
        let mut subscribers = self.0.subscribers.lock().unwrap();
        if self
            .0
            .active_session
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|s| s != session)
        {
            return Err(RpcError::new(
                "INVALID_REQUEST",
                "event belongs to replaced session",
            ));
        }
        subscribers.retain(|_, sub| {
            if sub.interface != interface || sub.event != name {
                return true;
            }
            let mut queue = sub.state.queue.lock().unwrap();
            if queue.len()
                >= sub
                    .state
                    .capacity
                    .min(self.0.max_events.load(Ordering::Acquire))
                || sub.state.bytes.load(Ordering::Acquire).saturating_add(size)
                    > self.0.max_bytes.load(Ordering::Acquire)
            {
                sub.state.overflow.store(true, Ordering::Release);
                sub.state.closed.store(true, Ordering::Release);
                queue.clear();
                sub.state.bytes.store(0, Ordering::Release);
                sub.state.changed.notify_waiters();
                return false;
            }
            sub.state.bytes.fetch_add(size, Ordering::AcqRel);
            queue.push_back((event.clone(), size));
            sub.state.changed.notify_one();
            true
        });
        Ok(())
    }
    pub fn forget_session(&self, session: &str) {
        self.0.last_sequence.lock().unwrap().remove(session);
        let current = self.0.active_session.read().unwrap().as_deref() == Some(session);
        if current {
            self.activate_session("");
        }
    }
    pub fn close(&self) {
        for (_, s) in std::mem::take(&mut *self.0.subscribers.lock().unwrap()) {
            s.state.closed.store(true, Ordering::Release);
            s.state.queue.lock().unwrap().clear();
            s.state.bytes.store(0, Ordering::Release);
            s.state.changed.notify_waiters();
        }
        self.0.last_sequence.lock().unwrap().clear();
    }
}

/// Process-free test harness using the same validators, dispatch gate and lifecycle hooks.
pub struct Harness {
    runtime: Arc<PluginRuntime>,
    client: CallContext,
    _host_context: Arc<RwLock<Option<CallContext>>>,
    events: EventHub,
    counter: AtomicU64,
}
struct HarnessPluginTarget {
    runtime: std::sync::Weak<PluginRuntime>,
}
impl ClientTransport for HarnessPluginTarget {
    fn invoke(
        &self,
        c: Contract,
        m: String,
        input: Value,
        o: InvokeOptions,
    ) -> PluginFuture<'static, Value> {
        let rt = self.runtime.upgrade();
        Box::pin(async move {
            let rt = rt.ok_or_else(|| RpcError::new("CONNECTION_CLOSED", "harness closed"))?;
            rt.handle("system.invoke".into(),json!({"interface":c.id(),"version":c.version(),"method":m,"input":input,"context":{"timeoutMs":o.timeout.as_millis().max(1)as u64,"callChain":o.call_chain}}),RequestContext{id:"h:harness:invoke".into(),cancellation:o.cancellation}).await
        })
    }
    fn emit(
        &self,
        _: Contract,
        _: String,
        _: Value,
        _: Option<String>,
    ) -> PluginFuture<'static, ()> {
        Box::pin(async {
            Err(RpcError::new(
                "INVALID_ARGUMENT",
                "host application context cannot emit plugin events",
            ))
        })
    }
}
struct HarnessHostTarget {
    registry: Registry,
    base: std::sync::Weak<RwLock<Option<CallContext>>>,
    hub: EventHub,
    contracts: BTreeMap<String, Contract>,
    sequence: AtomicU64,
    ready: Arc<AtomicBool>,
}
impl ClientTransport for HarnessHostTarget {
    fn invoke(
        &self,
        c: Contract,
        m: String,
        input: Value,
        o: InvokeOptions,
    ) -> PluginFuture<'static, Value> {
        let registry = self.registry.clone();
        let base = self.base.upgrade().and_then(|c| c.read().unwrap().clone());
        Box::pin(async move {
            let base =
                base.ok_or_else(|| RpcError::new("CONNECTION_CLOSED", "harness host unavailable"))?;
            registry.dispatch(json!({"interface":c.id(),"version":c.version(),"method":m,"input":input,"context":{"timeoutMs":o.timeout.as_millis().max(1)as u64,"callChain":o.call_chain}}),RequestContext{id:"p:harness:callback".into(),cancellation:o.cancellation},base,"h:harness".into(),Limits::default()).await
        })
    }
    fn emit(
        &self,
        c: Contract,
        e: String,
        p: Value,
        r: Option<String>,
    ) -> PluginFuture<'static, ()> {
        let h = self.hub.clone();
        let cs = self.contracts.clone();
        let n = self.sequence.fetch_add(1, Ordering::Relaxed);
        let ready = self.ready.load(Ordering::Acquire);
        Box::pin(async move {
            if !ready {
                return Err(RpcError::new("NOT_READY", "plugin not ready"));
            }
            let mut v = json!({"interface":c.id(),"version":c.version(),"event":e,"sessionId":"harness","sequence":n.to_string(),"payload":p});
            if let Some(r) = r {
                v["relatedRequestId"] = r.into();
            }
            h.accept("harness", &cs, v)
        })
    }
}
impl Harness {
    pub fn new(plugin: Plugin, host_services: Registry) -> PluginResult<Self> {
        host_services.check_complete()?;
        let contracts = plugin.registry.contracts();
        let runtime = PluginRuntime::new(plugin, "p:harness".into(), Limits::default())?;
        let target = Arc::new(HarnessPluginTarget {
            runtime: Arc::downgrade(&runtime),
        });
        let client = CallContext::new(target, Duration::from_secs(30));
        let host_context = Arc::new(RwLock::new(Some(client.clone())));
        let events = EventHub::default();
        let host = HarnessHostTarget {
            registry: host_services,
            base: Arc::downgrade(&host_context),
            hub: events.clone(),
            contracts,
            sequence: AtomicU64::new(1),
            ready: runtime.ready.clone(),
        };
        runtime.set_context(CallContext::new(Arc::new(host), Duration::from_secs(30)));
        Ok(Self {
            runtime,
            client,
            _host_context: host_context,
            events,
            counter: AtomicU64::new(0),
        })
    }
    async fn control(&self, m: &str, v: Value) -> PluginResult<Value> {
        self.runtime
            .clone()
            .handle(
                m.into(),
                v,
                RequestContext {
                    id: format!("h:harness:{}", self.counter.fetch_add(1, Ordering::Relaxed)),
                    cancellation: Cancellation::new(),
                },
            )
            .await
    }
    pub async fn initialize(&self, configuration: Value, snapshot: Value) -> PluginResult<()> {
        self.control(
            "system.initialize",
            json!({"configuration":configuration,"context":{},"snapshot":snapshot}),
        )
        .await?;
        Ok(())
    }
    pub async fn invoke(&self, c: &Contract, m: &str, v: Value) -> PluginResult<Value> {
        self.client
            .clone()
            .with_timeout(Duration::from_secs(30))
            .invoke(c, m, v)
            .await
    }
    pub fn context(&self) -> CallContext {
        self.client.clone().with_timeout(Duration::from_secs(30))
    }
    pub fn subscribe(
        &self,
        c: &Contract,
        event: &str,
        capacity: usize,
    ) -> PluginResult<Subscription> {
        self.events.subscribe(c, event, capacity)
    }
    pub async fn snapshot(&self) -> PluginResult<Value> {
        self.control("system.quiesce", json!({"timeoutMs":5000}))
            .await?;
        self.control("system.snapshot", json!({})).await
    }
    pub async fn shutdown(&self) -> PluginResult<()> {
        self.control("system.shutdown", json!({})).await?;
        self.events.close();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn initialize_task_event_waits_for_reply_without_delaying_ready_or_task_completion() {
        use tokio::{io::AsyncWriteExt, net::TcpListener, sync::oneshot};
        let contract = Contract::from_value(
            serde_json::from_str(include_str!(
                "../../../contracts/trusted-plugins/generated/counter.contract.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let registry = Registry::new();
        for method in ["get", "add"] {
            registry
                .register(contract.clone(), method, |_, _| {
                    Box::pin(async { Ok(json!({"count":"1"})) })
                })
                .unwrap();
        }
        let runtime = PluginRuntime::new(
            Plugin::new(registry),
            "p:sdk-fence".into(),
            Limits::default(),
        )
        .unwrap();
        let initialized = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let handler_runtime = runtime.clone();
        let handler_initialized = initialized.clone();
        let handler_release = release.clone();
        let handler: RequestHandler = Arc::new(move |method, params, request| {
            let runtime = handler_runtime.clone();
            let initialized = handler_initialized.clone();
            let release = handler_release.clone();
            Box::pin(async move {
                let initialize = method == "system.initialize";
                let result = runtime.handle(method, params, request).await;
                if initialize {
                    initialized.notify_one();
                    release.notified().await;
                }
                result
            })
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut host, _) = listener.accept().await.unwrap();
        let peer = Peer::new(stream, "sdk-fence".into(), 'p', Limits::default(), handler).unwrap();
        peer.hold_events_until_reply("system.initialize");
        let transport = PeerTransport::new(
            peer.clone(),
            BTreeMap::new(),
            BTreeMap::from([(contract.id().into(), contract.clone())]),
            runtime.ready.clone(),
        );
        runtime.set_context(CallContext::new(
            Arc::new(transport),
            Duration::from_secs(1),
        ));
        let initialize = json!({"jsonrpc":"2.0","id":"h:sdk-fence:1","method":"system.initialize","params":{"configuration":{},"context":{},"snapshot":null}});
        host.write_all(&encode_frame(&initialize, 4096, 64).unwrap())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), initialized.notified())
            .await
            .unwrap();
        assert!(runtime.ready.load(Ordering::Acquire));
        let context = runtime.base().unwrap();
        let event_contract = contract.clone();
        let (completed, completion) = oneshot::channel();
        runtime
            .resources
            .spawn(move |_| {
                Box::pin(async move {
                    let result = context
                        .emit(&event_contract, "changed", json!({"count":"1"}))
                        .await;
                    let _ = completed.send(result);
                })
            })
            .unwrap();
        // The tracked task can finish while its event is still fenced, so cleanup
        // cannot deadlock waiting for the initialize response to be queued.
        tokio::time::timeout(Duration::from_secs(1), completion)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        release.notify_one();
        let reply = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut host, 4096, 64))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply["id"], "h:sdk-fence:1");
        assert_eq!(reply["result"], Value::Null);
        let invoke = json!({"jsonrpc":"2.0","id":"h:sdk-fence:2","method":"system.invoke","params":{"interface":contract.id(),"version":contract.version(),"method":"get","input":{},"context":{"timeoutMs":1000,"callChain":[]}}});
        host.write_all(&encode_frame(&invoke, 4096, 64).unwrap())
            .await
            .unwrap();
        let mut got_event = false;
        let mut got_result = false;
        for _ in 0..2 {
            let frame =
                tokio::time::timeout(Duration::from_secs(1), read_frame(&mut host, 4096, 64))
                    .await
                    .unwrap()
                    .unwrap();
            if frame["method"] == "system.event" {
                got_event = true;
            } else {
                assert_eq!(frame["result"]["count"], "1");
                got_result = true;
            }
        }
        assert!(got_event && got_result);
        peer.close(RpcError::new("CONNECTION_CLOSED", "test finished"));
        runtime.disconnect_cleanup().await;
        assert!(!runtime.ready.load(Ordering::Acquire));
    }
    fn c() -> Contract {
        Contract::from_value(
            serde_json::from_str(include_str!(
                "../../../contracts/trusted-plugins/generated/greeter.contract.json"
            ))
            .unwrap(),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn harness_validates_and_lifecycle() {
        let r = Registry::new();
        r.register(c(), "greet", |v, _| {
            Box::pin(async move {
                Ok(json!({"message":format!("Hola, {}",v["name"].as_str().unwrap())}))
            })
        })
        .unwrap();
        let h = Harness::new(Plugin::new(r), Registry::new()).unwrap();
        assert_eq!(
            h.invoke(&c(), "greet", json!({"name":"Ana"}))
                .await
                .unwrap_err()
                .stable_code(),
            "NOT_READY"
        );
        h.initialize(json!({}), Value::Null).await.unwrap();
        assert_eq!(
            h.invoke(&c(), "greet", json!({"name":"Ana"}))
                .await
                .unwrap(),
            json!({"message":"Hola, Ana"})
        );
        assert!(h.invoke(&c(), "greet", json!({"name":""})).await.is_err());
        assert!(h.initialize(json!({}), Value::Null).await.is_err());
        assert!(h.snapshot().await.unwrap().is_null());
        h.shutdown().await.unwrap();
        h.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn uncooperative_managed_task_blocks_snapshot() {
        let resources = Resources::default();
        resources
            .spawn(|_| Box::pin(async { std::future::pending::<()>().await }))
            .unwrap();
        assert_eq!(
            resources
                .quiesce(Duration::from_millis(5))
                .await
                .unwrap_err()
                .stable_code(),
            "DEADLINE_EXCEEDED"
        );
        resources.shutdown(Duration::from_secs(1)).await.unwrap();
    }
    #[tokio::test]
    async fn saturated_subscriber_observable() {
        let c = Contract::from_value(
            serde_json::from_str(include_str!(
                "../../../contracts/trusted-plugins/generated/counter.contract.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let hub = EventHub::default();
        let mut sub = hub.subscribe(&c, "changed", 1).unwrap();
        let cs = BTreeMap::from([(c.id().to_string(), c)]);
        for n in 1..3 {
            hub.accept("s",&cs,json!({"interface":"example.counter","version":"1.0.0","event":"changed","sessionId":"s","sequence":n.to_string(),"payload":{"count":"1"}})).unwrap();
        }
        assert_eq!(sub.recv().await.unwrap_err().stable_code(), "OVERLOADED");
    }
}

#[cfg(test)]
mod dispatch_regressions {
    use super::*;
    fn contract(name: &str) -> Contract {
        let s = match name {
            "greeter" => {
                include_str!("../../../contracts/trusted-plugins/generated/greeter.contract.json")
            }
            "configuration" => include_str!(
                "../../../contracts/trusted-plugins/generated/app-configuration.contract.json"
            ),
            _ => include_str!("../../../contracts/trusted-plugins/generated/counter.contract.json"),
        };
        Contract::from_value(serde_json::from_str(s).unwrap()).unwrap()
    }
    #[tokio::test]
    async fn bound_and_independent_callback_cycles_reject_promptly() {
        for unbound in [false, true] {
            let plugin = Registry::new();
            plugin
                .register(contract("greeter"), "greet", |_, ctx| {
                    Box::pin(async move {
                        ctx.invoke(&contract("configuration"), "get", json!({"key":"prefix"}))
                            .await?;
                        Ok(json!({"message":"never"}))
                    })
                })
                .unwrap();
            let global: Arc<Mutex<Option<CallContext>>> = Arc::new(Mutex::new(None));
            let copy = global.clone();
            let host = Registry::new();
            host.register(contract("configuration"), "get", move |_, ctx| {
                let global = copy.clone();
                Box::pin(async move {
                    let context = if unbound {
                        global.lock().unwrap().clone().unwrap()
                    } else {
                        ctx
                    };
                    context
                        .invoke(&contract("greeter"), "greet", json!({"name":"cycle"}))
                        .await?;
                    Ok(json!({"value":"never"}))
                })
            })
            .unwrap();
            let h = Harness::new(Plugin::new(plugin), host).unwrap();
            *global.lock().unwrap() = Some(h.context());
            h.initialize(json!({}), Value::Null).await.unwrap();
            let error = tokio::time::timeout(
                Duration::from_millis(200),
                h.invoke(&contract("greeter"), "greet", json!({"name":"outer"})),
            )
            .await
            .unwrap()
            .unwrap_err();
            assert_eq!(error.stable_code(), "REENTRANT_CALL");
            h.shutdown().await.unwrap();
        }
    }
    #[tokio::test]
    async fn old_queued_events_are_purged_without_breaking_subscription() {
        let c = contract("counter");
        let hub = EventHub::default();
        hub.activate_session("old");
        let mut s = hub.subscribe(&c, "changed", 1).unwrap();
        let cs = BTreeMap::from([(c.id().into(), c)]);
        hub.accept("old",&cs,json!({"interface":"example.counter","version":"1.0.0","event":"changed","sessionId":"old","sequence":"1","payload":{"count":"1"}})).unwrap();
        hub.activate_session("new");
        hub.accept("new",&cs,json!({"interface":"example.counter","version":"1.0.0","event":"changed","sessionId":"new","sequence":"1","payload":{"count":"2"}})).unwrap();
        assert_eq!(s.recv().await.unwrap().unwrap()["sessionId"], "new");
    }
}

#[cfg(test)]
mod resource_registration_regression {
    use super::*;
    #[tokio::test]
    async fn factory_runs_only_after_tracking_and_quiesce_waits_for_it() {
        let resources = Resources::default();
        let returned = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(AtomicBool::new(false));
        let a = returned.clone();
        let b = observed.clone();
        resources
            .spawn(move |cancel| {
                b.store(a.load(Ordering::Acquire), Ordering::Release);
                Box::pin(async move {
                    cancel.cancelled().await;
                })
            })
            .unwrap();
        assert_eq!(resources.0.tasks.lock().unwrap().len(), 1);
        returned.store(true, Ordering::Release);
        resources.quiesce(Duration::from_secs(1)).await.unwrap();
        assert!(observed.load(Ordering::Acquire));
        assert!(resources.0.tasks.lock().unwrap().is_empty());
        assert!(resources.spawn(|_| Box::pin(async {})).is_err());
        assert!(resources.cleanup(|| Box::pin(async { Ok(()) })).is_err());
    }
}

#[cfg(test)]
mod draining_event_regression {
    use super::*;
    use tokio::sync::Notify;
    struct Finished(Arc<AtomicBool>);
    impl Lifecycle for Finished {
        fn snapshot(&self, _: CallContext) -> PluginFuture<'_, Value> {
            Box::pin(async move {
                if !self.0.load(Ordering::Acquire) {
                    return Err(RpcError::new(
                        "INTERNAL_ERROR",
                        "snapshot preceded handler completion",
                    ));
                }
                Ok(
                    json!({"stateVersion":1,"contract":"example.counter.state","data":{"count":"1"}}),
                )
            })
        }
    }
    #[tokio::test]
    async fn inflight_handler_can_emit_while_quiescing_before_snapshot() {
        let c = Contract::from_value(
            serde_json::from_str(include_str!(
                "../../../contracts/trusted-plugins/generated/counter.contract.json"
            ))
            .unwrap(),
        )
        .unwrap();
        let registry = Registry::new();
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let (e, r, f, event) = (
            entered.clone(),
            release.clone(),
            finished.clone(),
            c.clone(),
        );
        registry
            .register(c.clone(), "add", move |_, ctx| {
                let (e, r, f, event) = (e.clone(), r.clone(), f.clone(), event.clone());
                Box::pin(async move {
                    e.notify_one();
                    r.notified().await;
                    ctx.emit(&event, "changed", json!({"count":"1"})).await?;
                    f.store(true, Ordering::Release);
                    Ok(json!({"count":"1"}))
                })
            })
            .unwrap();
        registry
            .register(c.clone(), "get", |_, _| {
                Box::pin(async { Ok(json!({"count":"1"})) })
            })
            .unwrap();
        let h = Harness::new(
            Plugin::new(registry).with_lifecycle(Finished(finished)),
            Registry::new(),
        )
        .unwrap();
        h.initialize(json!({}), Value::Null).await.unwrap();
        let (called, snapshot, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                h.invoke(&c, "add", json!({"amount":1})),
                async {
                    entered.notified().await;
                    h.snapshot().await
                },
                async {
                    while h.runtime.state.load(Ordering::Acquire) != 3 {
                        tokio::task::yield_now().await;
                    }
                    release.notify_one();
                }
            )
        })
        .await
        .unwrap();
        assert_eq!(called.unwrap(), json!({"count":"1"}));
        assert_eq!(snapshot.unwrap()["data"]["count"], "1");
        h.shutdown().await.unwrap();
    }
}
