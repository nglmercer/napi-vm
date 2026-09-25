use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value as JsonValue, json};

use super::native_addon::NativeAddonPolicy;
use crate::error::VmErr;
use crate::host::{HostBridge, HostCallback, HostCallbackKind, HostEvent, WakeNotifier, WakeSlot};
use crate::interpreter::NativeAddonLoader;
use crate::value::{PromiseInner, PromiseState, Value};

mod bridge_script;
#[cfg(all(test, target_os = "linux"))]
mod tests;
mod wire;
mod wire_apply;

use bridge_script::NODE_BRIDGE;
use wire::{
    guest_call_result_to_value, guest_graph_node_snapshot, guest_to_wire, required_string_arg,
    wire_to_guest,
};
use wire_apply::{apply_guest_mutation, wire_to_guest_with_context};
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_WIRE_DEPTH: usize = 128;
const MAX_NATIVE_HANDLES: usize = 262_144;
const SIDECAR_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

static NEXT_GUEST_GRAPH_NODE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
struct LocalObjectTrap {
    object_id: u64,
    operation: ObjectOperation,
}

#[derive(Clone, Copy)]
enum ObjectOperation {
    Get,
    Set,
    Has,
    Delete,
    OwnKeys,
}

impl ObjectOperation {
    fn guest_name(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Set => "set",
            Self::Has => "has",
            Self::Delete => "deleteProperty",
            Self::OwnKeys => "ownKeys",
        }
    }
}

struct State {
    child: Child,
    stream: TcpStream,
    reader: Option<JoinHandle<()>>,
    response_rx: Receiver<JsonValue>,
    event_rx: Receiver<JsonValue>,
    pending_events: VecDeque<JsonValue>,
    request_id: u64,
    failed: bool,
    shutdown_requested: bool,
    shutdown_complete: bool,
    process_reaped: bool,
    next_local_handle: usize,
    local_handles: HashMap<usize, LocalObjectTrap>,
    object_proxies: HashMap<u64, Value>,
    proxy_ids: HashMap<usize, u64>,
    guest_callbacks: HashMap<u64, Value>,
    guest_callback_keys: HashMap<(usize, usize), u64>,
    guest_graph_nodes: HashMap<u64, Value>,
    guest_callback_graphs: HashMap<u64, HashSet<u64>>,
    host_symbols: HashMap<String, Value>,
    symbol_remote_ids: HashMap<u64, String>,
    next_guest_callback_id: u64,
    native_promises: HashMap<u64, Rc<RefCell<PromiseInner>>>,
}

#[derive(Default)]
struct WireEncodeContext {
    seen: HashMap<usize, u64>,
    nodes: HashMap<u64, Value>,
    callbacks: HashMap<u64, Value>,
    active_proxies: HashSet<usize>,
}

impl WireEncodeContext {
    fn register(&mut self, identity: usize, value: Value) -> Result<u64, VmErr> {
        if self.seen.len() >= MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg("guest graph node limit exceeded".into()));
        }
        let id = NEXT_GUEST_GRAPH_NODE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| VmErr::Msg("native graph id exhausted".into()))?;
        self.seen.insert(identity, id);
        self.nodes.insert(id, value);
        Ok(id)
    }

    fn from_callback_state(state: &State, callback_id: u64) -> Self {
        let mut context = Self {
            callbacks: state.guest_callbacks.clone(),
            ..Self::default()
        };
        if let Some(ids) = state.guest_callback_graphs.get(&callback_id) {
            for id in ids {
                if let Some(value) = state.guest_graph_nodes.get(id) {
                    context.nodes.insert(*id, value.clone());
                    if let Some(identity) = guest_graph_identity(value) {
                        context.seen.insert(identity, *id);
                    }
                }
            }
        }
        context
    }
}

#[derive(Default)]
struct WireDecodeContext {
    nodes: HashMap<String, Value>,
    callbacks: HashMap<u64, Value>,
}

impl WireDecodeContext {
    fn from_encode_context(encoded: &WireEncodeContext) -> Self {
        Self {
            nodes: encoded
                .nodes
                .iter()
                .map(|(id, value)| (format!("g:{id}"), value.clone()))
                .collect(),
            callbacks: encoded.callbacks.clone(),
        }
    }

    fn from_callback_state(state: &State, callback_id: u64) -> Self {
        let nodes = state
            .guest_callback_graphs
            .get(&callback_id)
            .into_iter()
            .flatten()
            .filter_map(|id| {
                state
                    .guest_graph_nodes
                    .get(id)
                    .map(|value| (format!("g:{id}"), value.clone()))
            })
            .collect();
        Self {
            nodes,
            callbacks: state.guest_callbacks.clone(),
        }
    }

    fn register(&mut self, id: String, value: Value) -> Result<(), VmErr> {
        if self.nodes.contains_key(&id) {
            return Err(VmErr::Msg("duplicate Node graph node id".into()));
        }
        if self.nodes.len() >= MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg("Node graph node limit exceeded".into()));
        }
        self.nodes.insert(id, value);
        Ok(())
    }
}

fn guest_graph_identity(value: &Value) -> Option<usize> {
    match value {
        Value::Array(array) => Some(Rc::as_ptr(array) as usize),
        Value::Object { props } => Some(Rc::as_ptr(props) as usize),
        Value::Proxy(proxy) => Some(Rc::as_ptr(proxy) as usize),
        Value::Class(class) => Some(Rc::as_ptr(&class.statics) as usize),
        _ => None,
    }
}

fn guest_callback_identity(value: &Value) -> Option<(usize, usize)> {
    match value {
        Value::Function(function) => Some((
            Rc::as_ptr(&function.body) as usize,
            function
                .closure
                .as_ref()
                .map_or(0, |closure| Rc::as_ptr(closure) as usize),
        )),
        Value::NativeFunction { name, callable } => {
            Some((*callable as *const () as usize, name.as_ptr() as usize))
        }
        Value::Class(class) => Some((
            Rc::as_ptr(&class.statics) as usize,
            Rc::as_ptr(&class.prototype) as usize,
        )),
        _ => None,
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl State {
    fn shutdown(&mut self) -> Result<(), VmErr> {
        if self.shutdown_complete {
            return Ok(());
        }

        let mut shutdown_error = None;
        if !self.failed && !self.shutdown_requested {
            self.shutdown_requested = true;
            if let Err(error) = write_frame(&mut self.stream, &json!({"shutdown": true})) {
                self.failed = true;
                shutdown_error = Some(error);
            } else if let Err(error) = self.stream.shutdown(Shutdown::Write) {
                self.failed = true;
                shutdown_error = Some(VmErr::Msg(format!(
                    "cannot finish Node sidecar shutdown request: {error}"
                )));
            }
        }

        let deadline = Instant::now() + SIDECAR_SHUTDOWN_TIMEOUT;
        while !self.process_reaped && Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.process_reaped = true;
                    if !status.success() && shutdown_error.is_none() {
                        shutdown_error = Some(VmErr::Msg(format!(
                            "Node sidecar exited with status {status} during shutdown"
                        )));
                    }
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => {
                    shutdown_error.get_or_insert_with(|| {
                        VmErr::Msg(format!(
                            "cannot inspect Node sidecar during shutdown: {error}"
                        ))
                    });
                    break;
                }
            }
        }

        if !self.process_reaped {
            self.failed = true;
            let _ = self.stream.shutdown(Shutdown::Both);
            let _ = self.child.kill();
            match self.child.wait() {
                Ok(_) => self.process_reaped = true,
                Err(error) => {
                    shutdown_error.get_or_insert_with(|| {
                        VmErr::Msg(format!("cannot stop Node sidecar: {error}"))
                    });
                }
            }
            shutdown_error.get_or_insert_with(|| {
                VmErr::Msg(
                    "Node sidecar shutdown timed out; the child process was terminated".into(),
                )
            });
        }

        let _ = self.stream.shutdown(Shutdown::Both);
        if let Some(reader) = self.reader.take()
            && reader.join().is_err()
        {
            shutdown_error.get_or_insert_with(|| {
                VmErr::Msg("Node sidecar event reader panicked during shutdown".into())
            });
        }
        self.shutdown_complete = true;
        shutdown_error.map_or(Ok(()), Err)
    }
}

struct StartupChild(Option<Child>);

impl StartupChild {
    fn as_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("startup child is present")
    }

    fn into_inner(mut self) -> Child {
        self.0.take().expect("startup child is present")
    }
}

impl Drop for StartupChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Loads allowlisted `.node` addons by running a Node.js sidecar process.
///
/// Addon functions and constructors invoke synchronously over a bounded
/// bridge. Native object instances use identity-preserving proxy values whose
/// property operations are forwarded to Node. Addon code runs with host
/// privileges and is not contained by the guest sandbox.
#[derive(Clone)]
pub struct NodeAddonSidecar {
    state: Rc<RefCell<State>>,
    runtime_info: NodeAddonRuntimeInfo,
    allowed_roots: Vec<PathBuf>,
    allowed_addons: HashMap<PathBuf, [u8; 32]>,
    wake: Arc<WakeSlot>,
}

/// Runtime versions reported by the Node process hosting native addons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeAddonRuntimeInfo {
    /// The `process.versions.node` value reported by the sidecar.
    pub node_version: String,
    /// The Node-API ABI version reported by `process.versions.napi`.
    pub napi_version: u32,
}

/// Host configuration for enabling CommonJS modules and trusted `.node`
/// addons in a Rust-embedded interpreter.
///
/// JavaScript modules still execute inside napi-vm. Each native addon must be
/// explicitly listed with [`Self::allow_native_addon`] or
/// [`Self::allow_native_addon_with_sha256`]. Its SHA-256 is pinned when the
/// runtime is configured and checked again when it is loaded. A trusted digest
/// can also be supplied by the host to validate the binary before Node starts.
/// The configured Node executable hosts the Node-API environment in a child
/// process, so it must be compatible with the addon's Node-API requirements.
#[derive(Clone, Debug)]
pub struct NodeAddonOptions {
    pub(crate) node_executable: OsString,
    pub(crate) policy: NativeAddonPolicy,
    pub(crate) minimum_napi_version: Option<u32>,
}

impl NodeAddonOptions {
    /// Configure the Node executable and filesystem roots visible to
    /// `require()`. Native addon loading stays disabled until at least one
    /// path is added with [`Self::allow_native_addon`] or
    /// [`Self::allow_native_addon_with_sha256`].
    pub fn new<I, P>(node_executable: impl AsRef<OsStr>, roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        Self::with_policy(node_executable, NativeAddonPolicy::new(roots))
    }

    /// Configure the Node executable and reuse a shared addon policy.
    pub fn with_policy(node_executable: impl AsRef<OsStr>, policy: NativeAddonPolicy) -> Self {
        Self {
            node_executable: node_executable.as_ref().to_owned(),
            policy,
            minimum_napi_version: None,
        }
    }

    /// Trust one specific native addon binary. Its bytes are pinned when the
    /// interpreter is configured. The path must be inside one of `roots`.
    /// Use [`Self::allow_native_addon_with_sha256`] when the host has an
    /// expected digest from a trusted build manifest.
    pub fn allow_native_addon(mut self, path: impl Into<PathBuf>) -> Self {
        self.policy = self.policy.allow_native_addon(path);
        self
    }

    /// Allow a native addon only when its bytes match `expected_sha256` from
    /// trusted host metadata. The loader checks the digest during setup and
    /// again immediately before loading the addon.
    pub fn allow_native_addon_with_sha256(
        mut self,
        path: impl Into<PathBuf>,
        expected_sha256: [u8; 32],
    ) -> Self {
        self.policy = self
            .policy
            .allow_native_addon_with_sha256(path, expected_sha256);
        self
    }

    /// Require the configured Node runtime to provide at least this Node-API
    /// version. The check runs during
    /// [`Interpreter::enable_node_addons`](crate::interpreter::Interpreter::enable_node_addons)
    /// and fails before any guest module or addon is loaded.
    pub fn minimum_napi_version(mut self, version: u32) -> Self {
        self.minimum_napi_version = Some(version);
        self
    }

    /// Set the application entry path used to resolve top-level `require()`.
    /// The path must exist and be inside one of `roots`.
    pub fn entry(mut self, path: impl Into<PathBuf>) -> Self {
        self.policy = self.policy.entry(path);
        self
    }
}

impl std::fmt::Debug for NodeAddonSidecar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeAddonSidecar").finish_non_exhaustive()
    }
}

impl NodeAddonSidecar {
    pub fn new(node_executable: impl AsRef<OsStr>) -> Result<Self, VmErr> {
        Self::new_with_policy(node_executable, Vec::new(), HashMap::new())
    }

    pub(super) fn new_with_policy(
        node_executable: impl AsRef<OsStr>,
        allowed_roots: Vec<PathBuf>,
        allowed_addons: HashMap<PathBuf, [u8; 32]>,
    ) -> Result<Self, VmErr> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| VmErr::Msg(format!("cannot bind Node bridge: {e}")))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| VmErr::Msg(format!("cannot configure Node bridge: {e}")))?;
        let address = listener
            .local_addr()
            .map_err(|e| VmErr::Msg(format!("cannot read Node bridge address: {e}")))?;
        let token = token();
        let mut child = StartupChild(Some(
            Command::new(node_executable)
                .arg("--no-warnings")
                .arg("-e")
                .arg(NODE_BRIDGE)
                .env("NAPI_VM_BRIDGE_PORT", address.port().to_string())
                .env("NAPI_VM_BRIDGE_TOKEN", &token)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|e| VmErr::Msg(format!("cannot start Node sidecar: {e}")))?,
        ));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Some(status) = child
                        .as_mut()
                        .try_wait()
                        .map_err(|e| VmErr::Msg(format!("cannot inspect Node sidecar: {e}")))?
                    {
                        return Err(VmErr::Msg(format!(
                            "Node sidecar exited during startup ({status})"
                        )));
                    }
                    if Instant::now() >= deadline {
                        return Err(VmErr::Msg("timed out starting Node sidecar".into()));
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => {
                    return Err(VmErr::Msg(format!("cannot accept Node bridge: {e}")));
                }
            }
        };
        stream
            .set_read_timeout(None)
            .map_err(|e| VmErr::Msg(format!("cannot configure Node bridge: {e}")))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| VmErr::Msg(format!("cannot set Node bridge timeout: {e}")))?;
        let hello = read_frame(&mut stream)?;
        if hello.get("hello").and_then(JsonValue::as_str) != Some(&token) {
            return Err(VmErr::Msg("Node sidecar authentication failed".into()));
        }
        let runtime_info = NodeAddonRuntimeInfo {
            node_version: hello
                .get("nodeVersion")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("Node sidecar did not report its Node version".into()))?
                .to_string(),
            napi_version: hello
                .get("napiVersion")
                .and_then(JsonValue::as_str)
                .and_then(|version| version.parse().ok())
                .ok_or_else(|| {
                    VmErr::Msg("Node sidecar does not expose a Node-API version".into())
                })?,
        };
        let mut read_stream = stream
            .try_clone()
            .map_err(|e| VmErr::Msg(format!("cannot clone Node bridge stream: {e}")))?;
        let (response_tx, response_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let wake = Arc::new(WakeSlot::new());
        let reader_wake = wake.clone();
        let reader = std::thread::Builder::new()
            .name("napi-vm-node-events".into())
            .spawn(move || {
                loop {
                    let Ok(frame) = read_frame(&mut read_stream) else {
                        break;
                    };
                    let is_event = frame.get("event").is_some();
                    let sender = if is_event { &event_tx } else { &response_tx };
                    if sender.send(frame).is_err() {
                        break;
                    }
                    // Responses complete a synchronous host call already
                    // waiting on the owner thread; only sidecar events need
                    // to wake an idle owner.
                    if is_event {
                        reader_wake.fire();
                    }
                }
            })
            .map_err(|e| VmErr::Msg(format!("cannot start Node bridge reader: {e}")))?;
        Ok(Self {
            state: Rc::new(RefCell::new(State {
                child: child.into_inner(),
                stream,
                reader: Some(reader),
                response_rx,
                event_rx,
                pending_events: VecDeque::new(),
                request_id: 1,
                failed: false,
                shutdown_requested: false,
                shutdown_complete: false,
                process_reaped: false,
                next_local_handle: 0,
                local_handles: HashMap::new(),
                object_proxies: HashMap::new(),
                proxy_ids: HashMap::new(),
                guest_callbacks: HashMap::new(),
                guest_callback_keys: HashMap::new(),
                guest_graph_nodes: HashMap::new(),
                guest_callback_graphs: HashMap::new(),
                host_symbols: HashMap::new(),
                symbol_remote_ids: HashMap::new(),
                next_guest_callback_id: 1,
                native_promises: HashMap::new(),
            })),
            runtime_info,
            allowed_roots,
            allowed_addons,
            wake,
        })
    }

    /// Return the Node and Node-API versions supplied by the hosting process.
    pub fn runtime_info(&self) -> &NodeAddonRuntimeInfo {
        &self.runtime_info
    }

    /// Gracefully stop the Node sidecar, falling back to terminating the child
    /// if its worker does not close within the shutdown deadline.
    pub fn shutdown(&self) -> Result<(), VmErr> {
        self.state.borrow_mut().shutdown()
    }

    /// Whether the sidecar shutdown sequence has completed.
    pub fn is_shutdown(&self) -> bool {
        self.state.borrow().shutdown_complete
    }

    fn preflight_addon_path(&self, filename: &Path) -> Result<PathBuf, VmErr> {
        if self.is_shutdown() {
            return Err(VmErr::Msg("Node sidecar has been shut down".into()));
        }
        let filename = std::fs::canonicalize(filename).map_err(|error| {
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
        crate::interpreter::native_addon_binary::validate_native_addon_binary(&filename)?;
        Ok(filename)
    }

    fn request(&self, message: JsonValue) -> Result<JsonValue, VmErr> {
        self.request_with_callback_handler(
            message,
            &mut |_| {
                Err(VmErr::Msg(
                    "synchronous guest callback was requested outside a VM host call".into(),
                ))
            },
            &WireEncodeContext::default(),
        )
    }

    fn persist_guest_graph(
        &self,
        graph: &WireEncodeContext,
        callback_scope: Option<u64>,
    ) -> Result<(), VmErr> {
        if graph.callbacks.is_empty() && callback_scope.is_none() {
            return Ok(());
        }
        let mut state = self.state.borrow_mut();
        let additional = graph
            .nodes
            .keys()
            .filter(|id| !state.guest_graph_nodes.contains_key(id))
            .count();
        if state.guest_graph_nodes.len().saturating_add(additional) > MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg(
                "persistent guest graph node limit exceeded".into(),
            ));
        }
        for (id, value) in &graph.nodes {
            state
                .guest_graph_nodes
                .entry(*id)
                .or_insert_with(|| value.clone());
        }
        if let Some(callback_id) = callback_scope {
            state
                .guest_callback_graphs
                .entry(callback_id)
                .or_default()
                .extend(graph.nodes.keys().copied());
        }
        let callback_ids = callback_scope
            .map(|callback_id| vec![callback_id])
            .unwrap_or_else(|| graph.callbacks.keys().copied().collect());
        for callback_id in callback_ids {
            state
                .guest_callback_graphs
                .entry(callback_id)
                .or_default()
                .extend(graph.nodes.keys().copied());
        }
        Ok(())
    }

    fn encode_context_for_callback(&self, callback_id: u64) -> WireEncodeContext {
        WireEncodeContext::from_callback_state(&self.state.borrow(), callback_id)
    }

    fn guest_graph_snapshots(
        &self,
        callback_id: u64,
    ) -> Result<(Vec<JsonValue>, WireEncodeContext), VmErr> {
        let (mut graph, nodes) = {
            let state = self.state.borrow();
            let mut graph = WireEncodeContext::from_callback_state(&state, callback_id);
            graph.callbacks.clear();
            let nodes = state
                .guest_callback_graphs
                .get(&callback_id)
                .into_iter()
                .flatten()
                .filter_map(|id| {
                    state
                        .guest_graph_nodes
                        .get(id)
                        .map(|value| (*id, value.clone()))
                })
                .collect::<Vec<_>>();
            (graph, nodes)
        };
        let mut snapshots = Vec::new();
        for (id, value) in nodes {
            if let Some(snapshot) = guest_graph_node_snapshot(self, id, &value, &mut graph)? {
                snapshots.push(snapshot);
            }
        }
        Ok((snapshots, graph))
    }

    fn persist_snapshot_graph(
        &self,
        graph: &WireEncodeContext,
        callback_id: u64,
    ) -> Result<(), VmErr> {
        self.persist_guest_graph(graph, Some(callback_id))?;
        for new_callback_id in graph.callbacks.keys().copied() {
            if new_callback_id != callback_id {
                self.persist_guest_graph(graph, Some(new_callback_id))?;
            }
        }
        Ok(())
    }

    fn request_with_callback_handler(
        &self,
        mut message: JsonValue,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
        guest_graph: &WireEncodeContext,
    ) -> Result<JsonValue, VmErr> {
        self.persist_guest_graph(guest_graph, None)?;
        let id = {
            let mut state = self.state.borrow_mut();
            if state.shutdown_requested || state.shutdown_complete {
                return Err(VmErr::Msg("Node sidecar has been shut down".into()));
            }
            if state.failed {
                return Err(VmErr::Msg(
                    "Node addon bridge is unavailable after a transport failure".into(),
                ));
            }
            let id = state.request_id;
            state.request_id = state
                .request_id
                .checked_add(1)
                .ok_or_else(|| VmErr::Msg("Node request id exhausted".into()))?;
            message
                .as_object_mut()
                .ok_or_else(|| VmErr::Msg("invalid internal Node request".into()))?
                .insert("requestId".into(), json!(id));
            if let Err(error) = write_frame(&mut state.stream, &message) {
                fail_state(&mut state);
                return Err(error);
            }
            id
        };

        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let event = {
                let mut state = self.state.borrow_mut();
                state
                    .pending_events
                    .pop_front()
                    .or_else(|| state.event_rx.try_recv().ok())
            };
            if let Some(event) = event {
                if event.get("event").and_then(JsonValue::as_str) == Some("syncGuestCallback") {
                    self.answer_sync_guest_callback(&event, callback_handler)?;
                } else {
                    self.state.borrow_mut().pending_events.push_back(event);
                }
            }

            let response = {
                let state = self.state.borrow();
                match state.response_rx.try_recv() {
                    Ok(response) => Some(response),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err(VmErr::Msg("Node sidecar disconnected".into()));
                    }
                    Err(mpsc::TryRecvError::Empty) => None,
                }
            };
            if let Some(response) = response {
                if response.get("requestId").and_then(JsonValue::as_u64) != Some(id) {
                    return Err(VmErr::Msg("Node response id mismatch".into()));
                }
                if response.get("ok").and_then(JsonValue::as_bool) == Some(true) {
                    return response
                        .get("value")
                        .cloned()
                        .ok_or_else(|| VmErr::Msg("Node response has no value".into()));
                }
                let error = response.get("error").unwrap_or(&JsonValue::Null);
                let name = error
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("Error");
                let message = error
                    .get("message")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("native addon failed");
                let code = error.get("code").and_then(JsonValue::as_str);
                let error = Value::Error(match code {
                    Some(code) => crate::value::ErrorData::with_code(name, message, code),
                    None => crate::value::ErrorData::new(name, message),
                });
                return Err(VmErr::Throw(error));
            }

            let now = Instant::now();
            if now >= deadline {
                let mut state = self.state.borrow_mut();
                fail_state(&mut state);
                return Err(VmErr::Msg(
                    "Node sidecar did not respond before timeout".into(),
                ));
            }
            let wait = (deadline - now).min(Duration::from_millis(2));
            let receive = {
                let state = self.state.borrow();
                state.response_rx.recv_timeout(wait)
            };
            match receive {
                Ok(response) => {
                    if response.get("requestId").and_then(JsonValue::as_u64) != Some(id) {
                        return Err(VmErr::Msg("Node response id mismatch".into()));
                    }
                    if response.get("ok").and_then(JsonValue::as_bool) == Some(true) {
                        return response
                            .get("value")
                            .cloned()
                            .ok_or_else(|| VmErr::Msg("Node response has no value".into()));
                    }
                    let error = response.get("error").unwrap_or(&JsonValue::Null);
                    let name = error
                        .get("name")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("Error");
                    let message = error
                        .get("message")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("native addon failed");
                    let code = error.get("code").and_then(JsonValue::as_str);
                    let error = Value::Error(match code {
                        Some(code) => crate::value::ErrorData::with_code(name, message, code),
                        None => crate::value::ErrorData::new(name, message),
                    });
                    return Err(VmErr::Throw(error));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(VmErr::Msg("Node sidecar disconnected".into()));
                }
            }
        }
    }

    fn answer_sync_guest_callback(
        &self,
        event: &JsonValue,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<(), VmErr> {
        let call_id = event
            .get("callId")
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| VmErr::Msg("Node synchronous callback id is invalid".into()))?;
        let callback_id = event
            .get("callbackId")
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| VmErr::Msg("Node callback event has an invalid id".into()))?;
        let callback = self.guest_callback_from_event(event)?;
        let result = callback_handler(callback);
        let mut graph = self.encode_context_for_callback(callback_id);
        let (mut ok, mut value) = match result {
            Ok(value) => match self.guest_to_wire_with_context(&value, 0, &mut graph) {
                Ok(value) => (true, value),
                Err(error) => (
                    false,
                    json!({"t":"error","name":"TypeError","message":error.to_string()}),
                ),
            },
            Err(VmErr::Throw(reason)) => {
                match self.guest_to_wire_with_context(&reason, 0, &mut graph) {
                    Ok(value) => (false, value),
                    Err(error) => (
                        false,
                        json!({"t":"error","name":"TypeError","message":error.to_string()}),
                    ),
                }
            }
            Err(error) => (
                false,
                json!({"t":"error","name":"Error","message":error.to_string()}),
            ),
        };
        self.persist_guest_graph(&graph, Some(callback_id))?;
        let snapshots = match self.guest_graph_snapshots(callback_id) {
            Ok((snapshots, snapshot_graph)) => {
                self.persist_snapshot_graph(&snapshot_graph, callback_id)?;
                snapshots
            }
            Err(error) => {
                ok = false;
                value = json!({"t":"error","name":"TypeError","message":error.to_string()});
                Vec::new()
            }
        };
        let response = json!({
            "event":"syncGuestCallbackResult",
            "callId":call_id,
            "ok":ok,
            "value":value,
            "snapshots":snapshots,
        });
        let mut state = self.state.borrow_mut();
        write_frame(&mut state.stream, &response)
    }

    fn guest_callback_from_event(&self, event: &JsonValue) -> Result<HostCallback, VmErr> {
        let callback_id = event
            .get("callbackId")
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| VmErr::Msg("Node callback event has an invalid id".into()))?;
        let (callback, mut graph) = {
            let state = self.state.borrow();
            let callback = state
                .guest_callbacks
                .get(&callback_id)
                .cloned()
                .ok_or_else(|| VmErr::Msg("Node callback handle is invalid".into()))?;
            (
                callback,
                WireDecodeContext::from_callback_state(&state, callback_id),
            )
        };
        if let Some(mutations) = event.get("guestMutations").and_then(JsonValue::as_array) {
            for mutation in mutations {
                apply_guest_mutation(self, mutation, &mut graph)?;
            }
        }
        let this_wire = event
            .get("thisValue")
            .cloned()
            .unwrap_or_else(|| json!({"t":"undefined"}));
        let this_value = wire_to_guest_with_context(self, &this_wire, 0, &mut graph)?;
        let args = event
            .get("args")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| VmErr::Msg("Node callback event has invalid arguments".into()))?
            .iter()
            .map(|arg| wire_to_guest_with_context(self, arg, 0, &mut graph))
            .collect::<Result<Vec<_>, _>>()?;
        let kind = match event.get("kind").and_then(JsonValue::as_str) {
            None | Some("call") => HostCallbackKind::Call,
            Some("construct") => HostCallbackKind::Construct,
            Some(_) => return Err(VmErr::Msg("Node callback event has an invalid kind".into())),
        };
        Ok(HostCallback {
            callback,
            this_value,
            args,
            kind,
        })
    }

    fn wire_to_guest(&self, value: &JsonValue, depth: usize) -> Result<Value, VmErr> {
        wire_to_guest(self, value, depth)
    }

    fn guest_to_wire_with_context(
        &self,
        value: &Value,
        depth: usize,
        graph: &mut WireEncodeContext,
    ) -> Result<JsonValue, VmErr> {
        let proxy_ids = self.state.borrow().proxy_ids.clone();
        guest_to_wire(self, value, depth, graph, &proxy_ids)
    }

    fn register_guest_callback(&self, callback: Value) -> Result<u64, VmErr> {
        let mut state = self.state.borrow_mut();
        let identity = guest_callback_identity(&callback);
        if let Some(id) = identity.and_then(|identity| state.guest_callback_keys.get(&identity)) {
            return Ok(*id);
        }
        if state.guest_callbacks.len() >= MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg("guest callback handle limit exceeded".into()));
        }
        let id = state.next_guest_callback_id;
        state.next_guest_callback_id = state
            .next_guest_callback_id
            .checked_add(1)
            .ok_or_else(|| VmErr::Msg("guest callback handle id exhausted".into()))?;
        state.guest_callbacks.insert(id, callback);
        if let Some(identity) = identity {
            state.guest_callback_keys.insert(identity, id);
        }
        Ok(id)
    }

    fn host_object(&self, object_id: u64) -> Result<Value, VmErr> {
        let mut state = self.state.borrow_mut();
        if let Some(proxy) = state.object_proxies.get(&object_id) {
            return Ok(proxy.clone());
        }
        if state.object_proxies.len() >= MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg("native object handle limit exceeded".into()));
        }

        let target = Value::object(vec![]);
        let mut traps = Vec::with_capacity(5);
        for operation in [
            ObjectOperation::Get,
            ObjectOperation::Set,
            ObjectOperation::Has,
            ObjectOperation::Delete,
            ObjectOperation::OwnKeys,
        ] {
            let local_id = usize::MAX
                .checked_sub(state.next_local_handle)
                .ok_or_else(|| VmErr::Msg("local host handle id exhausted".into()))?;
            state.next_local_handle = state
                .next_local_handle
                .checked_add(1)
                .ok_or_else(|| VmErr::Msg("local host handle id exhausted".into()))?;
            state.local_handles.insert(
                local_id,
                LocalObjectTrap {
                    object_id,
                    operation,
                },
            );
            traps.push((
                operation.guest_name().to_string(),
                Value::host_function(operation.guest_name(), local_id),
            ));
        }
        let handler = Value::object(traps);
        let proxy_data = Rc::new(crate::value::ProxyData { target, handler });
        let proxy_id = Rc::as_ptr(&proxy_data) as usize;
        let proxy = Value::Proxy(proxy_data);
        state.proxy_ids.insert(proxy_id, object_id);
        state.object_proxies.insert(object_id, proxy.clone());
        Ok(proxy)
    }

    fn host_promise(&self, promise_id: u64) -> Result<Value, VmErr> {
        let mut state = self.state.borrow_mut();
        if let Some(promise) = state.native_promises.get(&promise_id) {
            return Ok(Value::Promise(promise.clone()));
        }
        if state.native_promises.len() >= MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg("native promise handle limit exceeded".into()));
        }
        let promise = Value::pending_promise();
        promise.borrow_mut().external_pending = true;
        state.native_promises.insert(promise_id, promise.clone());
        Ok(Value::Promise(promise))
    }

    fn dispatch_object_trap_with_callback_handler(
        &self,
        trap: LocalObjectTrap,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        let mut graph = WireEncodeContext::default();
        let proxy_ids = self.state.borrow().proxy_ids.clone();
        let request = match trap.operation {
            ObjectOperation::Get => json!({
                "op": "get",
                "id": trap.object_id,
                "key": required_string_arg(&args, 1)?,
            }),
            ObjectOperation::Set => json!({
                "op": "set",
                "id": trap.object_id,
                "key": required_string_arg(&args, 1)?,
                "value": guest_to_wire(
                    self,
                    args.get(2).unwrap_or(&Value::Undefined),
                    0,
                    &mut graph,
                    &proxy_ids,
                )?,
            }),
            ObjectOperation::Has => json!({
                "op": "has",
                "id": trap.object_id,
                "key": required_string_arg(&args, 1)?,
            }),
            ObjectOperation::Delete => json!({
                "op": "delete",
                "id": trap.object_id,
                "key": required_string_arg(&args, 1)?,
            }),
            ObjectOperation::OwnKeys => json!({
                "op": "ownKeys",
                "id": trap.object_id,
            }),
        };
        let result = self.request_with_callback_handler(request, callback_handler, &graph)?;
        self.wire_to_guest(&result, 0)
    }
}

fn fail_state(state: &mut State) {
    state.failed = true;
    state.shutdown_requested = true;
    let _ = state.stream.shutdown(Shutdown::Both);
    let _ = state.child.kill();
    if state.child.wait().is_ok() {
        state.process_reaped = true;
    }
}

impl NativeAddonLoader for NodeAddonSidecar {
    fn preflight_addon(&self, filename: &Path) -> Result<(), VmErr> {
        self.preflight_addon_path(filename).map(|_| ())
    }

    fn load(&self, filename: &Path) -> Result<Value, VmErr> {
        let filename = self.preflight_addon_path(filename)?;
        let filename = filename
            .to_str()
            .ok_or_else(|| VmErr::Msg("native addon path is not UTF-8".into()))?;
        self.wire_to_guest(&self.request(json!({"op":"load","filename":filename}))?, 0)
    }
}

impl HostBridge for NodeAddonSidecar {
    fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        if self.is_shutdown() {
            return Ok(Vec::new());
        }
        let events = {
            let mut state = self.state.borrow_mut();
            let mut events = Vec::new();
            events.extend(state.pending_events.drain(..));
            if events.is_empty() {
                let first = if timeout.is_zero() {
                    state.event_rx.try_recv().ok()
                } else {
                    state.event_rx.recv_timeout(timeout).ok()
                };
                if let Some(event) = first {
                    events.push(event);
                    events.extend(state.event_rx.try_iter());
                }
            }
            events
        };

        let mut host_events = Vec::with_capacity(events.len());
        for event in events {
            match event.get("event").and_then(JsonValue::as_str) {
                Some("guestCallbackError") => {
                    let error = event.get("error").unwrap_or(&JsonValue::Null);
                    let name = error
                        .get("name")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("Error");
                    let message = error
                        .get("message")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("guest callback arguments could not be marshalled");
                    return Err(VmErr::Throw(Value::Error(crate::value::ErrorData::new(
                        name, message,
                    ))));
                }
                Some("guestCallback") => {}
                Some("syncGuestCallback") => {
                    return Err(VmErr::Msg(
                        "synchronous guest callback arrived outside a host call".into(),
                    ));
                }
                Some("hostPromiseSettled") => {
                    let promise_id = event
                        .get("promiseId")
                        .and_then(JsonValue::as_u64)
                        .ok_or_else(|| VmErr::Msg("Node promise event has an invalid id".into()))?;
                    let promise = self
                        .state
                        .borrow()
                        .native_promises
                        .get(&promise_id)
                        .cloned()
                        .ok_or_else(|| VmErr::Msg("Node promise handle is invalid".into()))?;
                    let state = match event.get("state").and_then(JsonValue::as_str) {
                        Some("fulfilled") => PromiseState::Fulfilled,
                        Some("rejected") => PromiseState::Rejected,
                        _ => {
                            return Err(VmErr::Msg(
                                "Node promise event has an invalid state".into(),
                            ));
                        }
                    };
                    let wire = event
                        .get("value")
                        .ok_or_else(|| VmErr::Msg("Node promise event has no value".into()))?;
                    host_events.push(HostEvent::PromiseSettled {
                        promise,
                        state,
                        value: self.wire_to_guest(wire, 0)?,
                    });
                    continue;
                }
                other => {
                    return Err(VmErr::Msg(format!(
                        "unknown Node sidecar event {:?}",
                        other
                    )));
                }
            }

            host_events.push(HostEvent::Callback(self.guest_callback_from_event(&event)?));
        }
        Ok(host_events)
    }

    fn set_wake_notifier(&self, notifier: WakeNotifier) {
        if self.is_shutdown() {
            return;
        }
        self.wake.set(notifier);
    }

    fn has_pending_host_work(&self, promise: &Rc<RefCell<PromiseInner>>) -> bool {
        promise.borrow().external_pending
    }

    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        self.call_host_with_this(id, Value::Undefined, args)
    }

    fn call_host_with_this(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        self.call_host_with_callback_handler(id, this_value, args, &mut |_| {
            Err(VmErr::Msg(
                "synchronous guest callback was requested outside a VM host call".into(),
            ))
        })
    }

    fn call_host_with_callback_handler(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        let trap = self.state.borrow().local_handles.get(&id).copied();
        if let Some(trap) = trap {
            return self.dispatch_object_trap_with_callback_handler(trap, args, callback_handler);
        }
        let proxy_ids = self.state.borrow().proxy_ids.clone();
        let mut graph = WireEncodeContext::default();
        let args = args
            .iter()
            .map(|v| guest_to_wire(self, v, 0, &mut graph, &proxy_ids))
            .collect::<Result<Vec<_>, _>>()?;
        let receiver = guest_to_wire(self, &this_value, 0, &mut graph, &proxy_ids)?;
        let response = self.request_with_callback_handler(
            json!({"op":"call","id":id,"args":args,"receiver":receiver}),
            callback_handler,
            &graph,
        )?;
        guest_call_result_to_value(self, &response, &graph)
    }
    fn construct_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        self.construct_host_with_callback_handler(id, args, &mut |_| {
            Err(VmErr::Msg(
                "synchronous guest callback was requested outside a VM host call".into(),
            ))
        })
    }
    fn construct_host_with_callback_handler(
        &self,
        id: usize,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        let proxy_ids = self.state.borrow().proxy_ids.clone();
        let mut graph = WireEncodeContext::default();
        let args = args
            .iter()
            .map(|v| guest_to_wire(self, v, 0, &mut graph, &proxy_ids))
            .collect::<Result<Vec<_>, _>>()?;
        let response = self.request_with_callback_handler(
            json!({"op":"construct","id":id,"args":args}),
            callback_handler,
            &graph,
        )?;
        guest_call_result_to_value(self, &response, &graph)
    }
}

fn token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}-{time:x}-{:x}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
fn write_frame(stream: &mut TcpStream, value: &JsonValue) -> Result<(), VmErr> {
    let body = serde_json::to_vec(value)
        .map_err(|e| VmErr::Msg(format!("cannot encode Node frame: {e}")))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(VmErr::Msg("Node frame exceeds size limit".into()));
    }
    let size = u32::try_from(body.len()).map_err(|_| VmErr::Msg("Node frame too large".into()))?;
    stream
        .write_all(&size.to_be_bytes())
        .and_then(|()| stream.write_all(&body))
        .map_err(|e| VmErr::Msg(format!("Node sidecar write failed: {e}")))
}
fn read_frame(stream: &mut TcpStream) -> Result<JsonValue, VmErr> {
    let mut header = [0; 4];
    stream
        .read_exact(&mut header)
        .map_err(|e| VmErr::Msg(format!("Node sidecar disconnected: {e}")))?;
    let size = u32::from_be_bytes(header) as usize;
    if size > MAX_FRAME_BYTES {
        return Err(VmErr::Msg("Node frame exceeds size limit".into()));
    }
    let mut body = vec![0; size];
    stream
        .read_exact(&mut body)
        .map_err(|e| VmErr::Msg(format!("Node sidecar disconnected: {e}")))?;
    serde_json::from_slice(&body).map_err(|e| VmErr::Msg(format!("invalid Node frame: {e}")))
}
