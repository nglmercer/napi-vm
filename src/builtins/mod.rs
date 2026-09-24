mod array;
mod bigint;
mod buffer;
mod collections;
mod date;
mod error;
mod function;
mod json;
mod math;
mod number;
pub(crate) mod object;
mod promise;
mod proxy;
mod reflect;
pub(crate) mod regexp;
mod string;
mod symbol;
mod typedarray;
mod web;

pub(crate) use function::{function_method, is_default_has_instance_method};
pub(crate) use promise::promise_method;

pub use array::array_method;
pub use bigint::bigint_method;
pub use collections::{collection_entries_of, collection_tag, describe_collection};
pub use date::{date_member, iso_string};
pub use error::error_to_string;
pub use number::number_method;
pub(crate) use regexp::compile as compile_regex;
pub use regexp::regexp_member;
pub use string::string_method;
pub use symbol::new_symbol;
pub use symbol::symbol_method;
pub(crate) use symbol::{
    is_iterator_symbol, symbol_for, symbol_for_key, symbol_key_for, well_known,
};
pub use typedarray::{
    array_buffer_member, data_view_member, read_element, shared_array_buffer_member, typed_member,
    write_element,
};

use crate::error::VmErr;
use crate::interpreter::{Env, Environment, Interpreter};
use crate::value::{BoxedPrimitive, PropAttrs, Value};
use std::rc::Rc;

pub fn setup_builtins(env: &Env) {
    let mut e = env.borrow_mut();

    let simple: &[&str] = &[
        "Boolean",
        "Map",
        "Set",
        "WeakMap",
        "WeakSet",
        "ArrayBuffer",
        "DataView",
        "SharedArrayBuffer",
        "Buffer",
        "Atomics",
        "RegExp",
        "Function",
        "globalThis",
        "self",
        "window",
        "URLSearchParams",
        "Headers",
        "Request",
        "Event",
        "EventTarget",
        "CustomEvent",
        "AbortController",
        "AbortSignal",
        "TextEncoder",
        "TextDecoder",
        "ReadableStream",
        "WritableStream",
        "TransformStream",
        "Blob",
        "File",
        "FormData",
        "queueMicrotask",
        "setTimeout",
        "setInterval",
        "clearTimeout",
        "clearInterval",
        "structuredClone",
        "Proxy",
        "undefined",
        "isNaN",
        "isFinite",
        "parseInt",
        "parseFloat",
        "encodeURI",
        "decodeURI",
        "encodeURIComponent",
        "decodeURIComponent",
        "escape",
        "unescape",
        "eval",
        "require",
        "exports",
        "__dirname",
        "__filename",
        "Worker",
        "SharedWorker",
        "MessageChannel",
        "MessagePort",
        "BroadcastChannel",
        "EventSource",
        "ByteLengthQueuingStrategy",
        "CountQueuingStrategy",
        "CompressionStream",
        "DecompressionStream",
        "DOMException",
        "Lock",
        "LockManager",
        "Navigation",
        "Navigator",
        "Notification",
        "PermissionStatus",
        "Permissions",
        "PushManager",
        "PushSubscription",
        "PushSubscriptionOptions",
        "Scheduler",
        "StorageManager",
        "Worklet",
        "CryptoKey",
        "GPU",
        "GPUAdapter",
        "GPUBindGroup",
        "GPUBuffer",
        "GPUCanvasContext",
        "GPUCommandBuffer",
        "GPUCommandEncoder",
        "GPUComputePassEncoder",
        "GPUComputePipeline",
        "GPUDevice",
        "GPUExternalTexture",
        "GPUPipelineLayout",
        "GPUQuerySet",
        "GPUQueue",
        "GPURenderBundle",
        "GPURenderBundleEncoder",
        "GPURenderPassEncoder",
        "GPURenderPipeline",
        "GPUSampler",
        "GPUShaderModule",
        "GPUTexture",
        "GPUTextureView",
        "WGSLLanguageFeatures",
        "importScripts",
        "close",
        "postMessage",
        "parentPort",
        "threadId",
        "workerData",
        "isMainThread",
        "WritableStreamDefaultWriter",
        "WritableStreamDefaultController",
        "ReadableStreamDefaultReader",
        "ReadableStreamBYOBReader",
        "ReadableStreamDefaultController",
        "ReadableByteStreamController",
        "TransformStreamDefaultController",
        "AudioData",
        "EncodedAudioChunk",
        "EncodedVideoChunk",
        "ImageBitmap",
        "OffscreenCanvas",
        "VideoFrame",
        "WebSocketStream",
        "Serial",
        "USB",
        "HID",
        "Bluetooth",
        "Clipboard",
        "Credential",
        "CredentialsContainer",
        "Geolocation",
        "GeolocationPosition",
        "GeolocationCoordinates",
        "GeolocationPositionError",
        "ServiceWorker",
        "ServiceWorkerContainer",
        "ServiceWorkerRegistration",
        "ServiceWorkerGlobalScope",
        "DedicatedWorkerGlobalScope",
        "SharedWorkerGlobalScope",
        "WorkerGlobalScope",
        "UnloadEvent",
    ];
    for name in simple {
        e.set(name, Value::object(vec![]));
    }

    // `globalThis`, `self` and `window` are not plain empty objects: they all
    // denote the global scope itself, so member access on them reads and writes
    // real globals (see `Interpreter::prop` / `assign_member`).
    e.set("globalThis", Value::GlobalObject);
    e.set("self", Value::GlobalObject);
    e.set("window", Value::GlobalObject);

    let with_members: &[(&str, &[&str])] = &[
        ("console", &["log", "error", "warn", "info", "debug", "dir"]),
        ("Object", &["keys", "values", "entries", "assign"]),
        ("Array", &["isArray", "from", "of"]),
        ("String", &["fromCharCode"]),
        ("Number", &["isNaN", "isFinite", "parseInt", "parseFloat"]),
        ("Promise", &["resolve", "reject", "all", "race"]),
        ("ArrayBuffer", &["isView"]),
        ("Date", &["now", "parse", "UTC"]),
        ("URL", &["createObjectURL", "revokeObjectURL"]),
        ("Response", &["json", "text", "redirect"]),
        ("WebSocket", &["CONNECTING", "OPEN", "CLOSING", "CLOSED"]),
        ("crypto", &["getRandomValues", "randomUUID", "subtle"]),
        ("navigator", &["userAgent", "language", "platform"]),
        ("performance", &["now"]),
        ("BigInt", &["asIntN", "asUintN"]),
        (
            "Reflect",
            &[
                "apply",
                "construct",
                "defineProperty",
                "deleteProperty",
                "get",
                "has",
                "set",
            ],
        ),
        ("Intl", &["DateTimeFormat", "NumberFormat"]),
        ("module", &["exports"]),
        (
            "process",
            &["env", "argv", "cwd", "pid", "platform", "version"],
        ),
        ("Buffer", &["alloc", "from", "concat", "isBuffer"]),
        (
            "location",
            &[
                "href", "protocol", "host", "pathname", "search", "hash", "origin",
            ],
        ),
        (
            "history",
            &[
                "length",
                "go",
                "back",
                "forward",
                "pushState",
                "replaceState",
            ],
        ),
        ("screen", &["width", "height"]),
        (
            "localStorage",
            &["getItem", "setItem", "removeItem", "clear"],
        ),
        (
            "sessionStorage",
            &["getItem", "setItem", "removeItem", "clear"],
        ),
        ("indexedDB", &["open", "deleteDatabase"]),
        ("caches", &["open", "has", "delete", "keys", "match"]),
        ("Cache", &["match", "add", "put", "delete", "keys"]),
        ("CacheStorage", &["open", "has", "delete", "keys"]),
        (
            "SubtleCrypto",
            &[
                "encrypt",
                "decrypt",
                "sign",
                "verify",
                "digest",
                "generateKey",
                "deriveKey",
                "deriveBits",
                "importKey",
                "exportKey",
                "wrapKey",
                "unwrapKey",
            ],
        ),
        (
            "MessageEvent",
            &["data", "origin", "lastEventId", "source", "ports"],
        ),
        (
            "ErrorEvent",
            &["message", "filename", "lineno", "colno", "error"],
        ),
        ("PromiseRejectionEvent", &["promise", "reason"]),
        ("CloseEvent", &["code", "reason", "wasClean"]),
        ("HashChangeEvent", &["oldURL", "newURL"]),
        ("PopStateEvent", &["state"]),
        (
            "StorageEvent",
            &["key", "oldValue", "newValue", "url", "storageArea"],
        ),
        ("SubmitEvent", &["submitter"]),
        ("FormDataEvent", &["formData"]),
        ("ProgressEvent", &["lengthComputable", "loaded", "total"]),
        ("PageTransitionEvent", &["persisted"]),
        ("BeforeUnloadEvent", &["returnValue"]),
        ("UIEvent", &["detail", "view", "which"]),
        (
            "MouseEvent",
            &[
                "screenX",
                "screenY",
                "clientX",
                "clientY",
                "ctrlKey",
                "shiftKey",
                "altKey",
                "metaKey",
                "button",
                "buttons",
                "relatedTarget",
            ],
        ),
        (
            "KeyboardEvent",
            &[
                "key",
                "code",
                "location",
                "ctrlKey",
                "shiftKey",
                "altKey",
                "metaKey",
                "repeat",
                "isComposing",
            ],
        ),
        (
            "TouchEvent",
            &["touches", "targetTouches", "changedTouches"],
        ),
        (
            "Touch",
            &[
                "identifier",
                "target",
                "screenX",
                "screenY",
                "clientX",
                "clientY",
                "pageX",
                "pageY",
            ],
        ),
        ("WheelEvent", &["deltaX", "deltaY", "deltaZ", "deltaMode"]),
        ("DragEvent", &["dataTransfer"]),
        ("FocusEvent", &["relatedTarget"]),
        ("InputEvent", &["data", "inputType", "isComposing"]),
        ("CompositionEvent", &["data"]),
        (
            "PointerEvent",
            &[
                "pointerId",
                "width",
                "height",
                "pressure",
                "pointerType",
                "isPrimary",
            ],
        ),
        (
            "AnimationEvent",
            &["animationName", "elapsedTime", "pseudoElement"],
        ),
        (
            "TransitionEvent",
            &["propertyName", "elapsedTime", "pseudoElement"],
        ),
        ("ClipboardEvent", &["clipboardData"]),
        (
            "SecurityPolicyViolationEvent",
            &["documentURI", "referrer", "blockedURI", "violatedDirective"],
        ),
        ("JSON", &["parse", "stringify"]),
    ];
    for (name, members) in with_members {
        let props: Vec<(String, Value)> = members
            .iter()
            .map(|m| (m.to_string(), Value::Undefined))
            .collect();
        e.set(name, Value::object(props));
    }

    e.set(
        "Math",
        Value::object(vec![
            ("PI".to_string(), Value::Number(std::f64::consts::PI)),
            ("E".to_string(), Value::Number(std::f64::consts::E)),
            ("LN2".to_string(), Value::Number(std::f64::consts::LN_2)),
            ("LN10".to_string(), Value::Number(std::f64::consts::LN_10)),
            ("LOG2E".to_string(), Value::Number(std::f64::consts::LOG2_E)),
            (
                "LOG10E".to_string(),
                Value::Number(std::f64::consts::LOG10_E),
            ),
            (
                "SQRT1_2".to_string(),
                Value::Number(std::f64::consts::FRAC_1_SQRT_2),
            ),
            ("SQRT2".to_string(), Value::Number(std::f64::consts::SQRT_2)),
            ("abs".to_string(), Value::Undefined),
            ("floor".to_string(), Value::Undefined),
            ("ceil".to_string(), Value::Undefined),
            ("round".to_string(), Value::Undefined),
            ("sqrt".to_string(), Value::Undefined),
            ("pow".to_string(), Value::Undefined),
            ("min".to_string(), Value::Undefined),
            ("max".to_string(), Value::Undefined),
            ("random".to_string(), Value::Undefined),
        ]),
    );

    install_functions(&mut e);

    e.set("Infinity", Value::Number(f64::INFINITY));
    e.set("NaN", Value::Number(f64::NAN));
}

/// Overwrite the placeholder members above with real native implementations.
fn install_functions(e: &mut crate::interpreter::Environment) {
    math::install(e);
    object::install(e);
    function::install(e);
    array::install(e);
    string::install(e);
    number::install(e);
    json::install(e);
    date::install(e);
    error::install(e);
    promise::install(e);
    reflect::install(e);
    collections::install(e);
    regexp::install(e);
    bigint::install(e);
    typedarray::install(e);
    buffer::install(e);
    proxy::install(e);
    web::install(e);
    symbol::install(e);
    // Global functions.
    e.set("parseInt", nf("parseInt", number::parse_int));
    e.set("parseFloat", nf("parseFloat", number::parse_float));
    e.set("isNaN", nf("isNaN", global_is_nan));
    e.set("isFinite", nf("isFinite", global_is_finite));

    // console: route output to the host's stdout/stderr.
    if let Some(c) = e.get("console") {
        c.set_prop("log".to_string(), nf("log", console_out))
            .expect("built-in console property");
        c.set_prop("info".to_string(), nf("info", console_out))
            .expect("built-in console property");
        c.set_prop("debug".to_string(), nf("debug", console_out))
            .expect("built-in console property");
        c.set_prop("error".to_string(), nf("error", console_err))
            .expect("built-in console property");
        c.set_prop("warn".to_string(), nf("warn", console_err))
            .expect("built-in console property");
        c.set_prop("dir".to_string(), nf("dir", console_dir))
            .expect("built-in console property");
    }
}

/// Give a callable built-in its own prototype object and the standard
/// `Function.prototype` parent. The prototype object itself is prepared by
/// the builtin installer so each constructor can define its own methods.
pub(crate) fn set_builtin_constructor_prototype(
    e: &Environment,
    constructor: &Value,
    prototype: Value,
) {
    constructor
        .set_prop("prototype".into(), prototype)
        .expect("built-in constructor prototype");
    if let Value::Object { props } = constructor {
        props.meta.borrow_mut().set_attrs(
            "prototype",
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: false,
            },
        );
        if let Some(function_prototype) = e
            .get("Function")
            .and_then(|function| function.get_prop("prototype"))
        {
            props.set_proto(Some(std::rc::Rc::new(function_prototype)));
        }
    }
}

/// Install the shared prototype used by boxed primitive values. The wrapper
/// value is also the prototype object for string, number, boolean, bigint,
/// and symbol, so the VM can preserve primitive identity while exposing the
/// ordinary `Object.getPrototypeOf` relationship.
pub(crate) fn install_primitive_prototype(
    e: &Environment,
    constructor: &Value,
    primitive: Value,
    methods: Vec<(&str, Value)>,
) {
    let object_prototype = e
        .get("Object")
        .and_then(|object| object.get_prop("prototype"))
        .map(Rc::new);
    let prototype = Value::boxed_primitive(primitive)
        .expect("primitive constructors install primitive prototype values");
    if let Value::Object { props } = &prototype {
        props.set_proto(object_prototype);
    }
    prototype
        .set_prop("constructor".into(), constructor.clone())
        .expect("primitive prototype constructor");
    for (name, method) in &methods {
        prototype
            .set_prop((*name).into(), method.clone())
            .expect("primitive prototype method");
    }
    if let Value::Object { props } = &prototype {
        let mut meta = props.meta.borrow_mut();
        meta.set_attrs(
            "constructor",
            PropAttrs {
                enumerable: false,
                ..PropAttrs::default()
            },
        );
        for (name, _) in &methods {
            meta.set_attrs(
                name,
                PropAttrs {
                    enumerable: false,
                    ..PropAttrs::default()
                },
            );
        }
    }
    set_builtin_constructor_prototype(e, constructor, prototype);
}

// ===========================================================================
// Shared helpers for the native function implementations in the sub-modules.
// ===========================================================================

pub(crate) type NativeFn = fn(&mut Interpreter, Value, Vec<Value>) -> Result<Value, VmErr>;

fn nf(name: &str, callable: NativeFn) -> Value {
    Value::NativeFunction {
        name: name.into(),
        callable,
    }
}

/// Make a built-in namespace object callable.
///
/// `String`, `Number`, `Array` and friends are objects so they can carry their
/// statics, but they are also functions. Installing the implementation in an
/// internal slot lets `Interpreter::call_this` and `Interpreter::ctor` find it
/// while keeping the statics where property access expects them. `construct`
/// is only needed where `new X(…)` differs from `X(…)`.
fn make_callable(target: &Value, call: NativeFn, construct: Option<NativeFn>) {
    target
        .set_prop(
            crate::interpreter::call::CALL_SLOT.to_string(),
            nf("call", call),
        )
        .expect("built-in call slot");
    if let Some(construct) = construct {
        target
            .set_prop(
                crate::interpreter::call::CONSTRUCT_SLOT.to_string(),
                nf("construct", construct),
            )
            .expect("built-in construct slot");
    }
}

fn arg_num(args: &[Value], i: usize) -> f64 {
    args.get(i).map(|v| v.to_number()).unwrap_or(f64::NAN)
}

fn arr_items(this: &Value) -> Vec<Value> {
    match this {
        Value::Array(a) => a.borrow().clone(),
        _ => vec![],
    }
}

fn str_this(interp: &Interpreter, this: &Value) -> Result<String, VmErr> {
    match this {
        Value::String(s) => Ok(s.clone()),
        Value::Object { props } => match props.meta.borrow().boxed_primitive.as_ref() {
            Some(BoxedPrimitive::String(value)) => Ok(value.clone()),
            _ => interp.vs(this),
        },
        _ => interp.vs(this),
    }
}

/// Display a value the way `Array.prototype.join` / string coercion does:
/// `null`/`undefined` become the empty string.
fn join_str(interp: &Interpreter, v: &Value) -> Result<String, VmErr> {
    match v {
        Value::Null | Value::Undefined => Ok(String::new()),
        _ => interp.vs(v),
    }
}

// --- Global functions -------------------------------------------------------

fn global_is_nan(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let n = a.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
    Ok(Value::Bool(n.is_nan()))
}
fn global_is_finite(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let n = a.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
    Ok(Value::Bool(n.is_finite()))
}

// --- console ----------------------------------------------------------------

/// Format console arguments the way `console.log` does: each value stringified
/// and joined with a single space.
fn console_fmt(interp: &mut Interpreter, a: &[Value]) -> Result<String, VmErr> {
    let mut output = crate::format::BoundedOutput::new(crate::value::MAX_STRING_LEN);
    for (index, value) in a.iter().enumerate() {
        if index > 0 {
            output.push_char(' ')?;
        }
        let rendered = interp.display_string(value)?;
        output.push_str(&rendered)?;
    }
    Ok(output.finish())
}

fn console_out(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    println!("{}", console_fmt(interp, &a)?);
    Ok(Value::Undefined)
}

fn console_err(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    eprintln!("{}", console_fmt(interp, &a)?);
    Ok(Value::Undefined)
}

/// `console.dir`: print each value with the pretty, multi-line, indented
/// expander (`bindings::to_string_pretty`) — the sandbox-native analogue of
/// Node's `util.inspect`. Nested objects/arrays render as an indented tree
/// instead of the opaque `[object Object]` that `console.log` uses.
/// Cycle- and depth-safe by construction of that formatter.
///
/// Values are type-colored (keys cyan, strings green, numbers blue, booleans
/// yellow, null/undefined dimmed) whenever stdout is a TTY, honoring
/// `NO_COLOR`/`FORCE_COLOR`. Like Node, an options object overrides the
/// auto-detection: `console.dir(obj, { colors: true })` forces ANSI codes
/// even into a pipe, `{ colors: false }` suppresses them.
fn console_dir(_interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    // Read the boolean options out of a trailing options object, if present.
    let colors_opt = match a.get(1) {
        Some(Value::Object { props, .. }) => {
            let b = props.borrow();
            b.iter()
                .find(|(k, _)| k == "colors")
                .and_then(|(_, v)| match v {
                    Value::Bool(x) => Some(*x),
                    _ => None,
                })
        }
        _ => None,
    };
    let colors = colors_opt.unwrap_or_else(crate::format::colors_enabled);

    // Only the values are printed; a trailing options object is not a value
    // to inspect (matches Node's `console.dir(obj, options)` signature).
    let values = if matches!(a.get(1), Some(Value::Object { .. })) && a.len() == 2 {
        &a[..1]
    } else {
        &a[..]
    };

    let mut output = crate::format::BoundedOutput::new(crate::value::MAX_STRING_LEN);
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push_char(' ')?;
        }
        let rendered = crate::format::try_to_string_pretty_colored(value, colors)?;
        output.push_str(&rendered)?;
    }
    println!("{}", output.finish());
    Ok(Value::Undefined)
}
