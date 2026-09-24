//! `ArrayBuffer`, the typed-array views, and `DataView`.
//!
//! A buffer is a byte vector; a view is a window onto one, with an element
//! type, a byte offset and a length. Two views over the same buffer see each
//! other's writes, which is the whole point of the type — so the buffer is
//! shared (`Rc<RefCell<Vec<u8>>>`) and the view holds only the window.

use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{
    Buffer, BufferBacking, PromiseState, SharedAtomicOp, SharedBuffer, TypedArrayData, TypedKind,
    Value,
};

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
    let object_prototype = e
        .get("Object")
        .and_then(|object| object.get_prop("prototype"))
        .map(Rc::new);

    if let Some(namespace) = e.get("ArrayBuffer") {
        namespace
            .set_prop(
                "isView".to_string(),
                super::nf("isView", array_buffer_is_view),
            )
            .expect("built-in ArrayBuffer property");
        super::make_callable(&namespace, new_array_buffer, None);
        let prototype = make_prototype(
            object_prototype.clone(),
            namespace.clone(),
            [("slice", super::nf("slice", array_buffer_slice))],
        );
        super::set_builtin_constructor_prototype(e, &namespace, prototype);
    }
    if let Some(namespace) = e.get("SharedArrayBuffer") {
        super::make_callable(&namespace, new_shared_array_buffer, None);
        let prototype = make_prototype(
            object_prototype.clone(),
            namespace.clone(),
            [("slice", super::nf("slice", shared_array_buffer_slice))],
        );
        super::set_builtin_constructor_prototype(e, &namespace, prototype);
    }
    if let Some(namespace) = e.get("Atomics") {
        for (name, method) in [
            ("isLockFree", atomics_is_lock_free as _),
            ("load", atomics_load as _),
            ("store", atomics_store as _),
            ("add", atomics_add as _),
            ("sub", atomics_sub as _),
            ("and", atomics_and as _),
            ("or", atomics_or as _),
            ("xor", atomics_xor as _),
            ("exchange", atomics_exchange as _),
            ("compareExchange", atomics_compare_exchange as _),
            ("wait", atomics_wait as _),
            ("waitAsync", atomics_wait_async as _),
            ("notify", atomics_notify as _),
        ] {
            namespace
                .set_prop(name.to_string(), super::nf(name, method))
                .expect("built-in Atomics property");
        }
    }
    if let Some(namespace) = e.get("DataView") {
        super::make_callable(&namespace, new_data_view, None);
        let prototype = data_view_prototype(object_prototype.clone(), namespace.clone());
        super::set_builtin_constructor_prototype(e, &namespace, prototype);
    }

    let typed_array_prototype = typed_array_prototype(object_prototype.clone());
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
        if let Value::Object { props } = &namespace {
            props.meta.borrow_mut().set_attrs(
                "BYTES_PER_ELEMENT",
                crate::value::PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: false,
                },
            );
        }
        super::make_callable(&namespace, new_typed_array, None);
        let prototype = Value::object_with_proto(
            vec![
                ("constructor".into(), namespace.clone()),
                (
                    "BYTES_PER_ELEMENT".into(),
                    Value::Number(kind.size() as f64),
                ),
            ],
            Some(Rc::new(typed_array_prototype.clone())),
        );
        if let Value::Object { props } = &prototype {
            for name in ["constructor", "BYTES_PER_ELEMENT"] {
                props.meta.borrow_mut().set_attrs(
                    name,
                    crate::value::PropAttrs {
                        enumerable: false,
                        ..crate::value::PropAttrs::default()
                    },
                );
            }
            props.meta.borrow_mut().set_attrs(
                "BYTES_PER_ELEMENT",
                crate::value::PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: false,
                },
            );
        }
        super::set_builtin_constructor_prototype(e, &namespace, prototype);
        e.set(name, namespace);
    }
}

fn make_prototype<const N: usize>(
    parent: Option<Rc<Value>>,
    constructor: Value,
    methods: [(&str, Value); N],
) -> Value {
    let mut properties = vec![("constructor".to_string(), constructor)];
    properties.extend(
        methods
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone())),
    );
    let prototype = Value::object_with_proto(properties, parent);
    if let Value::Object { props } = &prototype {
        let mut metadata = props.meta.borrow_mut();
        metadata.set_attrs(
            "constructor",
            crate::value::PropAttrs {
                enumerable: false,
                ..crate::value::PropAttrs::default()
            },
        );
        for (name, _) in methods {
            metadata.set_attrs(
                name,
                crate::value::PropAttrs {
                    enumerable: false,
                    ..crate::value::PropAttrs::default()
                },
            );
        }
    }
    prototype
}

fn typed_array_prototype(object_prototype: Option<Rc<Value>>) -> Value {
    const METHODS: &[&str] = &[
        "set",
        "subarray",
        "slice",
        "fill",
        "at",
        "toString",
        "join",
        "map",
        "filter",
        "forEach",
        "reduce",
        "some",
        "every",
        "find",
        "findIndex",
        "indexOf",
        "lastIndexOf",
        "includes",
        "reverse",
        "sort",
        "keys",
        "values",
        "entries",
    ];
    let prototype = Value::object_with_proto(Vec::new(), object_prototype);
    for name in METHODS {
        prototype
            .set_prop((*name).into(), typed_prototype_method(name))
            .expect("typed-array prototype method");
    }
    if let Some(Value::Symbol(ref symbol)) = super::well_known("iterator") {
        let slot = crate::interpreter::symbol_slot_key(symbol);
        prototype
            .set_prop(slot.clone(), typed_prototype_method("values"))
            .expect("typed-array iterator method");
        if let Value::Object { props } = &prototype {
            props
                .meta
                .borrow_mut()
                .set_symbol_key(&slot, symbol.clone());
        }
    }
    if let Value::Object { props } = &prototype {
        let mut metadata = props.meta.borrow_mut();
        for name in METHODS {
            metadata.set_attrs(
                name,
                crate::value::PropAttrs {
                    enumerable: false,
                    ..crate::value::PropAttrs::default()
                },
            );
        }
        if let Some(Value::Symbol(ref symbol)) = super::well_known("iterator") {
            metadata.set_attrs(
                &crate::interpreter::symbol_slot_key(symbol),
                crate::value::PropAttrs {
                    enumerable: false,
                    ..crate::value::PropAttrs::default()
                },
            );
        }
    }
    prototype
}

fn data_view_prototype(object_prototype: Option<Rc<Value>>, constructor: Value) -> Value {
    let methods = [
        ("getInt8", data_view_get_int8 as super::NativeFn),
        ("getUint8", data_view_get_uint8),
        ("getInt16", data_view_get_int16),
        ("getUint16", data_view_get_uint16),
        ("getInt32", data_view_get_int32),
        ("getUint32", data_view_get_uint32),
        ("getFloat32", data_view_get_float32),
        ("getFloat64", data_view_get_float64),
        ("getBigInt64", data_view_get_bigint64),
        ("getBigUint64", data_view_get_biguint64),
        ("setInt8", data_view_set_int8),
        ("setUint8", data_view_set_uint8),
        ("setInt16", data_view_set_int16),
        ("setUint16", data_view_set_uint16),
        ("setInt32", data_view_set_int32),
        ("setUint32", data_view_set_uint32),
        ("setFloat32", data_view_set_float32),
        ("setFloat64", data_view_set_float64),
        ("setBigInt64", data_view_set_bigint64),
        ("setBigUint64", data_view_set_biguint64),
    ];
    let mut properties = vec![("constructor".to_string(), constructor)];
    properties.extend(
        methods
            .iter()
            .map(|(name, function)| (name.to_string(), super::nf(name, *function))),
    );
    let prototype = Value::object_with_proto(properties, object_prototype);
    if let Value::Object { props } = &prototype {
        let mut metadata = props.meta.borrow_mut();
        metadata.set_attrs(
            "constructor",
            crate::value::PropAttrs {
                enumerable: false,
                ..crate::value::PropAttrs::default()
            },
        );
        for (name, _) in methods {
            metadata.set_attrs(
                name,
                crate::value::PropAttrs {
                    enumerable: false,
                    ..crate::value::PropAttrs::default()
                },
            );
        }
    }
    prototype
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

fn new_shared_array_buffer(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let length = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    if !length.is_finite() || length < 0.0 {
        return Err(range_err("Invalid shared array buffer length"));
    }
    let length = length as usize;
    if length > crate::value::MAX_ARRAY_LEN * 8 {
        return Err(range_err("Invalid shared array buffer length"));
    }
    let buffer = SharedBuffer::zeroed(length)
        .ok_or_else(|| range_err("Invalid shared array buffer length"))?;
    Ok(Value::SharedArrayBuffer(buffer))
}

fn atomics_is_lock_free(
    interp: &mut Interpreter,
    _: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let Some(value) = args.first() else {
        return Ok(Value::Bool(false));
    };
    if matches!(value, Value::BigInt(_) | Value::Symbol(_)) {
        return Err(VmErr::Msg(
            "TypeError: cannot convert value to a lock-free size".into(),
        ));
    }
    let size = interp.tn(value).trunc();
    let size = if size.is_finite() && size >= 0.0 {
        size as usize
    } else {
        0
    };
    Ok(Value::Bool(SharedBuffer::is_lock_free(size)))
}

#[derive(Clone, Copy)]
enum AtomicsMethod {
    Load,
    Store,
    Add,
    Sub,
    And,
    Or,
    Xor,
    Exchange,
    CompareExchange,
}

fn atomics_load(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    atomics(i, &a, AtomicsMethod::Load)
}
fn atomics_store(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    atomics(i, &a, AtomicsMethod::Store)
}
fn atomics_add(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    atomics(i, &a, AtomicsMethod::Add)
}
fn atomics_sub(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    atomics(i, &a, AtomicsMethod::Sub)
}
fn atomics_and(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    atomics(i, &a, AtomicsMethod::And)
}
fn atomics_or(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    atomics(i, &a, AtomicsMethod::Or)
}
fn atomics_xor(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    atomics(i, &a, AtomicsMethod::Xor)
}
fn atomics_exchange(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    atomics(i, &a, AtomicsMethod::Exchange)
}
fn atomics_compare_exchange(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    atomics(i, &a, AtomicsMethod::CompareExchange)
}

/// `Atomics.wait` can only block the current agent. napi-vm does not yet have
/// guest worker agents, so report the unsupported blocking case clearly while
/// still handling the specification's immediate `not-equal` and zero-timeout
/// results. Async waits are backed by the VM's ordinary job/timer queues.
fn atomics_wait(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let (kind, shared, offset) = atomics_wait_location(interp, &args)?;
    let expected = atomics_argument_bits(
        interp,
        kind,
        args.get(2)
            .ok_or_else(|| VmErr::Msg("TypeError: Atomics expected value is required".into()))?,
    )?;
    let timeout = atomics_timeout(interp, &args, 3)?;
    let observed = shared.atomic_load(offset, kind.size()).ok_or_else(|| {
        VmErr::Msg("TypeError: atomic access is unaligned or unavailable on this target".into())
    })?;
    if observed != expected {
        return Ok(Value::String("not-equal".into()));
    }
    if timeout == 0.0 {
        return Ok(Value::String("timed-out".into()));
    }
    Err(VmErr::Msg(
        "TypeError: Atomics.wait requires worker-agent support; use Atomics.waitAsync".into(),
    ))
}

fn atomics_wait_async(
    interp: &mut Interpreter,
    _: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let (kind, shared, offset) = atomics_wait_location(interp, &args)?;
    let expected = atomics_argument_bits(
        interp,
        kind,
        args.get(2)
            .ok_or_else(|| VmErr::Msg("TypeError: Atomics expected value is required".into()))?,
    )?;
    let timeout = atomics_timeout(interp, &args, 3)?;
    let observed = shared.atomic_load(offset, kind.size()).ok_or_else(|| {
        VmErr::Msg("TypeError: atomic access is unaligned or unavailable on this target".into())
    })?;
    if observed != expected {
        return Ok(atomics_wait_result(
            false,
            Value::String("not-equal".into()),
        ));
    }
    if timeout == 0.0 {
        return Ok(atomics_wait_result(
            false,
            Value::String("timed-out".into()),
        ));
    }

    let promise = Value::pending_promise();
    let key = (shared.wait_identity(), offset);
    let waiter_id = interp
        .jobs
        .borrow_mut()
        .register_atomics_waiter(key, promise.clone());
    if timeout.is_finite() {
        interp.jobs.borrow_mut().push_timer_job(
            timeout,
            crate::interpreter::Job::AtomicsWaitTimeout { key, waiter_id },
        );
    }
    Ok(atomics_wait_result(true, Value::Promise(promise)))
}

fn atomics_notify(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let (kind, shared, offset) = atomics_wait_location(interp, &args)?;
    let count = match args.get(2) {
        None | Some(Value::Undefined) => usize::MAX,
        Some(Value::BigInt(_) | Value::Symbol(_)) => {
            return Err(VmErr::Msg(
                "TypeError: cannot convert value to an Atomics notify count".into(),
            ));
        }
        Some(value) => {
            let count = interp.tn(value);
            if count.is_nan() || count <= 0.0 {
                0
            } else if count.is_infinite() || count >= usize::MAX as f64 {
                usize::MAX
            } else {
                count.trunc() as usize
            }
        }
    };
    let _ = kind;
    let waiters = interp
        .jobs
        .borrow_mut()
        .take_atomics_waiters((shared.wait_identity(), offset), count);
    let notified = waiters.len();
    for promise in waiters {
        crate::interpreter::jobs::settle(
            &interp.jobs,
            &promise,
            PromiseState::Fulfilled,
            Value::String("ok".into()),
        );
    }
    Ok(Value::Number(notified as f64))
}

fn atomics_wait_result(async_: bool, value: Value) -> Value {
    Value::object(vec![
        ("async".into(), Value::Bool(async_)),
        ("value".into(), value),
    ])
}

fn atomics_wait_location(
    interp: &mut Interpreter,
    args: &[Value],
) -> Result<(TypedKind, SharedBuffer, usize), VmErr> {
    let Some(Value::TypedArray(view)) = args.first() else {
        return Err(VmErr::Msg(
            "TypeError: Atomics wait requires an Int32Array or BigInt64Array".into(),
        ));
    };
    if !matches!(view.kind, TypedKind::Int32 | TypedKind::BigInt64) {
        return Err(VmErr::Msg(
            "TypeError: Atomics wait requires an Int32Array or BigInt64Array".into(),
        ));
    }
    let BufferBacking::Shared(shared) = &view.buffer else {
        return Err(VmErr::Msg(
            "TypeError: Atomics wait requires a SharedArrayBuffer".into(),
        ));
    };
    let index_value = args
        .get(1)
        .ok_or_else(|| VmErr::Msg("TypeError: Atomics index is required".into()))?;
    if matches!(index_value, Value::BigInt(_) | Value::Symbol(_)) {
        return Err(VmErr::Msg(
            "TypeError: cannot convert value to an Atomics index".into(),
        ));
    }
    let index = interp.tn(index_value).trunc();
    if !index.is_finite() || index < 0.0 || index >= view.effective_length() as f64 {
        return Err(range_err("Atomics index is outside the typed array"));
    }
    let offset = view
        .effective_byte_offset()
        .checked_add(index as usize * view.kind.size())
        .ok_or_else(|| range_err("Atomics index is outside the typed array"))?;
    Ok((view.kind, shared.clone(), offset))
}

fn atomics_timeout(interp: &Interpreter, args: &[Value], index: usize) -> Result<f64, VmErr> {
    let Some(value) = args.get(index) else {
        return Ok(f64::INFINITY);
    };
    if matches!(value, Value::Undefined) {
        return Ok(f64::INFINITY);
    }
    if matches!(value, Value::BigInt(_) | Value::Symbol(_)) {
        return Err(VmErr::Msg(
            "TypeError: cannot convert value to an Atomics timeout".into(),
        ));
    }
    let timeout = interp.tn(value);
    if timeout.is_nan() {
        Ok(f64::INFINITY)
    } else if timeout <= 0.0 {
        Ok(0.0)
    } else {
        Ok(timeout)
    }
}

fn atomics(
    interp: &mut Interpreter,
    args: &[Value],
    method: AtomicsMethod,
) -> Result<Value, VmErr> {
    let Some(Value::TypedArray(view)) = args.first() else {
        return Err(VmErr::Msg(
            "TypeError: Atomics requires an integer typed array".into(),
        ));
    };
    if !matches!(
        view.kind,
        TypedKind::Int8
            | TypedKind::Uint8
            | TypedKind::Int16
            | TypedKind::Uint16
            | TypedKind::Int32
            | TypedKind::Uint32
            | TypedKind::BigInt64
            | TypedKind::BigUint64
    ) {
        return Err(VmErr::Msg(
            "TypeError: Atomics requires an integer typed array".into(),
        ));
    }
    let index_value = args
        .get(1)
        .ok_or_else(|| VmErr::Msg("TypeError: Atomics index is required".into()))?;
    if matches!(index_value, Value::BigInt(_) | Value::Symbol(_)) {
        return Err(VmErr::Msg(
            "TypeError: cannot convert value to an Atomics index".into(),
        ));
    }
    let index = interp.tn(index_value).trunc();
    if !index.is_finite() || index < 0.0 {
        return Err(range_err("Atomics index is outside the typed array"));
    }
    let index = index as usize;
    if index >= view.effective_length() {
        return Err(range_err("Atomics index is outside the typed array"));
    }
    let width = view.kind.size();
    let offset = view.effective_byte_offset() + index * width;
    let result = match method {
        AtomicsMethod::Load => atomics_load_bits(&view.buffer, offset, width),
        AtomicsMethod::Store => {
            let value = atomics_argument_bits(
                interp,
                view.kind,
                args.get(2).ok_or_else(|| {
                    VmErr::Msg("TypeError: Atomics store value is required".into())
                })?,
            )?;
            if !atomics_store_bits(&view.buffer, offset, width, value) {
                return Err(VmErr::Msg(
                    "TypeError: atomic access is unaligned or unavailable on this target".into(),
                ));
            }
            return Ok(atomics_value(view.kind, value));
        }
        operation => {
            let value = atomics_argument_bits(
                interp,
                view.kind,
                args.get(2)
                    .ok_or_else(|| VmErr::Msg("TypeError: Atomics value is required".into()))?,
            )?;
            let replacement = if matches!(operation, AtomicsMethod::CompareExchange) {
                atomics_argument_bits(
                    interp,
                    view.kind,
                    args.get(3).ok_or_else(|| {
                        VmErr::Msg("TypeError: Atomics replacement value is required".into())
                    })?,
                )?
            } else {
                0
            };
            let operation = match operation {
                AtomicsMethod::Add => SharedAtomicOp::Add,
                AtomicsMethod::Sub => SharedAtomicOp::Sub,
                AtomicsMethod::And => SharedAtomicOp::And,
                AtomicsMethod::Or => SharedAtomicOp::Or,
                AtomicsMethod::Xor => SharedAtomicOp::Xor,
                AtomicsMethod::Exchange => SharedAtomicOp::Exchange,
                AtomicsMethod::CompareExchange => SharedAtomicOp::CompareExchange,
                AtomicsMethod::Load | AtomicsMethod::Store => unreachable!(),
            };
            atomics_rmw_bits(&view.buffer, offset, width, operation, value, replacement)
        }
    };
    result
        .map(|value| atomics_value(view.kind, value))
        .ok_or_else(|| {
            VmErr::Msg("TypeError: atomic access is unaligned or unavailable on this target".into())
        })
}

fn atomics_load_bits(buffer: &BufferBacking, offset: usize, width: usize) -> Option<u64> {
    match buffer {
        BufferBacking::Shared(shared) => shared.atomic_load(offset, width),
        BufferBacking::Array(_) => {
            let bytes = buffer.read(offset, width)?;
            let mut bits = [0; 8];
            bits[..width].copy_from_slice(&bytes);
            Some(u64::from_le_bytes(bits))
        }
    }
}

fn atomics_store_bits(buffer: &BufferBacking, offset: usize, width: usize, value: u64) -> bool {
    match buffer {
        BufferBacking::Shared(shared) => shared.atomic_store(offset, width, value),
        BufferBacking::Array(_) => buffer.write(offset, &value.to_le_bytes()[..width]),
    }
}

fn atomics_rmw_bits(
    buffer: &BufferBacking,
    offset: usize,
    width: usize,
    operation: SharedAtomicOp,
    value: u64,
    replacement: u64,
) -> Option<u64> {
    if let BufferBacking::Shared(buffer) = buffer {
        return buffer.atomic_rmw(offset, width, operation, value, replacement);
    }

    let previous = atomics_load_bits(buffer, offset, width)?;
    let mask = if width == 8 {
        u64::MAX
    } else {
        (1_u64 << (width * 8)) - 1
    };
    let next = match operation {
        SharedAtomicOp::Add => previous.wrapping_add(value) & mask,
        SharedAtomicOp::Sub => previous.wrapping_sub(value) & mask,
        SharedAtomicOp::And => previous & value,
        SharedAtomicOp::Or => previous | value,
        SharedAtomicOp::Xor => previous ^ value,
        SharedAtomicOp::Exchange => value,
        SharedAtomicOp::CompareExchange => {
            if previous == value {
                replacement
            } else {
                previous
            }
        }
    };
    if !atomics_store_bits(buffer, offset, width, next) {
        return None;
    }
    Some(previous)
}

fn atomics_argument_bits(
    interp: &Interpreter,
    kind: TypedKind,
    value: &Value,
) -> Result<u64, VmErr> {
    if matches!(kind, TypedKind::BigInt64 | TypedKind::BigUint64) {
        let bigint = match value {
            Value::BigInt(value) => value.as_ref().clone(),
            Value::Bool(false) => crate::bigint::BigInt::zero(),
            Value::Bool(true) => crate::bigint::BigInt::from_i64(1),
            Value::String(value) => crate::bigint::BigInt::parse(value)
                .map_err(|_| VmErr::Msg("SyntaxError: invalid BigInt value".into()))?,
            _ => {
                return Err(VmErr::Msg(
                    "TypeError: Atomics BigInt typed arrays require a BigInt value".into(),
                ));
            }
        };
        let wrapped = bigint.as_n_bit(64, false).map_err(VmErr::Msg)?;
        return wrapped
            .to_decimal()
            .parse()
            .map_err(|_| VmErr::Msg("RangeError: invalid Atomics BigInt value".into()));
    }
    if matches!(value, Value::BigInt(_) | Value::Symbol(_)) {
        return Err(VmErr::Msg(
            "TypeError: cannot convert value to an Atomics Number element".into(),
        ));
    }
    let bits = to_int(interp.tn(value)) as u32 as u64;
    let width = kind.size();
    Ok(if width == 4 {
        bits
    } else {
        bits & ((1_u64 << (width * 8)) - 1)
    })
}

fn atomics_value(kind: TypedKind, bits: u64) -> Value {
    match kind {
        TypedKind::Int8 => Value::Number(bits as u8 as i8 as f64),
        TypedKind::Uint8 => Value::Number(bits as u8 as f64),
        TypedKind::Int16 => Value::Number(bits as u16 as i16 as f64),
        TypedKind::Uint16 => Value::Number(bits as u16 as f64),
        TypedKind::Int32 => Value::Number(bits as u32 as i32 as f64),
        TypedKind::Uint32 => Value::Number(bits as u32 as f64),
        TypedKind::BigInt64 => Value::BigInt(Rc::new(crate::bigint::BigInt::from_i64(bits as i64))),
        TypedKind::BigUint64 => Value::BigInt(Rc::new(
            crate::bigint::BigInt::parse(&bits.to_string()).expect("u64 decimal is a BigInt"),
        )),
        TypedKind::Uint8Clamped | TypedKind::Float32 | TypedKind::Float64 => {
            unreachable!("Atomics cannot access non-integer typed arrays")
        }
    }
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
        Some(Value::SharedArrayBuffer(buffer)) => {
            let byte_offset = a.get(1).map(|v| v.to_number()).unwrap_or(0.0);
            if !byte_offset.is_finite() || byte_offset < 0.0 {
                return Err(range_err("Invalid typed array offset"));
            }
            let byte_offset = byte_offset as usize;
            let available = buffer.len();
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

fn typed(
    kind: TypedKind,
    buffer: impl Into<BufferBacking>,
    byte_offset: usize,
    length: usize,
) -> Value {
    typed_with_buffer(kind, buffer, byte_offset, length, false)
}

pub(crate) fn typed_with_buffer(
    kind: TypedKind,
    buffer: impl Into<BufferBacking>,
    byte_offset: usize,
    length: usize,
    is_buffer: bool,
) -> Value {
    Value::TypedArray(Rc::new(TypedArrayData {
        kind,
        buffer: buffer.into(),
        byte_offset,
        length,
        is_buffer,
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
    let at = view.effective_byte_offset() + index * view.kind.size();
    let bytes = view.buffer.read(at, view.kind.size())?;
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
    view.buffer.write(at, &bytes);
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
        "buffer" => view.buffer.to_value(),
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
fn typed_delegate_named(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
    name: &str,
) -> Result<Value, VmErr> {
    let view = require(&this)?;
    let array = Value::array(read_all(&view));
    let method = interp.member(&array, name)?;
    interp.call_this(&method, array, a)
}

macro_rules! typed_delegate_method {
    ($function:ident, $name:literal) => {
        fn $function(
            interp: &mut Interpreter,
            this: Value,
            args: Vec<Value>,
        ) -> Result<Value, VmErr> {
            typed_delegate_named(interp, this, args, $name)
        }
    };
}

typed_delegate_method!(typed_map, "map");
typed_delegate_method!(typed_filter, "filter");
typed_delegate_method!(typed_for_each, "forEach");
typed_delegate_method!(typed_reduce, "reduce");
typed_delegate_method!(typed_some, "some");
typed_delegate_method!(typed_every, "every");
typed_delegate_method!(typed_find, "find");
typed_delegate_method!(typed_find_index, "findIndex");
typed_delegate_method!(typed_index_of, "indexOf");
typed_delegate_method!(typed_last_index_of, "lastIndexOf");
typed_delegate_method!(typed_includes, "includes");
typed_delegate_method!(typed_reverse, "reverse");
typed_delegate_method!(typed_sort, "sort");
typed_delegate_method!(typed_keys, "keys");
typed_delegate_method!(typed_values, "values");
typed_delegate_method!(typed_entries, "entries");

fn typed_prototype_method(name: &str) -> Value {
    let callable: super::NativeFn = match name {
        "set" => typed_set,
        "subarray" => typed_subarray,
        "slice" => typed_slice,
        "fill" => typed_fill,
        "at" => typed_at,
        "toString" | "join" => typed_join,
        "map" => typed_map,
        "filter" => typed_filter,
        "forEach" => typed_for_each,
        "reduce" => typed_reduce,
        "some" => typed_some,
        "every" => typed_every,
        "find" => typed_find,
        "findIndex" => typed_find_index,
        "indexOf" => typed_index_of,
        "lastIndexOf" => typed_last_index_of,
        "includes" => typed_includes,
        "reverse" => typed_reverse,
        "sort" => typed_sort,
        "keys" => typed_keys,
        "values" => typed_values,
        "entries" => typed_entries,
        _ => unreachable!("unknown typed-array prototype method"),
    };
    super::nf(name, callable)
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
    Ok(typed_with_buffer(
        view.kind,
        view.buffer.clone(),
        view.effective_byte_offset() + start * view.kind.size(),
        end - start,
        view.is_buffer,
    ))
}

/// `slice`: a *copy*, so writes are not shared.
fn typed_slice(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = require(&this)?;
    let (start, end) = window(view.effective_length(), &a);
    let size = view.kind.size();
    let copy = new_buffer((end - start) * size)?;
    let from = view.effective_byte_offset() + start * size;
    let to = view.effective_byte_offset() + end * size;
    if let Some(source) = view.buffer.read(from, to - from) {
        copy.borrow_mut().copy_from_slice(&source);
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

// --- ArrayBuffer members ----------------------------------------------------

pub fn array_buffer_member(buffer: &Buffer, key: &str) -> Option<Value> {
    Some(match key {
        "byteLength" => Value::Number(buffer.borrow().len() as f64),
        _ => return None,
    })
}

pub fn shared_array_buffer_member(buffer: &SharedBuffer, key: &str) -> Option<Value> {
    Some(match key {
        "byteLength" => Value::Number(buffer.len() as f64),
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

fn shared_array_buffer_slice(
    _: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let Value::SharedArrayBuffer(buffer) = &this else {
        return Err(VmErr::Msg("TypeError: not a SharedArrayBuffer".to_string()));
    };
    let (start, end) = window(buffer.len(), &a);
    let copy = SharedBuffer::zeroed(end - start)
        .ok_or_else(|| range_err("Invalid shared array buffer length"))?;
    let bytes = buffer
        .read(start, end - start)
        .ok_or_else(|| range_err("Invalid shared array buffer range"))?;
    copy.write(0, &bytes);
    Ok(Value::SharedArrayBuffer(copy))
}

// --- DataView ---------------------------------------------------------------

fn new_data_view(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let Some(value @ (Value::ArrayBuffer(_) | Value::SharedArrayBuffer(_))) = a.first() else {
        return Err(VmErr::Msg(
            "TypeError: First argument to DataView constructor must be an ArrayBuffer".to_string(),
        ));
    };
    if matches!(value, Value::ArrayBuffer(buffer) if buffer.is_detached()) {
        return Err(VmErr::Msg(
            "TypeError: Cannot construct a DataView from a detached ArrayBuffer".to_string(),
        ));
    }
    let backing = match value {
        Value::ArrayBuffer(buffer) => BufferBacking::Array(buffer.clone()),
        Value::SharedArrayBuffer(buffer) => BufferBacking::Shared(buffer.clone()),
        _ => unreachable!(),
    };
    let available = backing.len();
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
        buffer: backing,
        byte_offset,
        length: byte_length,
        is_buffer: false,
    })))
}

/// `DataView` accessors. Each `get`/`set` pair names its element type, and
/// takes a `littleEndian` flag defaulting to big-endian — the opposite of the
/// typed arrays, which is what the specification says.
pub fn data_view_member(view: &Rc<TypedArrayData>, key: &str) -> Option<Value> {
    Some(match key {
        "byteLength" => Value::Number(view.effective_length() as f64),
        "byteOffset" => Value::Number(view.effective_byte_offset() as f64),
        "buffer" => view.buffer.to_value(),
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
        is_buffer: false,
    }))
}

fn swap_if_big_endian(slot: &Rc<TypedArrayData>, little_endian: bool) {
    if little_endian || slot.kind.size() == 1 {
        return;
    }
    let at = slot.byte_offset;
    let size = slot.kind.size();
    if let Some(mut bytes) = slot.buffer.read(at, size) {
        bytes.reverse();
        slot.buffer.write(at, &bytes);
    }
}

fn data_view_get_kind(this: Value, a: Vec<Value>, kind: TypedKind) -> Result<Value, VmErr> {
    let slot = data_view_slot(&this, &a, kind)?;
    let little_endian = a.get(1).map(|v| v.is_truthy()).unwrap_or(false);
    swap_if_big_endian(&slot, little_endian);
    let value = read_element(&slot, 0).unwrap_or(Value::Undefined);
    // Restore the bytes: the read is not supposed to mutate the buffer.
    swap_if_big_endian(&slot, little_endian);
    Ok(value)
}

fn data_view_set_kind(this: Value, a: Vec<Value>, kind: TypedKind) -> Result<Value, VmErr> {
    let slot = data_view_slot(&this, &a, kind)?;
    let value = a.get(1).cloned().unwrap_or(Value::Undefined);
    write_element(&slot, 0, &value)?;
    let little_endian = a.get(2).map(|v| v.is_truthy()).unwrap_or(false);
    swap_if_big_endian(&slot, little_endian);
    Ok(Value::Undefined)
}

macro_rules! data_view_accessors {
    ($(($get_fn:ident, $set_fn:ident, $kind:ident, $get_name:literal, $set_name:literal)),* $(,)?) => {
        $(
            fn $get_fn(_: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
                data_view_get_kind(this, args, TypedKind::$kind)
            }

            fn $set_fn(_: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
                data_view_set_kind(this, args, TypedKind::$kind)
            }
        )*
    };
}

data_view_accessors!(
    (
        data_view_get_int8,
        data_view_set_int8,
        Int8,
        "getInt8",
        "setInt8"
    ),
    (
        data_view_get_uint8,
        data_view_set_uint8,
        Uint8,
        "getUint8",
        "setUint8"
    ),
    (
        data_view_get_int16,
        data_view_set_int16,
        Int16,
        "getInt16",
        "setInt16"
    ),
    (
        data_view_get_uint16,
        data_view_set_uint16,
        Uint16,
        "getUint16",
        "setUint16"
    ),
    (
        data_view_get_int32,
        data_view_set_int32,
        Int32,
        "getInt32",
        "setInt32"
    ),
    (
        data_view_get_uint32,
        data_view_set_uint32,
        Uint32,
        "getUint32",
        "setUint32"
    ),
    (
        data_view_get_float32,
        data_view_set_float32,
        Float32,
        "getFloat32",
        "setFloat32"
    ),
    (
        data_view_get_float64,
        data_view_set_float64,
        Float64,
        "getFloat64",
        "setFloat64"
    ),
    (
        data_view_get_bigint64,
        data_view_set_bigint64,
        BigInt64,
        "getBigInt64",
        "setBigInt64"
    ),
    (
        data_view_get_biguint64,
        data_view_set_biguint64,
        BigUint64,
        "getBigUint64",
        "setBigUint64"
    ),
);

#[cfg(test)]
mod tests {
    use crate::interpreter::Interpreter;
    use crate::value::{PromiseState, Value};
    use std::process::Command;

    #[test]
    fn atomics_shared_buffer_fixture_matches_node_and_bun() {
        let fixture = r#"(() => {
  const shared = new SharedArrayBuffer(16);
  const bytes = new Uint8Array(shared);
  const signed = new Int16Array(shared, 2, 1);
  const bits = new Uint32Array(shared, 4, 1);
  const big = new BigInt64Array(shared, 8, 1);
  const byteResults = [
    Atomics.store(bytes, 0, 255),
    Atomics.add(bytes, 0, 2),
    Atomics.compareExchange(bytes, 0, 1, 42),
    Atomics.load(bytes, 0),
  ];
  const fractionalIndexResults = [
    Atomics.store(bytes, -0.5, 43),
    Atomics.load(bytes, 0.9),
  ];
  const signedResults = [
    Atomics.store(signed, 0, -3),
    Atomics.add(signed, 0, 10),
    Atomics.sub(signed, 0, 10),
    Atomics.load(signed, 0),
  ];
  const bitwiseResults = [
    Atomics.store(bits, 0, 10),
    Atomics.and(bits, 0, 12),
    Atomics.or(bits, 0, 1),
    Atomics.xor(bits, 0, 3),
    Atomics.exchange(bits, 0, 7),
    Atomics.load(bits, 0),
  ];
  const bigIntResults = [
    String(Atomics.store(big, 0, -9007199254740993n)),
    String(Atomics.add(big, 0, 2n)),
    String(Atomics.load(big, 0)),
  ];
  const errorName = (callback) => {
    try { callback(); return 'none'; } catch (error) { return error.name; }
  };
  const ordinary = new Int32Array([5]);
  const ordinaryResults = [
    Atomics.load(ordinary, 0),
    Atomics.add(ordinary, 0, 3),
    Atomics.store(ordinary, 0, 9),
    Atomics.compareExchange(ordinary, 0, 9, 12),
    Atomics.load(ordinary, 0),
  ];
  return JSON.stringify({
    byteResults,
    fractionalIndexResults,
    signedResults,
    bitwiseResults,
    bigIntResults,
    ordinaryResults,
    lockFree: [1, 2, 4, 8, 3].map(size => Atomics.isLockFree(size)),
    invalidFloatArray: errorName(() => Atomics.add(new Float32Array(1), 0, 1)),
    nonSharedBuffer: errorName(() => Atomics.load(new Int32Array(1), 0)),
  });
})()"#;
        let mut interpreter = Interpreter::with_builtins();
        let result = interpreter.eval_source(fixture).unwrap();
        let Value::String(ref result) = result else {
            panic!("Atomics fixture returned {result:?}");
        };
        let expected: serde_json::Value = serde_json::from_str(result).unwrap();
        assert_eq!(
            expected["byteResults"],
            serde_json::json!([255, 255, 1, 42])
        );
        assert_eq!(
            expected["fractionalIndexResults"],
            serde_json::json!([43, 43])
        );
        assert_eq!(
            expected["signedResults"],
            serde_json::json!([-3, -3, 7, -3])
        );
        assert_eq!(
            expected["bitwiseResults"],
            serde_json::json!([10, 10, 8, 9, 10, 7])
        );
        assert_eq!(
            expected["bigIntResults"],
            serde_json::json!([
                "-9007199254740993",
                "-9007199254740993",
                "-9007199254740991"
            ])
        );
        assert_eq!(
            expected["ordinaryResults"],
            serde_json::json!([5, 5, 9, 9, 12])
        );
        assert_eq!(expected["invalidFloatArray"], "TypeError");
        assert_eq!(expected["nonSharedBuffer"], "none");
        assert!(matches!(
            interpreter.eval_source("Atomics.isLockFree(1.5)").unwrap(),
            Value::Bool(true)
        ));

        for runtime in ["node", "bun"] {
            let Ok(reference) = Command::new(runtime)
                .args(["-e", &format!("process.stdout.write({fixture})")])
                .output()
            else {
                continue;
            };
            if !reference.status.success() {
                eprintln!(
                    "skipping {runtime} Atomics comparison: {}",
                    String::from_utf8_lossy(&reference.stderr)
                );
                continue;
            }
            let actual: serde_json::Value = serde_json::from_slice(&reference.stdout).unwrap();
            assert_eq!(expected, actual, "{runtime} Atomics behavior differed");
        }
    }

    #[test]
    fn atomics_wait_async_and_notify_match_node_and_bun() {
        let fixture = r#"async function runWaiters() {
  const shared = new SharedArrayBuffer(8);
  const words = new Int32Array(shared);
  const first = Atomics.waitAsync(words, 0, 0, 50);
  const second = Atomics.waitAsync(words, 0, 0, 50);
  const notEqual = Atomics.waitAsync(words, 1, 1, 50);
  const timedOut = Atomics.waitAsync(words, 1, 0, 0);
  Atomics.store(words, 1, 7);
  const nanTimeout = Atomics.waitAsync(words, 1, 7, NaN);
  const defaultTimeout = Atomics.waitAsync(words, 1, 7, undefined);
  let notified = [];
  setTimeout(() => {
    notified = [Atomics.notify(words, 0, 1), Atomics.notify(words, 1, 2)];
  }, 10);
  const firstValue = await first.value;
  const secondValue = await second.value;
  const nanValue = await nanTimeout.value;
  const defaultValue = await defaultTimeout.value;
  return JSON.stringify({
    async: [first.async, second.async, notEqual.async, timedOut.async, nanTimeout.async, defaultTimeout.async],
    immediate: [notEqual.value, timedOut.value],
    notified,
    values: [firstValue, secondValue, nanValue, defaultValue],
  });
}
runWaiters()"#;
        let mut interpreter = Interpreter::with_builtins();
        let result = interpreter.eval_source(fixture).unwrap();
        let Some(promise) = result.as_promise() else {
            panic!("waitAsync fixture did not return a Promise: {result:?}");
        };
        let inner = promise.borrow();
        assert_eq!(inner.state, PromiseState::Fulfilled);
        let Value::String(ref result) = inner.value else {
            panic!("waitAsync fixture returned {:?}", inner.value);
        };
        let expected: serde_json::Value = serde_json::from_str(result).unwrap();
        assert_eq!(
            expected,
            serde_json::json!({
                "async": [true, true, false, false, true, true],
                "immediate": ["not-equal", "timed-out"],
                "notified": [1, 2],
                "values": ["ok", "timed-out", "ok", "ok"],
            })
        );

        for runtime in ["node", "bun"] {
            let Ok(reference) = Command::new(runtime)
                .args([
                    "-e",
                    &format!(
                        "setTimeout(() => {{}}, 100); {fixture}.then(value => process.stdout.write(value))"
                    ),
                ])
                .output()
            else {
                continue;
            };
            if !reference.status.success() {
                eprintln!(
                    "skipping {runtime} Atomics.waitAsync comparison: {}",
                    String::from_utf8_lossy(&reference.stderr)
                );
                continue;
            }
            let actual: serde_json::Value = serde_json::from_slice(&reference.stdout).unwrap();
            assert_eq!(
                expected, actual,
                "{runtime} Atomics.waitAsync behavior differed"
            );
        }
    }

    #[test]
    fn atomics_wait_async_tracks_structured_cloned_shared_data() {
        let fixture = r#"async function runClonedWaiter() {
  const shared = new SharedArrayBuffer(4);
  const waitingView = new Int32Array(shared);
  const notifyingView = new Int32Array(structuredClone(shared));
  const waiter = Atomics.waitAsync(waitingView, 0, 0, 30);
  let notified = -1;
  setTimeout(() => { notified = Atomics.notify(notifyingView, 0); }, 1);
  const value = await waiter.value;
  return JSON.stringify({ notified, value });
}
runClonedWaiter()"#;
        let mut interpreter = Interpreter::with_builtins();
        let result = interpreter.eval_source(fixture).unwrap();
        let Some(promise) = result.as_promise() else {
            panic!("cloned-buffer waitAsync fixture did not return a Promise: {result:?}");
        };
        let inner = promise.borrow();
        assert_eq!(inner.state, PromiseState::Fulfilled);
        let Value::String(ref result) = inner.value else {
            panic!("cloned-buffer waitAsync fixture returned {:?}", inner.value);
        };
        let expected = serde_json::json!({"notified": 1, "value": "ok"});
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(result).unwrap(),
            expected
        );

        let Ok(node) = Command::new("node")
            .args([
                "-e",
                &format!(
                    "setTimeout(() => {{}}, 50); {fixture}.then(value => process.stdout.write(value))"
                ),
            ])
            .output()
        else {
            return;
        };
        if !node.status.success() {
            eprintln!(
                "skipping Node shared-clone Atomics comparison: {}",
                String::from_utf8_lossy(&node.stderr)
            );
            return;
        }
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&node.stdout).unwrap(),
            expected
        );

        // Bun 1.4.0 does not share its wait list across structured-cloned
        // SharedArrayBuffer wrappers. Keep that observable mismatch explicit
        // instead of normalizing it to Node's behavior.
        if let Ok(bun) = Command::new("bun")
            .args([
                "-e",
                &format!(
                    "setTimeout(() => {{}}, 50); {fixture}.then(value => process.stdout.write(value))"
                ),
            ])
            .output()
            && bun.status.success()
        {
            let actual: serde_json::Value = serde_json::from_slice(&bun.stdout).unwrap();
            let bun_1_4_known_difference =
                serde_json::json!({"notified": 0, "value": "timed-out"});
            assert!(
                actual == expected || actual == bun_1_4_known_difference,
                "unexpected Bun Atomics clone-wait result: {actual}"
            );
        }
    }
}
