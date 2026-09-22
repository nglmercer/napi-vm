use std::cell::RefCell;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value as JsonValue, json};

use crate::error::VmErr;
use crate::host::HostBridge;
use crate::interpreter::NativeAddonLoader;
use crate::value::{MAX_ARRAY_LEN, MAX_OBJECT_PROPS, MAX_STRING_LEN, Value};

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_WIRE_DEPTH: usize = 128;

const NODE_BRIDGE: &str = r#"
'use strict';
const net = require('node:net');
const socket = net.connect({host:'127.0.0.1',port:Number(process.env.NAPI_VM_BRIDGE_PORT)});
let input = Buffer.alloc(0);
let serial = Promise.resolve();
let nextHandle = 1;
const refs = new Map();
function hold(fn, receiver) {
  if (refs.size >= 262144) throw new RangeError('native function handle limit exceeded');
  const id=nextHandle++;
  refs.set(id,{fn,receiver});
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
    return {t:'function',v:hold(value,receiver)};
  }
  if (value && typeof value.then === 'function') {
    const e=new TypeError('async native exports are unsupported'); e.code='ERR_NAPI_VM_ASYNC_NATIVE_EXPORT_UNSUPPORTED'; throw e;
  }
  if (Buffer.isBuffer(value)) return {t:'bytes',v:Array.from(value)};
  if (ArrayBuffer.isView(value)) return {t:'bytes',v:Array.from(new Uint8Array(value.buffer,value.byteOffset,value.byteLength))};
  if (value instanceof ArrayBuffer) return {t:'bytes',v:Array.from(new Uint8Array(value))};
  if (active.has(value)) throw new TypeError('cyclic native values are unsupported');
  active.add(value);
  let result;
  if (Array.isArray(value)) {
    if (value.length>262144) throw new RangeError('native array exceeds the VM limit');
    result={t:'array',v:Array.from(value,v=>encode(v,value,depth+1,active))};
  } else {
    const proto=Object.getPrototypeOf(value);
    if(proto!==Object.prototype&&proto!==null)throw new TypeError('native objects with custom prototypes are unsupported');
    const entries=new Map();
    for (const key of Object.keys(value)) entries.set(key,encode(value[key],value,depth+1,active));
    if(entries.size>262144)throw new RangeError('native object exceeds the VM limit');
    result={t:'object',v:Array.from(entries,([k,v])=>[k,v])};
  }
  active.delete(value); return result;
}
function decode(value,depth) {
  if(depth>128)throw new RangeError('guest argument depth exceeded');
  switch(value.t) {
    case 'undefined':return undefined; case 'null':return null;
    case 'boolean':return value.v;
    case 'number':if(value.v==='-0')return -0;if(value.v==='NaN')return NaN;if(value.v==='Infinity')return Infinity;if(value.v==='-Infinity')return -Infinity;return Number(value.v);
    case 'string':return value.v; case 'bigint':return BigInt(value.v);
    case 'bytes':return Buffer.from(value.v);
    case 'array':return value.v.map(v=>decode(v,depth+1));
    case 'object':{const o={};for(const [k,v]of value.v)Object.defineProperty(o,k,{value:decode(v,depth+1),enumerable:true,writable:true,configurable:true});return o;}
    default:throw new TypeError('unsupported napi-vm argument');
  }
}
async function dispatch(r) {
  try {
    let result;
    if(r.op==='load')result=require(r.filename);
    else {
      const entry=refs.get(r.id);if(!entry)throw new Error('native function handle is invalid');
      const args=r.args.map(v=>decode(v,0));
      result=r.op==='construct'?Reflect.construct(entry.fn,args):Reflect.apply(entry.fn,entry.receiver,args);
    }
    return {requestId:r.requestId,ok:true,value:encode(result,undefined,0,new Set())};
  } catch(e) {
    return {requestId:r.requestId,ok:false,error:{name:typeof e?.name==='string'?e.name:'Error',message:typeof e?.message==='string'?e.message:String(e),code:typeof e?.code==='string'?e.code:undefined}};
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
    request_id: u64,
    failed: bool,
}
impl Drop for State {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
/// Addon exports cross into napi-vm as host functions and invoke synchronously
/// over a bounded bridge. Native addon code runs with host privileges and is
/// not contained by the guest sandbox.
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
            .set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| VmErr::Msg(format!("cannot set Node bridge timeout: {e}")))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| VmErr::Msg(format!("cannot set Node bridge timeout: {e}")))?;
        let hello = read_frame(&mut stream)?;
        if hello.get("hello").and_then(JsonValue::as_str) != Some(&token) {
            return Err(VmErr::Msg("Node sidecar authentication failed".into()));
        }
        Ok(Self {
            state: Rc::new(RefCell::new(State {
                child: child.into_inner(),
                stream,
                request_id: 1,
                failed: false,
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
        let response = match read_frame(&mut state.stream) {
            Ok(response) => response,
            Err(error) => {
                fail_state(&mut state);
                return Err(error);
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
        wire_to_guest(&self.request(json!({"op":"load","filename":filename}))?, 0)
    }
}

impl HostBridge for NodeAddonSidecar {
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        let args = args
            .iter()
            .map(|v| guest_to_wire(v, 0, &mut Vec::new()))
            .collect::<Result<Vec<_>, _>>()?;
        wire_to_guest(&self.request(json!({"op":"call","id":id,"args":args}))?, 0)
    }
    fn construct_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        let args = args
            .iter()
            .map(|v| guest_to_wire(v, 0, &mut Vec::new()))
            .collect::<Result<Vec<_>, _>>()?;
        wire_to_guest(
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

fn guest_to_wire(v: &Value, depth: usize, active: &mut Vec<usize>) -> Result<JsonValue, VmErr> {
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
                .map(|x| guest_to_wire(x, depth + 1, active))
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
                .map(|(k, x)| Ok(json!([k, guest_to_wire(x, depth + 1, active)?])))
                .collect::<Result<Vec<_>, VmErr>>()?;
            active.pop();
            json!({"t":"object","v":wire})
        }
        Value::ArrayBuffer(bytes) => json!({"t":"bytes","v":bytes.borrow().as_slice()}),
        Value::BigInt(x) => json!({"t":"bigint","v":x.to_string()}),
        Value::Function(_) | Value::HostFunction { .. } => {
            return Err(VmErr::Msg(
                "guest callbacks cannot be passed to Node addons yet".into(),
            ));
        }
        _ => {
            return Err(VmErr::Msg(
                "this guest value cannot cross the Node addon bridge yet".into(),
            ));
        }
    })
}

fn wire_to_guest(v: &JsonValue, depth: usize) -> Result<Value, VmErr> {
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
                name: "nodeAddon".into(),
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
                    .map(|x| wire_to_guest(x, depth + 1))
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
                props.push((key.to_string(), wire_to_guest(&pair[1], depth + 1)?));
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
  if (napi_get_undefined(env, &result) != napi_ok) return NULL;
  if (napi_resolve_deferred(env, deferred, result) != napi_ok) return NULL;
  return promise;
}

static napi_value init(napi_env env, napi_value exports) {
  napi_value fn;
  if (napi_create_function(env, "add", NAPI_AUTO_LENGTH, add, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "add", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "big", NAPI_AUTO_LENGTH, big, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "big", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "fail", NAPI_AUTO_LENGTH, fail, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "fail", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseResult", NAPI_AUTO_LENGTH, promise_result, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseResult", fn) != napi_ok) return NULL;
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
        let async_error = interpreter
            .eval_source(
                "try { require('./fixture.node').promiseResult(); } catch (error) { ({name:error.name, code:error.code}); }",
            )
            .unwrap();
        assert!(
            matches!(async_error.get_prop("name"), Some(Value::String(ref name)) if name == "TypeError")
        );
        assert!(
            matches!(async_error.get_prop("code"), Some(Value::String(ref code)) if code == "ERR_NAPI_VM_ASYNC_NATIVE_EXPORT_UNSUPPORTED")
        );

        drop(interpreter);
        fs::remove_dir_all(root).unwrap();
    }
}
