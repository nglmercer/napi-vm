//! `ArrayBuffer`, the typed-array views, and `DataView`.
//!
//! A buffer is a byte vector; a view is a window onto one, with an element
//! type, a byte offset and a length. Two views over the same buffer see each
//! other's writes, which is the whole point of the type — so the buffer is
//! shared (`Rc<RefCell<Vec<u8>>>`) and the view holds only the window.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{Buffer, TypedArrayData, TypedKind, Value};

/// Every typed-array constructor, in the order the specification lists them.
const KINDS: &[(&str, TypedKind)] = &[
    ("Int8Array", TypedKind::Int8),
    ("Uint8Array", TypedKind::Uint8),
    ("Uint8ClampedArray", TypedKind::Uint8Clamped),
    ("Int16Array", TypedKind::Int16),
    ("Uint16Array", TypedKind::Uint16),
    ("Int32Array", TypedKind::Int32),
    ("Uint32Array", TypedKind::Uint32),
    ("Float32Array", TypedKind::Float32),
    ("Float64Array", TypedKind::Float64),
    ("BigInt64Array", TypedKind::BigInt64),
    ("BigUint64Array", TypedKind::BigUint64),
];

pub(super) fn install(e: &mut Environment) {
    if let Some(namespace) = e.get("ArrayBuffer") {
        namespace
            .set_prop(
                "isView".to_string(),
                super::nf("isView", array_buffer_is_view),
            )
            .expect("built-in ArrayBuffer property");
        super::make_callable(&namespace, new_array_buffer, None);
    }
    if let Some(namespace) = e.get("DataView") {
        super::make_callable(&namespace, new_data_view, None);
    }
    for (name, kind) in KINDS {
        // The constructors are not in the pre-seeded global list, so declare
        // them here with their element size as a static.
        let namespace = Value::object(vec![(
            "BYTES_PER_ELEMENT".to_string(),
            Value::Number(kind.size() as f64),
        )]);
        namespace
            .set_prop("of".to_string(), super::nf("of", typed_of))
            .expect("built-in typed-array property");
        namespace
            .set_prop("from".to_string(), super::nf("from", typed_from))
            .expect("built-in typed-array property");
        namespace
            .set_prop(KIND_SLOT.to_string(), Value::String(name.to_string()))
            .expect("built-in typed-array property");
        super::make_callable(&namespace, new_typed_array, None);
        e.set(name, namespace);
    }
}

/// Slot naming which typed-array constructor a namespace object is, so one
/// native function can serve all eleven.
const KIND_SLOT: &str = "__symbol_typed_kind__";

fn kind_of(this: &Value) -> TypedKind {
    match &this.get_prop(KIND_SLOT) {
        Some(Value::String(name)) => KINDS
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, kind)| *kind)
            .unwrap_or(TypedKind::Uint8),
        _ => TypedKind::Uint8,
    }
}

fn range_err(message: &str) -> VmErr {
    VmErr::Msg(format!("RangeError: {}", message))
}

fn new_buffer(byte_length: usize) -> Result<Buffer, VmErr> {
    if byte_length > crate::value::MAX_ARRAY_LEN * 8 {
        return Err(range_err("Invalid array buffer length"));
    }
    Ok(Buffer::zeroed(byte_length))
}

fn new_array_buffer(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let length = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    if !length.is_finite() || length < 0.0 {
        return Err(range_err("Invalid array buffer length"));
    }
    Ok(Value::ArrayBuffer(new_buffer(length as usize)?))
}

fn array_buffer_is_view(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(matches!(
        a.first(),
        Some(Value::TypedArray(_)) | Some(Value::DataView(_))
    )))
}

/// `new Int32Array(…)`: a length, a buffer (with an optional window), or
/// anything iterable.
fn new_typed_array(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let kind = kind_of(&this);
    let size = kind.size();
    match a.first() {
        None | Some(Value::Undefined) => Ok(typed(kind, new_buffer(0)?, 0, 0)),
        Some(Value::Number(n)) => {
            if !n.is_finite() || *n < 0.0 || n.fract() != 0.0 {
                return Err(range_err("Invalid typed array length"));
            }
            let length = *n as usize;
            Ok(typed(kind, new_buffer(length * size)?, 0, length))
        }
        Some(Value::ArrayBuffer(buffer)) => {
            if buffer.is_detached() {
                return Err(VmErr::Msg(
                    "TypeError: Cannot construct a typed array from a detached ArrayBuffer"
                        .to_string(),
                ));
            }
            let byte_offset = a.get(1).map(|v| v.to_number()).unwrap_or(0.0);
            if !byte_offset.is_finite() || byte_offset < 0.0 {
                return Err(range_err("Invalid typed array offset"));
            }
            let byte_offset = byte_offset as usize;
            let available = buffer.borrow().len();
            if byte_offset > available || !byte_offset.is_multiple_of(size) {
                return Err(range_err("Start offset is outside the buffer"));
            }
            let length = match a.get(2) {
                Some(Value::Undefined) | None => (available - byte_offset) / size,
                Some(v) => v.to_number().max(0.0) as usize,
            };
            if byte_offset + length * size > available {
                return Err(range_err("Invalid typed array length"));
            }
            Ok(typed(kind, buffer.clone(), byte_offset, length))
        }
        // A typed array or any iterable copies element-wise.
        Some(source) => {
            let items = match source {
                Value::TypedArray(view) => read_all(view),
                other => interp.iterate(other)?,
            };
            let view = typed(kind, new_buffer(items.len() * size)?, 0, items.len());
            let Value::TypedArray(data) = &view else {
                unreachable!("typed() returns a typed array");
            };
            for (index, item) in items.iter().enumerate() {
                write_element(data, index, item)?;
            }
            Ok(view)
        }
    }
}

fn typed(kind: TypedKind, buffer: Buffer, byte_offset: usize, length: usize) -> Value {
    Value::TypedArray(Rc::new(TypedArrayData {
        kind,
        buffer,
        byte_offset,
        length,
    }))
}

fn typed_of(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    new_typed_array(interp, this, vec![Value::array(a)])
}

fn typed_from(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let source = a.first().cloned().unwrap_or(Value::Undefined);
    let items = match &source {
        Value::TypedArray(view) => read_all(view),
        Value::Object { .. } => {
            // An array-like: read `length` and the index properties.
            let length = interp.member(&source, "length")?.to_number();
            let length = if length.is_finite() && length > 0.0 {
                (length as usize).min(crate::value::MAX_ARRAY_LEN)
            } else {
                0
            };
            let mut out = Vec::with_capacity(length.min(1024));
            for index in 0..length {
                out.push(interp.member(&source, &index.to_string())?);
            }
            out
        }
        other => interp.iterate(other)?,
    };
    let mapped = match a.get(1) {
        Some(mapper) if !matches!(mapper, Value::Undefined | Value::Null) => {
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.into_iter().enumerate() {
                out.push(interp.call_this(
                    mapper,
                    Value::Undefined,
                    vec![item, Value::Number(index as f64)],
                )?);
            }
            out
        }
        _ => items,
    };
    new_typed_array(interp, this, vec![Value::array(mapped)])
}

// --- Element access ---------------------------------------------------------

/// Read element `index`, or `undefined` when it is out of range.
pub fn read_element(view: &Rc<TypedArrayData>, index: usize) -> Option<Value> {
    if index >= view.effective_length() {
        return None;
    }
    let buffer = view.buffer.borrow();
    let at = view.effective_byte_offset() + index * view.kind.size();
    let bytes = buffer.get(at..at + view.kind.size())?;
    Some(match view.kind {
        TypedKind::Int8 => Value::Number(bytes[0] as i8 as f64),
        TypedKind::Uint8 | TypedKind::Uint8Clamped => Value::Number(bytes[0] as f64),
        TypedKind::Int16 => Value::Number(i16::from_le_bytes([bytes[0], bytes[1]]) as f64),
        TypedKind::Uint16 => Value::Number(u16::from_le_bytes([bytes[0], bytes[1]]) as f64),
        TypedKind::Int32 => Value::Number(i32::from_le_bytes(bytes.try_into().ok()?) as f64),
        TypedKind::Uint32 => Value::Number(u32::from_le_bytes(bytes.try_into().ok()?) as f64),
        TypedKind::Float32 => Value::Number(f32::from_le_bytes(bytes.try_into().ok()?) as f64),
        TypedKind::Float64 => Value::Number(f64::from_le_bytes(bytes.try_into().ok()?)),
        TypedKind::BigInt64 => Value::BigInt(Rc::new(crate::bigint::BigInt::from_i64(
            i64::from_le_bytes(bytes.try_into().ok()?),
        ))),
        TypedKind::BigUint64 => {
            let value = u64::from_le_bytes(bytes.try_into().ok()?);
            Value::BigInt(Rc::new(
                crate::bigint::BigInt::parse(&value.to_string()).ok()?,
            ))
        }
    })
}

/// Write element `index`, converting and wrapping as the element type
/// requires. Out-of-range indices are ignored, as they are on a typed array.
pub fn write_element(view: &Rc<TypedArrayData>, index: usize, value: &Value) -> Result<(), VmErr> {
    if index >= view.effective_length() {
        return Ok(());
    }
    let size = view.kind.size();
    let at = view.effective_byte_offset() + index * size;
    let bytes: Vec<u8> = match view.kind {
        TypedKind::BigInt64 | TypedKind::BigUint64 => {
            let Some(big) = value.as_bigint() else {
                return Err(VmErr::Msg(
                    "TypeError: Cannot convert a non-BigInt value to a BigInt element".to_string(),
                ));
            };
            let wrapped = big.as_n_bit(64, false).map_err(VmErr::Msg)?;
            // `as_n_bit(64, false)` yields a non-negative magnitude that fits
            // 64 bits, so the decimal parse is exact.
            let unsigned: u64 = wrapped.to_decimal().parse().unwrap_or(0);
            unsigned.to_le_bytes().to_vec()
        }
        kind => {
            let number = value.to_number();
            match kind {
                TypedKind::Int8 => (to_int(number) as i8).to_le_bytes().to_vec(),
                TypedKind::Uint8 => (to_int(number) as u8).to_le_bytes().to_vec(),
                // The clamped view saturates instead of wrapping, and rounds
                // to nearest rather than truncating.
                TypedKind::Uint8Clamped => {
                    let clamped = if number.is_nan() {
                        0.0
                    } else {
                        number.round_ties_even().clamp(0.0, 255.0)
                    };
                    vec![clamped as u8]
                }
                TypedKind::Int16 => (to_int(number) as i16).to_le_bytes().to_vec(),
                TypedKind::Uint16 => (to_int(number) as u16).to_le_bytes().to_vec(),
                TypedKind::Int32 => to_int(number).to_le_bytes().to_vec(),
                TypedKind::Uint32 => (to_int(number) as u32).to_le_bytes().to_vec(),
                TypedKind::Float32 => (number as f32).to_le_bytes().to_vec(),
                _ => number.to_le_bytes().to_vec(),
            }
        }
    };
    let mut buffer = view.buffer.borrow_mut();
    if at + size <= buffer.len() {
        buffer[at..at + size].copy_from_slice(&bytes);
    }
    Ok(())
}

/// `ToInt32`: truncate towards zero and wrap modulo 2³², which is how the
/// integer views convert a `Number`.
fn to_int(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let truncated = value.trunc();
    let wrapped = truncated.rem_euclid(4_294_967_296.0);
    if wrapped >= 2_147_483_648.0 {
        (wrapped - 4_294_967_296.0) as i32
    } else {
        wrapped as i32
    }
}

fn read_all(view: &Rc<TypedArrayData>) -> Vec<Value> {
    (0..view.effective_length())
        .map(|index| read_element(view, index).unwrap_or(Value::Undefined))
        .collect()
}

// --- Instance members -------------------------------------------------------

/// Properties and methods on a typed array.
pub fn typed_member(view: &Rc<TypedArrayData>, key: &str) -> Option<Value> {
    let length = view.effective_length();
    Some(match key {
        "length" => Value::Number(length as f64),
        "byteLength" => Value::Number((length * view.kind.size()) as f64),
        "byteOffset" => Value::Number(view.effective_byte_offset() as f64),
        "BYTES_PER_ELEMENT" => Value::Number(view.kind.size() as f64),
        "buffer" => Value::ArrayBuffer(view.buffer.clone()),
        "set" => super::nf("set", typed_set),
        "subarray" => super::nf("subarray", typed_subarray),
        "slice" => super::nf("slice", typed_slice),
        "fill" => super::nf("fill", typed_fill),
        "at" => super::nf("at", typed_at),
        "toString" | "join" => super::nf("join", typed_join),
        // The iteration methods work on the elements, so they go through an
        // ordinary array rather than being reimplemented eleven times.
        "map" | "filter" | "forEach" | "reduce" | "some" | "every" | "find" | "findIndex"
        | "indexOf" | "includes" | "reverse" | "sort" | "keys" | "values" | "entries" => {
            super::nf(key, typed_delegate)
        }
        crate::interpreter::SYMBOL_ITERATOR_SLOT => super::nf("[Symbol.iterator]", typed_iterator),
        _ => return None,
    })
}

fn require(this: &Value) -> Result<Rc<TypedArrayData>, VmErr> {
    match this {
        Value::TypedArray(view) => Ok(view.clone()),
        _ => Err(VmErr::Msg("TypeError: not a typed array".to_string())),
    }
}

/// Run an `Array.prototype` method over a copy of the elements.
///
/// The methods that produce a new collection return a plain array here rather
/// than a typed one. That differs from the specification for `map`, `filter`
/// and `slice`; it is the honest report of what this implementation does.
fn typed_delegate(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = require(&this)?;
    let name = current_method(interp)?;
    let array = Value::array(read_all(&view));
    let method = interp.member(&array, &name)?;
    interp.call_this(&method, array, a)
}

thread_local! {
    /// The method name the current `typed_delegate` call stands for. A native
    /// function is a bare pointer, so the name it was reached under is not
    /// otherwise recoverable inside the call.
    static ACTIVE_METHOD: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Record which method a delegating typed-array member was resolved as.
pub fn note_method(name: &str) {
    ACTIVE_METHOD.with(|active| *active.borrow_mut() = name.to_string());
}

fn current_method(_: &Interpreter) -> Result<String, VmErr> {
    Ok(ACTIVE_METHOD.with(|active| active.borrow().clone()))
}

fn typed_at(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = require(&this)?;
    let length = view.effective_length();
    let index = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    let index = if index < 0.0 {
        length as f64 + index
    } else {
        index
    };
    if index < 0.0 || index >= length as f64 {
        return Ok(Value::Undefined);
    }
    Ok(read_element(&view, index as usize).unwrap_or(Value::Undefined))
}

fn typed_join(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = require(&this)?;
    let separator = match a.first() {
        Some(Value::Undefined) | None => ",".to_string(),
        Some(v) => interp.vs(v)?,
    };
    let parts = read_all(&view)
        .iter()
        .map(|v| interp.vs(v))
        .collect::<Result<Vec<_>, _>>()?;
    Value::checked_string(parts.join(&separator))
}

/// `set(source, offset)`: copy elements in, converting as needed.
fn typed_set(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = require(&this)?;
    let offset = a.get(1).map(|v| v.to_number()).unwrap_or(0.0);
    if !offset.is_finite() || offset < 0.0 {
        return Err(range_err("Invalid offset"));
    }
    let offset = offset as usize;
    let items = match a.first() {
        Some(Value::TypedArray(source)) => read_all(source),
        Some(Value::Array(items)) => items.borrow().clone(),
        Some(other) => interp.iterate(other)?,
        None => Vec::new(),
    };
    if offset + items.len() > view.effective_length() {
        return Err(range_err("Source is too large"));
    }
    for (index, item) in items.iter().enumerate() {
        write_element(&view, offset + index, item)?;
    }
    Ok(Value::Undefined)
}

/// Resolve a `[start, end)` window over `length` elements, applying the
/// negative-index and clamping rules the array methods share.
fn window(length: usize, a: &[Value]) -> (usize, usize) {
    let resolve = |value: Option<&Value>, default: usize| -> usize {
        match value {
            Some(Value::Undefined) | None => default,
            Some(v) => {
                let n = v.to_number();
                if !n.is_finite() {
                    return if n > 0.0 { length } else { 0 };
                }
                if n < 0.0 {
                    ((length as f64 + n).max(0.0)) as usize
                } else {
                    (n as usize).min(length)
                }
            }
        }
    };
    let start = resolve(a.first(), 0);
    let end = resolve(a.get(1), length).max(start);
    (start, end)
}

/// `subarray`: a *view* over the same buffer, so writes are shared.
fn typed_subarray(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = require(&this)?;
    let (start, end) = window(view.effective_length(), &a);
    Ok(typed(
        view.kind,
        view.buffer.clone(),
        view.effective_byte_offset() + start * view.kind.size(),
        end - start,
    ))
}

/// `slice`: a *copy*, so writes are not shared.
fn typed_slice(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = require(&this)?;
    let (start, end) = window(view.effective_length(), &a);
    let size = view.kind.size();
    let copy = new_buffer((end - start) * size)?;
    {
        let source = view.buffer.borrow();
        let from = view.effective_byte_offset() + start * size;
        let to = view.effective_byte_offset() + end * size;
        if to <= source.len() {
            copy.borrow_mut().copy_from_slice(&source[from..to]);
        }
    }
    Ok(typed(view.kind, copy, 0, end - start))
}

fn typed_fill(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = require(&this)?;
    let value = a.first().cloned().unwrap_or(Value::Undefined);
    let (start, end) = window(view.effective_length(), &a[1.min(a.len())..]);
    for index in start..end {
        write_element(&view, index, &value)?;
    }
    Ok(this)
}

fn typed_iterator(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let view = require(&this)?;
    let array = Value::array(read_all(&view));
    let iterator = interp.prop(
        &array,
        &Value::String(crate::interpreter::SYMBOL_ITERATOR_SLOT.to_string()),
    )?;
    interp.call_this(&iterator, array, vec![])
}

// --- ArrayBuffer members ----------------------------------------------------

pub fn array_buffer_member(buffer: &Buffer, key: &str) -> Option<Value> {
    Some(match key {
        "byteLength" => Value::Number(buffer.borrow().len() as f64),
        "slice" => super::nf("slice", array_buffer_slice),
        _ => return None,
    })
}

fn array_buffer_slice(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let Value::ArrayBuffer(buffer) = &this else {
        return Err(VmErr::Msg("TypeError: not an ArrayBuffer".to_string()));
    };
    if buffer.is_detached() {
        return Err(VmErr::Msg(
            "TypeError: Cannot slice a detached ArrayBuffer".to_string(),
        ));
    }
    let length = buffer.borrow().len();
    let (start, end) = window(length, &a);
    let copy = new_buffer(end - start)?;
    copy.borrow_mut()
        .copy_from_slice(&buffer.borrow()[start..end]);
    Ok(Value::ArrayBuffer(copy))
}

// --- DataView ---------------------------------------------------------------

fn new_data_view(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let Some(Value::ArrayBuffer(buffer)) = a.first() else {
        return Err(VmErr::Msg(
            "TypeError: First argument to DataView constructor must be an ArrayBuffer".to_string(),
        ));
    };
    if buffer.is_detached() {
        return Err(VmErr::Msg(
            "TypeError: Cannot construct a DataView from a detached ArrayBuffer".to_string(),
        ));
    }
    let available = buffer.borrow().len();
    let byte_offset = a.get(1).map(|v| v.to_number()).unwrap_or(0.0).max(0.0) as usize;
    if byte_offset > available {
        return Err(range_err("Start offset is outside the buffer"));
    }
    let byte_length = match a.get(2) {
        Some(Value::Undefined) | None => available - byte_offset,
        Some(v) => v.to_number().max(0.0) as usize,
    };
    if byte_offset + byte_length > available {
        return Err(range_err("Invalid DataView length"));
    }
    Ok(Value::DataView(Rc::new(TypedArrayData {
        kind: TypedKind::Uint8,
        buffer: buffer.clone(),
        byte_offset,
        length: byte_length,
    })))
}

/// `DataView` accessors. Each `get`/`set` pair names its element type, and
/// takes a `littleEndian` flag defaulting to big-endian — the opposite of the
/// typed arrays, which is what the specification says.
pub fn data_view_member(view: &Rc<TypedArrayData>, key: &str) -> Option<Value> {
    Some(match key {
        "byteLength" => Value::Number(view.effective_length() as f64),
        "byteOffset" => Value::Number(view.effective_byte_offset() as f64),
        "buffer" => Value::ArrayBuffer(view.buffer.clone()),
        _ if key.starts_with("get") && element_kind(&key[3..]).is_some() => {
            super::nf(key, data_view_get)
        }
        _ if key.starts_with("set") && element_kind(&key[3..]).is_some() => {
            super::nf(key, data_view_set)
        }
        _ => return None,
    })
}

fn element_kind(name: &str) -> Option<TypedKind> {
    Some(match name {
        "Int8" => TypedKind::Int8,
        "Uint8" => TypedKind::Uint8,
        "Int16" => TypedKind::Int16,
        "Uint16" => TypedKind::Uint16,
        "Int32" => TypedKind::Int32,
        "Uint32" => TypedKind::Uint32,
        "Float32" => TypedKind::Float32,
        "Float64" => TypedKind::Float64,
        "BigInt64" => TypedKind::BigInt64,
        "BigUint64" => TypedKind::BigUint64,
        _ => return None,
    })
}

/// A `DataView` element access, resolved as a one-element typed array over the
/// requested byte offset so the conversion code is shared.
fn data_view_slot(this: &Value, a: &[Value], kind: TypedKind) -> Result<Rc<TypedArrayData>, VmErr> {
    let Value::DataView(view) = this else {
        return Err(VmErr::Msg("TypeError: not a DataView".to_string()));
    };
    if view.buffer.is_detached() {
        return Err(VmErr::Msg(
            "TypeError: Cannot access a DataView backed by a detached ArrayBuffer".to_string(),
        ));
    }
    let offset = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    if !offset.is_finite() || offset < 0.0 {
        return Err(range_err("Offset is outside the DataView"));
    }
    let offset = offset as usize;
    if offset + kind.size() > view.effective_length() {
        return Err(range_err("Offset is outside the DataView"));
    }
    Ok(Rc::new(TypedArrayData {
        kind,
        buffer: view.buffer.clone(),
        byte_offset: view.effective_byte_offset() + offset,
        length: 1,
    }))
}

fn swap_if_big_endian(slot: &Rc<TypedArrayData>, little_endian: bool) {
    if little_endian || slot.kind.size() == 1 {
        return;
    }
    let mut buffer = slot.buffer.borrow_mut();
    let at = slot.byte_offset;
    let size = slot.kind.size();
    if at + size <= buffer.len() {
        buffer[at..at + size].reverse();
    }
}

fn data_view_get(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let kind = element_kind(&current_method(interp)?[3..])
        .ok_or_else(|| VmErr::Msg("TypeError: unknown DataView accessor".to_string()))?;
    let slot = data_view_slot(&this, &a, kind)?;
    let little_endian = a.get(1).map(|v| v.is_truthy()).unwrap_or(false);
    swap_if_big_endian(&slot, little_endian);
    let value = read_element(&slot, 0).unwrap_or(Value::Undefined);
    // Restore the bytes: the read is not supposed to mutate the buffer.
    swap_if_big_endian(&slot, little_endian);
    Ok(value)
}

fn data_view_set(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let kind = element_kind(&current_method(interp)?[3..])
        .ok_or_else(|| VmErr::Msg("TypeError: unknown DataView accessor".to_string()))?;
    let slot = data_view_slot(&this, &a, kind)?;
    let value = a.get(1).cloned().unwrap_or(Value::Undefined);
    write_element(&slot, 0, &value)?;
    let little_endian = a.get(2).map(|v| v.is_truthy()).unwrap_or(false);
    swap_if_big_endian(&slot, little_endian);
    Ok(Value::Undefined)
}
