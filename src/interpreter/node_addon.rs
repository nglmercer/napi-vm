use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value as JsonValue, json};

use crate::error::VmErr;
use crate::host::{HostBridge, HostCallback, HostEvent};
use crate::interpreter::NativeAddonLoader;
use crate::value::{
    MAX_ARRAY_LEN, MAX_OBJECT_PROPS, MAX_STRING_LEN, PromiseInner, PromiseState, Value,
};

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_WIRE_DEPTH: usize = 128;
const MAX_NATIVE_HANDLES: usize = 262_144;

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

const NODE_BRIDGE: &str = r#"
'use strict';
const net = require('node:net');
const socket = net.connect({host:'127.0.0.1',port:Number(process.env.NAPI_VM_BRIDGE_PORT)});
let input = Buffer.alloc(0);
let serial = Promise.resolve();
let nextHandle = 1;
const refs = new Map();
const objectIds = new WeakMap();
const functionIds = new WeakMap();
const promiseIds = new WeakMap();
let dispatchDepth = 0;
function hold(value, receiver) {
  const kind=typeof value;
  if(kind==='function'){
    const existing=functionIds.get(value);
    if(existing!==undefined)return existing;
  }else{
    const existing=objectIds.get(value);
    if(existing!==undefined)return existing;
  }
  if (refs.size >= 262144) throw new RangeError('native function handle limit exceeded');
  const id=nextHandle++;
  refs.set(id,{value,receiver});
  if(kind==='function')functionIds.set(value,id);
  else objectIds.set(value,id);
  return id;
}
function send(message) {
  const body = Buffer.from(JSON.stringify(message));
  const header = Buffer.allocUnsafe(4);
  header.writeUInt32BE(body.length, 0);
  socket.write(Buffer.concat([header, body]));
}
function encode(value, receiver, depth, active) {
  if (depth > 128) throw new RangeError('bridge depth exceeded');
  if (value === undefined) return {t:'undefined'};
  if (value === null) return {t:'null'};
  if (typeof value === 'boolean') return {t:'boolean',v:value};
  if (typeof value === 'number') return {t:'number',v:Object.is(value,-0)?'-0':String(value)};
  if (typeof value === 'string') return {t:'string',v:value};
  if (typeof value === 'bigint') return {t:'bigint',v:value.toString()};
  if (typeof value === 'symbol') throw new TypeError('symbols are unsupported');
  if (typeof value === 'function') {
    return {t:'function',v:hold(value,receiver),n:value.name};
  }
  if (value && typeof value.then === 'function') {
    let id=promiseIds.get(value);
    if(id===undefined){
      id=hold(value,undefined);
      promiseIds.set(value,id);
      Promise.resolve(value).then(
        result=>send({event:'hostPromiseSettled',promiseId:id,state:'fulfilled',value:encode(result,undefined,0,new Set())}),
        reason=>send({event:'hostPromiseSettled',promiseId:id,state:'rejected',value:encode(reason,undefined,0,new Set())})
      ).catch(error=>send({event:'hostPromiseSettled',promiseId:id,state:'rejected',value:{t:'error',name:'TypeError',message:'native promise result could not be marshalled: '+String(error)}}));
    }
    return {t:'hostPromise',v:id};
  }
  if (Buffer.isBuffer(value)) return {t:'bytes',v:Array.from(value)};
  if (ArrayBuffer.isView(value)) return {t:'bytes',v:Array.from(new Uint8Array(value.buffer,value.byteOffset,value.byteLength))};
  if (value instanceof ArrayBuffer) return {t:'bytes',v:Array.from(new Uint8Array(value))};
  if (active.has(value)) throw new TypeError('cyclic native values are unsupported');
  if (Array.isArray(value)) {
    if (value.length>262144) throw new RangeError('native array exceeds the VM limit');
    active.add(value);
    const result={t:'array',v:Array.from(value,v=>encode(v,value,depth+1,active))};
    active.delete(value);
    return result;
  }
  return {t:'hostObject',v:hold(value,undefined)};
}
function decode(value,depth) {
  if(depth>128)throw new RangeError('guest argument depth exceeded');
  switch(value.t) {
    case 'undefined':return undefined; case 'null':return null;
    case 'boolean':return value.v;
    case 'number':if(value.v==='-0')return -0;if(value.v==='NaN')return NaN;if(value.v==='Infinity')return Infinity;if(value.v==='-Infinity')return -Infinity;return Number(value.v);
    case 'string':return value.v; case 'bigint':return BigInt(value.v);
    case 'bytes':return Buffer.from(value.v);
    case 'hostObject':{const entry=refs.get(value.v);if(!entry||!entry.value||typeof entry.value!=='object')throw new TypeError('native object handle is invalid');return entry.value;}
    case 'guestCallback':return function(...args){
      if(dispatchDepth>0){const e=new TypeError('synchronous guest callbacks are unsupported');e.code='ERR_NAPI_VM_SYNC_GUEST_CALLBACK_UNSUPPORTED';throw e;}
      try{send({event:'guestCallback',callbackId:value.v,thisValue:encode(this,undefined,0,new Set()),args:args.map(v=>encode(v,undefined,0,new Set()))});}
      catch(e){send({event:'guestCallbackError',callbackId:value.v,error:{name:typeof e?.name==='string'?e.name:'Error',message:typeof e?.message==='string'?e.message:String(e)}});}
      return undefined;
    };
    case 'function':{const entry=refs.get(value.v);if(!entry||typeof entry.value!=='function')throw new TypeError('native function handle is invalid');return entry.value;}
    case 'array':return value.v.map(v=>decode(v,depth+1));
    case 'object':{const o={};for(const [k,v]of value.v)Object.defineProperty(o,k,{value:decode(v,depth+1),enumerable:true,writable:true,configurable:true});return o;}
    default:throw new TypeError('unsupported napi-vm argument');
  }
}
async function dispatch(r) {
  dispatchDepth++;
  try {
    let result, receiver;
    if(r.op==='load')result=require(r.filename);
    else if(['get','set','has','delete','ownKeys'].includes(r.op)){
      const entry=refs.get(r.id);if(!entry||!entry.value||typeof entry.value!=='object')throw new Error('native object handle is invalid');
      const object=entry.value;
      if(r.op==='get'){result=Reflect.get(object,r.key,object);receiver=object;}
      else if(r.op==='set')result=Reflect.set(object,r.key,decode(r.value,0),object);
      else if(r.op==='has')result=Reflect.has(object,r.key);
      else if(r.op==='delete')result=Reflect.deleteProperty(object,r.key);
      else result=Object.keys(object);
    }
    else {
      const entry=refs.get(r.id);if(!entry||typeof entry.value!=='function')throw new Error('native function handle is invalid');
      const args=r.args.map(v=>decode(v,0));
      if(r.op==='construct')result=Reflect.construct(entry.value,args);
      else result=Reflect.apply(entry.value,Object.hasOwn(r,'receiver')?decode(r.receiver,0):entry.receiver,args);
    }
    return {requestId:r.requestId,ok:true,value:encode(result,receiver,0,new Set())};
  } catch(e) {
    return {requestId:r.requestId,ok:false,error:{name:typeof e?.name==='string'?e.name:'Error',message:typeof e?.message==='string'?e.message:String(e),code:typeof e?.code==='string'?e.code:undefined}};
  } finally {
    dispatchDepth--;
  }
}
function consume() {
  while(input.length>=4) {
    const n=input.readUInt32BE(0);if(n>16777216){socket.destroy(new Error('frame too large'));return;}
    if(input.length<n+4)return;
    const body=input.subarray(4,n+4);input=input.subarray(n+4);
    let r;try{r=JSON.parse(body.toString('utf8'));}catch(e){socket.destroy(e);return;}
    serial=serial.then(()=>dispatch(r)).then(send).catch(e=>socket.destroy(e));
  }
}
socket.on('connect',()=>send({hello:process.env.NAPI_VM_BRIDGE_TOKEN}));
socket.on('data',c=>{input=Buffer.concat([input,c]);consume();});
socket.on('error',e=>process.stderr.write('napi-vm sidecar: '+e.message+'\n'));
"#;

struct State {
    child: Child,
    stream: TcpStream,
    reader: Option<JoinHandle<()>>,
    response_rx: Receiver<JsonValue>,
    event_rx: Receiver<JsonValue>,
    request_id: u64,
    failed: bool,
    next_local_handle: usize,
    local_handles: HashMap<usize, LocalObjectTrap>,
    object_proxies: HashMap<u64, Value>,
    proxy_ids: HashMap<usize, u64>,
    guest_callbacks: HashMap<u64, Value>,
    next_guest_callback_id: u64,
    native_promises: HashMap<u64, Rc<RefCell<PromiseInner>>>,
}
impl Drop for State {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
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
}

impl std::fmt::Debug for NodeAddonSidecar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeAddonSidecar").finish_non_exhaustive()
    }
}

impl NodeAddonSidecar {
    pub fn new(node_executable: impl AsRef<OsStr>) -> Result<Self, VmErr> {
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
        let mut read_stream = stream
            .try_clone()
            .map_err(|e| VmErr::Msg(format!("cannot clone Node bridge stream: {e}")))?;
        let (response_tx, response_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let reader = std::thread::Builder::new()
            .name("napi-vm-node-events".into())
            .spawn(move || {
                loop {
                    let Ok(frame) = read_frame(&mut read_stream) else {
                        break;
                    };
                    let sender = if frame.get("event").is_some() {
                        &event_tx
                    } else {
                        &response_tx
                    };
                    if sender.send(frame).is_err() {
                        break;
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
                request_id: 1,
                failed: false,
                next_local_handle: 0,
                local_handles: HashMap::new(),
                object_proxies: HashMap::new(),
                proxy_ids: HashMap::new(),
                guest_callbacks: HashMap::new(),
                next_guest_callback_id: 1,
                native_promises: HashMap::new(),
            })),
        })
    }

    fn request(&self, mut message: JsonValue) -> Result<JsonValue, VmErr> {
        let mut state = self.state.borrow_mut();
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
        let response = match state.response_rx.recv_timeout(Duration::from_secs(60)) {
            Ok(response) => response,
            Err(error) => {
                fail_state(&mut state);
                return Err(VmErr::Msg(format!("Node sidecar did not respond: {error}")));
            }
        };
        if response.get("requestId").and_then(JsonValue::as_u64) != Some(id) {
            fail_state(&mut state);
            return Err(VmErr::Msg("Node response id mismatch".into()));
        }
        if response.get("ok").and_then(JsonValue::as_bool) == Some(true) {
            return match response.get("value").cloned() {
                Some(value) => Ok(value),
                None => {
                    fail_state(&mut state);
                    Err(VmErr::Msg("Node response has no value".into()))
                }
            };
        }
        let e = response.get("error").unwrap_or(&JsonValue::Null);
        let name = e.get("name").and_then(JsonValue::as_str).unwrap_or("Error");
        let message = e
            .get("message")
            .and_then(JsonValue::as_str)
            .unwrap_or("native addon failed");
        let code = e.get("code").and_then(JsonValue::as_str);
        let error = Value::Error(match code {
            Some(code) => crate::value::ErrorData::with_code(name, message, code),
            None => crate::value::ErrorData::new(name, message),
        });
        Err(VmErr::Throw(error))
    }

    fn wire_to_guest(&self, value: &JsonValue, depth: usize) -> Result<Value, VmErr> {
        wire_to_guest(self, value, depth)
    }

    fn guest_to_wire(&self, value: &Value, depth: usize) -> Result<JsonValue, VmErr> {
        let proxy_ids = self.state.borrow().proxy_ids.clone();
        guest_to_wire(self, value, depth, &mut Vec::new(), &proxy_ids)
    }

    fn register_guest_callback(&self, callback: Value) -> Result<u64, VmErr> {
        let mut state = self.state.borrow_mut();
        if state.guest_callbacks.len() >= MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg("guest callback handle limit exceeded".into()));
        }
        let id = state.next_guest_callback_id;
        state.next_guest_callback_id = state
            .next_guest_callback_id
            .checked_add(1)
            .ok_or_else(|| VmErr::Msg("guest callback handle id exhausted".into()))?;
        state.guest_callbacks.insert(id, callback);
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
                Value::HostFunction {
                    name: operation.guest_name().into(),
                    id: local_id,
                },
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

    fn dispatch_object_trap(
        &self,
        trap: LocalObjectTrap,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
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
                "value": self.guest_to_wire(args.get(2).unwrap_or(&Value::Undefined), 0)?,
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
        let result = self.request(request)?;
        self.wire_to_guest(&result, 0)
    }
}

fn fail_state(state: &mut State) {
    state.failed = true;
    let _ = state.stream.shutdown(Shutdown::Both);
    let _ = state.child.kill();
    let _ = state.child.wait();
}

impl NativeAddonLoader for NodeAddonSidecar {
    fn load(&self, filename: &Path) -> Result<Value, VmErr> {
        let filename = filename
            .to_str()
            .ok_or_else(|| VmErr::Msg("native addon path is not UTF-8".into()))?;
        self.wire_to_guest(&self.request(json!({"op":"load","filename":filename}))?, 0)
    }
}

impl HostBridge for NodeAddonSidecar {
    fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        let events = {
            let state = self.state.borrow_mut();
            let first = if timeout.is_zero() {
                state.event_rx.try_recv().ok()
            } else {
                state.event_rx.recv_timeout(timeout).ok()
            };
            let mut events = Vec::new();
            if let Some(event) = first {
                events.push(event);
                events.extend(state.event_rx.try_iter());
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

            let callback_id = event
                .get("callbackId")
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| VmErr::Msg("Node callback event has an invalid id".into()))?;
            let callback = self
                .state
                .borrow()
                .guest_callbacks
                .get(&callback_id)
                .cloned()
                .ok_or_else(|| VmErr::Msg("Node callback handle is invalid".into()))?;
            let this_wire = event
                .get("thisValue")
                .cloned()
                .unwrap_or_else(|| json!({"t":"undefined"}));
            let this_value = self.wire_to_guest(&this_wire, 0)?;
            let args = event
                .get("args")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("Node callback event has invalid arguments".into()))?
                .iter()
                .map(|arg| self.wire_to_guest(arg, 0))
                .collect::<Result<Vec<_>, _>>()?;
            host_events.push(HostEvent::Callback(HostCallback {
                callback,
                this_value,
                args,
            }));
        }
        Ok(host_events)
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
        let trap = self.state.borrow().local_handles.get(&id).copied();
        if let Some(trap) = trap {
            return self.dispatch_object_trap(trap, args);
        }
        let args = args
            .iter()
            .map(|v| self.guest_to_wire(v, 0))
            .collect::<Result<Vec<_>, _>>()?;
        let receiver = self.guest_to_wire(&this_value, 0)?;
        self.wire_to_guest(
            &self.request(json!({"op":"call","id":id,"args":args,"receiver":receiver}))?,
            0,
        )
    }
    fn construct_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        let args = args
            .iter()
            .map(|v| self.guest_to_wire(v, 0))
            .collect::<Result<Vec<_>, _>>()?;
        self.wire_to_guest(
            &self.request(json!({"op":"construct","id":id,"args":args}))?,
            0,
        )
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

fn required_string_arg(args: &[Value], index: usize) -> Result<String, VmErr> {
    match args.get(index) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(VmErr::Msg("native object property key is invalid".into())),
    }
}

fn guest_to_wire(
    sidecar: &NodeAddonSidecar,
    v: &Value,
    depth: usize,
    active: &mut Vec<usize>,
    proxy_ids: &HashMap<usize, u64>,
) -> Result<JsonValue, VmErr> {
    if depth > MAX_WIRE_DEPTH {
        return Err(VmErr::Msg("guest value exceeds bridge depth limit".into()));
    }
    Ok(match v {
        Value::Undefined => json!({"t":"undefined"}),
        Value::Null => json!({"t":"null"}),
        Value::Bool(x) => json!({"t":"boolean","v":x}),
        Value::Number(x) => {
            json!({"t":"number","v":if x.is_nan(){"NaN".into()}else if *x==f64::INFINITY{"Infinity".into()}else if *x==f64::NEG_INFINITY{"-Infinity".into()}else if *x==0.0&&x.is_sign_negative(){"-0".into()}else{x.to_string()}})
        }
        Value::String(x) => {
            if x.len() > MAX_STRING_LEN {
                return Err(VmErr::Msg("guest string exceeds bridge limit".into()));
            }
            json!({"t":"string","v":x})
        }
        Value::Array(a) => {
            let id = Rc::as_ptr(a) as usize;
            if active.contains(&id) {
                return Err(VmErr::Msg("cyclic guest arguments are unsupported".into()));
            }
            let items = a.borrow().clone();
            if items.len() > MAX_ARRAY_LEN {
                return Err(VmErr::Msg("guest array exceeds limit".into()));
            }
            active.push(id);
            let wire = items
                .iter()
                .map(|x| guest_to_wire(sidecar, x, depth + 1, active, proxy_ids))
                .collect::<Result<Vec<_>, _>>()?;
            active.pop();
            json!({"t":"array","v":wire})
        }
        Value::Object { props } => {
            let id = Rc::as_ptr(props) as usize;
            if active.contains(&id) {
                return Err(VmErr::Msg("cyclic guest arguments are unsupported".into()));
            }
            let entries = props.borrow().clone();
            if entries.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg("guest object exceeds limit".into()));
            }
            active.push(id);
            let wire = entries
                .iter()
                .filter(|(k, _)| !crate::interpreter::is_internal_key(k))
                .map(|(k, x)| {
                    Ok(json!([
                        k,
                        guest_to_wire(sidecar, x, depth + 1, active, proxy_ids)?
                    ]))
                })
                .collect::<Result<Vec<_>, VmErr>>()?;
            active.pop();
            json!({"t":"object","v":wire})
        }
        Value::ArrayBuffer(bytes) => json!({"t":"bytes","v":bytes.borrow().as_slice()}),
        Value::BigInt(x) => json!({"t":"bigint","v":x.to_string()}),
        Value::Proxy(proxy) => {
            let proxy_id = Rc::as_ptr(proxy) as usize;
            match proxy_ids.get(&proxy_id) {
                Some(object_id) => json!({"t":"hostObject","v":object_id}),
                None => {
                    return Err(VmErr::Msg(
                        "guest-created proxies cannot cross the Node addon bridge yet".into(),
                    ));
                }
            }
        }
        Value::Function(_) => {
            let callback_id = sidecar.register_guest_callback(v.clone())?;
            json!({"t":"guestCallback","v":callback_id})
        }
        Value::HostFunction { id, .. } => {
            if sidecar.state.borrow().local_handles.contains_key(id) {
                return Err(VmErr::Msg(
                    "native object proxy traps cannot be passed as callbacks".into(),
                ));
            }
            json!({"t":"function","v":id})
        }
        _ => {
            return Err(VmErr::Msg(
                "this guest value cannot cross the Node addon bridge yet".into(),
            ));
        }
    })
}

fn wire_to_guest(sidecar: &NodeAddonSidecar, v: &JsonValue, depth: usize) -> Result<Value, VmErr> {
    if depth > MAX_WIRE_DEPTH {
        return Err(VmErr::Msg(
            "native result exceeds bridge depth limit".into(),
        ));
    }
    let t = v
        .get("t")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| VmErr::Msg("invalid Node value tag".into()))?;
    match t {
        "undefined" => Ok(Value::Undefined),
        "null" => Ok(Value::Null),
        "boolean" => v
            .get("v")
            .and_then(JsonValue::as_bool)
            .map(Value::Bool)
            .ok_or_else(|| VmErr::Msg("invalid Node boolean".into())),
        "number" => {
            let s = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node number".into()))?;
            let n = match s {
                "NaN" => f64::NAN,
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                "-0" => -0.0,
                _ => s
                    .parse()
                    .map_err(|e| VmErr::Msg(format!("invalid Node number: {e}")))?,
            };
            Ok(Value::Number(n))
        }
        "string" => v
            .get("v")
            .and_then(JsonValue::as_str)
            .map(|s| Value::String(s.to_string()))
            .ok_or_else(|| VmErr::Msg("invalid Node string".into())),
        "bigint" => {
            let s = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node BigInt".into()))?;
            let n = crate::bigint::BigInt::parse(s).map_err(VmErr::Msg)?;
            Ok(Value::BigInt(Rc::new(n)))
        }
        "hostObject" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| VmErr::Msg("invalid Node object id".into()))?;
            sidecar.host_object(id)
        }
        "hostPromise" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| VmErr::Msg("invalid Node promise id".into()))?;
            sidecar.host_promise(id)
        }
        "error" => {
            let name = v.get("name").and_then(JsonValue::as_str).unwrap_or("Error");
            let message = v
                .get("message")
                .and_then(JsonValue::as_str)
                .unwrap_or("native promise result could not be marshalled");
            Ok(Value::Error(crate::value::ErrorData::new(name, message)))
        }
        "bytes" => {
            let bytes = v
                .get("v")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("invalid Node bytes".into()))?
                .iter()
                .map(|b| {
                    b.as_u64()
                        .filter(|n| *n <= 255)
                        .map(|n| n as u8)
                        .ok_or_else(|| VmErr::Msg("invalid Node byte".into()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Value::ArrayBuffer(Rc::new(RefCell::new(bytes))))
        }
        "function" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| VmErr::Msg("invalid Node function id".into()))?;
            Ok(Value::HostFunction {
                name: v
                    .get("n")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("nodeAddon")
                    .into(),
                id,
            })
        }
        "array" => {
            let a = v
                .get("v")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("invalid Node array".into()))?;
            if a.len() > MAX_ARRAY_LEN {
                return Err(VmErr::Msg("Node array exceeds VM limit".into()));
            }
            Value::checked_array(
                a.iter()
                    .map(|x| wire_to_guest(sidecar, x, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
        "object" => {
            let a = v
                .get("v")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("invalid Node object".into()))?;
            if a.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg("Node object exceeds VM limit".into()));
            }
            let mut props = Vec::with_capacity(a.len());
            for item in a {
                let pair = item
                    .as_array()
                    .filter(|p| p.len() == 2)
                    .ok_or_else(|| VmErr::Msg("invalid Node property".into()))?;
                let key = pair[0]
                    .as_str()
                    .ok_or_else(|| VmErr::Msg("invalid Node property key".into()))?;
                props.push((
                    key.to_string(),
                    wire_to_guest(sidecar, &pair[1], depth + 1)?,
                ));
            }
            Value::checked_object(props)
        }
        _ => Err(VmErr::Msg(format!("unknown Node value tag '{t}'"))),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::interpreter::{FileCommonJsLoader, Interpreter};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command as ProcessCommand;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn loads_and_invokes_a_real_node_api_addon() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-node-addon-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let node = ProcessCommand::new("node").arg("--version").output();
        let cc = ProcessCommand::new("cc").arg("--version").output();
        let mut include_dirs = Vec::new();
        if let Some(include) = std::env::var_os("NODE_INCLUDE_DIR") {
            include_dirs.push(PathBuf::from(include));
        }
        include_dirs.push(PathBuf::from("/usr/include/node"));
        include_dirs.push(PathBuf::from("/usr/local/include/node"));
        let include = include_dirs
            .into_iter()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(node), Ok(cc), Some(include)) = (node, cc, include) else {
            eprintln!(
                "skipping real Node-API addon test: Node, cc, or Node headers are unavailable"
            );
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(node.status.success(), "node --version failed");
        assert!(cc.status.success(), "cc --version failed");

        let source = root.join("fixture.c");
        let addon = root.join("fixture.node");
        fs::write(
            &source,
            r#"
#include <node_api.h>
#include <stdlib.h>
#include <unistd.h>

static napi_value add(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2];
  double left = 0, right = 0;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2) return NULL;
  if (napi_get_value_double(env, argv[0], &left) != napi_ok) return NULL;
  if (napi_get_value_double(env, argv[1], &right) != napi_ok) return NULL;
  napi_value result;
  if (napi_create_double(env, left + right, &result) != napi_ok) return NULL;
  return result;
}

static napi_value echo(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1) return NULL;
  return argv[0];
}

static napi_value big(napi_env env, napi_callback_info info) {
  napi_value result;
  if (napi_create_bigint_int64(env, 9007199254740993LL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value fail(napi_env env, napi_callback_info info) {
  napi_throw_type_error(env, "E_FIXTURE", "fixture failure");
  return NULL;
}

static napi_value promise_result(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, result;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok) return NULL;
  if (napi_create_string_utf8(env, "native-promise-value", NAPI_AUTO_LENGTH, &result) != napi_ok) return NULL;
  if (napi_resolve_deferred(env, deferred, result) != napi_ok) return NULL;
  return promise;
}

static napi_value promise_reject(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, reason;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok) return NULL;
  if (napi_create_string_utf8(env, "native-rejection", NAPI_AUTO_LENGTH, &reason) != napi_ok) return NULL;
  if (napi_reject_deferred(env, deferred, reason) != napi_ok) return NULL;
  return promise;
}

static napi_value promise_pending(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok) return NULL;
  return promise;
}

static napi_value counter_constructor(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], self;
  if (napi_get_cb_info(env, info, &argc, argv, &self, NULL) != napi_ok) return NULL;
  napi_value initial;
  if (argc > 0) initial = argv[0];
  else if (napi_create_double(env, 0, &initial) != napi_ok) return NULL;
  if (napi_set_named_property(env, self, "count", initial) != napi_ok) return NULL;
  return self;
}

static napi_value counter_increment(napi_env env, napi_callback_info info) {
  napi_value self, value, next;
  if (napi_get_cb_info(env, info, NULL, NULL, &self, NULL) != napi_ok) return NULL;
  double count = 0;
  if (napi_get_named_property(env, self, "count", &value) != napi_ok) return NULL;
  if (napi_get_value_double(env, value, &count) != napi_ok) return NULL;
  if (napi_create_double(env, count + 1, &next) != napi_ok) return NULL;
  if (napi_set_named_property(env, self, "count", next) != napi_ok) return NULL;
  return next;
}

static napi_value counter_self(napi_env env, napi_callback_info info) {
  napi_value self;
  if (napi_get_cb_info(env, info, NULL, NULL, &self, NULL) != napi_ok) return NULL;
  return self;
}

typedef struct {
  napi_async_work work;
  napi_ref callback;
} callback_work;

static void execute_callback_work(napi_env env, void *data) {
  usleep(50000);
}

static void complete_callback_work(napi_env env, napi_status status, void *data) {
  callback_work *work = (callback_work *)data;
  napi_value callback, receiver, value;
  if (napi_get_reference_value(env, work->callback, &callback) == napi_ok &&
      napi_get_global(env, &receiver) == napi_ok &&
      napi_create_string_utf8(env, "async-value", NAPI_AUTO_LENGTH, &value) == napi_ok) {
    napi_call_function(env, receiver, callback, 1, &value, NULL);
  }
  napi_delete_reference(env, work->callback);
  napi_delete_async_work(env, work->work);
  free(work);
}

static napi_value on_later(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], resource_name;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1) return NULL;
  callback_work *work = (callback_work *)calloc(1, sizeof(callback_work));
  if (!work) return NULL;
  if (napi_create_reference(env, argv[0], 1, &work->callback) != napi_ok) { free(work); return NULL; }
  if (napi_create_string_utf8(env, "fixture callback", NAPI_AUTO_LENGTH, &resource_name) != napi_ok ||
      napi_create_async_work(env, NULL, resource_name, execute_callback_work,
                             complete_callback_work, work, &work->work) != napi_ok ||
      napi_queue_async_work(env, work->work) != napi_ok) {
    napi_delete_reference(env, work->callback);
    free(work);
    return NULL;
  }
  napi_value result;
  napi_get_undefined(env, &result);
  return result;
}

static napi_value on_sync(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], receiver, value, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1) return NULL;
  if (napi_get_global(env, &receiver) != napi_ok ||
      napi_create_string_utf8(env, "sync-value", NAPI_AUTO_LENGTH, &value) != napi_ok) return NULL;
  if (napi_call_function(env, receiver, argv[0], 1, &value, &result) != napi_ok) return NULL;
  napi_get_undefined(env, &result);
  return result;
}

static napi_value init(napi_env env, napi_value exports) {
  napi_value fn;
  if (napi_create_function(env, "add", NAPI_AUTO_LENGTH, add, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "add", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "echo", NAPI_AUTO_LENGTH, echo, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "echo", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "big", NAPI_AUTO_LENGTH, big, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "big", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "fail", NAPI_AUTO_LENGTH, fail, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "fail", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseResult", NAPI_AUTO_LENGTH, promise_result, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseResult", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseReject", NAPI_AUTO_LENGTH, promise_reject, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseReject", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promisePending", NAPI_AUTO_LENGTH, promise_pending, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promisePending", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "onLater", NAPI_AUTO_LENGTH, on_later, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "onLater", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "onSync", NAPI_AUTO_LENGTH, on_sync, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "onSync", fn) != napi_ok) return NULL;
  napi_property_descriptor counter_methods[] = {
    {"increment", NULL, counter_increment, NULL, NULL, NULL, napi_default, NULL},
    {"self", NULL, counter_self, NULL, NULL, NULL, napi_default, NULL},
  };
  napi_value counter;
  if (napi_define_class(env, "Counter", NAPI_AUTO_LENGTH, counter_constructor, NULL,
                        2, counter_methods, &counter) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "Counter", counter) != napi_ok) return NULL;
  return exports;
}

NAPI_MODULE(NODE_GYP_MODULE_NAME, init)
"#,
        )
        .unwrap();
        let compile = ProcessCommand::new("cc")
            .arg("-shared")
            .arg("-fPIC")
            .arg("-DNODE_GYP_MODULE_NAME=fixture")
            .arg(format!("-I{}", include.display()))
            .arg(&source)
            .arg("-o")
            .arg(&addon)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "could not compile Node-API fixture: {}",
            String::from_utf8_lossy(&compile.stderr)
        );

        let provider = Rc::new(NodeAddonSidecar::new("node").unwrap());
        let loader = FileCommonJsLoader::new([&root])
            .unwrap()
            .allow_native_addon(&addon)
            .unwrap()
            .with_native_addon_loader(provider.clone());
        let mut interpreter = Interpreter::with_builtins();
        interpreter.set_host_bridge(provider);
        interpreter.set_commonjs_entry(root.join("main.cjs").to_string_lossy().into_owned());
        interpreter.set_commonjs_loader(Rc::new(loader)).unwrap();
        let result = interpreter
            .eval_source("require('./fixture.node').add(19, 23);")
            .unwrap();
        assert!(matches!(result, Value::Number(value) if value == 42.0));
        let bigint = interpreter
            .eval_source("String(require('./fixture.node').big());")
            .unwrap();
        assert!(matches!(bigint, Value::String(ref value) if value == "9007199254740993"));
        let error = interpreter
            .eval_source(
                "try { require('./fixture.node').fail(); } catch (error) { ({name:error.name, message:error.message, code:error.code}); }",
            )
            .unwrap();
        assert!(
            matches!(error.get_prop("name"), Some(Value::String(ref name)) if name == "TypeError")
        );
        assert!(
            matches!(error.get_prop("message"), Some(Value::String(ref message)) if message == "fixture failure")
        );
        assert!(
            matches!(error.get_prop("code"), Some(Value::String(ref code)) if code == "E_FIXTURE")
        );
        let async_result = interpreter
            .eval_source(
                "await require('./fixture.node').promiseResult().then(value => value + '-chained');",
            )
            .unwrap();
        assert!(
            matches!(async_result, Value::String(ref value) if value == "native-promise-value-chained")
        );
        let async_function_result = interpreter
            .eval_source(
                "async function readNativePromise() { return await require('./fixture.node').promiseResult(); } await readNativePromise();",
            )
            .unwrap();
        assert!(
            matches!(async_function_result, Value::String(ref value) if value == "native-promise-value")
        );
        let unrelated_await = interpreter
            .eval_source(
                "globalThis.pendingNativePromise = require('./fixture.node').promisePending(); await Promise.resolve(); 'unrelated-await-completed';",
            )
            .unwrap();
        assert!(
            matches!(unrelated_await, Value::String(ref value) if value == "unrelated-await-completed")
        );
        let async_rejection = interpreter
            .eval_source(
                "try { await require('./fixture.node').promiseReject(); } catch (reason) { reason; }",
            )
            .unwrap();
        assert!(
            matches!(async_rejection, Value::String(ref reason) if reason == "native-rejection")
        );
        let sync_error = interpreter
            .eval_source(
                "let syncCallbackRan = false; try { require('./fixture.node').onSync(() => { syncCallbackRan = true; }); } catch (error) { ({name:error.name, code:error.code, ran:syncCallbackRan}); }",
            )
            .unwrap();
        assert!(
            matches!(sync_error.get_prop("name"), Some(Value::String(ref name)) if name == "TypeError")
        );
        assert!(
            matches!(sync_error.get_prop("code"), Some(Value::String(ref code)) if code == "ERR_NAPI_VM_SYNC_GUEST_CALLBACK_UNSUPPORTED")
        );
        assert!(matches!(
            sync_error.get_prop("ran"),
            Some(Value::Bool(false))
        ));
        interpreter
            .eval_source(
                "globalThis.callbackValues = []; globalThis.callbackThisType = ''; require('./fixture.node').onLater(function(value) { callbackValues.push(value); callbackThisType = typeof this; });",
            )
            .unwrap();
        assert!(
            interpreter
                .run_event_loop_once(Duration::from_secs(2))
                .unwrap()
        );
        let callback_value = interpreter.eval_source("callbackValues[0];").unwrap();
        assert!(matches!(callback_value, Value::String(ref value) if value == "async-value"));
        let callback_this = interpreter.eval_source("callbackThisType;").unwrap();
        assert!(matches!(callback_this, Value::String(ref value) if value == "object"));
        let counter = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const counter = new addon.Counter(19); counter.increment(); counter.count = 41; counter.increment(); const other = new addon.Counter(5); counter.increment.call(other); const spread = {...counter}; const assigned = Object.assign({}, counter); let iterated = ''; for (const key in counter) iterated += key; ({count:counter.count, receiver:other.count, same:counter === counter.self(), keys:Object.keys(counter).join(','), values:Object.values(counter).join(','), entries:Object.entries(counter)[0][0] + ':' + Object.entries(counter)[0][1], spread:spread.count, assigned:assigned.count, iterated, has:'count' in counter});",
            )
            .unwrap();
        assert!(matches!(counter.get_prop("count"), Some(Value::Number(count)) if count == 42.0));
        assert!(matches!(counter.get_prop("receiver"), Some(Value::Number(value)) if value == 6.0));
        assert!(matches!(counter.get_prop("same"), Some(Value::Bool(true))));
        assert!(
            matches!(counter.get_prop("keys"), Some(Value::String(ref keys)) if keys == "count")
        );
        assert!(
            matches!(counter.get_prop("values"), Some(Value::String(ref values)) if values == "42")
        );
        assert!(
            matches!(counter.get_prop("entries"), Some(Value::String(ref entries)) if entries == "count:42")
        );
        assert!(matches!(counter.get_prop("spread"), Some(Value::Number(value)) if value == 42.0));
        assert!(
            matches!(counter.get_prop("assigned"), Some(Value::Number(value)) if value == 42.0)
        );
        assert!(
            matches!(counter.get_prop("iterated"), Some(Value::String(ref iterated)) if iterated == "count")
        );
        assert!(matches!(counter.get_prop("has"), Some(Value::Bool(true))));
        let roundtrip = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const counter = new addon.Counter(5); addon.echo(counter) === counter;",
            )
            .unwrap();
        assert!(matches!(roundtrip, Value::Bool(true)));

        drop(interpreter);
        fs::remove_dir_all(root).unwrap();
    }
}
