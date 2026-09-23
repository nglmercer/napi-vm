use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
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
    MAX_ARRAY_LEN, MAX_OBJECT_PROPS, MAX_STRING_LEN, PromiseInner, PromiseState, SymbolData,
    TypedArrayData, TypedKind, Value,
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
const { Worker } = require('node:worker_threads');
const socket = net.connect({host:'127.0.0.1',port:Number(process.env.NAPI_VM_BRIDGE_PORT)});
let input = Buffer.alloc(0);
let connected = false;
let workerReady = false;
let syncCallbackActive = false;
const pendingSyncCallbacks = new Map();
function send(message) {
  const body = Buffer.from(JSON.stringify(message));
  const header = Buffer.allocUnsafe(4);
  header.writeUInt32BE(body.length, 0);
  socket.write(Buffer.concat([header, body]));
}
function maybeSendHello() {
  if (connected && workerReady) send({hello:process.env.NAPI_VM_BRIDGE_TOKEN});
}
function finishSyncCallback(message) {
  const pending = pendingSyncCallbacks.get(message.callId);
  if (!pending) {
    socket.destroy(new Error('unknown synchronous guest callback response'));
    return;
  }
  pendingSyncCallbacks.delete(message.callId);
  syncCallbackActive = pendingSyncCallbacks.size > 0;
  let response = {ok:message.ok===true,value:message.value};
  let bytes = Buffer.from(JSON.stringify(response));
  if (bytes.length > pending.shared.byteLength - 8) {
    response = {ok:false,value:{t:'error',name:'RangeError',message:'synchronous guest callback result exceeds the bridge limit'}};
    bytes = Buffer.from(JSON.stringify(response));
  }
  new Uint8Array(pending.shared,8,bytes.length).set(bytes);
  const words = new Int32Array(pending.shared,0,2);
  Atomics.store(words,1,bytes.length);
  Atomics.store(words,0,response.ok?1:2);
  Atomics.notify(words,0);
}
function consume() {
  while(input.length>=4) {
    const n=input.readUInt32BE(0);if(n>16777216){socket.destroy(new Error('frame too large'));return;}
    if(input.length<n+4)return;
    const body=input.subarray(4,n+4);input=input.subarray(n+4);
    let message;try{message=JSON.parse(body.toString('utf8'));}catch(e){socket.destroy(e);return;}
    if(message.event==='syncGuestCallbackResult') finishSyncCallback(message);
    else if(message.requestId!==undefined){
      if(syncCallbackActive){
        send({requestId:message.requestId,ok:false,error:{name:'TypeError',message:'native addon calls from a synchronous guest callback are not supported',code:'ERR_NAPI_VM_REENTRANT_ADDON_CALL'}});
      } else if(!workerReady){
        send({requestId:message.requestId,ok:false,error:{name:'Error',message:'native addon worker is not ready'}});
      } else worker.postMessage({kind:'request',request:message});
    } else {socket.destroy(new Error('invalid napi-vm bridge frame'));return;}
  }
}
function addonWorkerMain() {
  'use strict';
  const { parentPort } = require('node:worker_threads');
  const MAX_SYNC_CALLBACK_RESULT_BYTES = 1024 * 1024;
  let nextHandle=1, nextCallbackCall=1, nextNativeSymbolId=1, dispatchDepth=0;
  const refs=new Map(), objectIds=new WeakMap(), functionIds=new WeakMap(), promiseIds=new WeakMap();
  const symbols=new Map(), symbolIds=new Map();
  const wellKnownSymbols=[undefined,Symbol.iterator,Symbol.asyncIterator,Symbol.toStringTag,Symbol.hasInstance,Symbol.toPrimitive,Symbol.species,Symbol.unscopables,Symbol.isConcatSpreadable,Symbol.match,Symbol.matchAll,Symbol.replace,Symbol.search,Symbol.split];
  function symbolId(value){let id=symbolIds.get(value);if(id===undefined){if(symbols.size>=262144)throw new RangeError('symbol handle limit exceeded');id='n:'+nextNativeSymbolId++;symbols.set(id,value);symbolIds.set(value,id);}return id;}
  function hold(value,receiver){
    const kind=typeof value,ids=kind==='function'?functionIds:objectIds,existing=ids.get(value);
    if(existing!==undefined)return existing;
    if(refs.size>=262144)throw new RangeError('native handle limit exceeded');
    const id=nextHandle++;refs.set(id,{value,receiver});ids.set(value,id);return id;
  }
  function event(message){
    if(message.event==='syncGuestCallback')parentPort.postMessage({kind:'syncGuestCallback',callId:message.callId,shared:message.shared,message});
    else parentPort.postMessage({kind:'event',message});
  }
  function encode(value,receiver,depth,active){
    if(depth>128)throw new RangeError('bridge depth exceeded');
    if(value===undefined)return {t:'undefined'};if(value===null)return {t:'null'};
    if(typeof value==='boolean')return {t:'boolean',v:value};
    if(typeof value==='number')return {t:'number',v:Object.is(value,-0)?'-0':String(value)};
    if(typeof value==='string')return {t:'string',v:value};
    if(typeof value==='bigint')return {t:'bigint',v:value.toString()};
    if(typeof value==='symbol')return {t:'symbol',v:symbolId(value),description:value.description};
    if(typeof value==='function')return {t:'function',v:hold(value,receiver),n:value.name};
    if(value&&typeof value.then==='function'){
      let id=promiseIds.get(value);
      if(id===undefined){id=hold(value,undefined);promiseIds.set(value,id);Promise.resolve(value).then(
        result=>event({event:'hostPromiseSettled',promiseId:id,state:'fulfilled',value:encode(result,undefined,0,new Set())}),
        reason=>event({event:'hostPromiseSettled',promiseId:id,state:'rejected',value:encode(reason,undefined,0,new Set())})
      ).catch(error=>event({event:'hostPromiseSettled',promiseId:id,state:'rejected',value:{t:'error',name:'TypeError',message:'native promise result could not be marshalled: '+String(error)}}));}
      return {t:'hostPromise',v:id};
    }
    if(value instanceof Error)return {t:'error',name:value.name,message:value.message,code:typeof value.code==='string'?value.code:undefined};
    if(value instanceof Date)return {t:'date',v:Number.isNaN(value.getTime())?'NaN':String(value.getTime())};
    if(value instanceof RegExp)return {t:'regexp',source:value.source,flags:value.flags,lastIndex:String(value.lastIndex)};
    if(Buffer.isBuffer(value))return {t:'hostObject',v:hold(value,undefined)};
    if(value instanceof DataView)return {t:'dataView',length:value.byteLength,bytes:Array.from(new Uint8Array(value.buffer,value.byteOffset,value.byteLength))};
    if(ArrayBuffer.isView(value))return {t:'typedArray',kind:value.constructor.name,length:value.length,bytes:Array.from(new Uint8Array(value.buffer,value.byteOffset,value.byteLength))};
    if(value instanceof ArrayBuffer)return {t:'arrayBuffer',v:Array.from(new Uint8Array(value))};
    if(active.has(value))throw new TypeError('cyclic native values are unsupported');
    if(Array.isArray(value)){if(value.length>262144)throw new RangeError('native array exceeds the VM limit');active.add(value);const result={t:'array',v:Array.from(value,v=>encode(v,value,depth+1,active))};active.delete(value);return result;}
    return {t:'hostObject',v:hold(value,undefined)};
  }
  function decode(value,depth){
    if(depth>128)throw new RangeError('guest argument depth exceeded');
    switch(value.t){
      case 'undefined':return undefined;case 'null':return null;case 'boolean':return value.v;
      case 'number':if(value.v==='-0')return -0;if(value.v==='NaN')return NaN;if(value.v==='Infinity')return Infinity;if(value.v==='-Infinity')return -Infinity;return Number(value.v);
      case 'string':return value.v;case 'bigint':return BigInt(value.v);
      case 'symbol':{let symbol=symbols.get(value.v);if(symbol===undefined){const guestId=value.v.startsWith('g:')?Number(value.v.slice(2)):0;symbol=wellKnownSymbols[guestId];if(symbol===undefined){if(symbols.size>=262144)throw new RangeError('symbol handle limit exceeded');symbol=Symbol(value.description);}symbols.set(value.v,symbol);symbolIds.set(symbol,value.v);}return symbol;}
      case 'date':return new Date(value.v==='NaN'?NaN:Number(value.v));
      case 'regexp':{const regex=new RegExp(value.source,value.flags);regex.lastIndex=Number(value.lastIndex||0);return regex;}
      case 'arrayBuffer':return Uint8Array.from(value.v).buffer;
      case 'typedArray':{const Constructor=globalThis[value.kind];if(typeof Constructor!=='function')throw new TypeError('unsupported guest typed array kind');const bytes=Uint8Array.from(value.bytes);return new Constructor(bytes.buffer,0,value.length);}
      case 'dataView':{const bytes=Uint8Array.from(value.bytes);return new DataView(bytes.buffer,0,value.length);}
      case 'bytes':return Buffer.from(value.v);
      case 'error':{const error=new Error(value.message||'guest callback threw');error.name=value.name||'Error';if(value.code)error.code=value.code;return error;}
      case 'hostObject':{const entry=refs.get(value.v);if(!entry||!entry.value||typeof entry.value!=='object')throw new TypeError('native object handle is invalid');return entry.value;}
      case 'guestCallback':return function(...args){
        const payload={callbackId:value.v,thisValue:encode(this,undefined,0,new Set()),args:args.map(v=>encode(v,undefined,0,new Set()))};
        if(dispatchDepth===0){event({event:'guestCallback',...payload});return undefined;}
        const callId=nextCallbackCall++,shared=new SharedArrayBuffer(MAX_SYNC_CALLBACK_RESULT_BYTES+8),words=new Int32Array(shared,0,2);
        event({event:'syncGuestCallback',callId,shared,...payload});
        const status=Atomics.wait(words,0,0,60000);if(status==='timed-out')throw new Error('timed out waiting for synchronous guest callback');
        const length=Atomics.load(words,1);if(length<0||length>MAX_SYNC_CALLBACK_RESULT_BYTES)throw new RangeError('invalid synchronous guest callback response size');
        const response=JSON.parse(Buffer.from(new Uint8Array(shared,8,length)).toString('utf8'));
        if(!response.ok)throw decode(response.value,0);return decode(response.value,0);
      };
      case 'function':{const entry=refs.get(value.v);if(!entry||typeof entry.value!=='function')throw new TypeError('native function handle is invalid');return entry.value;}
      case 'array':return value.v.map(v=>decode(v,depth+1));
      case 'object':{const o={};for(const [k,v]of value.v)Object.defineProperty(o,k,{value:decode(v,depth+1),enumerable:true,writable:true,configurable:true});return o;}
      default:throw new TypeError('unsupported napi-vm argument');
    }
  }
  async function dispatch(r){
    dispatchDepth++;
    try{
      let result,receiver;
      if(r.op==='load')result=require(r.filename);
      else if(['get','set','has','delete','ownKeys'].includes(r.op)){
        const entry=refs.get(r.id);if(!entry||!entry.value||typeof entry.value!=='object')throw new Error('native object handle is invalid');
        const object=entry.value;
        if(r.op==='get'){result=Reflect.get(object,r.key,object);receiver=object;}
        else if(r.op==='set')result=Reflect.set(object,r.key,decode(r.value,0),object);
        else if(r.op==='has')result=Reflect.has(object,r.key);
        else if(r.op==='delete')result=Reflect.deleteProperty(object,r.key);
        else result=Object.keys(object);
      }else{
        const entry=refs.get(r.id);if(!entry||typeof entry.value!=='function')throw new Error('native function handle is invalid');
        const args=r.args.map(v=>decode(v,0));
        if(r.op==='construct')result=Reflect.construct(entry.value,args);
        else result=Reflect.apply(entry.value,Object.hasOwn(r,'receiver')?decode(r.receiver,0):entry.receiver,args);
      }
      return {requestId:r.requestId,ok:true,value:encode(result,receiver,0,new Set())};
    }catch(e){return {requestId:r.requestId,ok:false,error:{name:typeof e?.name==='string'?e.name:'Error',message:typeof e?.message==='string'?e.message:String(e),code:typeof e?.code==='string'?e.code:undefined}};}
    finally{dispatchDepth--;}
  }
  parentPort.on('message',message=>{
    if(message.kind==='request')dispatch(message.request).then(response=>parentPort.postMessage({kind:'response',message:response}),error=>parentPort.postMessage({kind:'response',message:{requestId:message.request.requestId,ok:false,error:{name:'Error',message:String(error)}}}));
  });
  parentPort.postMessage({kind:'ready'});
}
const worker = new Worker('('+addonWorkerMain.toString()+')()', {eval:true});
worker.on('message',message=>{
  if(message.kind==='ready'){workerReady=true;maybeSendHello();}
  else if(message.kind==='response')send(message.message);
  else if(message.kind==='event')send(message.message);
  else if(message.kind==='syncGuestCallback'){
    syncCallbackActive=true;pendingSyncCallbacks.set(message.callId,message);send(message.message);
  }
});
worker.on('error',error=>socket.destroy(error));
worker.on('exit',code=>{if(code!==0)socket.destroy(new Error('Node addon worker exited with code '+code));});
socket.on('connect',()=>{connected=true;maybeSendHello();});
socket.on('data',c=>{input=Buffer.concat([input,c]);consume();});
socket.on('error',e=>process.stderr.write('napi-vm sidecar: '+e.message+'\n'));
"#;

struct State {
    child: Child,
    stream: TcpStream,
    reader: Option<JoinHandle<()>>,
    response_rx: Receiver<JsonValue>,
    event_rx: Receiver<JsonValue>,
    pending_events: VecDeque<JsonValue>,
    request_id: u64,
    failed: bool,
    next_local_handle: usize,
    local_handles: HashMap<usize, LocalObjectTrap>,
    object_proxies: HashMap<u64, Value>,
    proxy_ids: HashMap<usize, u64>,
    guest_callbacks: HashMap<u64, Value>,
    host_symbols: HashMap<String, Value>,
    symbol_remote_ids: HashMap<u64, String>,
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
                pending_events: VecDeque::new(),
                request_id: 1,
                failed: false,
                next_local_handle: 0,
                local_handles: HashMap::new(),
                object_proxies: HashMap::new(),
                proxy_ids: HashMap::new(),
                guest_callbacks: HashMap::new(),
                host_symbols: HashMap::new(),
                symbol_remote_ids: HashMap::new(),
                next_guest_callback_id: 1,
                native_promises: HashMap::new(),
            })),
        })
    }

    fn request(&self, message: JsonValue) -> Result<JsonValue, VmErr> {
        self.request_with_callback_handler(message, &mut |_| {
            Err(VmErr::Msg(
                "synchronous guest callback was requested outside a VM host call".into(),
            ))
        })
    }

    fn request_with_callback_handler(
        &self,
        mut message: JsonValue,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<JsonValue, VmErr> {
        let id = {
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
        let callback = self.guest_callback_from_event(event)?;
        let result = callback_handler(callback);
        let (ok, value) = match result {
            Ok(value) => match self.guest_to_wire(&value, 0) {
                Ok(value) => (true, value),
                Err(error) => (
                    false,
                    json!({"t":"error","name":"TypeError","message":error.to_string()}),
                ),
            },
            Err(VmErr::Throw(reason)) => match self.guest_to_wire(&reason, 0) {
                Ok(value) => (false, value),
                Err(error) => (
                    false,
                    json!({"t":"error","name":"TypeError","message":error.to_string()}),
                ),
            },
            Err(error) => (
                false,
                json!({"t":"error","name":"Error","message":error.to_string()}),
            ),
        };
        let response = json!({
            "event":"syncGuestCallbackResult",
            "callId":call_id,
            "ok":ok,
            "value":value,
        });
        let mut state = self.state.borrow_mut();
        write_frame(&mut state.stream, &response)
    }

    fn guest_callback_from_event(&self, event: &JsonValue) -> Result<HostCallback, VmErr> {
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
        Ok(HostCallback {
            callback,
            this_value,
            args,
        })
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

    fn dispatch_object_trap_with_callback_handler(
        &self,
        trap: LocalObjectTrap,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
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
        let result = self.request_with_callback_handler(request, callback_handler)?;
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
        let args = args
            .iter()
            .map(|v| self.guest_to_wire(v, 0))
            .collect::<Result<Vec<_>, _>>()?;
        let receiver = self.guest_to_wire(&this_value, 0)?;
        self.wire_to_guest(
            &self.request_with_callback_handler(
                json!({"op":"call","id":id,"args":args,"receiver":receiver}),
                callback_handler,
            )?,
            0,
        )
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
        let args = args
            .iter()
            .map(|v| self.guest_to_wire(v, 0))
            .collect::<Result<Vec<_>, _>>()?;
        self.wire_to_guest(
            &self.request_with_callback_handler(
                json!({"op":"construct","id":id,"args":args}),
                callback_handler,
            )?,
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

fn typed_kind(name: &str) -> Option<TypedKind> {
    Some(match name {
        "Int8Array" => TypedKind::Int8,
        "Uint8Array" => TypedKind::Uint8,
        "Uint8ClampedArray" => TypedKind::Uint8Clamped,
        "Int16Array" => TypedKind::Int16,
        "Uint16Array" => TypedKind::Uint16,
        "Int32Array" => TypedKind::Int32,
        "Uint32Array" => TypedKind::Uint32,
        "Float32Array" => TypedKind::Float32,
        "Float64Array" => TypedKind::Float64,
        "BigInt64Array" => TypedKind::BigInt64,
        "BigUint64Array" => TypedKind::BigUint64,
        _ => return None,
    })
}

fn wire_bytes(value: &JsonValue) -> Result<Vec<u8>, VmErr> {
    value
        .as_array()
        .ok_or_else(|| VmErr::Msg("invalid Node bytes".into()))?
        .iter()
        .map(|byte| {
            byte.as_u64()
                .filter(|value| *value <= 255)
                .map(|value| value as u8)
                .ok_or_else(|| VmErr::Msg("invalid Node byte".into()))
        })
        .collect()
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
        Value::ArrayBuffer(bytes) => json!({"t":"arrayBuffer","v":bytes.borrow().as_slice()}),
        Value::TypedArray(view) => {
            let start = view.byte_offset;
            let byte_len = view
                .length
                .checked_mul(view.kind.size())
                .ok_or_else(|| VmErr::Msg("guest typed array exceeds the bridge limit".into()))?;
            let end = start
                .checked_add(byte_len)
                .ok_or_else(|| VmErr::Msg("guest typed array exceeds the bridge limit".into()))?;
            let bytes = view.buffer.borrow();
            let slice = bytes
                .get(start..end)
                .ok_or_else(|| VmErr::Msg("guest typed array has an invalid byte range".into()))?;
            json!({"t":"typedArray","kind":view.kind.name(),"length":view.length,"bytes":slice})
        }
        Value::DataView(view) => {
            let start = view.byte_offset;
            let end = start
                .checked_add(view.length)
                .ok_or_else(|| VmErr::Msg("guest DataView exceeds the bridge limit".into()))?;
            let bytes = view.buffer.borrow();
            let slice = bytes
                .get(start..end)
                .ok_or_else(|| VmErr::Msg("guest DataView has an invalid byte range".into()))?;
            json!({"t":"dataView","length":view.length,"bytes":slice})
        }
        Value::BigInt(x) => json!({"t":"bigint","v":x.to_string()}),
        Value::Date(milliseconds) => {
            let value = milliseconds.get();
            let wire = if value.is_nan() {
                "NaN".to_string()
            } else if value == f64::INFINITY {
                "Infinity".to_string()
            } else if value == f64::NEG_INFINITY {
                "-Infinity".to_string()
            } else {
                value.to_string()
            };
            json!({"t":"date","v":wire})
        }
        Value::RegExp(data) => json!({
            "t":"regexp",
            "source":data.regex.source,
            "flags":data.regex.flags,
            "lastIndex":data.last_index.get().to_string(),
        }),
        Value::Symbol(symbol) => {
            let remote_id = sidecar
                .state
                .borrow()
                .symbol_remote_ids
                .get(&symbol.id)
                .cloned()
                .unwrap_or_else(|| format!("g:{}", symbol.id));
            json!({
                "t":"symbol",
                "v":remote_id,
                "description":symbol.description,
            })
        }
        Value::Error(error) => json!({
            "t":"error",
            "name":error.name,
            "message":error.message,
            "code":error.code,
        }),
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
        "date" => {
            let value = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node date".into()))?;
            let milliseconds = parse_wire_number(value)?;
            Ok(Value::Date(Rc::new(std::cell::Cell::new(milliseconds))))
        }
        "regexp" => {
            let source = v
                .get("source")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node regular expression source".into()))?;
            let flags = v
                .get("flags")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node regular expression flags".into()))?;
            if source.len() > MAX_STRING_LEN || flags.len() > MAX_STRING_LEN {
                return Err(VmErr::Msg(
                    "Node regular expression exceeds the VM string limit".into(),
                ));
            }
            let last_index = v
                .get("lastIndex")
                .and_then(JsonValue::as_str)
                .unwrap_or("0")
                .parse::<f64>()
                .unwrap_or(0.0);
            let regex = crate::builtins::compile_regex(source, flags)?;
            let Value::RegExp(data) = &regex else {
                unreachable!("regex compiler returns a RegExp")
            };
            if last_index.is_finite() && last_index >= 0.0 {
                data.last_index.set(last_index as usize);
            }
            Ok(regex)
        }
        "symbol" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node symbol id".into()))?;
            let description = v
                .get("description")
                .and_then(JsonValue::as_str)
                .map(str::to_string);
            if description
                .as_ref()
                .is_some_and(|description| description.len() > MAX_STRING_LEN)
            {
                return Err(VmErr::Msg(
                    "Node symbol description exceeds VM limit".into(),
                ));
            }
            if let Some(guest_id) = id.strip_prefix("g:") {
                let guest_id = guest_id
                    .parse::<u64>()
                    .map_err(|_| VmErr::Msg("invalid guest symbol id".into()))?;
                return Ok(Value::Symbol(Rc::new(SymbolData {
                    id: guest_id,
                    description,
                })));
            }
            if let Some(symbol) = sidecar.state.borrow().host_symbols.get(id).cloned() {
                return Ok(symbol);
            }
            if !id.starts_with("n:") {
                return Err(VmErr::Msg("invalid native symbol id".into()));
            }
            if sidecar.state.borrow().host_symbols.len() >= MAX_NATIVE_HANDLES {
                return Err(VmErr::Msg("native symbol handle limit exceeded".into()));
            }
            let symbol = crate::builtins::new_symbol(description);
            let Value::Symbol(data) = &symbol else {
                unreachable!("new_symbol returns a Symbol")
            };
            let mut state = sidecar.state.borrow_mut();
            state.symbol_remote_ids.insert(data.id, id.to_string());
            state.host_symbols.insert(id.to_string(), symbol.clone());
            Ok(symbol)
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
            let error = match v.get("code").and_then(JsonValue::as_str) {
                Some(code) => crate::value::ErrorData::with_code(name, message, code),
                None => crate::value::ErrorData::new(name, message),
            };
            Ok(Value::Error(error))
        }
        "arrayBuffer" | "bytes" => {
            let bytes = wire_bytes(
                v.get("v")
                    .ok_or_else(|| VmErr::Msg("invalid Node bytes".into()))?,
            )?;
            Ok(Value::ArrayBuffer(Rc::new(RefCell::new(bytes))))
        }
        "typedArray" => {
            let kind_name = v
                .get("kind")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node typed array kind".into()))?;
            let kind = typed_kind(kind_name)
                .ok_or_else(|| VmErr::Msg("unsupported Node typed array kind".into()))?;
            let length = v
                .get("length")
                .and_then(JsonValue::as_u64)
                .and_then(|length| usize::try_from(length).ok())
                .ok_or_else(|| VmErr::Msg("invalid Node typed array length".into()))?;
            let bytes = wire_bytes(
                v.get("bytes")
                    .ok_or_else(|| VmErr::Msg("invalid Node typed array bytes".into()))?,
            )?;
            if length.checked_mul(kind.size()) != Some(bytes.len()) {
                return Err(VmErr::Msg(
                    "Node typed array has an invalid byte length".into(),
                ));
            }
            Ok(Value::TypedArray(Rc::new(TypedArrayData {
                kind,
                buffer: Rc::new(RefCell::new(bytes)),
                byte_offset: 0,
                length,
            })))
        }
        "dataView" => {
            let length = v
                .get("length")
                .and_then(JsonValue::as_u64)
                .and_then(|length| usize::try_from(length).ok())
                .ok_or_else(|| VmErr::Msg("invalid Node DataView length".into()))?;
            let bytes = wire_bytes(
                v.get("bytes")
                    .ok_or_else(|| VmErr::Msg("invalid Node DataView bytes".into()))?,
            )?;
            if length != bytes.len() {
                return Err(VmErr::Msg(
                    "Node DataView has an invalid byte length".into(),
                ));
            }
            Ok(Value::DataView(Rc::new(TypedArrayData {
                kind: TypedKind::Uint8,
                buffer: Rc::new(RefCell::new(bytes)),
                byte_offset: 0,
                length,
            })))
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

fn parse_wire_number(value: &str) -> Result<f64, VmErr> {
    match value {
        "NaN" => Ok(f64::NAN),
        "Infinity" => Ok(f64::INFINITY),
        "-Infinity" => Ok(f64::NEG_INFINITY),
        "-0" => Ok(-0.0),
        _ => value
            .parse()
            .map_err(|error| VmErr::Msg(format!("invalid Node number: {error}"))),
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
#include <string.h>
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

static napi_value date_value(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  double milliseconds;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_date_value(env, argv[0], &milliseconds) != napi_ok ||
      napi_create_double(env, milliseconds, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_date(napi_env env, napi_callback_info info) {
  napi_value result;
  if (napi_create_date(env, 123456.5, &result) != napi_ok) return NULL;
  return result;
}

static napi_value regex_source(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "source", &result) != napi_ok) return NULL;
  return result;
}

static napi_value regex_flags(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "flags", &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_regex(napi_env env, napi_callback_info info) {
  napi_value global, constructor, args[2], result;
  if (napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "RegExp", &constructor) != napi_ok ||
      napi_create_string_utf8(env, "a+", NAPI_AUTO_LENGTH, &args[0]) != napi_ok ||
      napi_create_string_utf8(env, "gi", NAPI_AUTO_LENGTH, &args[1]) != napi_ok ||
      napi_new_instance(env, constructor, 2, args, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_symbol(napi_env env, napi_callback_info info) {
  napi_value description, result;
  if (napi_create_string_utf8(env, "native", NAPI_AUTO_LENGTH, &description) != napi_ok ||
      napi_create_symbol(env, description, &result) != napi_ok) return NULL;
  return result;
}

static napi_value identity(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1) return NULL;
  return argv[0];
}

static napi_value is_symbol(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  napi_valuetype type;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_typeof(env, argv[0], &type) != napi_ok ||
      napi_create_int32(env, type == napi_symbol ? 1 : 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value is_node_iterator(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], global, symbol_constructor, iterator, result;
  bool equal = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Symbol", &symbol_constructor) != napi_ok ||
      napi_get_named_property(env, symbol_constructor, "iterator", &iterator) != napi_ok ||
      napi_strict_equals(env, argv[0], iterator, &equal) != napi_ok ||
      napi_create_int32(env, equal ? 1 : 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_buffer(napi_env env, napi_callback_info info) {
  const char bytes[] = {'a', 'b', 'c'};
  napi_value result;
  if (napi_create_buffer_copy(env, sizeof(bytes), bytes, NULL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value is_buffer(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  bool is_buffer = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_is_buffer(env, argv[0], &is_buffer) != napi_ok ||
      napi_create_int32(env, is_buffer ? 1 : 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value typed_array_length(napi_env env, napi_callback_info info) {
  size_t argc = 1, length = 0;
  napi_value argv[1], result;
  napi_typedarray_type kind;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_typedarray_info(env, argv[0], &kind, &length, NULL, NULL, NULL) != napi_ok ||
      napi_create_int32(env, kind == napi_uint16_array ? (int32_t)length : -1, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_typed_array(napi_env env, napi_callback_info info) {
  napi_value buffer, result;
  void *data = NULL;
  if (napi_create_arraybuffer(env, 4, &data, &buffer) != napi_ok) return NULL;
  uint16_t *items = (uint16_t *)data;
  items[0] = 300;
  items[1] = 400;
  if (napi_create_typedarray(env, napi_uint16_array, 2, buffer, 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_array_buffer(napi_env env, napi_callback_info info) {
  napi_value result;
  void *data = NULL;
  if (napi_create_arraybuffer(env, 3, &data, &result) != napi_ok) return NULL;
  memcpy(data, "xyz", 3);
  return result;
}

static napi_value array_buffer_length(napi_env env, napi_callback_info info) {
  size_t argc = 1, length = 0;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_arraybuffer_info(env, argv[0], NULL, &length) != napi_ok ||
      napi_create_int32(env, (int32_t)length, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_data_view(napi_env env, napi_callback_info info) {
  napi_value buffer, result;
  void *data = NULL;
  if (napi_create_arraybuffer(env, 2, &data, &buffer) != napi_ok) return NULL;
  ((uint8_t *)data)[0] = 17;
  ((uint8_t *)data)[1] = 29;
  if (napi_create_dataview(env, 2, buffer, 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value data_view_byte(napi_env env, napi_callback_info info) {
  size_t argc = 1, length = 0, offset = 0;
  napi_value argv[1], result;
  void *data = NULL;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_dataview_info(env, argv[0], &length, &data, NULL, &offset) != napi_ok || length < 2 ||
      napi_create_int32(env, ((uint8_t *)data)[1], &result) != napi_ok) return NULL;
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

static napi_value promise_reject_error(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, message, reason, code;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok ||
      napi_create_string_utf8(env, "native error rejection", NAPI_AUTO_LENGTH, &message) != napi_ok ||
      napi_create_error(env, NULL, message, &reason) != napi_ok ||
      napi_create_string_utf8(env, "E_NATIVE_REJECTION", NAPI_AUTO_LENGTH, &code) != napi_ok ||
      napi_set_named_property(env, reason, "code", code) != napi_ok ||
      napi_reject_deferred(env, deferred, reason) != napi_ok) return NULL;
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
  if (napi_create_function(env, "dateValue", NAPI_AUTO_LENGTH, date_value, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "dateValue", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeDate", NAPI_AUTO_LENGTH, make_date, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeDate", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "regexSource", NAPI_AUTO_LENGTH, regex_source, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "regexSource", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "regexFlags", NAPI_AUTO_LENGTH, regex_flags, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "regexFlags", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeRegex", NAPI_AUTO_LENGTH, make_regex, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeRegex", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeSymbol", NAPI_AUTO_LENGTH, make_symbol, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeSymbol", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "identity", NAPI_AUTO_LENGTH, identity, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "identity", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "isSymbol", NAPI_AUTO_LENGTH, is_symbol, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "isSymbol", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "isNodeIterator", NAPI_AUTO_LENGTH, is_node_iterator, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "isNodeIterator", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeBuffer", NAPI_AUTO_LENGTH, make_buffer, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeBuffer", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "isBuffer", NAPI_AUTO_LENGTH, is_buffer, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "isBuffer", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "typedArrayLength", NAPI_AUTO_LENGTH, typed_array_length, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "typedArrayLength", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeTypedArray", NAPI_AUTO_LENGTH, make_typed_array, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeTypedArray", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeArrayBuffer", NAPI_AUTO_LENGTH, make_array_buffer, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeArrayBuffer", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "arrayBufferLength", NAPI_AUTO_LENGTH, array_buffer_length, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "arrayBufferLength", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeDataView", NAPI_AUTO_LENGTH, make_data_view, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeDataView", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "dataViewByte", NAPI_AUTO_LENGTH, data_view_byte, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "dataViewByte", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "fail", NAPI_AUTO_LENGTH, fail, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "fail", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseResult", NAPI_AUTO_LENGTH, promise_result, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseResult", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseReject", NAPI_AUTO_LENGTH, promise_reject, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseReject", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseRejectError", NAPI_AUTO_LENGTH, promise_reject_error, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseRejectError", fn) != napi_ok) return NULL;
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
        let builtins = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const nativeDate = addon.makeDate(); const nativeRegex = addon.makeRegex(); const nativeSymbol = addon.makeSymbol(); const guestSymbol = Symbol('guest'); const nativeRegexMatched = nativeRegex.test('AAA'); ({inputDate:addon.dateValue(new Date(1700000000123)), nativeDate:nativeDate.getTime(), inputRegex:addon.regexSource(/a+/gi) + '/' + addon.regexFlags(/a+/gi), nativeRegex:nativeRegex.source + '/' + nativeRegex.flags, nativeRegexMatched, nativeRegexLastIndex:nativeRegex.lastIndex, nativeSymbolType:typeof nativeSymbol, nativeSymbolDescription:nativeSymbol.description, nativeSymbolNapiType:addon.isSymbol(nativeSymbol), guestSymbolNapiType:addon.isSymbol(guestSymbol), iteratorSymbolNapiType:addon.isNodeIterator(Symbol.iterator), nativeSymbolIdentity:addon.identity(nativeSymbol) === nativeSymbol, guestSymbolIdentity:addon.identity(guestSymbol) === guestSymbol});",
            )
            .unwrap();
        assert!(matches!(
            builtins.get_prop("inputDate"),
            Some(Value::Number(value)) if value == 1_700_000_000_123.0
        ));
        assert!(matches!(
            builtins.get_prop("nativeDate"),
            Some(Value::Number(value)) if value == 123_456.0
        ));
        assert!(matches!(
            builtins.get_prop("inputRegex"),
            Some(Value::String(ref value)) if value == "a+/gi"
        ));
        assert!(matches!(
            builtins.get_prop("nativeRegex"),
            Some(Value::String(ref value)) if value == "a+/gi"
        ));
        assert!(matches!(
            builtins.get_prop("nativeRegexMatched"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            builtins.get_prop("nativeRegexLastIndex"),
            Some(Value::Number(value)) if value == 3.0
        ));
        assert!(matches!(
            builtins.get_prop("nativeSymbolType"),
            Some(Value::String(ref value)) if value == "symbol"
        ));
        assert!(matches!(
            builtins.get_prop("nativeSymbolDescription"),
            Some(Value::String(ref value)) if value == "native"
        ));
        assert!(matches!(
            builtins.get_prop("nativeSymbolNapiType"),
            Some(Value::Number(value)) if value == 1.0
        ));
        assert!(matches!(
            builtins.get_prop("guestSymbolNapiType"),
            Some(Value::Number(value)) if value == 1.0
        ));
        assert!(matches!(
            builtins.get_prop("iteratorSymbolNapiType"),
            Some(Value::Number(value)) if value == 1.0
        ));
        assert!(matches!(
            builtins.get_prop("nativeSymbolIdentity"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            builtins.get_prop("guestSymbolIdentity"),
            Some(Value::Bool(true))
        ));
        let buffers = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const buffer = addon.makeBuffer(); const view = addon.makeDataView(); ({text:buffer.toString('utf8'), first:buffer[0], roundTrip:addon.isBuffer(buffer), inputLength:addon.typedArrayLength(new Uint16Array([300, 400])), typed:addon.makeTypedArray(), arrayBufferLength:addon.arrayBufferLength(new Uint8Array([1, 2, 3]).buffer), arrayBufferByte:new Uint8Array(addon.makeArrayBuffer())[1], dataViewLength:view.byteLength, dataViewByte:view.getUint8(1), dataViewRoundTrip:addon.dataViewByte(new DataView(new Uint8Array([4, 5]).buffer))});",
            )
            .unwrap();
        assert!(
            matches!(buffers.get_prop("text"), Some(Value::String(ref value)) if value == "abc")
        );
        assert!(matches!(buffers.get_prop("first"), Some(Value::Number(value)) if value == 97.0));
        assert!(
            matches!(buffers.get_prop("roundTrip"), Some(Value::Number(value)) if value == 1.0)
        );
        assert!(
            matches!(buffers.get_prop("inputLength"), Some(Value::Number(value)) if value == 2.0)
        );
        assert!(
            matches!(buffers.get_prop("arrayBufferLength"), Some(Value::Number(value)) if value == 3.0)
        );
        assert!(
            matches!(buffers.get_prop("arrayBufferByte"), Some(Value::Number(value)) if value == 121.0)
        );
        assert!(
            matches!(buffers.get_prop("dataViewLength"), Some(Value::Number(value)) if value == 2.0)
        );
        assert!(
            matches!(buffers.get_prop("dataViewByte"), Some(Value::Number(value)) if value == 29.0)
        );
        assert!(
            matches!(buffers.get_prop("dataViewRoundTrip"), Some(Value::Number(value)) if value == 5.0)
        );
        let typed_value = buffers.get_prop("typed").unwrap_or(Value::Undefined);
        let Value::TypedArray(typed) = &typed_value else {
            panic!("native Uint16Array was not preserved as a typed array");
        };
        assert_eq!(typed.kind, TypedKind::Uint16);
        assert_eq!(typed.length, 2);
        assert!(matches!(
            crate::builtins::read_element(typed, 0),
            Some(Value::Number(value)) if value == 300.0
        ));
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
        let async_error_rejection = interpreter
            .eval_source(
                "try { await require('./fixture.node').promiseRejectError(); } catch (error) { ({name:error.name, message:error.message, code:error.code}); }",
            )
            .unwrap();
        assert!(matches!(
            async_error_rejection.get_prop("name"),
            Some(Value::String(ref name)) if name == "Error"
        ));
        assert!(matches!(
            async_error_rejection.get_prop("message"),
            Some(Value::String(ref message)) if message == "native error rejection"
        ));
        assert!(matches!(
            async_error_rejection.get_prop("code"),
            Some(Value::String(ref code)) if code == "E_NATIVE_REJECTION"
        ));
        let sync_result = interpreter
            .eval_source(
                "globalThis.syncCallbackThisType = ''; const syncCallbackResult = require('./fixture.node').onSync(function(value) { syncCallbackThisType = typeof this; return value + '-reply'; }); ({value:syncCallbackResult, thisType:syncCallbackThisType});",
            )
            .unwrap();
        assert!(
            matches!(sync_result.get_prop("value"), Some(Value::String(ref value)) if value == "sync-value-reply")
        );
        assert!(
            matches!(sync_result.get_prop("thisType"), Some(Value::String(ref value)) if value == "object")
        );
        let sync_throw = interpreter
            .eval_source(
                "try { require('./fixture.node').onSync(() => { throw new TypeError('guest callback failure'); }); } catch (error) { ({name:error.name, message:error.message}); }",
            )
            .unwrap();
        assert!(
            matches!(sync_throw.get_prop("name"), Some(Value::String(ref name)) if name == "TypeError")
        );
        assert!(
            matches!(sync_throw.get_prop("message"), Some(Value::String(ref message)) if message == "guest callback failure")
        );
        let reentrant_addon_call = interpreter
            .eval_source(
                "require('./fixture.node').onSync(() => { try { require('./fixture.node').add(1, 2); return 'unexpected'; } catch (error) { return error.name + ':' + error.code; } });",
            )
            .unwrap();
        assert!(matches!(
            reentrant_addon_call,
            Value::String(ref value)
                if value == "TypeError:ERR_NAPI_VM_REENTRANT_ADDON_CALL"
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
