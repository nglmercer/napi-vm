//! The web-platform globals that are pure computation.
//!
//! `TextEncoder`/`TextDecoder`, `URLSearchParams` and `structuredClone` need
//! no capability: they transform values the guest already holds. APIs that
//! reach outside the sandbox are installed by their capability host. For
//! example, the fetch capability installs working `fetch`, `Request`,
//! `Response` and `Headers` globals over the inert placeholders after
//! permission has been granted.

use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{TypedArrayData, TypedKind, Value};

pub(super) fn install(e: &mut Environment) {
    if let Some(namespace) = e.get("TextEncoder") {
        namespace
            .set_prop("encode".to_string(), super::nf("encode", encode))
            .expect("built-in TextEncoder property");
        namespace
            .set_prop("encoding".to_string(), Value::String("utf-8".to_string()))
            .expect("built-in TextEncoder property");
        super::make_callable(&namespace, new_text_encoder, None);
    }
    if let Some(namespace) = e.get("TextDecoder") {
        namespace
            .set_prop("decode".to_string(), super::nf("decode", decode))
            .expect("built-in TextDecoder property");
        namespace
            .set_prop("encoding".to_string(), Value::String("utf-8".to_string()))
            .expect("built-in TextDecoder property");
        super::make_callable(&namespace, new_text_decoder, None);
    }
    if let Some(namespace) = e.get("URLSearchParams") {
        super::make_callable(&namespace, new_search_params, None);
    }
    e.set(
        "structuredClone",
        super::nf("structuredClone", structured_clone),
    );
}

// --- TextEncoder / TextDecoder ----------------------------------------------

/// An encoder or decoder instance. Both are stateless, so the instance carries
/// only its methods.
fn text_codec(encode_method: bool) -> Result<Value, VmErr> {
    let instance = Value::object(vec![(
        "encoding".to_string(),
        Value::String("utf-8".to_string()),
    )]);
    if encode_method {
        instance.set_prop("encode".to_string(), super::nf("encode", encode))?;
    } else {
        instance.set_prop("decode".to_string(), super::nf("decode", decode))?;
    }
    Ok(instance)
}

fn new_text_encoder(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    text_codec(true)
}

fn new_text_decoder(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    text_codec(false)
}

/// `encode(text)`: UTF-8 bytes as a `Uint8Array`.
fn encode(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let text = match a.first() {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Undefined) | None => String::new(),
        Some(other) => interp.vs(other)?,
    };
    let bytes = text.into_bytes();
    let length = bytes.len();
    Ok(Value::TypedArray(Rc::new(TypedArrayData {
        kind: TypedKind::Uint8,
        buffer: Rc::new(std::cell::RefCell::new(bytes)),
        byte_offset: 0,
        length,
    })))
}

/// `decode(bytes)`: the UTF-8 text of a buffer or view. Invalid sequences
/// become the replacement character, as the specification's non-fatal mode
/// requires.
fn decode(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let bytes = match a.first() {
        Some(Value::ArrayBuffer(buffer)) => buffer.borrow().clone(),
        Some(Value::TypedArray(view)) | Some(Value::DataView(view)) => {
            let source = view.buffer.borrow();
            let from = view.byte_offset.min(source.len());
            let to = (from + view.length * view.kind.size()).min(source.len());
            source[from..to].to_vec()
        }
        _ => Vec::new(),
    };
    Value::checked_string(String::from_utf8_lossy(&bytes).into_owned())
}

// --- URLSearchParams --------------------------------------------------------

/// Slot holding the `[key, value]` pairs, in insertion order — the same shape
/// the collections use.
const PAIRS_SLOT: &str = "__symbol_search_pairs__";

fn pairs_of(this: &Value) -> Option<Rc<crate::value::ArrayCell>> {
    this.get_prop(PAIRS_SLOT)?.as_array()
}

/// `new URLSearchParams(init)`: a query string, an array of pairs, or an
/// object of entries.
fn new_search_params(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let mut pairs: Vec<Value> = Vec::new();
    match a.first() {
        Some(Value::String(query)) => {
            for part in query.trim_start_matches('?').split('&') {
                if part.is_empty() {
                    continue;
                }
                let (key, value) = match part.split_once('=') {
                    Some((key, value)) => (key, value),
                    None => (part, ""),
                };
                pairs.push(Value::array(vec![
                    Value::String(percent_decode(key)),
                    Value::String(percent_decode(value)),
                ]));
            }
        }
        Some(Value::Array(items)) => {
            for entry in items.borrow().iter() {
                let key = entry.get_prop("0").unwrap_or(Value::Undefined);
                let value = entry.get_prop("1").unwrap_or(Value::Undefined);
                pairs.push(Value::array(vec![
                    Value::String(interp.vs(&key)?),
                    Value::String(interp.vs(&value)?),
                ]));
            }
        }
        Some(source @ Value::Object { .. }) => {
            let object = source.clone();
            let entries = interp.member(&object, "__none__").ok();
            let _ = entries;
            for key in interp.keys(&object) {
                let value = interp.member(&object, &key)?;
                pairs.push(Value::array(vec![
                    Value::String(key),
                    Value::String(interp.vs(&value)?),
                ]));
            }
        }
        _ => {}
    }

    let params = Value::object(vec![(PAIRS_SLOT.to_string(), Value::array(pairs))]);
    let methods: &[(&str, super::NativeFn)] = &[
        ("get", params_get),
        ("getAll", params_get_all),
        ("has", params_has),
        ("set", params_set),
        ("append", params_append),
        ("delete", params_delete),
        ("keys", params_keys),
        ("values", params_values),
        ("entries", params_entries),
        ("forEach", params_for_each),
        ("toString", params_to_string),
    ];
    for (name, callable) in methods {
        params.set_prop(name.to_string(), super::nf(name, *callable))?;
    }
    params.set_prop("size".to_string(), super::nf("get size", params_size))?;
    params.set_prop(
        crate::interpreter::SYMBOL_ITERATOR_SLOT.to_string(),
        super::nf("[Symbol.iterator]", params_entries),
    )?;
    Ok(params)
}

/// Does this `[key, value]` pair have the given key?
fn pair_key_is(pair: &Value, key: &str) -> bool {
    matches!(&pair.get_prop("0"), Some(Value::String(k)) if k == key)
}

fn key_arg(interp: &Interpreter, a: &[Value]) -> Result<String, VmErr> {
    match a.first() {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => interp.vs(other),
        None => Ok("undefined".to_string()),
    }
}

fn params_get(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let key = key_arg(interp, &a)?;
    let Some(pairs) = pairs_of(&this) else {
        return Ok(Value::Null);
    };
    let found = pairs
        .borrow()
        .iter()
        .find(|pair| pair_key_is(pair, &key))
        .and_then(|pair| pair.get_prop("1"));
    Ok(found.unwrap_or(Value::Null))
}

fn params_get_all(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let key = key_arg(interp, &a)?;
    let Some(pairs) = pairs_of(&this) else {
        return Value::checked_array(vec![]);
    };
    let found: Vec<Value> = pairs
        .borrow()
        .iter()
        .filter(|pair| pair_key_is(pair, &key))
        .filter_map(|pair| pair.get_prop("1"))
        .collect();
    Value::checked_array(found)
}

fn params_has(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let key = key_arg(interp, &a)?;
    let Some(pairs) = pairs_of(&this) else {
        return Ok(Value::Bool(false));
    };
    let present = pairs.borrow().iter().any(|pair| pair_key_is(pair, &key));
    Ok(Value::Bool(present))
}

/// `set(key, value)` replaces every existing entry for `key` with one.
fn params_set(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let key = key_arg(interp, &a)?;
    let value = match a.get(1) {
        Some(v) => interp.vs(v)?,
        None => String::new(),
    };
    let Some(pairs) = pairs_of(&this) else {
        return Ok(Value::Undefined);
    };
    let mut items = pairs.borrow_mut();
    items.retain(|pair| !pair_key_is(pair, &key));
    items.push(Value::array(vec![Value::String(key), Value::String(value)]));
    Ok(Value::Undefined)
}

fn params_append(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let key = key_arg(interp, &a)?;
    let value = match a.get(1) {
        Some(v) => interp.vs(v)?,
        None => String::new(),
    };
    if let Some(pairs) = pairs_of(&this) {
        if pairs.borrow().len() >= crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum array length exceeded"));
        }
        pairs
            .borrow_mut()
            .push(Value::array(vec![Value::String(key), Value::String(value)]));
    }
    Ok(Value::Undefined)
}

fn params_delete(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let key = key_arg(interp, &a)?;
    if let Some(pairs) = pairs_of(&this) {
        pairs.borrow_mut().retain(|pair| !pair_key_is(pair, &key));
    }
    Ok(Value::Undefined)
}

fn params_size(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(
        pairs_of(&this).map(|p| p.borrow().len()).unwrap_or(0) as f64,
    ))
}

fn params_projection(
    interp: &mut Interpreter,
    this: &Value,
    project: impl Fn(&Value) -> Value,
) -> Result<Value, VmErr> {
    let projected: Vec<Value> = pairs_of(this)
        .map(|pairs| pairs.borrow().iter().map(&project).collect())
        .unwrap_or_default();
    let array = Value::checked_array(projected)?;
    let iterator = interp.prop(
        &array,
        &Value::String(crate::interpreter::SYMBOL_ITERATOR_SLOT.to_string()),
    )?;
    interp.call_this(&iterator, array, vec![])
}

fn params_keys(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    params_projection(interp, &this, |pair| {
        pair.get_prop("0").unwrap_or(Value::Undefined)
    })
}

fn params_values(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    params_projection(interp, &this, |pair| {
        pair.get_prop("1").unwrap_or(Value::Undefined)
    })
}

fn params_entries(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    params_projection(interp, &this, |pair| pair.clone())
}

fn params_for_each(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let callback = a.first().cloned().unwrap_or(Value::Undefined);
    let snapshot: Vec<Value> = pairs_of(&this)
        .map(|pairs| pairs.borrow().clone())
        .unwrap_or_default();
    for pair in snapshot {
        let key = pair.get_prop("0").unwrap_or(Value::Undefined);
        let value = pair.get_prop("1").unwrap_or(Value::Undefined);
        interp.call_this(&callback, Value::Undefined, vec![value, key, this.clone()])?;
    }
    Ok(Value::Undefined)
}

fn params_to_string(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let snapshot: Vec<Value> = pairs_of(&this)
        .map(|pairs| pairs.borrow().clone())
        .unwrap_or_default();
    let mut out = String::new();
    for pair in snapshot {
        if !out.is_empty() {
            out.push('&');
        }
        let key = pair.get_prop("0").unwrap_or(Value::Undefined);
        let value = pair.get_prop("1").unwrap_or(Value::Undefined);
        out.push_str(&percent_encode(&interp.vs(&key)?));
        out.push('=');
        out.push_str(&percent_encode(&interp.vs(&value)?));
        if out.len() > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
    }
    Value::checked_string(out)
}

/// `application/x-www-form-urlencoded` encoding: unreserved characters pass
/// through, a space becomes `+`, everything else becomes `%XX`.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{:02X}", other)),
        }
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    // A stray `%` that is not an escape stands for itself.
                    Err(_) => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// --- structuredClone --------------------------------------------------------

/// `structuredClone(value)`: a deep copy that preserves shared references and
/// cycles, and copies the types the algorithm covers by value.
///
/// Functions, symbols and proxies have no structured-clone form; the
/// specification throws a `DataCloneError` for them, and so does this.
fn structured_clone(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let source = a.first().cloned().unwrap_or(Value::Undefined);
    let mut seen: Vec<(usize, Value)> = Vec::new();
    clone_value(&source, &mut seen, 0)
}

/// Maximum nesting `structuredClone` will follow, matching the marshalling
/// depth cap: a graph deeper than this is refused rather than overflowing the
/// native stack.
const MAX_CLONE_DEPTH: usize = 512;

fn clone_value(
    value: &Value,
    seen: &mut Vec<(usize, Value)>,
    depth: usize,
) -> Result<Value, VmErr> {
    if depth > MAX_CLONE_DEPTH {
        return Err(VmErr::Msg(
            "DataCloneError: value is too deeply nested to clone".to_string(),
        ));
    }
    Ok(match value {
        Value::Undefined
        | Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::BigInt(_) => value.clone(),
        Value::Date(ms) => Value::Date(Rc::new(std::cell::Cell::new(ms.get()))),
        Value::ArrayBuffer(bytes) => {
            Value::ArrayBuffer(Rc::new(std::cell::RefCell::new(bytes.borrow().clone())))
        }
        Value::TypedArray(view) | Value::DataView(view) => {
            let copy = Rc::new(std::cell::RefCell::new(view.buffer.borrow().clone()));
            let cloned = Rc::new(TypedArrayData {
                kind: view.kind,
                buffer: copy,
                byte_offset: view.byte_offset,
                length: view.length,
            });
            match value {
                Value::DataView(_) => Value::DataView(cloned),
                _ => Value::TypedArray(cloned),
            }
        }
        Value::Array(cell) => {
            let identity = Rc::as_ptr(cell) as usize;
            if let Some((_, existing)) = seen.iter().find(|(id, _)| *id == identity) {
                return Ok(existing.clone());
            }
            // The clone is registered *before* its elements are copied, so a
            // cycle resolves to it rather than recursing.
            let cloned = Value::array(Vec::new());
            seen.push((identity, cloned.clone()));
            let source = cell.borrow().clone();
            let Value::Array(target) = &cloned else {
                unreachable!("just built an array");
            };
            for item in source {
                let copied = clone_value(&item, seen, depth + 1)?;
                target.borrow_mut().push(copied);
            }
            cloned
        }
        Value::Object { props } => {
            let identity = Rc::as_ptr(props) as usize;
            if let Some((_, existing)) = seen.iter().find(|(id, _)| *id == identity) {
                return Ok(existing.clone());
            }
            let cloned = Value::object(Vec::new());
            seen.push((identity, cloned.clone()));
            let entries = props.borrow().clone();
            for (key, item) in entries {
                if crate::interpreter::is_internal_key(&key) {
                    continue;
                }
                let copied = clone_value(&item, seen, depth + 1)?;
                cloned.set_prop(key, copied)?;
            }
            cloned
        }
        Value::Function(_)
        | Value::NativeFunction { .. }
        | Value::HostFunction { .. }
        | Value::Class(_)
        | Value::Symbol(_)
        | Value::Proxy(_) => {
            return Err(VmErr::Msg(
                "DataCloneError: value could not be cloned".to_string(),
            ));
        }
        other => other.clone(),
    })
}
