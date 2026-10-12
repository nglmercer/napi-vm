use crate::{PluginFuture, PluginResult, RpcError, normalize_value};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::{Notify, mpsc, oneshot},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Validated framing, pending request, queue and lifecycle timeout resource bounds.
pub struct Limits {
    pub max_frame_bytes: usize,
    pub max_depth: usize,
    pub max_pending_calls: usize,
    pub max_queued_bytes: usize,
    pub max_call_chain_length: usize,
    pub call_timeout_ms: u64,
    pub shutdown_timeout_ms: u64,
    pub startup_timeout_ms: u64,
    pub max_log_bytes: usize,
    pub max_event_queue: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_frame_bytes: 8 * 1024 * 1024,
            max_depth: 64,
            max_pending_calls: 256,
            max_queued_bytes: 16 * 1024 * 1024,
            max_call_chain_length: 16,
            call_timeout_ms: 30_000,
            shutdown_timeout_ms: 5_000,
            startup_timeout_ms: 10_000,
            max_log_bytes: 1024 * 1024,
            max_event_queue: 256,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> PluginResult<()> {
        if self.max_frame_bytes == 0
            || self.max_frame_bytes > 64 * 1024 * 1024
            || self.max_depth == 0
            || self.max_depth > 64
            || self.max_pending_calls == 0
            || self.max_pending_calls > 65536
            || self.max_queued_bytes < self.max_frame_bytes
            || self.max_queued_bytes > 256 * 1024 * 1024
            || self.max_call_chain_length == 0
            || self.max_call_chain_length > 64
            || self.call_timeout_ms == 0
            || self.call_timeout_ms > 86_400_000
            || self.shutdown_timeout_ms == 0
            || self.startup_timeout_ms == 0
            || self.max_log_bytes > 64 * 1024 * 1024
            || self.max_event_queue == 0
            || self.max_event_queue > 65536
        {
            return Err(RpcError::new(
                "INVALID_ARGUMENT",
                "invalid transport limits",
            ));
        }
        Ok(())
    }
    pub fn stricter(&self, b: &Self) -> Self {
        Self {
            max_frame_bytes: self.max_frame_bytes.min(b.max_frame_bytes),
            max_depth: self.max_depth.min(b.max_depth),
            max_pending_calls: self.max_pending_calls.min(b.max_pending_calls),
            max_queued_bytes: self.max_queued_bytes.min(b.max_queued_bytes),
            max_call_chain_length: self.max_call_chain_length.min(b.max_call_chain_length),
            call_timeout_ms: self.call_timeout_ms.min(b.call_timeout_ms),
            shutdown_timeout_ms: self.shutdown_timeout_ms.min(b.shutdown_timeout_ms),
            startup_timeout_ms: self.startup_timeout_ms.min(b.startup_timeout_ms),
            max_log_bytes: self.max_log_bytes.min(b.max_log_bytes),
            max_event_queue: self.max_event_queue.min(b.max_event_queue),
        }
    }
}
#[derive(Clone, Default, Debug)]
pub struct Cancellation(Arc<CancelState>);
#[derive(Default, Debug)]
struct CancelState {
    cancelled: AtomicBool,
    notify: Notify,
}
impl Cancellation {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        if !self.0.cancelled.swap(true, Ordering::AcqRel) {
            self.0.notify.notify_waiters();
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }
    pub fn check(&self) -> PluginResult<()> {
        if self.is_cancelled() {
            Err(RpcError::new(
                "CANCELLED",
                "call was cancelled; completed effects are not rolled back",
            ))
        } else {
            Ok(())
        }
    }
    pub async fn cancelled(&self) {
        loop {
            let n = self.0.notify.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            n.await;
        }
    }
}
#[derive(Clone)]
pub struct RequestContext {
    pub id: String,
    pub cancellation: Cancellation,
}
pub type RequestHandler =
    Arc<dyn Fn(String, Value, RequestContext) -> PluginFuture<'static, Value> + Send + Sync>;
pub type EventHandler = Arc<dyn Fn(Value) -> PluginResult<()> + Send + Sync>;
pub type ResultHandler = Box<dyn FnOnce(&Value) -> PluginResult<()> + Send>;
struct PendingGuard {
    peer: Peer,
    id: String,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        if self
            .peer
            .0
            .pending
            .lock()
            .unwrap()
            .remove(&self.id)
            .is_some()
        {
            let _ = self.peer.notify("system.cancel", json!({"id":self.id}));
        }
    }
}
struct Pending {
    send: oneshot::Sender<PluginResult<Value>>,
    business: bool,
    accept_result: Option<ResultHandler>,
}
struct QueuedFrame {
    request_id: Option<String>,
    data: Vec<u8>,
    bytes: Arc<AtomicUsize>,
    ack: Option<oneshot::Sender<()>>,
}
impl Drop for QueuedFrame {
    fn drop(&mut self) {
        self.bytes.fetch_sub(self.data.len(), Ordering::AcqRel);
    }
}
struct Inner {
    session: String,
    prefix: char,
    limits: RwLock<Limits>,
    control: mpsc::Sender<QueuedFrame>,
    business: mpsc::Sender<QueuedFrame>,
    events: mpsc::Sender<QueuedFrame>,
    event_fence: Mutex<Option<String>>,
    event_fence_changed: Notify,
    control_bytes: Arc<AtomicUsize>,
    business_bytes: Arc<AtomicUsize>,
    counter: AtomicU64,
    pending: Mutex<HashMap<String, Pending>>,
    active: Mutex<HashMap<String, (Cancellation, usize, bool)>>,
    active_bytes: AtomicUsize,
    handler: RequestHandler,
    event_handler: RwLock<Option<EventHandler>>,
    closed: Cancellation,
    reason: Mutex<Option<RpcError>>,
    late_responses: AtomicU64,
    unknown_notifications: AtomicU64,
}
#[derive(Clone)]
pub struct Peer(Arc<Inner>);
#[derive(Clone)]
pub struct WeakPeer(std::sync::Weak<Inner>);
impl WeakPeer {
    pub fn upgrade(&self) -> Option<Peer> {
        self.0.upgrade().map(Peer)
    }
}
impl Peer {
    pub fn new(
        stream: TcpStream,
        session: String,
        prefix: char,
        limits: Limits,
        handler: RequestHandler,
    ) -> PluginResult<Self> {
        limits.validate()?;
        if !matches!(prefix, 'h' | 'p') || session.is_empty() || session.len() > 128 {
            return Err(RpcError::new("INVALID_ARGUMENT", "invalid peer identity"));
        }
        let (ctx, mut crx) = mpsc::channel::<QueuedFrame>(64);
        let (btx, mut brx) = mpsc::channel::<QueuedFrame>(limits.max_pending_calls);
        let (etx, mut erx) = mpsc::channel::<QueuedFrame>(limits.max_event_queue);
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let p = Self(Arc::new(Inner {
            session,
            prefix,
            limits: RwLock::new(limits),
            control: ctx,
            business: btx,
            events: etx,
            event_fence: Mutex::new(None),
            event_fence_changed: Notify::new(),
            control_bytes: queued_bytes.clone(),
            business_bytes: queued_bytes,
            counter: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
            active_bytes: AtomicUsize::new(0),
            handler,
            event_handler: RwLock::new(None),
            closed: Cancellation::new(),
            reason: Mutex::new(None),
            late_responses: AtomicU64::new(0),
            unknown_notifications: AtomicU64::new(0),
        }));
        let (mut rd, mut wr) = stream.into_split();
        let writer = p.clone();
        tokio::spawn(async move {
            loop {
                let events_enabled = writer.0.event_fence.lock().unwrap().is_none();
                // Control retains priority; normal RPC and events share fair scheduling.
                let frame = tokio::select! {biased;_ = writer.0.closed.cancelled()=>break,_=writer.0.event_fence_changed.notified()=>continue,v=crx.recv()=>v,v=async { tokio::select! {v=brx.recv()=>v,v=erx.recv(),if events_enabled=>v} }=>v};
                let Some(mut frame) = frame else {
                    break;
                };
                if frame
                    .request_id
                    .as_ref()
                    .is_some_and(|id| !writer.0.pending.lock().unwrap().contains_key(id))
                {
                    continue;
                }
                let result = tokio::select! {biased;_=writer.0.closed.cancelled()=>break,r=wr.write_all(&frame.data)=>r};
                if let Err(e) = result {
                    writer.close(e.into());
                    break;
                }
                if let Some(ack) = frame.ack.take() {
                    let _ = ack.send(());
                }
            }
            let _ = wr.shutdown().await;
        });
        let reader = p.clone();
        tokio::spawn(async move {
            loop {
                let lim = reader.limits();
                let result = tokio::select! {biased;_=reader.0.closed.cancelled()=>break,v=read_frame(&mut rd,lim.max_frame_bytes,lim.max_depth)=>v};
                match result {
                    Ok(v) => {
                        if let Err(e) = reader.receive(v) {
                            reader.close(e);
                            break;
                        }
                    }
                    Err(e) => {
                        reader.close(e);
                        break;
                    }
                }
            }
        });
        Ok(p)
    }
    pub fn downgrade(&self) -> WeakPeer {
        WeakPeer(Arc::downgrade(&self.0))
    }
    pub fn session_id(&self) -> &str {
        &self.0.session
    }
    pub fn endpoint_id(&self) -> String {
        format!("{}:{}", self.0.prefix, self.0.session)
    }
    pub fn remote_endpoint_id(&self) -> String {
        format!(
            "{}:{}",
            if self.0.prefix == 'h' { 'p' } else { 'h' },
            self.0.session
        )
    }
    pub fn limits(&self) -> Limits {
        self.0.limits.read().unwrap().clone()
    }
    pub fn negotiate_limits(&self, limits: Limits) -> PluginResult<()> {
        limits.validate()?;
        let mut current = self.0.limits.write().unwrap();
        *current = current.stricter(&limits);
        Ok(())
    }
    pub fn set_event_handler(&self, h: EventHandler) {
        *self.0.event_handler.write().unwrap() = Some(h);
    }
    /// Hold outbound events in their bounded queue until this successful reply is queued.
    /// Other requests and responses remain live during initialization.
    pub fn hold_events_until_reply(&self, method: &str) {
        *self.0.event_fence.lock().unwrap() = Some(method.to_string());
        self.0.event_fence_changed.notify_one();
    }
    fn release_event_fence(&self, method: &str) {
        let mut fence = self.0.event_fence.lock().unwrap();
        if fence.as_deref() == Some(method) {
            *fence = None;
            self.0.event_fence_changed.notify_one();
        }
    }
    pub fn is_closed(&self) -> bool {
        self.0.closed.is_cancelled()
    }
    pub fn late_response_count(&self) -> u64 {
        self.0.late_responses.load(Ordering::Relaxed)
    }
    pub fn unknown_notification_count(&self) -> u64 {
        self.0.unknown_notifications.load(Ordering::Relaxed)
    }
    pub fn close(&self, error: RpcError) {
        let mut reason = self.0.reason.lock().unwrap();
        if reason.is_some() {
            return;
        }
        *reason = Some(error.clone());
        drop(reason);
        self.0.closed.cancel();
        for (_, p) in self.0.pending.lock().unwrap().drain() {
            let _ = p.send.send(Err(error.clone()));
        }
        for (c, _, _) in self.0.active.lock().unwrap().values() {
            c.cancel();
        }
    }
    pub async fn closed(&self) -> RpcError {
        self.0.closed.cancelled().await;
        self.0
            .reason
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| RpcError::new("CONNECTION_CLOSED", "peer closed"))
    }
    pub fn cancellation(&self) -> Cancellation {
        self.0.closed.clone()
    }
    fn enqueue(&self, value: Value, priority: bool) -> PluginResult<()> {
        if self.is_closed() {
            return Err(RpcError::new("CONNECTION_CLOSED", "peer is closed"));
        }
        let limits = self.limits();
        let data = encode_frame(&value, limits.max_frame_bytes, limits.max_depth)?;
        let (tx, bytes, max) = if priority {
            (
                &self.0.control,
                &self.0.control_bytes,
                limits.max_queued_bytes + 1024 * 1024,
            )
        } else {
            (
                if value.get("method").and_then(Value::as_str) == Some("system.event") {
                    &self.0.events
                } else {
                    &self.0.business
                },
                &self.0.business_bytes,
                limits.max_queued_bytes,
            )
        };
        bytes
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(data.len()).filter(|&v| v <= max)
            })
            .map_err(|_| RpcError::new("OVERLOADED", "outbound queued byte limit exceeded"))?;
        let request_id = if value.get("method").is_some() {
            value.get("id").and_then(Value::as_str).map(str::to_string)
        } else {
            None
        };
        tx.try_send(QueuedFrame {
            request_id,
            data,
            bytes: bytes.clone(),
            ack: None,
        })
        .map_err(|e| {
            RpcError::new(
                "OVERLOADED",
                format!(
                    "outbound queue rejected frame: {}",
                    if matches!(e, mpsc::error::TrySendError::Full(_)) {
                        "full"
                    } else {
                        "closed"
                    }
                ),
            )
        })
    }
    pub async fn flush(&self) -> PluginResult<()> {
        let (tx, rx) = oneshot::channel();
        self.0
            .control
            .try_send(QueuedFrame {
                request_id: None,
                data: Vec::new(),
                bytes: self.0.control_bytes.clone(),
                ack: Some(tx),
            })
            .map_err(|_| RpcError::new("OVERLOADED", "control flush queue is full"))?;
        tokio::select! {r=rx=>r.map_err(|_|RpcError::new("CONNECTION_CLOSED","flush failed")),_=self.0.closed.cancelled()=>Err(RpcError::new("CONNECTION_CLOSED","peer closed"))}
    }
    pub fn notify(&self, method: &str, params: Value) -> PluginResult<()> {
        self.enqueue(
            json!({"jsonrpc":"2.0","method":method,"params":params}),
            method != "system.event",
        )
    }
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancellation: Option<Cancellation>,
    ) -> PluginResult<Value> {
        self.request_with_result_handler(method, params, timeout, cancellation, None)
            .await
    }
    /// Validate/admit a successful response in wire order, before the next frame.
    /// The handler must be synchronous and must not block the transport reader.
    pub async fn request_with_result_handler(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancellation: Option<Cancellation>,
        accept_result: Option<ResultHandler>,
    ) -> PluginResult<Value> {
        let business = method == "system.invoke";
        let lim = self.limits();
        let timeout = timeout.min(Duration::from_millis(if business {
            lim.call_timeout_ms
        } else {
            timeout.as_millis().min(u64::MAX as u128) as u64
        }));
        let count = self
            .0
            .counter
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| {
                RpcError::new(
                    "OVERLOADED",
                    "request counter exhausted; create a new session",
                )
            })?;
        let id = format!("{}:{}:{}", self.0.prefix, self.0.session, count);
        let (send, receive) = oneshot::channel();
        {
            let mut pending = self.0.pending.lock().unwrap();
            let n = pending.values().filter(|p| p.business == business).count();
            if n >= if business { lim.max_pending_calls } else { 32 } {
                return Err(RpcError::new(
                    "OVERLOADED",
                    "outbound pending call limit exceeded",
                ));
            }
            pending.insert(
                id.clone(),
                Pending {
                    send,
                    business,
                    accept_result,
                },
            );
        }
        let _pending_guard = PendingGuard {
            peer: self.clone(),
            id: id.clone(),
        };
        if let Err(e) = self.enqueue(
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            !business,
        ) {
            self.0.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        let cancel = cancellation.unwrap_or_default();
        let result = tokio::select! {biased;r=receive=>r.unwrap_or_else(|_|Err(RpcError::new("CONNECTION_CLOSED","response channel closed"))),_=cancel.cancelled()=>Err(RpcError::new("CANCELLED","local call cancelled; remote effects may have occurred")),_=tokio::time::sleep(timeout)=>Err(RpcError::new("DEADLINE_EXCEEDED","local deadline exceeded; remote effects may have occurred"))};
        let removed = self.0.pending.lock().unwrap().remove(&id).is_some();
        if removed {
            let _ = self.notify("system.cancel", json!({"id":id}));
        }
        result
    }
    fn receive(&self, v: Value) -> PluginResult<()> {
        let o = v.as_object().ok_or_else(|| {
            RpcError::new(
                "INVALID_REQUEST",
                "batch and non-object RPC envelopes are unsupported",
            )
        })?;
        if v["jsonrpc"] != "2.0" {
            return Err(RpcError::new("INVALID_REQUEST", "jsonrpc must be 2.0"));
        }
        if let Some(method) = o.get("method") {
            let method = method
                .as_str()
                .filter(|m| !m.is_empty() && m.len() <= 128)
                .ok_or_else(|| RpcError::new("INVALID_REQUEST", "method must be bounded string"))?;
            if o.keys()
                .any(|k| !["jsonrpc", "id", "method", "params"].contains(&k.as_str()))
                || !v["params"].is_object()
            {
                return Err(RpcError::new(
                    "INVALID_REQUEST",
                    "invalid request fields or params",
                ));
            }
            if let Some(id) = o.get("id") {
                let id = id
                    .as_str()
                    .ok_or_else(|| {
                        RpcError::new("INVALID_REQUEST", "request IDs must be namespaced strings")
                    })?
                    .to_string();
                self.check_id(&id, false)?;
                let business = method == "system.invoke";
                let size = serde_json::to_vec(&v).map_err(RpcError::from)?.len();
                let limits = self.limits();
                let cancel = Cancellation::new();
                {
                    let mut active = self.0.active.lock().unwrap();
                    if active.contains_key(&id) {
                        return Err(RpcError::new(
                            "INVALID_REQUEST",
                            "duplicate active request ID",
                        ));
                    }
                    if active.values().filter(|(_, _, b)| *b == business).count()
                        >= if business {
                            limits.max_pending_calls
                        } else {
                            32
                        }
                        || active
                            .values()
                            .filter(|(_, _, b)| *b == business)
                            .map(|(_, n, _)| *n)
                            .sum::<usize>()
                            .saturating_add(size)
                            > limits.max_queued_bytes
                    {
                        drop(active);
                        return self.respond(
                            &id,
                            Err(RpcError::new(
                                "OVERLOADED",
                                "inbound active request bounds exceeded",
                            )),
                        );
                    }
                    self.0.active_bytes.fetch_add(size, Ordering::AcqRel);
                    active.insert(id.clone(), (cancel.clone(), size, business));
                }
                let h = self.0.handler.clone();
                let peer = self.clone();
                let method = method.to_string();
                let params = v["params"].clone();
                tokio::spawn(async move {
                    let shutdown = method == "system.shutdown";
                    let response_method = method.clone();
                    let context = RequestContext {
                        id: id.clone(),
                        cancellation: cancel,
                    };
                    let call = tokio::spawn(async move { h(method, params, context).await });
                    let result = match call.await {
                        Ok(v) => v,
                        Err(e) => Err(RpcError::new(
                            "INTERNAL_ERROR",
                            format!("handler task failed: {e}"),
                        )),
                    };
                    if let Some((_, n, _)) = peer.0.active.lock().unwrap().remove(&id) {
                        peer.0.active_bytes.fetch_sub(n, Ordering::AcqRel);
                    }
                    let succeeded = result.is_ok();
                    let shutdown = shutdown && succeeded;
                    if let Err(e) = peer.respond(&id, result) {
                        peer.close(e);
                    } else {
                        if succeeded {
                            peer.release_event_fence(&response_method);
                        }
                        if shutdown {
                            let _ = peer.flush().await;
                            peer.close(RpcError::new(
                                "CONNECTION_CLOSED",
                                "graceful shutdown completed",
                            ));
                        }
                    }
                });
            } else {
                match method {
                    "system.cancel" => {
                        if v["params"].as_object().is_none_or(|o| o.len() != 1) {
                            return Err(RpcError::new(
                                "INVALID_REQUEST",
                                "cancellation requires only id",
                            ));
                        }
                        let id = v["params"]["id"].as_str().ok_or_else(|| {
                            RpcError::new("INVALID_REQUEST", "cancellation requires id")
                        })?;
                        self.check_id(id, false)?;
                        if let Some((c, _, _)) = self.0.active.lock().unwrap().get(id) {
                            c.cancel();
                        }
                    }
                    "system.event" => {
                        let h = self.0.event_handler.read().unwrap().clone();
                        if let Some(h) = h {
                            h(v["params"].clone())?;
                        }
                    }
                    _ => {
                        self.0.unknown_notifications.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        } else {
            if o.keys()
                .any(|k| !["jsonrpc", "id", "result", "error"].contains(&k.as_str()))
                || o.contains_key("result") == o.contains_key("error")
            {
                return Err(RpcError::new(
                    "INVALID_REQUEST",
                    "response requires exactly one result/error",
                ));
            }
            let id = v["id"]
                .as_str()
                .ok_or_else(|| RpcError::new("INVALID_REQUEST", "response id must be string"))?;
            self.check_id(id, true)?;
            let result = if o.contains_key("result") {
                Ok(v["result"].clone())
            } else {
                let e: RpcError = serde_json::from_value(v["error"].clone())
                    .map_err(|_| RpcError::new("INVALID_REQUEST", "malformed RPC error"))?;
                if e.message.len() > 16384
                    || !e.data.is_object()
                    || e.data["code"].as_str().is_none()
                {
                    return Err(RpcError::new("INVALID_REQUEST", "invalid error metadata"));
                }
                Err(e)
            };
            let pending = self.0.pending.lock().unwrap().remove(id);
            if let Some(p) = pending {
                // A cancelled/timed-out waiter may have dropped its receiver just
                // before its pending guard can remove this entry on another task.
                if p.send.is_closed() {
                    self.0.late_responses.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
                let result = result.and_then(|value| {
                    if let Some(accept_result) = p.accept_result {
                        accept_result(&value)?;
                    }
                    Ok(value)
                });
                let _ = p.send.send(result);
            } else {
                self.0.late_responses.fetch_add(1, Ordering::Relaxed);
            }
        }
        Ok(())
    }
    fn check_id(&self, id: &str, outbound: bool) -> PluginResult<()> {
        let prefix = if outbound {
            self.0.prefix
        } else if self.0.prefix == 'h' {
            'p'
        } else {
            'h'
        };
        let p = format!("{prefix}:{}:", self.0.session);
        let valid = id.strip_prefix(&p).is_some_and(|n| {
            !n.is_empty()
                && n != "0"
                && n.len() <= 20
                && n.bytes().all(|b| b.is_ascii_digit())
                && (n.len() == 1 || !n.starts_with('0'))
                && n.parse::<u64>().is_ok()
        });
        if !valid {
            return Err(RpcError::new(
                "INVALID_REQUEST",
                "request ID namespace or counter is invalid",
            ));
        }
        Ok(())
    }
    fn respond(&self, id: &str, result: PluginResult<Value>) -> PluginResult<()> {
        let v = match result {
            Ok(r) => json!({"jsonrpc":"2.0","id":id,"result":r}),
            Err(e) => json!({"jsonrpc":"2.0","id":id,"error":e}),
        };
        self.enqueue(v, true)
    }
}
pub fn encode_frame(value: &Value, max: usize, depth: usize) -> PluginResult<Vec<u8>> {
    let mut v = value.clone();
    normalize_value(&mut v, 0, depth)?;
    let data = serde_json::to_vec(&v)?;
    if data.is_empty() || data.len() > max || data.len() > u32::MAX as usize {
        return Err(RpcError::new("OVERLOADED", "frame size limit exceeded"));
    }
    let mut out = Vec::with_capacity(data.len() + 4);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&data);
    Ok(out)
}
pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    max: usize,
    depth: usize,
) -> PluginResult<Value> {
    let mut header = [0u8; 4];
    reader.read_exact(&mut header).await?;
    let n = u32::from_be_bytes(header) as usize;
    if n == 0 || n > max {
        return Err(RpcError::new(
            "INVALID_REQUEST",
            "frame length is zero or exceeds cap",
        ));
    }
    let mut bytes = vec![0; n];
    reader.read_exact(&mut bytes).await?;
    let s = std::str::from_utf8(&bytes)
        .map_err(|_| RpcError::new("PARSE_ERROR", "frame contains invalid UTF-8"))?;
    let mut v: Value =
        serde_json::from_str(s).map_err(|e| RpcError::new("PARSE_ERROR", e.to_string()))?;
    normalize_value(&mut v, 0, depth)?;
    Ok(v)
}
pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
    max: usize,
    depth: usize,
) -> PluginResult<()> {
    writer.write_all(&encode_frame(value, max, depth)?).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    #[tokio::test]
    async fn event_fence_orders_reply_first_and_preserves_host_callbacks() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut remote, _) = listener.accept().await.unwrap();
        let handler: RequestHandler = Arc::new(|_, _, _| Box::pin(async { Ok(Value::Null) }));
        let peer = Peer::new(stream, "fence".into(), 'p', Limits::default(), handler).unwrap();
        peer.hold_events_until_reply("system.initialize");
        peer.notify("system.event", json!({"sequence":"1"}))
            .unwrap();
        let callback = peer.request("system.invoke", json!({}), Duration::from_secs(1), None);
        tokio::pin!(callback);
        let outgoing = tokio::select! {
            value = &mut callback => panic!("callback settled before response: {value:?}"),
            frame = read_frame(&mut remote, 1024, 64) => frame.unwrap(),
        };
        assert_eq!(outgoing["method"], "system.invoke");
        peer.receive(json!({"jsonrpc":"2.0","id":outgoing["id"],"result":null}))
            .unwrap();
        callback.await.unwrap();
        peer.receive(
            json!({"jsonrpc":"2.0","id":"h:fence:1","method":"system.initialize","params":{}}),
        )
        .unwrap();
        let reply = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut remote, 1024, 64))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply["id"], "h:fence:1");
        assert_eq!(reply["result"], Value::Null);
        let event = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut remote, 1024, 64))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(event["method"], "system.event");
        peer.close(RpcError::new("CONNECTION_CLOSED", "test finished"));
    }
    #[tokio::test]
    async fn held_events_share_budget_and_failure_or_close_discards_them() {
        for fail_initialize in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let stream = TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap();
            let (mut remote, _) = listener.accept().await.unwrap();
            let handler: RequestHandler = Arc::new(|_, _, _| {
                Box::pin(async { Err(RpcError::new("INTERNAL_ERROR", "initialize failed")) })
            });
            let limits = Limits {
                max_frame_bytes: 256,
                max_queued_bytes: 256,
                ..Default::default()
            };
            let peer = Peer::new(stream, "failed-fence".into(), 'p', limits, handler).unwrap();
            peer.hold_events_until_reply("system.initialize");
            let event = json!({"value":"x".repeat(80)});
            peer.notify("system.event", event.clone()).unwrap();
            assert_eq!(
                peer.notify("system.event", event)
                    .unwrap_err()
                    .stable_code(),
                "OVERLOADED"
            );
            let held = peer.0.business_bytes.load(Ordering::Acquire);
            assert!(held > 0 && held <= peer.limits().max_queued_bytes);
            assert_eq!(held, peer.0.control_bytes.load(Ordering::Acquire));
            if fail_initialize {
                peer.receive(json!({"jsonrpc":"2.0","id":"h:failed-fence:1","method":"system.initialize","params":{}})).unwrap();
                let reply =
                    tokio::time::timeout(Duration::from_secs(1), read_frame(&mut remote, 1024, 64))
                        .await
                        .unwrap()
                        .unwrap();
                assert_eq!(reply["error"]["data"]["code"], "INTERNAL_ERROR");
                assert!(peer.0.event_fence.lock().unwrap().is_some());
            }
            peer.close(RpcError::new("CONNECTION_CLOSED", "test finished"));
            tokio::time::timeout(Duration::from_secs(1), async {
                while peer.0.business_bytes.load(Ordering::Acquire) != 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_secs(1), read_frame(&mut remote, 1024, 64))
                    .await
                    .unwrap()
                    .is_err()
            );
        }
    }
    #[tokio::test]
    async fn dropped_response_waiter_cannot_run_lifecycle_admission() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (_remote, _) = listener.accept().await.unwrap();
        let handler: RequestHandler = Arc::new(|_, _, _| Box::pin(async { Ok(Value::Null) }));
        let peer = Peer::new(stream, "dropped".into(), 'h', Limits::default(), handler).unwrap();
        let admitted = Arc::new(AtomicBool::new(false));
        let admission = admitted.clone();
        let (send, receive) = oneshot::channel();
        peer.0.pending.lock().unwrap().insert(
            "h:dropped:1".into(),
            Pending {
                send,
                business: false,
                accept_result: Some(Box::new(move |_| {
                    admission.store(true, Ordering::Release);
                    Ok(())
                })),
            },
        );
        // Model the interval after the waiter is dropped but before its guard
        // acquires the pending mutex to remove the request.
        drop(receive);
        peer.receive(json!({"jsonrpc":"2.0","id":"h:dropped:1","result":null}))
            .unwrap();
        assert!(!admitted.load(Ordering::Acquire));
        assert_eq!(peer.late_response_count(), 1);
        peer.close(RpcError::new("CONNECTION_CLOSED", "test finished"));
    }
    #[tokio::test]
    async fn result_admission_precedes_following_event_without_polling_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut remote, _) = listener.accept().await.unwrap();
        let handler: RequestHandler = Arc::new(|_, _, _| Box::pin(async { Ok(Value::Null) }));
        let peer = Peer::new(stream, "order".into(), 'h', Limits::default(), handler).unwrap();
        let ready = Arc::new(AtomicBool::new(false));
        let event_ready = ready.clone();
        peer.set_event_handler(Arc::new(move |_| {
            if event_ready.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err(RpcError::new("NOT_READY", "event preceded admission"))
            }
        }));
        let admitted = ready.clone();
        let request = peer.request_with_result_handler(
            "system.initialize",
            json!({}),
            Duration::from_secs(1),
            None,
            Some(Box::new(move |value| {
                assert!(value.is_null());
                admitted.store(true, Ordering::Release);
                Ok(())
            })),
        );
        tokio::pin!(request);
        let outgoing = tokio::select! {
            result = &mut request => panic!("request settled before response: {result:?}"),
            frame = read_frame(&mut remote, 1024, 64) => frame.unwrap(),
        };
        // No await between frames: the waiting request cannot win a scheduler race.
        peer.receive(json!({"jsonrpc":"2.0","id":outgoing["id"],"result":null}))
            .unwrap();
        peer.receive(json!({"jsonrpc":"2.0","method":"system.event","params":{}}))
            .unwrap();
        assert!(ready.load(Ordering::Acquire));
        assert_eq!(request.await.unwrap(), Value::Null);
        peer.close(RpcError::new("CONNECTION_CLOSED", "test finished"));
    }
    #[tokio::test]
    async fn invalid_failed_and_late_results_cannot_admit_a_lifecycle() {
        for mode in ["invalid", "timeout", "cancel", "closed", "remote-error"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let stream = TcpStream::connect(listener.local_addr().unwrap())
                .await
                .unwrap();
            let (mut remote, _) = listener.accept().await.unwrap();
            let handler: RequestHandler = Arc::new(|_, _, _| Box::pin(async { Ok(Value::Null) }));
            let peer = Peer::new(stream, mode.into(), 'h', Limits::default(), handler).unwrap();
            let admitted = Arc::new(AtomicBool::new(false));
            let admission = admitted.clone();
            let cancellation = Cancellation::new();
            let request = peer.request_with_result_handler(
                "system.initialize",
                json!({}),
                Duration::from_millis(if mode == "timeout" { 50 } else { 1000 }),
                Some(cancellation.clone()),
                Some(Box::new(move |value| {
                    if !value.is_null() {
                        return Err(RpcError::new(
                            "INVALID_RESULT",
                            "initialize must return null",
                        ));
                    }
                    admission.store(true, Ordering::Release);
                    Ok(())
                })),
            );
            tokio::pin!(request);
            let outgoing = tokio::select! {
                result = &mut request => panic!("request settled before response: {result:?}"),
                frame = read_frame(&mut remote, 1024, 64) => frame.unwrap(),
            };
            if mode == "cancel" {
                cancellation.cancel();
            }
            if mode == "closed" {
                peer.close(RpcError::new("CONNECTION_CLOSED", "test closed"));
            }
            let late = matches!(mode, "timeout" | "cancel" | "closed");
            if late {
                assert!(request.as_mut().await.is_err());
            }
            let reply = if mode == "remote-error" {
                json!({"jsonrpc":"2.0","id":outgoing["id"],"error":RpcError::new("INTERNAL_ERROR", "initialize failed")})
            } else {
                json!({"jsonrpc":"2.0","id":outgoing["id"],"result":if mode == "invalid" { json!({}) } else { Value::Null }})
            };
            peer.receive(reply).unwrap();
            if !late {
                assert!(request.await.is_err());
            }
            assert!(!admitted.load(Ordering::Acquire), "{mode}");
            peer.close(RpcError::new("CONNECTION_CLOSED", "test finished"));
        }
    }
    #[tokio::test]
    async fn fragmented_frames_and_bounds() {
        let (a, mut b) = tokio::io::duplex(256);
        let mut a = a;
        let bytes = encode_frame(&json!({"text":"😀"}), 1024, 64).unwrap();
        tokio::spawn(async move {
            for v in bytes {
                b.write_all(&[v]).await.unwrap();
            }
        });
        assert_eq!(
            read_frame(&mut a, 1024, 64).await.unwrap(),
            json!({"text":"😀"})
        );
        let (mut a, mut b) = tokio::io::duplex(8);
        b.write_all(&0u32.to_be_bytes()).await.unwrap();
        assert!(read_frame(&mut a, 8, 64).await.is_err());
    }
    #[tokio::test]
    async fn requests_out_of_order_and_cancellation() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(l.local_addr().unwrap()).await.unwrap();
        let (server, _) = l.accept().await.unwrap();
        let h: RequestHandler = Arc::new(|m, v, c| {
            Box::pin(async move {
                if m == "wait" {
                    c.cancellation.cancelled().await;
                    Err(RpcError::new("CANCELLED", "cancelled"))
                } else {
                    Ok(v)
                }
            })
        });
        let a = Peer::new(client, "test".into(), 'h', Limits::default(), h.clone()).unwrap();
        let b = Peer::new(server, "test".into(), 'p', Limits::default(), h).unwrap();
        let slow = a.request("wait", json!({}), Duration::from_millis(20), None);
        let fast = a.request("echo", json!({"v":1}), Duration::from_secs(1), None);
        let (s, f) = tokio::join!(slow, fast);
        assert_eq!(s.unwrap_err().stable_code(), "DEADLINE_EXCEEDED");
        assert_eq!(f.unwrap(), json!({"v":1}));
        a.close(RpcError::new("CONNECTION_CLOSED", "test done"));
        b.closed().await;
    }
    #[tokio::test]
    async fn control_and_business_share_one_reserved_byte_budget() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (_remote, _) = listener.accept().await.unwrap();
        let handler: RequestHandler = Arc::new(|_, _, _| Box::pin(async { Ok(Value::Null) }));
        let peer = Peer::new(stream, "budget".into(), 'h', Limits::default(), handler).unwrap();
        let payload = json!({"data":"x".repeat(5*1024*1024)});
        for _ in 0..3 {
            peer.notify("system.event", payload.clone()).unwrap();
        }
        let before = peer.0.business_bytes.load(Ordering::Acquire);
        assert!(before <= peer.limits().max_queued_bytes);
        assert_eq!(before, peer.0.control_bytes.load(Ordering::Acquire));
        assert_eq!(
            peer.notify("system.ping", payload)
                .unwrap_err()
                .stable_code(),
            "OVERLOADED"
        );
        assert_eq!(before, peer.0.control_bytes.load(Ordering::Acquire));
        peer.notify("system.ping", json!({"data":"y".repeat(1536*1024)}))
            .unwrap();
        let total = peer.0.control_bytes.load(Ordering::Acquire);
        assert!(total > peer.limits().max_queued_bytes);
        assert!(total <= peer.limits().max_queued_bytes + 1024 * 1024);
        assert_eq!(
            peer.notify("system.event", json!({}))
                .unwrap_err()
                .stable_code(),
            "OVERLOADED"
        );
        peer.close(RpcError::new("CONNECTION_CLOSED", "budget test complete"));
    }
}
