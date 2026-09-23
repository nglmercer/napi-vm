//! Structured marshalling of values across the NAPI boundary.
//!
//! The free functions in the parent module are string-only (`runCode` /
//! `getGlobal` return strings). This module passes *real* structured values
//! across the boundary so Node can expose functions to the VM and call VM
//! functions with live arguments. It is built directly on the stable raw
//! `napi_sys` ABI: the VM is single-threaded, so a persisted `napi_ref` to
//! a JavaScript function can be stored and invoked synchronously on the
//! same thread that drives the interpreter.
//!
//! Each marshalling helper is a *safe* function that confines all FFI to a
//! single `unsafe` block around its body; the `env` handle always
//! originates from a live N-API callback, so the operations are sound.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;
use std::sync::Arc;

use napi::sys;

use crate::error::VmErr;
use crate::value::{Buffer, MAX_ARRAY_LEN, MAX_OBJECT_PROPS, MAX_STRING_LEN, Value};

#[inline]
pub(super) fn chk(status: sys::napi_status) -> Result<(), VmErr> {
    if status == sys::Status::napi_ok {
        Ok(())
    } else {
        Err(VmErr::Msg(format!("napi call failed (status {})", status)))
    }
}

/// Create a JS string from a Rust `&str`.
pub(super) fn make_str(env: sys::napi_env, s: &str) -> Result<sys::napi_value, VmErr> {
    unsafe {
        let mut out = ptr::null_mut();
        chk(sys::napi_create_string_utf8(
            env,
            s.as_ptr() as *const c_char,
            s.len() as isize,
            &mut out,
        ))?;
        Ok(out)
    }
}

/// Set a string-valued named property on an object.
fn set_str_prop(
    env: sys::napi_env,
    obj: sys::napi_value,
    key: &str,
    val: &str,
) -> Result<(), VmErr> {
    let sv = make_str(env, val)?;
    define_named_property(env, obj, key, sv)
}

/// Define an own data property. Unlike ordinary `[[Set]]`, this does not
/// invoke the legacy `Object.prototype.__proto__` setter for guest-controlled
/// keys.
fn define_named_property(
    env: sys::napi_env,
    obj: sys::napi_value,
    key: &str,
    value: sys::napi_value,
) -> Result<(), VmErr> {
    let key = CString::new(key).map_err(|_| VmErr::Msg("object key contains NUL".to_string()))?;
    let descriptor = sys::napi_property_descriptor {
        utf8name: key.as_ptr(),
        name: ptr::null_mut(),
        method: None,
        getter: None,
        setter: None,
        value,
        attributes: sys::PropertyAttributes::writable
            | sys::PropertyAttributes::enumerable
            | sys::PropertyAttributes::configurable,
        data: ptr::null_mut(),
    };
    chk(unsafe { sys::napi_define_properties(env, obj, 1, &descriptor) })
}

/// Maximum nesting marshalled across the NAPI boundary in either direction.
/// A guest (or host) structure deeper than this yields a catchable error
/// instead of overflowing the native stack in the recursive walkers below.
const MAX_MARSHAL_DEPTH: usize = 512;

pub(super) fn to_napi(env: sys::napi_env, v: &Value) -> Result<sys::napi_value, VmErr> {
    to_napi_d(env, v, 0, &mut HashMap::new())
}

thread_local! {
    /// The VM a value is currently crossing *out of*.
    ///
    /// A VM function has no wire form; what crosses is a host callable that
    /// re-enters the interpreter, and building one needs a handle to the VM.
    /// The marshaller is a free function reached from several entry points, so
    /// the handle is installed around the crossing rather than threaded
    /// through every signature.
    static EXPORTING_VM: RefCell<Option<Arc<super::vm::VMState>>> = const { RefCell::new(None) };
}

/// Run `f` with `state` as the VM values are crossing out of, so functions can
/// be exported as host callables.
///
/// Restores the previous handle on the way out, including on the error path,
/// so a nested crossing cannot leave the wrong VM installed.
pub(super) fn exporting_from<R>(state: &Arc<super::vm::VMState>, f: impl FnOnce() -> R) -> R {
    let previous = EXPORTING_VM.with(|slot| slot.borrow_mut().replace(state.clone()));
    let result = f();
    EXPORTING_VM.with(|slot| *slot.borrow_mut() = previous);
    result
}

fn exporting_vm() -> Option<Arc<super::vm::VMState>> {
    EXPORTING_VM.with(|slot| slot.borrow().clone())
}

/// Marshal a VM `Value` into a raw N-API value.
///
/// Functions, promises, generators and other VM-only values have no faithful
/// representation in this direction yet and are surfaced as `undefined`.
fn to_napi_d(
    env: sys::napi_env,
    v: &Value,
    depth: usize,
    active: &mut HashMap<usize, sys::napi_value>,
) -> Result<sys::napi_value, VmErr> {
    if depth > MAX_MARSHAL_DEPTH {
        return Err(VmErr::Msg("value is too deep to marshal".to_string()));
    }
    unsafe {
        let mut out = ptr::null_mut();
        match v {
            Value::Undefined => chk(sys::napi_get_undefined(env, &mut out))?,
            Value::Null => chk(sys::napi_get_null(env, &mut out))?,
            Value::Bool(b) => chk(sys::napi_get_boolean(env, *b, &mut out))?,
            Value::Number(n) => chk(sys::napi_create_double(env, *n, &mut out))?,
            Value::String(s) => {
                if s.len() > MAX_STRING_LEN {
                    return Err(VmErr::Msg(
                        "RangeError: Maximum string length exceeded".to_string(),
                    ));
                }
                return make_str(env, s);
            }
            Value::Array(items) => {
                let identity = std::rc::Rc::as_ptr(items) as usize;
                // A repeated reference reuses the host object already built
                // for it, which is what lets a cyclic graph cross intact
                // instead of recursing forever.
                if let Some(existing) = active.get(&identity) {
                    return Ok(*existing);
                }
                let length = items.borrow().len();
                if length > MAX_ARRAY_LEN {
                    return Err(VmErr::Msg(
                        "RangeError: Maximum array length exceeded".to_string(),
                    ));
                }
                chk(sys::napi_create_array_with_length(env, length, &mut out))?;
                active.insert(identity, out);
                for i in 0..length {
                    let item = items.borrow()[i].clone();
                    let ev = to_napi_d(env, &item, depth + 1, active)?;
                    chk(sys::napi_set_element(env, out, i as u32, ev))?;
                }
            }
            // A VM `Map`/`Set` becomes a host one, built through the host's
            // own constructor so it is a real `Map`, not an object shaped like
            // its internals.
            Value::Object { .. } if crate::builtins::collection_entries_of(v).is_some() => {
                let (kind, entries) =
                    crate::builtins::collection_entries_of(v).expect("checked in the guard");
                out = make_collection(env, kind, &entries, depth, active)?;
            }
            Value::Object { props, .. } => {
                let identity = std::rc::Rc::as_ptr(props) as usize;
                if let Some(existing) = active.get(&identity) {
                    return Ok(*existing);
                }
                chk(sys::napi_create_object(env, &mut out))?;
                active.insert(identity, out);
                let entries = props.borrow().clone();
                if entries.len() > MAX_OBJECT_PROPS {
                    return Err(VmErr::Msg(
                        "RangeError: Maximum object property count exceeded".to_string(),
                    ));
                }
                let meta_enumerable: Vec<bool> = entries
                    .iter()
                    .map(|(k, _)| props.meta.borrow().attrs_of(k).enumerable)
                    .collect();
                for ((k, val), enumerable) in entries.iter().zip(meta_enumerable) {
                    // The VM's internal slots (symbol keys, private fields)
                    // are not part of the object's observable shape.
                    if !enumerable || crate::interpreter::is_internal_key(k) {
                        continue;
                    }
                    let ev = to_napi_d(env, val, depth + 1, active)?;
                    define_named_property(env, out, k, ev)?;
                }
            }
            Value::Error(e) => {
                chk(sys::napi_create_object(env, &mut out))?;
                set_str_prop(env, out, "name", &e.name)?;
                set_str_prop(env, out, "message", &e.message)?;
            }
            Value::Date(ms) => chk(sys::napi_create_date(env, ms.get(), &mut out))?,
            Value::BigInt(value) => {
                let (negative, words) = value.to_words();
                chk(sys::napi_create_bigint_words(
                    env,
                    i32::from(negative),
                    words.len(),
                    words.as_ptr(),
                    &mut out,
                ))?;
            }
            Value::Symbol(symbol) => {
                let description = match &symbol.description {
                    Some(text) => make_str(env, text)?,
                    None => ptr::null_mut(),
                };
                chk(sys::napi_create_symbol(env, description, &mut out))?;
            }
            Value::RegExp(re) => {
                // No `napi_create_regexp` exists, so the pattern crosses as a
                // plain object the host can feed to its own `RegExp`.
                chk(sys::napi_create_object(env, &mut out))?;
                set_str_prop(env, out, "source", &re.regex.source)?;
                set_str_prop(env, out, "flags", &re.regex.flags)?;
            }
            Value::ArrayBuffer(bytes) => {
                out = make_array_buffer(env, &bytes.borrow())?;
            }
            Value::SharedArrayBuffer(_) => {
                return Err(VmErr::Msg(
                    "SharedArrayBuffer cannot cross the napi-vm Node-API binding without shared-memory support".into(),
                ));
            }
            Value::TypedArray(view) | Value::DataView(view) => {
                out = make_typed_array(env, v, view)?;
            }
            // A proxy crosses as what it wraps: the traps are guest code and
            // cannot follow the value out.
            Value::Proxy(proxy) => {
                return to_napi_d(env, &proxy.target, depth, active);
            }
            Value::Promise(promise) => {
                out = make_promise(env, promise, depth, active)?;
            }
            // A function crosses as a host callable that re-enters the VM —
            // but only when there is a VM to re-enter. Reached from a context
            // with no VM handle (the string-only entry points), it stays
            // `undefined`, as it always did.
            value if super::export_fn::is_callable(value) => {
                out = match exporting_vm() {
                    Some(state) => super::export_fn::export(env, &state, value)?,
                    None => {
                        let mut undefined = ptr::null_mut();
                        chk(sys::napi_get_undefined(env, &mut undefined))?;
                        undefined
                    }
                };
            }
            _ => chk(sys::napi_get_undefined(env, &mut out))?,
        }
        Ok(out)
    }
}

/// Build a host `Map` or `Set` from a VM collection's entries.
///
/// The host's own constructor is used, so the result is a real `Map` rather
/// than an object shaped like one — which matters for `instanceof` and for the
/// methods the host will call on it.
fn make_collection(
    env: sys::napi_env,
    kind: &str,
    entries: &[(Value, Value)],
    depth: usize,
    active: &mut HashMap<usize, sys::napi_value>,
) -> Result<sys::napi_value, VmErr> {
    // The weak collections are deliberately not bridged: their contents are
    // not enumerable in the host either, so there is nothing to hand over.
    let (constructor_name, keyed) = match kind {
        "Map" => ("Map", true),
        "Set" => ("Set", false),
        _ => return Err(VmErr::Msg(format!("cannot marshal a {}", kind))),
    };
    unsafe {
        let mut global = ptr::null_mut();
        chk(sys::napi_get_global(env, &mut global))?;
        let key = CString::new(constructor_name).expect("literal has no NUL");
        let mut constructor = ptr::null_mut();
        chk(sys::napi_get_named_property(
            env,
            global,
            key.as_ptr(),
            &mut constructor,
        ))?;

        let mut seed = ptr::null_mut();
        chk(sys::napi_create_array_with_length(
            env,
            entries.len(),
            &mut seed,
        ))?;
        for (index, (entry_key, entry_value)) in entries.iter().enumerate() {
            let element = if keyed {
                let mut pair = ptr::null_mut();
                chk(sys::napi_create_array_with_length(env, 2, &mut pair))?;
                let k = to_napi_d(env, entry_key, depth + 1, active)?;
                let v = to_napi_d(env, entry_value, depth + 1, active)?;
                chk(sys::napi_set_element(env, pair, 0, k))?;
                chk(sys::napi_set_element(env, pair, 1, v))?;
                pair
            } else {
                to_napi_d(env, entry_key, depth + 1, active)?
            };
            chk(sys::napi_set_element(env, seed, index as u32, element))?;
        }

        let mut out = ptr::null_mut();
        chk(sys::napi_new_instance(env, constructor, 1, &seed, &mut out))?;
        Ok(out)
    }
}

/// Copy bytes into a fresh N-API `ArrayBuffer`.
fn make_array_buffer(env: sys::napi_env, bytes: &[u8]) -> Result<sys::napi_value, VmErr> {
    unsafe {
        let mut data = ptr::null_mut();
        let mut buffer = ptr::null_mut();
        chk(sys::napi_create_arraybuffer(
            env,
            bytes.len(),
            &mut data,
            &mut buffer,
        ))?;
        if !bytes.is_empty() {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data as *mut u8, bytes.len());
        }
        Ok(buffer)
    }
}

/// Build the matching N-API typed array (or `DataView`) over a copy of the
/// view's bytes. The copy is deliberate: the VM's buffer is `Rc`-backed and
/// must not be handed to a garbage collector that could outlive it.
fn make_typed_array(
    env: sys::napi_env,
    value: &Value,
    view: &std::rc::Rc<crate::value::TypedArrayData>,
) -> Result<sys::napi_value, VmErr> {
    let element_bytes = view.kind.size();
    let byte_offset = view.effective_byte_offset();
    let byte_length = if matches!(value, Value::DataView(_)) {
        view.effective_length()
    } else {
        view.effective_length() * element_bytes
    };
    if view.buffer.is_shared() {
        return Err(VmErr::Msg(
            "a view over SharedArrayBuffer cannot cross the napi-vm Node-API binding without shared-memory support".into(),
        ));
    }
    let source = view.buffer.snapshot();
    let from = byte_offset.min(source.len());
    let to = (from + byte_length).min(source.len());
    let buffer = make_array_buffer(env, &source[from..to])?;

    unsafe {
        let mut out = ptr::null_mut();
        if matches!(value, Value::DataView(_)) {
            chk(sys::napi_create_dataview(
                env,
                to - from,
                buffer,
                0,
                &mut out,
            ))?;
            return Ok(out);
        }
        let kind = match view.kind {
            crate::value::TypedKind::Int8 => sys::TypedarrayType::int8_array,
            crate::value::TypedKind::Uint8 => sys::TypedarrayType::uint8_array,
            crate::value::TypedKind::Uint8Clamped => sys::TypedarrayType::uint8_clamped_array,
            crate::value::TypedKind::Int16 => sys::TypedarrayType::int16_array,
            crate::value::TypedKind::Uint16 => sys::TypedarrayType::uint16_array,
            crate::value::TypedKind::Int32 => sys::TypedarrayType::int32_array,
            crate::value::TypedKind::Uint32 => sys::TypedarrayType::uint32_array,
            crate::value::TypedKind::Float32 => sys::TypedarrayType::float32_array,
            crate::value::TypedKind::Float64 => sys::TypedarrayType::float64_array,
            crate::value::TypedKind::BigInt64 => sys::TypedarrayType::bigint64_array,
            crate::value::TypedKind::BigUint64 => sys::TypedarrayType::biguint64_array,
        };
        chk(sys::napi_create_typedarray(
            env,
            kind,
            (to - from) / element_bytes,
            buffer,
            0,
            &mut out,
        ))?;
        Ok(out)
    }
}

/// Bridge a VM promise to a host one.
///
/// The event loop has already drained by the time a value crosses out, so a
/// promise is normally settled and the host one is created settled to match. A
/// promise still pending has nothing left that could settle it, and crosses as
/// a rejection saying so rather than as a promise the host would await forever.
fn make_promise(
    env: sys::napi_env,
    promise: &std::rc::Rc<std::cell::RefCell<crate::value::PromiseInner>>,
    depth: usize,
    active: &mut HashMap<usize, sys::napi_value>,
) -> Result<sys::napi_value, VmErr> {
    use crate::value::PromiseState;
    let (state, inner_value) = {
        let inner = promise.borrow();
        (inner.state, inner.value.clone())
    };
    unsafe {
        let mut deferred = ptr::null_mut();
        let mut out = ptr::null_mut();
        chk(sys::napi_create_promise(env, &mut deferred, &mut out))?;
        match state {
            PromiseState::Fulfilled => {
                let settled = to_napi_d(env, &inner_value, depth + 1, active)?;
                chk(sys::napi_resolve_deferred(env, deferred, settled))?;
            }
            PromiseState::Rejected => {
                let reason = to_napi_d(env, &inner_value, depth + 1, active)?;
                chk(sys::napi_reject_deferred(env, deferred, reason))?;
            }
            PromiseState::Pending => {
                let reason = make_str(env, "VM promise never settled")?;
                chk(sys::napi_reject_deferred(env, deferred, reason))?;
            }
        }
        Ok(out)
    }
}

/// Read a raw N-API string into a Rust `String`.
fn read_string(env: sys::napi_env, raw: sys::napi_value) -> Result<String, VmErr> {
    unsafe {
        let mut len: usize = 0;
        chk(sys::napi_get_value_string_utf8(
            env,
            raw,
            ptr::null_mut(),
            0,
            &mut len,
        ))?;
        if len > MAX_STRING_LEN {
            return Err(VmErr::Msg(
                "RangeError: Maximum string length exceeded".to_string(),
            ));
        }
        let mut buf: Vec<u8> = vec![0; len + 1];
        let mut copied: usize = 0;
        chk(sys::napi_get_value_string_utf8(
            env,
            raw,
            buf.as_mut_ptr() as *mut c_char,
            buf.len(),
            &mut copied,
        ))?;
        Ok(String::from_utf8_lossy(&buf[..copied]).into_owned())
    }
}

/// Read a named property as a string, returning `""` when the property is
/// absent or not a string. Uses `napi_get_named_property`, which reads
/// non-enumerable own properties too (e.g. an `Error`'s `message`).
pub(super) fn get_named_str(
    env: sys::napi_env,
    obj: sys::napi_value,
    key: &str,
) -> Result<String, VmErr> {
    unsafe {
        let ck =
            CString::new(key).map_err(|_| VmErr::Msg("object key contains NUL".to_string()))?;
        let mut pv = ptr::null_mut();
        chk(sys::napi_get_named_property(env, obj, ck.as_ptr(), &mut pv))?;
        let mut t: sys::napi_valuetype = 0;
        chk(sys::napi_typeof(env, pv, &mut t))?;
        if t == sys::ValueType::napi_string {
            read_string(env, pv)
        } else {
            Ok(String::new())
        }
    }
}

/// Copy the bytes out of a host `ArrayBuffer`.
fn read_array_buffer(
    env: sys::napi_env,
    raw: sys::napi_value,
) -> Result<crate::value::Buffer, VmErr> {
    unsafe {
        let mut data = ptr::null_mut();
        let mut length: usize = 0;
        chk(sys::napi_get_arraybuffer_info(
            env,
            raw,
            &mut data,
            &mut length,
        ))?;
        if length > MAX_ARRAY_LEN * 8 {
            return Err(VmErr::Msg(
                "RangeError: Maximum array length exceeded".to_string(),
            ));
        }
        let bytes = std::slice::from_raw_parts(data as *const u8, length).to_vec();
        Ok(Buffer::owned(bytes))
    }
}

/// Copy a host typed array into a VM one of the matching element type.
fn read_typed_array(env: sys::napi_env, raw: sys::napi_value) -> Result<Value, VmErr> {
    use crate::value::TypedKind;
    unsafe {
        let mut kind: sys::napi_typedarray_type = 0;
        let mut length: usize = 0;
        let mut data = ptr::null_mut();
        let mut buffer = ptr::null_mut();
        let mut byte_offset: usize = 0;
        chk(sys::napi_get_typedarray_info(
            env,
            raw,
            &mut kind,
            &mut length,
            &mut data,
            &mut buffer,
            &mut byte_offset,
        ))?;
        let kind = match kind {
            sys::TypedarrayType::int8_array => TypedKind::Int8,
            sys::TypedarrayType::uint8_array => TypedKind::Uint8,
            sys::TypedarrayType::uint8_clamped_array => TypedKind::Uint8Clamped,
            sys::TypedarrayType::int16_array => TypedKind::Int16,
            sys::TypedarrayType::uint16_array => TypedKind::Uint16,
            sys::TypedarrayType::int32_array => TypedKind::Int32,
            sys::TypedarrayType::uint32_array => TypedKind::Uint32,
            sys::TypedarrayType::float32_array => TypedKind::Float32,
            sys::TypedarrayType::float64_array => TypedKind::Float64,
            sys::TypedarrayType::bigint64_array => TypedKind::BigInt64,
            _ => TypedKind::BigUint64,
        };
        if length > MAX_ARRAY_LEN {
            return Err(VmErr::Msg(
                "RangeError: Maximum array length exceeded".to_string(),
            ));
        }
        // The view's own bytes are copied, so the VM's buffer starts at zero
        // rather than carrying the host's offset.
        let byte_length = length * kind.size();
        let bytes = std::slice::from_raw_parts(data as *const u8, byte_length).to_vec();
        Ok(Value::TypedArray(std::rc::Rc::new(
            crate::value::TypedArrayData {
                kind,
                buffer: Buffer::owned(bytes).into(),
                byte_offset: 0,
                length,
                is_buffer: false,
            },
        )))
    }
}

pub(super) fn from_napi(env: sys::napi_env, raw: sys::napi_value) -> Result<Value, VmErr> {
    from_napi_d(env, raw, 0, &mut HashSet::new())
}

/// Marshal a raw N-API value into a VM `Value`.
///
/// JavaScript functions are not marshalled into callable VM values here; use
/// `Vm.exposeFunction` to make a Node function callable from the VM.
fn from_napi_d(
    env: sys::napi_env,
    raw: sys::napi_value,
    depth: usize,
    active: &mut HashSet<usize>,
) -> Result<Value, VmErr> {
    if depth > MAX_MARSHAL_DEPTH {
        return Err(VmErr::Msg("value is too deep to marshal".to_string()));
    }
    unsafe {
        if raw.is_null() {
            return Ok(Value::Undefined);
        }
        let mut t: sys::napi_valuetype = 0;
        chk(sys::napi_typeof(env, raw, &mut t))?;
        Ok(match t {
            sys::ValueType::napi_undefined => Value::Undefined,
            sys::ValueType::napi_null => Value::Null,
            sys::ValueType::napi_boolean => {
                let mut b = false;
                chk(sys::napi_get_value_bool(env, raw, &mut b))?;
                Value::Bool(b)
            }
            sys::ValueType::napi_number => {
                let mut n = 0.0;
                chk(sys::napi_get_value_double(env, raw, &mut n))?;
                Value::Number(n)
            }
            sys::ValueType::napi_string => {
                let value = read_string(env, raw)?;
                if value.len() > MAX_STRING_LEN {
                    return Err(VmErr::Msg(
                        "RangeError: Maximum string length exceeded".to_string(),
                    ));
                }
                Value::String(value)
            }
            sys::ValueType::napi_bigint => {
                // The size query must pass a null `sign_bit` *and* a null
                // `words`; N-API rejects the call otherwise.
                let mut count: usize = 0;
                chk(sys::napi_get_value_bigint_words(
                    env,
                    raw,
                    ptr::null_mut(),
                    &mut count,
                    ptr::null_mut(),
                ))?;
                let mut sign: i32 = 0;
                let mut words = vec![0u64; count];
                chk(sys::napi_get_value_bigint_words(
                    env,
                    raw,
                    &mut sign,
                    &mut count,
                    words.as_mut_ptr(),
                ))?;
                words.truncate(count);
                Value::BigInt(std::rc::Rc::new(crate::bigint::BigInt::from_words(
                    sign != 0,
                    &words,
                )))
            }
            // A host symbol becomes a *fresh* VM symbol carrying the same
            // description. Symbol identity cannot cross the boundary: the two
            // realms have separate registries.
            sys::ValueType::napi_symbol => {
                let description = get_named_str(env, raw, "description").ok();
                crate::builtins::new_symbol(description.filter(|d| !d.is_empty()))
            }
            sys::ValueType::napi_object => {
                let mut is_date = false;
                chk(sys::napi_is_date(env, raw, &mut is_date))?;
                if is_date {
                    let mut ms = 0.0;
                    chk(sys::napi_get_date_value(env, raw, &mut ms))?;
                    return Ok(Value::Date(std::rc::Rc::new(std::cell::Cell::new(ms))));
                }

                let mut is_buffer = false;
                chk(sys::napi_is_arraybuffer(env, raw, &mut is_buffer))?;
                if is_buffer {
                    return Ok(Value::ArrayBuffer(read_array_buffer(env, raw)?));
                }

                let mut is_typed = false;
                chk(sys::napi_is_typedarray(env, raw, &mut is_typed))?;
                if is_typed {
                    return read_typed_array(env, raw);
                }

                // A JS `Error` carries its `message` as a *non-enumerable*
                // property, so the generic enumerable-key walk below would drop
                // it. Surface it as a plain object with `name`/`message`, which
                // is exactly how the VM's own `Error` instances are shaped, so
                // `catch (e) { e.message }` works across the boundary.
                let mut is_error = false;
                chk(sys::napi_is_error(env, raw, &mut is_error))?;
                if is_error {
                    let name = get_named_str(env, raw, "name").unwrap_or_else(|_| "Error".into());
                    let message = get_named_str(env, raw, "message").unwrap_or_default();
                    return Value::checked_object(vec![
                        ("name".to_string(), Value::String(name)),
                        ("message".to_string(), Value::String(message)),
                    ]);
                }

                // N-API values are allowed to be cyclic. Keep an identity set
                // while walking a single object graph so a host object such as
                // `const a = {}; a.self = a` becomes a catchable boundary
                // error instead of recursing until the native stack overflows.
                let identity = raw as usize;
                if !active.insert(identity) {
                    return Err(VmErr::Msg(
                        "TypeError: Cannot marshal a cyclic host value".to_string(),
                    ));
                }

                let mut is_array = false;
                chk(sys::napi_is_array(env, raw, &mut is_array))?;
                if is_array {
                    let mut len: u32 = 0;
                    chk(sys::napi_get_array_length(env, raw, &mut len))?;
                    let mut items =
                        Vec::with_capacity(len.min(crate::value::MAX_ARRAY_LEN as u32) as usize);
                    for i in 0..len {
                        if i as usize >= crate::value::MAX_ARRAY_LEN {
                            return Err(VmErr::Msg(
                                "RangeError: Maximum array length exceeded".to_string(),
                            ));
                        }
                        let mut ev = ptr::null_mut();
                        chk(sys::napi_get_element(env, raw, i, &mut ev))?;
                        items.push(from_napi_d(env, ev, depth + 1, active)?);
                    }
                    active.remove(&identity);
                    Value::checked_array(items)?
                } else {
                    let mut names = ptr::null_mut();
                    chk(sys::napi_get_property_names(env, raw, &mut names))?;
                    let mut len: u32 = 0;
                    chk(sys::napi_get_array_length(env, names, &mut len))?;
                    if len as usize > MAX_OBJECT_PROPS {
                        active.remove(&identity);
                        return Err(VmErr::Msg(
                            "RangeError: Maximum object property count exceeded".to_string(),
                        ));
                    }
                    let mut props = Vec::with_capacity(len as usize);
                    for i in 0..len {
                        let mut key = ptr::null_mut();
                        chk(sys::napi_get_element(env, names, i, &mut key))?;
                        let key_str = read_string(env, key)?;
                        if key_str.len() > MAX_STRING_LEN {
                            active.remove(&identity);
                            return Err(VmErr::Msg(
                                "RangeError: Maximum property name length exceeded".to_string(),
                            ));
                        }
                        let mut pv = ptr::null_mut();
                        chk(sys::napi_get_property(env, raw, key, &mut pv))?;
                        props.push((key_str, from_napi_d(env, pv, depth + 1, active)?));
                    }
                    active.remove(&identity);
                    Value::checked_object(props)?
                }
            }
            // Functions and externals have no VM representation; use
            // `Vm.exposeFunction` to make a host function callable.
            _ => Value::Undefined,
        })
    }
}

/// An owned, thread-safe representation used by the N-API host bridge.
///
/// `Value` deliberately remains a single-threaded type: it contains `Rc` and
/// `RefCell` and is never sent to the worker thread. The bridge copies a value
/// into this representation while it still owns the interpreter lock, then
/// reconstructs a fresh VM value on the receiving side. This is also where
/// cycle, depth, string, array, and property-count limits are enforced.
#[derive(Debug, Clone)]
pub(super) enum WireValue {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<WireValue>),
    Object(Vec<(String, WireValue)>),
    Error { name: String, message: String },
}

impl WireValue {
    pub(super) fn from_value(value: &Value) -> Result<Self, VmErr> {
        let mut active = HashSet::new();
        Self::from_value_d(value, 0, &mut active)
    }

    fn from_value_d(
        value: &Value,
        depth: usize,
        active: &mut HashSet<usize>,
    ) -> Result<Self, VmErr> {
        if depth > MAX_MARSHAL_DEPTH {
            return Err(VmErr::Msg("value is too deep to marshal".to_string()));
        }

        match value {
            Value::Undefined => Ok(Self::Undefined),
            Value::Null => Ok(Self::Null),
            Value::Bool(value) => Ok(Self::Bool(*value)),
            Value::Number(value) => Ok(Self::Number(*value)),
            Value::String(value) => {
                if value.len() > MAX_STRING_LEN {
                    return Err(VmErr::Msg(
                        "RangeError: Maximum string length exceeded".to_string(),
                    ));
                }
                Ok(Self::String(value.clone()))
            }
            Value::Array(items) => {
                let identity = std::rc::Rc::as_ptr(items) as usize;
                if !active.insert(identity) {
                    return Err(VmErr::Msg(
                        "TypeError: Cannot marshal a cyclic VM value".to_string(),
                    ));
                }
                let items = items.borrow();
                if items.len() > MAX_ARRAY_LEN {
                    active.remove(&identity);
                    return Err(VmErr::Msg(
                        "RangeError: Maximum array length exceeded".to_string(),
                    ));
                }
                let result = items
                    .iter()
                    .map(|item| Self::from_value_d(item, depth + 1, active))
                    .collect::<Result<Vec<_>, _>>();
                active.remove(&identity);
                result.map(Self::Array)
            }
            Value::Object { props, .. } => {
                let identity = std::rc::Rc::as_ptr(props) as usize;
                if !active.insert(identity) {
                    return Err(VmErr::Msg(
                        "TypeError: Cannot marshal a cyclic VM value".to_string(),
                    ));
                }
                let props = props.borrow();
                if props.len() > MAX_OBJECT_PROPS {
                    active.remove(&identity);
                    return Err(VmErr::Msg(
                        "RangeError: Maximum object property count exceeded".to_string(),
                    ));
                }
                let result = props
                    .iter()
                    .map(|(key, value)| {
                        if key.len() > MAX_STRING_LEN {
                            return Err(VmErr::Msg(
                                "RangeError: Maximum property name length exceeded".to_string(),
                            ));
                        }
                        Ok((key.clone(), Self::from_value_d(value, depth + 1, active)?))
                    })
                    .collect::<Result<Vec<_>, _>>();
                active.remove(&identity);
                result.map(Self::Object)
            }
            Value::Error(error) => Ok(Self::Error {
                name: error.name.clone(),
                message: error.message.clone(),
            }),
            // VM-only callable/stateful values do not have a safe wire form.
            // Preserve the existing boundary behavior by exposing them as
            // `undefined` rather than moving their Rc-backed internals.
            _ => Ok(Self::Undefined),
        }
    }

    pub(super) fn into_value(self) -> Value {
        match self {
            Self::Undefined => Value::Undefined,
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(value),
            Self::Number(value) => Value::Number(value),
            Self::String(value) => Value::String(value),
            Self::Array(values) => Value::array(values.into_iter().map(Self::into_value).collect()),
            Self::Object(values) => Value::object(
                values
                    .into_iter()
                    .map(|(key, value)| (key, value.into_value()))
                    .collect(),
            ),
            Self::Error { name, message } => {
                Value::Error(crate::value::ErrorData::new(&name, message))
            }
        }
    }
}
