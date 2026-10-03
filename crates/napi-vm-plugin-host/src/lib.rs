#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
mod manifest;
pub mod services;
pub use manifest::*;
use napi_vm_plugin_protocol::*;
pub use napi_vm_plugin_protocol::{Contract, Limits};
use napi_vm_plugin_sdk::{
    CallContext, ClientTransport, EventHub, InvokeOptions, PeerTransport, Registry, Resources,
    Subscription,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use services::{ManagedService, ManagedServiceSpec};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex, RwLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    net::TcpListener,
    process::{Child, Command},
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::Instant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
/// Observable instance lifecycle state. Business calls are accepted only in `Ready`.
pub enum Status {
    Discovered,
    Starting,
    Ready,
    Draining,
    Stopping,
    Stopped,
    Failed,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Serializable noncredential instance status and selected runtime information.
pub struct InstanceMetadata {
    pub instance_id: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub status: Status,
    pub session_id: Option<String>,
    pub selected_runtime: Option<String>,
    pub runtime_version: Option<String>,
    pub failure: Option<RpcError>,
}
/// Runtime-only connection credentials. Never serialized into package metadata or snapshots.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConnection {
    pub url: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}
#[derive(Clone)]
/// Per-load runtime selection, runtime-only credentials and initialization values.
/// The default verifies package integrity; native launches require no JavaScript runtime.
pub struct LoadOptions {
    pub runtime: RuntimeOptions,
    pub environment: BTreeMap<String, String>,
    pub configuration: Value,
    pub context: Value,
    pub services: BTreeMap<String, ServiceConnection>,
}
impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            runtime: RuntimeOptions {
                verify_integrity: true,
                ..Default::default()
            },
            environment: BTreeMap::new(),
            configuration: json!({}),
            context: json!({}),
            services: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Default)]
/// Replacement options for a stable instance. State transfer requires successful quiescence
/// and a compatible snapshot unless `force_without_state` is explicitly set.
pub struct ReloadOptions {
    pub replacement_manifest: Option<PathBuf>,
    pub force_without_state: bool,
    pub runtime: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct LogChunk {
    pub stream: String,
    pub text: String,
}
#[derive(Default)]
struct Logs {
    chunks: VecDeque<LogChunk>,
    bytes: usize,
    dropped: u64,
}
impl Logs {
    fn push(&mut self, stream: &str, bytes: &[u8], limit: usize, token: &str) {
        let mut text = String::from_utf8_lossy(bytes).replace(token, "[bootstrap token redacted]");
        if text.len() > limit {
            let mut start = text.len() - limit;
            while !text.is_char_boundary(start) {
                start += 1;
            }
            self.dropped += start as u64;
            text = text.split_off(start);
        }
        let n = text.len();
        while self.bytes + n > limit {
            if let Some(v) = self.chunks.pop_front() {
                self.bytes -= v.text.len();
                self.dropped += v.text.len() as u64;
            } else {
                break;
            }
        }
        if n > 0 {
            self.bytes += n;
            self.chunks.push_back(LogChunk {
                stream: stream.into(),
                text,
            });
        }
    }
}
struct Record {
    metadata: InstanceMetadata,
    prepared: Prepared,
    session: Option<Arc<Session>>,
    snapshot: Value,
    options: LoadOptions,
    history: Vec<Status>,
}
struct Instance {
    record: Mutex<Record>,
    lifecycle_busy: AtomicBool,
    events: EventHub,
    logs: Arc<Mutex<Logs>>,
    secrets: Mutex<Vec<String>>,
    services: tokio::sync::Mutex<Vec<ManagedService>>,
}
struct LifecycleLease(Arc<Instance>);
impl Drop for LifecycleLease {
    fn drop(&mut self) {
        self.0.lifecycle_busy.store(false, Ordering::Release);
    }
}
impl Instance {
    fn lease(self: &Arc<Self>) -> PluginResult<LifecycleLease> {
        if self
            .lifecycle_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(RpcError::new(
                "NOT_READY",
                "another lifecycle operation is active",
            ));
        }
        Ok(LifecycleLease(self.clone()))
    }
    fn transition(&self, status: Status) {
        let mut r = self.record.lock().unwrap();
        r.metadata.status = status;
        r.history.push(status);
        if r.history.len() > 256 {
            r.history.remove(0);
        }
    }
}
struct Session {
    peer: Peer,
    process: ProcessOwner,
    id: String,
    event_sequence: Arc<AtomicU64>,
}
struct ProcessOwner {
    pid: Option<u32>,
    stop: mpsc::Sender<oneshot::Sender<PluginResult<()>>>,
}
impl ProcessOwner {
    async fn stop(&self) -> PluginResult<()> {
        let (tx, rx) = oneshot::channel();
        if self.stop.send(tx).await.is_err() {
            return Ok(());
        }
        rx.await.unwrap_or(Ok(()))
    }
}
struct HostInner {
    limits: Limits,
    services: Registry,
    instances: RwLock<BTreeMap<String, Arc<Instance>>>,
    shutting_down: AtomicBool,
    startup_gate: tokio::sync::RwLock<()>,
    resources: Resources,
}
impl Drop for HostInner {
    fn drop(&mut self) {
        self.resources.abort();
        for i in self.instances.read().unwrap().values() {
            if let Some(s) = &i.record.lock().unwrap().session {
                s.peer.close(RpcError::new(
                    "CONNECTION_CLOSED",
                    "host dropped; explicit shutdown is preferred",
                ));
            }
        }
    }
}
#[derive(Clone)]
/// Cloneable Tokio host owning direct plugin children and registered callback handlers.
/// Keep this alive until explicit asynchronous shutdown completes.
pub struct Host {
    inner: Arc<HostInner>,
}
#[derive(Clone)]
/// A stable instance handle. Reload replaces its session while retaining this identity.
pub struct PluginHandle {
    host: Weak<HostInner>,
    instance: Arc<Instance>,
    limits: Limits,
}
impl Default for Host {
    fn default() -> Self {
        Self::new(Limits::default()).expect("default limits are valid")
    }
}
impl Host {
    pub fn new(limits: Limits) -> PluginResult<Self> {
        limits.validate()?;
        Ok(Self {
            inner: Arc::new(HostInner {
                limits,
                services: Registry::new(),
                instances: RwLock::new(BTreeMap::new()),
                shutting_down: AtomicBool::new(false),
                startup_gate: tokio::sync::RwLock::new(()),
                resources: Resources::default(),
            }),
        })
    }
    pub fn register(
        &self,
        contract: Contract,
        method: &str,
        handler: impl Fn(Value, CallContext) -> PluginFuture<'static, Value> + Send + Sync + 'static,
    ) -> PluginResult<()> {
        if !self.inner.instances.read().unwrap().is_empty() {
            return Err(RpcError::new(
                "NOT_READY",
                "register host services before loading plugins",
            ));
        }
        self.inner.services.register(contract, method, handler)
    }
    pub fn list(&self) -> Vec<InstanceMetadata> {
        self.inner
            .instances
            .read()
            .unwrap()
            .values()
            .map(|i| i.record.lock().unwrap().metadata.clone())
            .collect()
    }
    pub fn get(&self, id: &str) -> Option<PluginHandle> {
        self.inner
            .instances
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .map(|instance| PluginHandle {
                host: Arc::downgrade(&self.inner),
                instance,
                limits: self.inner.limits.clone(),
            })
    }
    pub async fn load(
        &self,
        path: impl AsRef<Path>,
        options: LoadOptions,
    ) -> PluginResult<PluginHandle> {
        self.load_with_services(path, options, Vec::new()).await
    }
    pub async fn load_with_services(
        &self,
        path: impl AsRef<Path>,
        mut options: LoadOptions,
        service_specs: Vec<ManagedServiceSpec>,
    ) -> PluginResult<PluginHandle> {
        // Shutdown closes admission, then waits for admitted starts to finish.
        // This covers preparation before the instance enters the registry too.
        let _startup = self.inner.startup_gate.read().await;
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(RpcError::new("NOT_READY", "host is shutting down"));
        }
        self.inner.services.check_complete()?;
        let prepared = prepare(path.as_ref(), &options.runtime).await?;
        // Preflight every sidecar before any sidecar or plugin executes.
        let mut names = std::collections::BTreeSet::new();
        for s in &service_specs {
            if !names.insert(s.name.clone())
                || options.services.contains_key(&s.name)
                || !prepared
                    .manifest
                    .dependencies
                    .services
                    .iter()
                    .any(|r| r.name == s.name)
            {
                return Err(RpcError::new(
                    "INVALID_ARGUMENT",
                    "managed service must be a unique declared requirement",
                ));
            }
            let requirement = prepared
                .manifest
                .dependencies
                .services
                .iter()
                .find(|r| r.name == s.name)
                .unwrap();
            if requirement.owner.as_deref() != Some("host")
                || requirement.readiness.as_ref().is_none_or(|r| {
                    r.path != s.readiness.path || r.status != s.readiness.expected_status
                })
                || requirement.shutdown.as_deref() != Some("terminate")
            {
                return Err(RpcError::new(
                    "INVALID_ARGUMENT",
                    "managed service adapter must match declared owner/readiness/shutdown",
                ));
            }
            s.preflight().await?;
            options.services.insert(
                s.name.clone(),
                ServiceConnection {
                    url: s.endpoint.clone(),
                    headers: BTreeMap::new(),
                },
            );
        }
        if prepared
            .manifest
            .dependencies
            .services
            .iter()
            .any(|r| r.owner.as_deref() == Some("host") && !names.contains(&r.name))
        {
            return Err(RpcError::new(
                "NOT_READY",
                "host-owned service requires an explicit managed service adapter",
            ));
        }
        preflight_requirements(&prepared, &self.inner.services, &options)?;
        let id = random_hex(16)?;
        let instance = Arc::new(Instance {
            record: Mutex::new(Record {
                metadata: InstanceMetadata {
                    instance_id: id.clone(),
                    plugin_id: prepared.manifest.id.clone(),
                    plugin_version: prepared.manifest.version.clone(),
                    status: Status::Discovered,
                    session_id: None,
                    selected_runtime: None,
                    runtime_version: None,
                    failure: None,
                },
                prepared: prepared.clone(),
                session: None,
                snapshot: Value::Null,
                options: options.clone(),
                history: vec![Status::Discovered],
            }),
            lifecycle_busy: AtomicBool::new(false),
            events: EventHub::with_limits(
                self.inner.limits.max_event_queue,
                self.inner.limits.max_queued_bytes,
            ),
            logs: Arc::new(Mutex::new(Logs::default())),
            secrets: Mutex::new(Vec::new()),
            services: tokio::sync::Mutex::new(Vec::new()),
        });
        self.inner
            .instances
            .write()
            .unwrap()
            .insert(id.clone(), instance.clone());
        let handle = PluginHandle {
            host: Arc::downgrade(&self.inner),
            instance: instance.clone(),
            limits: self.inner.limits.clone(),
        };
        let _lease = instance.lease()?;
        let result = async {
            let mut started = Vec::new();
            for spec in service_specs {
                match ManagedService::start(spec).await {
                    Ok(s) => started.push(s),
                    Err(e) => {
                        for s in &mut started {
                            let _ = s.shutdown().await;
                        }
                        return Err(e);
                    }
                }
            }
            *instance.services.lock().await = started;
            self.start(instance.clone(), prepared, options, Value::Null)
                .await
        }
        .await;
        if let Err(e) = result {
            set_failed(&instance, e.clone());
            shutdown_services(&instance).await;
            return Err(sanitize_error(&instance, e));
        }
        Ok(handle)
    }
    async fn start(
        &self,
        instance: Arc<Instance>,
        prepared: Prepared,
        options: LoadOptions,
        snapshot: Value,
    ) -> PluginResult<()> {
        instance.transition(Status::Starting);
        let session_id = random_hex(16)?;
        instance.record.lock().unwrap().metadata.session_id = Some(session_id.clone());
        instance.events.activate_session(&session_id);
        let token = random_hex(32)?;
        let instance_id = instance.record.lock().unwrap().metadata.instance_id.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let mut command = Command::new(&prepared.command);
        command
            .args(&prepared.arguments)
            .current_dir(&prepared.directory)
            .envs(&options.environment)
            .env(
                "NAPI_VM_PLUGIN_ENDPOINT",
                listener.local_addr()?.to_string(),
            )
            .env("NAPI_VM_PLUGIN_TOKEN", &token)
            .env("NAPI_VM_PLUGIN_INSTANCE_ID", &instance_id)
            .env("NAPI_VM_PLUGIN_SESSION_ID", &session_id)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| {
            RpcError::new(
                "PLUGIN_EXITED",
                format!(
                    "failed to launch selected runtime {}: {e}; no alternate runtime was executed",
                    prepared.selected_runtime
                ),
            )
        })?;
        let mut drainers = Vec::new();
        let mut secrets = connection_secrets(&options);
        secrets.push(token.clone());
        *instance.secrets.lock().unwrap() = secrets.clone();
        if let Some(stdout) = child.stdout.take() {
            drainers.push(drain_logs(
                stdout,
                instance.logs.clone(),
                "stdout",
                self.inner.limits.max_log_bytes,
                secrets.clone(),
            ));
        }
        if let Some(stderr) = child.stderr.take() {
            drainers.push(drain_logs(
                stderr,
                instance.logs.clone(),
                "stderr",
                self.inner.limits.max_log_bytes,
                secrets.clone(),
            ));
        }
        let deadline = Instant::now() + Duration::from_millis(self.inner.limits.startup_timeout_ms);
        let requirements = self.inner.services.identities();
        let limits = self.inner.limits.clone();
        let expected = prepared.clone();
        let sid = session_id.clone();
        let iid = instance_id.clone();
        let handshake = async {
            let mut attempts = 0;
            loop {
                if attempts >= 32 {
                    return Err(RpcError::new(
                        "OVERLOADED",
                        "bootstrap connection attempt limit exceeded",
                    ));
                }
                let (mut stream, _) = listener.accept().await?;
                attempts += 1;
                let frame = match tokio::time::timeout(
                    Duration::from_millis(500)
                        .min(deadline.saturating_duration_since(Instant::now())),
                    read_frame(&mut stream, 64 * 1024, limits.max_depth),
                )
                .await
                {
                    Ok(Ok(v)) => v,
                    _ => continue,
                };
                // Wrong tokens never consume the one valid session slot.
                if frame["params"]["token"] != token {
                    continue;
                }
                validate_hello(&frame, &expected, &iid, &sid, &requirements)?;
                write_frame(&mut stream,&json!({"jsonrpc":"2.0","id":frame["id"],"result":{"protocol":{"major":1,"minor":0},"interfaces":requirements,"limits":limits,"extensions":[]}}),64*1024,64).await?;
                return Ok(stream);
            }
        };
        let stream = tokio::select! {r=tokio::time::timeout_at(deadline,handshake)=>match r{Ok(v)=>v,Err(_)=>Err(RpcError::new("DEADLINE_EXCEEDED","plugin startup/hello deadline exceeded"))},s=child.wait()=>Err(RpcError::new("PLUGIN_EXITED",format!("plugin exited before hello: {s:?}")))};
        let stream = match stream {
            Ok(s) => s,
            Err(mut e) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                for d in drainers {
                    let _ = d.await;
                }
                // Await pipe EOF before exposing the bounded, already-redacted
                // plugin output. A CLI caller otherwise sees only the host's
                // exit status and loses the child's actual startup error.
                let logs = instance.logs.lock().unwrap();
                e.data["runtime"] = json!(prepared.selected_runtime);
                e.data["logs"] = json!(&logs.chunks);
                e.data["droppedLogBytes"] = json!(logs.dropped);
                return Err(e);
            }
        };
        drop(listener);
        let callback_peer: Arc<RwLock<Option<WeakPeer>>> = Arc::new(RwLock::new(None));
        let callback_cell = callback_peer.clone();
        let event_sequence = Arc::new(AtomicU64::new(1));
        let callback_sequence = event_sequence.clone();
        let callback_events = instance.events.clone();
        let registry = self.inner.services.clone();
        let host_contracts = registry.contracts();
        let plugin_identities: BTreeMap<_, _> = prepared
            .manifest
            .provides
            .keys()
            .map(|id| (id.clone(), prepared.contracts[id].identity()))
            .collect();
        let callback_limits = self.inner.limits.clone();
        let callback_resources = self.inner.resources.clone();
        let allowed_callbacks = prepared.manifest.requires_host.clone();
        let endpoint = format!("h:{session_id}");
        let handler: RequestHandler = Arc::new(move |method, params, request| {
            let peer = callback_cell
                .read()
                .unwrap()
                .as_ref()
                .and_then(WeakPeer::upgrade);
            let registry = registry.clone();
            let allowed_callbacks = allowed_callbacks.clone();
            let locals = host_contracts.clone();
            let remotes = plugin_identities.clone();
            let limits = callback_limits.clone();
            let resources = callback_resources.clone();
            let endpoint = endpoint.clone();
            let event_sequence = callback_sequence.clone();
            let events = callback_events.clone();
            Box::pin(async move {
                if method == "system.ping" {
                    if params != json!({}) {
                        return Err(RpcError::new(
                            "INVALID_ARGUMENT",
                            "ping parameters must be empty",
                        ));
                    }
                    return Ok(Value::Null);
                }
                if method != "system.invoke" {
                    return Err(RpcError::new(
                        "METHOD_NOT_FOUND",
                        "plugin may only invoke negotiated host services or ping",
                    ));
                }
                if !params["interface"]
                    .as_str()
                    .is_some_and(|id| allowed_callbacks.contains_key(id))
                {
                    return Err(RpcError::new(
                        "CONTRACT_MISMATCH",
                        "host callback interface was not negotiated",
                    ));
                }
                let peer =
                    peer.ok_or_else(|| RpcError::new("NOT_READY", "host peer not installed"))?;
                let base = CallContext::new(
                    Arc::new(
                        PeerTransport::new(peer, remotes, locals, Arc::new(AtomicBool::new(true)))
                            .with_sequence(event_sequence)
                            .with_events(events),
                    ),
                    Duration::from_millis(limits.call_timeout_ms),
                )
                .with_resources(resources);
                registry
                    .dispatch(params, request, base, endpoint, limits)
                    .await
            })
        });
        let peer = Peer::new(
            stream,
            session_id.clone(),
            'h',
            self.inner.limits.clone(),
            handler,
        )?;
        *callback_peer.write().unwrap() = Some(peer.downgrade());
        let weak = Arc::downgrade(&instance);
        let sid = session_id.clone();
        let contracts = prepared.contracts.clone();
        let hub = instance.events.clone();
        peer.set_event_handler(Arc::new(move |event| {
            let Some(i) = weak.upgrade() else {
                return Err(RpcError::new("CONNECTION_CLOSED", "instance removed"));
            };
            let m = i.record.lock().unwrap().metadata.clone();
            if m.session_id.as_deref() != Some(&sid)
                || !matches!(m.status, Status::Ready | Status::Draining)
            {
                return Err(RpcError::new(
                    "NOT_READY",
                    "event arrived outside current ready/draining session",
                ));
            }
            hub.accept(&sid, &contracts, event)
        }));
        let process = supervise(
            child,
            drainers,
            peer.clone(),
            Arc::downgrade(&instance),
            session_id.clone(),
            Duration::from_millis(self.inner.limits.shutdown_timeout_ms),
        );
        let session = Arc::new(Session {
            peer: peer.clone(),
            process,
            id: session_id.clone(),
            event_sequence,
        });
        {
            let mut r = instance.record.lock().unwrap();
            r.session = Some(session.clone());
            r.metadata.session_id = Some(session_id);
            r.metadata.selected_runtime = Some(prepared.selected_runtime.clone());
            r.metadata.runtime_version = Some(prepared.runtime_version.clone());
            r.metadata.plugin_id = prepared.manifest.id.clone();
            r.metadata.plugin_version = prepared.manifest.version.clone();
            r.metadata.failure = None;
            r.prepared = prepared.clone();
            r.options = options.clone();
        }
        let mut context = options.context.clone();
        if !context.is_object() {
            peer.close(RpcError::new(
                "INVALID_ARGUMENT",
                "application context must be an object",
            ));
            session.process.stop().await?;
            return Err(RpcError::new(
                "INVALID_ARGUMENT",
                "context must be an object",
            ));
        }
        context["services"] = serde_json::to_value(&options.services)?;
        context["capabilities"] =
            serde_json::to_value(&prepared.manifest.dependencies.capabilities)?;
        context["nativeDependencies"] = serde_json::to_value(&prepared.native_paths)?;
        let ready_instance = instance.clone();
        let ready_session = session.clone();
        let result = peer.request_with_result_handler(
            "system.initialize",
            json!({"configuration":options.configuration,"context":context,"snapshot":snapshot}),
            deadline.saturating_duration_since(Instant::now()),
            None,
            Some(Box::new(move |value| {
                if !value.is_null() {
                    return Err(RpcError::new("INVALID_RESULT", "initialize response must be null"));
                }
                let mut record = ready_instance.record.lock().unwrap();
                if !admit_ready(&mut record.metadata, &ready_session.id, ready_session.peer.is_closed()) {
                    return Err(RpcError::new("PLUGIN_EXITED", "plugin disconnected during initialization"));
                }
                record.history.push(Status::Ready);
                Ok(())
            })),
        ).await;
        match result {
            Ok(_) => {
                let ready = {
                    let record = instance.record.lock().unwrap();
                    record.metadata.status == Status::Ready
                        && record.metadata.session_id.as_deref() == Some(&session.id)
                        && !peer.is_closed()
                };
                if !ready {
                    session.process.stop().await?;
                    return Err(RpcError::new(
                        "PLUGIN_EXITED",
                        "plugin disconnected during initialization",
                    ));
                }
                Ok(())
            }
            Err(e) => {
                peer.close(e.clone());
                let _ = session.process.stop().await;
                Err(e)
            }
        }
    }
    pub async fn reload(&self, id: &str, options: ReloadOptions) -> PluginResult<PluginHandle> {
        let _startup = self.inner.startup_gate.read().await;
        if self.inner.shutting_down.load(Ordering::Acquire) {
            return Err(RpcError::new("NOT_READY", "host is shutting down"));
        }
        let handle = self
            .get(id)
            .ok_or_else(|| RpcError::new("NOT_READY", "unknown instance"))?;
        let instance = handle.instance.clone();
        let _lease = instance.lease()?;
        let (old, old_prepared, mut load, snapshot, status) = {
            let r = instance.record.lock().unwrap();
            (
                r.session.clone(),
                r.prepared.clone(),
                r.options.clone(),
                r.snapshot.clone(),
                r.metadata.status,
            )
        };
        if !matches!(
            status,
            Status::Ready | Status::Draining | Status::Failed | Status::Stopped
        ) {
            return Err(RpcError::new(
                "NOT_READY",
                "instance cannot reload in its current state",
            ));
        }
        if let Some(runtime) = &options.runtime {
            load.runtime.runtime = if runtime == "native" || runtime == "executable" {
                None
            } else {
                Some(runtime.clone())
            };
        }
        let replacement = options
            .replacement_manifest
            .unwrap_or(old_prepared.manifest_path.clone());
        let (replacement_manifest, _, _) = read_manifest(&replacement, false).await?;
        if matches!(replacement_manifest.launch, Launch::Executable { .. })
            && options.runtime.is_none()
        {
            load.runtime.runtime = None;
        }
        let prepared = prepare(&replacement, &load.runtime).await?;
        preflight_requirements(&prepared, &self.inner.services, &load)?;
        for service in &prepared.manifest.dependencies.services {
            if service.owner.as_deref() == Some("host")
                && !old_prepared
                    .manifest
                    .dependencies
                    .services
                    .iter()
                    .any(|old| {
                        old.name == service.name
                            && old.owner == service.owner
                            && old.readiness == service.readiness
                            && old.shutdown == service.shutdown
                    })
            {
                return Err(RpcError::new(
                    "NOT_READY",
                    "replacement requires a different host-owned service; load a new explicitly configured instance",
                ));
            }
        }

        for (id, v) in &old_prepared.manifest.provides {
            if prepared.manifest.provides.get(id) != Some(v)
                || prepared.contracts.get(id).map(Contract::identity)
                    != old_prepared.contracts.get(id).map(Contract::identity)
            {
                return Err(RpcError::new(
                    "CONTRACT_MISMATCH",
                    "replacement changes stable business contract identity",
                ));
            }
        }
        let snapshot = if options.force_without_state {
            Value::Null
        } else if let Some(old) = &old {
            if matches!(status, Status::Ready | Status::Draining) {
                instance.transition(Status::Draining);
                let timeout = Duration::from_millis(self.inner.limits.call_timeout_ms);
                old.peer
                    .request(
                        "system.quiesce",
                        json!({"timeoutMs":timeout.as_millis()as u64}),
                        timeout,
                        None,
                    )
                    .await
                    .map_err(|error| sanitize_error(&instance, error))?;
                let snapshot = old
                    .peer
                    .request("system.snapshot", json!({}), timeout, None)
                    .await
                    .map_err(|error| sanitize_error(&instance, error))?;
                validate_snapshot(&old_prepared, &snapshot)?;
                validate_snapshot(&prepared, &snapshot)?;
                instance.record.lock().unwrap().snapshot = snapshot.clone();
                snapshot
            } else {
                snapshot
            }
        } else {
            snapshot
        };
        if let Some(old) = old {
            instance.transition(Status::Stopping);
            let _ = old
                .peer
                .request(
                    "system.shutdown",
                    json!({}),
                    Duration::from_millis(self.inner.limits.shutdown_timeout_ms),
                    None,
                )
                .await;
            old.process.stop().await?;
            old.peer.close(RpcError::new(
                "CONNECTION_CLOSED",
                "old session was replaced",
            ));
            instance.events.forget_session(&old.id);
            instance.record.lock().unwrap().session = None;
        }
        instance.transition(Status::Stopped);
        if let Err(e) = self.start(instance.clone(), prepared, load, snapshot).await {
            set_failed(&instance, e.clone());
            return Err(sanitize_error(&instance, e));
        }
        Ok(handle)
    }
    pub async fn unload(&self, id: &str) -> PluginResult<()> {
        let Some(handle) = self.get(id) else {
            return Ok(());
        };
        let i = handle.instance.clone();
        let _lease = i.lease()?;
        let session = i.record.lock().unwrap().session.clone();
        let mut error = None;
        if let Some(s) = session {
            if !s.peer.is_closed() {
                i.transition(Status::Draining);
                let timeout = Duration::from_millis(self.inner.limits.shutdown_timeout_ms);
                if let Err(e) = s
                    .peer
                    .request(
                        "system.quiesce",
                        json!({"timeoutMs":timeout.as_millis()as u64}),
                        timeout,
                        None,
                    )
                    .await
                {
                    error = Some(e);
                }
                i.transition(Status::Stopping);
                if let Err(e) = s
                    .peer
                    .request("system.shutdown", json!({}), timeout, None)
                    .await
                    && !matches!(e.stable_code(), "CONNECTION_CLOSED" | "PLUGIN_EXITED")
                {
                    error.get_or_insert(e);
                }
            }
            if let Err(e) = s.process.stop().await {
                error.get_or_insert(e);
            }
            s.peer
                .close(RpcError::new("CONNECTION_CLOSED", "instance unloaded"));
            i.events.forget_session(&s.id);
            i.record.lock().unwrap().session = None;
        }
        shutdown_services(&i).await;
        i.events.close();
        i.transition(Status::Stopped);
        if let Some(e) = error {
            Err(sanitize_error(&i, e))
        } else {
            Ok(())
        }
    }
    /// Emit a registered host-service event to one current plugin session.
    pub async fn emit(
        &self,
        instance_id: &str,
        contract: &Contract,
        event: &str,
        payload: Value,
    ) -> PluginResult<()> {
        contract.event(event, &payload)?;
        let local = self.inner.services.contracts();
        if local.get(contract.id()).map(Contract::identity) != Some(contract.identity()) {
            return Err(RpcError::new(
                "CONTRACT_MISMATCH",
                "host event contract is not registered",
            ));
        }
        let handle = self
            .get(instance_id)
            .ok_or_else(|| RpcError::new("NOT_READY", "unknown instance"))?;
        let session = {
            let r = handle.instance.record.lock().unwrap();
            if r.metadata.status != Status::Ready {
                return Err(RpcError::new(
                    "NOT_READY",
                    "instance must be READY for host events",
                ));
            }
            if !r
                .prepared
                .manifest
                .requires_host
                .contains_key(contract.id())
            {
                return Err(RpcError::new(
                    "CONTRACT_MISMATCH",
                    "plugin did not negotiate this host event contract",
                ));
            }
            r.session
                .clone()
                .ok_or_else(|| RpcError::new("CONNECTION_CLOSED", "session unavailable"))?
        };
        PeerTransport::new(
            session.peer.clone(),
            BTreeMap::new(),
            local,
            Arc::new(AtomicBool::new(true)),
        )
        .with_sequence(session.event_sequence.clone())
        .emit(contract.clone(), event.to_string(), payload, None)
        .await
    }
    pub async fn shutdown(&self) -> PluginResult<()> {
        self.inner.shutting_down.store(true, Ordering::Release);
        let _startup = self.inner.startup_gate.write().await;
        let ids: Vec<_> = self
            .inner
            .instances
            .read()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut first = None;
        for id in ids {
            if let Err(e) = self.unload(&id).await {
                first.get_or_insert(e);
            }
        }
        let timeout = Duration::from_millis(self.inner.limits.shutdown_timeout_ms);
        if let Err(e) = self.inner.services.quiesce(timeout).await {
            first.get_or_insert(e);
        }
        if let Err(e) = self.inner.resources.quiesce(timeout).await {
            first.get_or_insert(e);
        }
        if let Err(e) = self.inner.resources.shutdown(timeout).await {
            first.get_or_insert(e);
        }
        if let Some(e) = first { Err(e) } else { Ok(()) }
    }
}
impl PluginHandle {
    /// Quiesce, stop, and reap this instance through its owning host.
    /// Repeated shutdown is safe. Keep the host alive until this completes;
    /// handles do not prolong host ownership. After host drop this is a no-op.
    /// The host owns only the direct child process.
    pub async fn shutdown(&self) -> PluginResult<()> {
        let Some(inner) = self.host.upgrade() else {
            return Ok(());
        };
        Host { inner }.unload(&self.instance_id()).await
    }

    pub fn pid(&self) -> Option<u32> {
        self.instance
            .record
            .lock()
            .unwrap()
            .session
            .as_ref()
            .and_then(|s| s.process.pid)
    }
    pub fn instance_id(&self) -> String {
        self.instance
            .record
            .lock()
            .unwrap()
            .metadata
            .instance_id
            .clone()
    }
    pub fn metadata(&self) -> InstanceMetadata {
        self.instance.record.lock().unwrap().metadata.clone()
    }
    pub fn status(&self) -> Status {
        self.metadata().status
    }
    pub fn history(&self) -> Vec<Status> {
        self.instance.record.lock().unwrap().history.clone()
    }
    pub fn context(&self) -> CallContext {
        CallContext::new(
            Arc::new(StableTarget {
                instance: Arc::downgrade(&self.instance),
            }),
            Duration::from_millis(self.limits.call_timeout_ms),
        )
    }
    pub async fn invoke(
        &self,
        contract: &Contract,
        method: &str,
        input: Value,
    ) -> PluginResult<Value> {
        self.context().invoke(contract, method, input).await
    }
    pub fn contract(&self, id: &str) -> PluginResult<Contract> {
        self.instance
            .record
            .lock()
            .unwrap()
            .prepared
            .contracts
            .get(id)
            .cloned()
            .ok_or_else(|| RpcError::new("CONTRACT_MISMATCH", "contract not loaded"))
    }
    pub fn subscribe(
        &self,
        contract: &Contract,
        event: &str,
        capacity: usize,
    ) -> PluginResult<Subscription> {
        let loaded = self.contract(contract.id())?;
        if loaded.identity() != contract.identity() {
            return Err(RpcError::new(
                "CONTRACT_MISMATCH",
                "subscription contract mismatch",
            ));
        }
        self.instance
            .events
            .subscribe(contract, event, capacity.min(self.limits.max_event_queue))
    }
    pub fn logs(&self) -> (Vec<LogChunk>, u64) {
        let l = self.instance.logs.lock().unwrap();
        (l.chunks.iter().cloned().collect(), l.dropped)
    }
    pub fn retained_snapshot(&self) -> Value {
        self.instance.record.lock().unwrap().snapshot.clone()
    }
}
struct StableTarget {
    instance: Weak<Instance>,
}
impl ClientTransport for StableTarget {
    fn invoke(
        &self,
        c: Contract,
        m: String,
        v: Value,
        o: InvokeOptions,
    ) -> PluginFuture<'static, Value> {
        let instance = self.instance.upgrade();
        Box::pin(async move {
            let i = instance
                .ok_or_else(|| RpcError::new("CONNECTION_CLOSED", "instance is no longer owned"))?;
            let (s, contract) = {
                let r = i.record.lock().unwrap();
                if r.metadata.status != Status::Ready {
                    return Err(RpcError::new("NOT_READY", "plugin instance is not READY"));
                }
                (r.session.clone(), r.prepared.contracts.get(c.id()).cloned())
            };
            if contract.is_none_or(|x| x.identity() != c.identity()) {
                return Err(RpcError::new(
                    "CONTRACT_MISMATCH",
                    "client contract does not match loaded plugin",
                ));
            }
            let s = s.ok_or_else(|| RpcError::new("CONNECTION_CLOSED", "session unavailable"))?;
            PeerTransport::new(
                s.peer.clone(),
                BTreeMap::from([(c.id().into(), c.identity())]),
                BTreeMap::new(),
                Arc::new(AtomicBool::new(true)),
            )
            .invoke(c, m, v, o)
            .await
            .map_err(|error| sanitize_error(&i, error))
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
                "plugin handle cannot emit plugin-owned events",
            ))
        })
    }
}
fn admit_ready(metadata: &mut InstanceMetadata, session: &str, closed: bool) -> bool {
    if closed
        || metadata.status != Status::Starting
        || metadata.session_id.as_deref() != Some(session)
    {
        return false;
    }
    metadata.status = Status::Ready;
    true
}
fn random_hex(size: usize) -> PluginResult<String> {
    let mut b = vec![0; size];
    getrandom::fill(&mut b)
        .map_err(|_| RpcError::new("INTERNAL_ERROR", "OS randomness unavailable"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}
fn set_failed(i: &Instance, error: RpcError) {
    let error = sanitize_error(i, error);
    i.transition(Status::Failed);
    i.record.lock().unwrap().metadata.failure = Some(error);
}
async fn shutdown_services(i: &Instance) {
    let mut owned = std::mem::take(&mut *i.services.lock().await);
    for s in &mut owned {
        let _ = s.shutdown().await;
    }
}
fn preflight_requirements(p: &Prepared, r: &Registry, o: &LoadOptions) -> PluginResult<()> {
    let identities = r.identities();
    for (id, v) in &p.manifest.requires_host {
        if identities.get(id) != p.contracts.get(id).map(Contract::identity).as_ref()
            || identities[id].version != *v
        {
            return Err(RpcError::new(
                "CONTRACT_MISMATCH",
                format!("host does not provide required interface {id}"),
            ));
        }
    }
    for c in &p.manifest.dependencies.capabilities {
        if !identities.contains_key(c) {
            return Err(RpcError::new(
                "CONTRACT_MISMATCH",
                format!("host capability {c} is unavailable"),
            ));
        }
    }
    for s in &p.manifest.dependencies.services {
        let c = o.services.get(&s.name).ok_or_else(|| {
            RpcError::new(
                "NOT_READY",
                format!(
                    "external service {} must be injected before loading",
                    s.name
                ),
            )
        })?;
        if !(c.url.starts_with("http://") || c.url.starts_with("https://"))
            || c.url.len() > 8192
            || c.url.chars().any(|c| c.is_control())
            || c.headers.len() > 64
            || c.headers.iter().any(|(k, v)| {
                k.is_empty()
                    || k.len() > 128
                    || k.chars().any(|c| c.is_control())
                    || v.len() > 16384
                    || v.contains(['\r', '\n'])
            })
        {
            return Err(RpcError::new(
                "INVALID_ARGUMENT",
                "invalid runtime service connection",
            ));
        }
    }
    if !o.context.is_object() {
        return Err(RpcError::new("INVALID_ARGUMENT", "context must be object"));
    }
    Ok(())
}
fn validate_snapshot(p: &Prepared, s: &Value) -> PluginResult<()> {
    if s.is_null() {
        if p.contracts
            .values()
            .any(|c| !c.descriptor["state"].is_null())
        {
            return Err(RpcError::new(
                "STATE_INCOMPATIBLE",
                "stateful plugin must provide a snapshot",
            ));
        }
        return Ok(());
    }
    p.contracts
        .values()
        .find(|c| c.descriptor["state"]["contract"] == s["contract"])
        .ok_or_else(|| {
            RpcError::new(
                "STATE_INCOMPATIBLE",
                "replacement does not declare snapshot state contract",
            )
        })?
        .snapshot(s)
}
fn validate_hello(
    v: &Value,
    p: &Prepared,
    instance: &str,
    session: &str,
    services: &BTreeMap<String, InterfaceIdentity>,
) -> PluginResult<()> {
    let o = v
        .as_object()
        .ok_or_else(|| RpcError::new("INVALID_REQUEST", "hello must be request object"))?;
    if o.len() != 4
        || v["jsonrpc"] != "2.0"
        || v["method"] != "system.hello"
        || !v["id"]
            .as_str()
            .is_some_and(|id| id.starts_with(&format!("p:{session}:")))
    {
        return Err(RpcError::new(
            "INVALID_REQUEST",
            "first request must be system.hello with session request ID",
        ));
    }
    let x = &v["params"];
    let keys = [
        "token",
        "instanceId",
        "sessionId",
        "pluginId",
        "pluginVersion",
        "protocol",
        "interfaces",
        "requiresHost",
        "extensions",
    ];
    if !x
        .as_object()
        .is_some_and(|m| m.len() == keys.len() && m.keys().all(|k| keys.contains(&k.as_str())))
        || x["instanceId"] != instance
        || x["sessionId"] != session
        || x["pluginId"] != p.manifest.id
        || x["pluginVersion"] != p.manifest.version
    {
        return Err(RpcError::new(
            "PROTOCOL_MISMATCH",
            "hello launch identity does not match manifest/session",
        ));
    }
    let protocol: ProtocolRange = serde_json::from_value(x["protocol"].clone())?;
    if protocol.major != 1 || protocol.min_minor > 0 || protocol.max_minor < protocol.min_minor {
        return Err(RpcError::new(
            "PROTOCOL_MISMATCH",
            "no overlapping protocol version",
        ));
    }
    if !x["extensions"].is_array() {
        return Err(RpcError::new(
            "INVALID_ARGUMENT",
            "extensions must be array",
        ));
    }
    let provides: BTreeMap<String, InterfaceIdentity> =
        serde_json::from_value(x["interfaces"].clone())?;
    let expected: BTreeMap<_, _> = p
        .manifest
        .provides
        .keys()
        .map(|id| (id.clone(), p.contracts[id].identity()))
        .collect();
    if provides != expected {
        return Err(RpcError::new(
            "CONTRACT_MISMATCH",
            "plugin interfaces differ from packaged generated contracts",
        ));
    }
    let requires: BTreeMap<String, InterfaceIdentity> =
        serde_json::from_value(x["requiresHost"].clone())?;
    let expected: BTreeMap<_, _> = p
        .manifest
        .requires_host
        .keys()
        .map(|id| (id.clone(), p.contracts[id].identity()))
        .collect();
    if requires != expected || requires.iter().any(|(id, c)| services.get(id) != Some(c)) {
        return Err(RpcError::new(
            "CONTRACT_MISMATCH",
            "plugin required host interfaces mismatch",
        ));
    }
    Ok(())
}
// Preserve encoded and decoded URL credentials because plugins may log either.
fn decode_url_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && at + 2 < bytes.len()
            && let (Some(a), Some(b)) = (
                (bytes[at + 1] as char).to_digit(16),
                (bytes[at + 2] as char).to_digit(16),
            )
        {
            decoded.push((a * 16 + b) as u8);
            at += 3;
        } else {
            decoded.push(if bytes[at] == b'+' { b' ' } else { bytes[at] });
            at += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}
fn connection_secrets(options: &LoadOptions) -> Vec<String> {
    let mut secrets: Vec<String> = options
        .environment
        .values()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect();
    for service in options.services.values() {
        secrets.extend(service.headers.values().filter(|s| !s.is_empty()).cloned());
        for (name, value) in &service.headers {
            if (name.eq_ignore_ascii_case("authorization")
                || name.eq_ignore_ascii_case("proxy-authorization"))
                && let Some((_, credential)) = value.split_once(' ')
                && !credential.trim().is_empty()
            {
                secrets.push(credential.trim().into());
            }
        }
        secrets.push(service.url.clone());
        if let Some((_, rest)) = service.url.split_once("://") {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            if let Some((userinfo, _)) = authority.rsplit_once('@') {
                for part in userinfo.split(':') {
                    if !part.is_empty() {
                        secrets.push(part.into());
                        secrets.push(decode_url_component(part));
                    }
                }
            }
        }
        if let Some((_, query)) = service.url.split_once('?') {
            for pair in query.split('#').next().unwrap_or("").split('&') {
                if let Some((_, value)) = pair.split_once('=')
                    && !value.is_empty()
                {
                    secrets.push(value.into());
                    secrets.push(decode_url_component(value));
                }
            }
        }
    }
    secrets.retain(|s| !s.is_empty());
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    secrets.dedup();
    secrets
}
fn sanitize_error(instance: &Instance, error: RpcError) -> RpcError {
    redact_error(error, &instance.secrets.lock().unwrap())
}
fn redact_error(mut error: RpcError, secrets: &[String]) -> RpcError {
    fn text(value: &mut String, secrets: &[String]) {
        for secret in secrets {
            *value = value.replace(secret, "[redacted]");
        }
    }
    fn redact(value: &mut Value, secrets: &[String], envelope: bool) {
        match value {
            Value::String(value) => text(value, secrets),
            Value::Array(items) => {
                for item in items {
                    redact(item, secrets, false);
                }
            }
            Value::Object(items) => {
                let entries = std::mem::take(items);
                for (mut key, mut item) in entries {
                    // Protocol and domain identifiers are public contract data, not
                    // credential payloads. Coincidentally matching environment values
                    // must never destroy stable error classification or envelope names.
                    if envelope && matches!(key.as_str(), "code" | "domainCode") {
                        items.insert(key, item);
                        continue;
                    }
                    if !envelope || key != "data" {
                        text(&mut key, secrets);
                    }
                    redact(&mut item, secrets, false);
                    items.insert(key, item);
                }
            }
            _ => {}
        }
    }
    text(&mut error.message, secrets);
    redact(&mut error.data, secrets, true);
    error
}

fn drain_logs<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    mut stream: R,
    logs: Arc<Mutex<Logs>>,
    name: &'static str,
    limit: usize,
    mut secrets: Vec<String>,
) -> JoinHandle<()> {
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    secrets.dedup();
    let keep = secrets
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(1)
        .saturating_sub(1);
    tokio::spawn(async move {
        let mut bytes = [0u8; 8192];
        let mut pending = Vec::new();
        loop {
            let n: usize = stream.read(&mut bytes).await.unwrap_or_default();
            pending.extend_from_slice(&bytes[..n]);
            for secret in &secrets {
                let secret = secret.as_bytes();
                if secret.is_empty() {
                    continue;
                }
                let mut at = 0;
                while at + secret.len() <= pending.len() {
                    if pending[at..].starts_with(secret) {
                        pending.splice(at..at + secret.len(), b"[redacted]".iter().copied());
                        at += 10;
                    } else {
                        at += 1;
                    }
                }
            }
            let emit = if n == 0 {
                pending.len()
            } else {
                pending.len().saturating_sub(keep)
            };
            if emit > 0 {
                logs.lock()
                    .unwrap()
                    .push(name, &pending[..emit], limit, "\0");
                pending.drain(..emit);
            }
            if n == 0 {
                break;
            }
        }
    })
}

fn supervise(
    mut child: Child,
    drainers: Vec<JoinHandle<()>>,
    peer: Peer,
    instance: Weak<Instance>,
    session: String,
    grace: Duration,
) -> ProcessOwner {
    let pid = child.id();
    let (tx, mut rx) = mpsc::channel::<oneshot::Sender<PluginResult<()>>>(4);
    tokio::spawn(async move {
        let mut stopped = false;
        let mut response = None;
        let result = tokio::select! {
            status=child.wait()=>status.map(|s|{if !s.success(){Err(RpcError::new("PLUGIN_EXITED",format!("plugin exited with {s}")))}else{Ok(())}}).unwrap_or_else(|e|Err(e.into())),
            command=rx.recv()=>{stopped=true;response=command;match tokio::time::timeout(grace,child.wait()).await{Ok(Ok(_))=>Ok(()),Ok(Err(e))=>Err(e.into()),Err(_)=>{let _=child.start_kill();child.wait().await.map(|_|()).map_err(Into::into)}}},
            _=peer.closed()=>{let _=child.start_kill();child.wait().await.map(|_|()).map_err(Into::into)}
        };
        peer.close(RpcError::new(
            "PLUGIN_EXITED",
            "plugin direct child has exited",
        ));
        for mut d in drainers {
            if tokio::time::timeout(Duration::from_secs(1), &mut d)
                .await
                .is_err()
            {
                d.abort();
            }
        }
        if let Some(i) = instance.upgrade() {
            let current = {
                let r = i.record.lock().unwrap();
                r.metadata.session_id.as_deref() == Some(&session)
                    && !matches!(r.metadata.status, Status::Stopping | Status::Stopped)
            };
            if current && !stopped {
                set_failed(
                    &i,
                    result.clone().err().unwrap_or_else(|| {
                        RpcError::new("PLUGIN_EXITED", "plugin exited unexpectedly")
                    }),
                );
            }
        }
        if let Some(tx) = response {
            let _ = tx.send(result);
        }
    });
    ProcessOwner { stop: tx, pid }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_log_chunks_keep_utf8_tails_and_count_dropped_bytes() {
        for limit in [0, 1, 2, 3, 8, 16] {
            let input = "prefix 日本語 😀 tail";
            let mut logs = Logs::default();
            logs.push("stderr", input.as_bytes(), limit, "\0");
            let output: String = logs.chunks.iter().map(|log| log.text.as_str()).collect();
            assert!(output.len() <= limit);
            assert!(input.ends_with(&output));
            assert_eq!(logs.dropped as usize + output.len(), input.len());
            assert_eq!(logs.bytes, output.len());
        }
    }

    #[test]
    fn error_redaction_preserves_contract_codes_and_envelope_names() {
        let mut error = RpcError::new("INTERNAL_ERROR", "INTERNAL_ERROR private-key");
        error.data["detail"] = json!({"private-key":"INTERNAL_ERROR", "code":"private-key"});
        let redacted = redact_error(
            error,
            &["INTERNAL_ERROR".into(), "private-key".into(), "code".into()],
        );
        assert_eq!(redacted.stable_code(), "INTERNAL_ERROR");
        assert_eq!(redacted.message, "[redacted] [redacted]");
        assert_eq!(redacted.data["detail"], json!({"[redacted]":"[redacted]"}));
        let domain = redact_error(
            RpcError::domain("DOMAIN", "DOMAIN", json!({"code":"DOMAIN"})),
            &["DOMAIN".into(), "code".into(), "data".into()],
        );
        assert_eq!(domain.stable_code(), "APPLICATION_ERROR");
        assert_eq!(domain.data["domainCode"], "DOMAIN");
        assert_eq!(domain.data["data"], json!({"[redacted]":"[redacted]"}));
    }

    #[test]
    fn service_credentials_cover_encoded_and_decoded_urls() {
        let options = LoadOptions {
            services: BTreeMap::from([("fixture".into(), ServiceConnection {
                url: "http://user:pass%20word@localhost:1234/?token=private%2Bkey&other=query+secret".into(),
                headers: BTreeMap::from([("Authorization".into(), "Bearer header-secret".into())]),
            })]),
            ..Default::default()
        };
        let secrets = connection_secrets(&options);
        for expected in [
            "user",
            "pass%20word",
            "pass word",
            "private%2Bkey",
            "private+key",
            "query+secret",
            "query secret",
            "Bearer header-secret",
            "header-secret",
        ] {
            assert!(
                secrets.iter().any(|secret| secret == expected),
                "missing {expected}"
            );
        }
    }

    #[tokio::test]
    async fn log_redaction_spans_arbitrary_chunks() {
        let secret = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let (mut tx, rx) = tokio::io::duplex(1024);
        let logs = Arc::new(Mutex::new(Logs::default()));
        let task = drain_logs(
            rx,
            logs.clone(),
            "stdout",
            1024,
            vec![secret.into(), "private-header-value".into()],
        );
        use tokio::io::AsyncWriteExt;
        for b in format!("prefix {secret} private-header-value suffix").bytes() {
            tx.write_all(&[b]).await.unwrap();
        }
        drop(tx);
        task.await.unwrap();
        let output = logs
            .lock()
            .unwrap()
            .chunks
            .iter()
            .map(|x| x.text.as_str())
            .collect::<String>();
        assert!(!output.contains(secret));
        assert!(!output.contains("private-header-value"));
        assert_eq!(output, "prefix [redacted] [redacted] suffix");
    }
    #[test]
    fn target_and_package_paths_are_explicit() {
        for p in [
            "../secret",
            "/absolute",
            "foo\\bar",
            "C:/escape",
            "a//b",
            "./a/../b",
        ] {
            assert!(relative_path(p).is_err(), "{p}");
        }
        assert!(relative_path("./assets/日本語 file.txt").is_ok());
        assert!(Target::current().matches_current());
        let mut wrong = Target::current();
        wrong.arch = "wrong".into();
        assert!(!wrong.matches_current());
    }
    #[test]
    fn retention_stays_bounded() {
        let mut logs = Logs::default();
        for _ in 0..100 {
            logs.push("stdout", b"12345678", 32, "secret");
        }
        assert!(logs.bytes <= 32);
        assert!(logs.dropped > 0);
    }
}

#[cfg(test)]
mod ownership_regressions {
    use super::*;
    #[test]
    fn ready_never_overwrites_failed_or_wrong_generation() {
        let mut m = InstanceMetadata {
            instance_id: "i".into(),
            plugin_id: "example.test".into(),
            plugin_version: "1.0.0".into(),
            status: Status::Failed,
            session_id: Some("s".into()),
            selected_runtime: None,
            runtime_version: None,
            failure: None,
        };
        assert!(!admit_ready(&mut m, "s", false));
        assert_eq!(m.status, Status::Failed);
        m.status = Status::Starting;
        assert!(!admit_ready(&mut m, "old", false));
        assert!(!admit_ready(&mut m, "s", true));
        assert!(admit_ready(&mut m, "s", false));
        assert_eq!(m.status, Status::Ready);
    }
    #[tokio::test]
    async fn host_owns_author_tasks_until_shutdown() {
        let h = Host::default();
        let (started, ready) = oneshot::channel();
        let (done, completed) = oneshot::channel();
        h.inner
            .resources
            .spawn(move |cancel| {
                Box::pin(async move {
                    let _ = started.send(());
                    cancel.cancelled().await;
                    let _ = done.send(());
                })
            })
            .unwrap();
        ready.await.unwrap();
        h.shutdown().await.unwrap();
        completed.await.unwrap();
        assert!(h.inner.resources.spawn(|_| Box::pin(async {})).is_err());
    }
}
