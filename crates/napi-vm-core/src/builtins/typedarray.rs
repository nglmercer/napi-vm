//! `ArrayBuffer`, the typed-array views, and `DataView`.
//!
//! A buffer is a byte vector; a view is a window onto one, with an element
//! type, a byte offset and a length. Two views over the same buffer see each
//! other's writes, which is the whole point of the type — so the buffer is
//! shared (`Rc<RefCell<Vec<u8>>>`) and the view holds only the window.

mod encoding;
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

    for name in ["ArrayBuffer", "SharedArrayBuffer"] {
        let constructor = e.get(name).expect("buffer constructor");
        constructor
            .set_prop("name".into(), Value::String(name.into()))
            .unwrap();
        constructor
            .property_cell()
            .unwrap()
            .meta
            .borrow_mut()
            .set_attrs(
                "name",
                crate::value::PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        constructor
            .set_prop("length".into(), Value::Number(1.0))
            .expect("constructor length");
        if let Value::Object { props } = &constructor {
            props.meta.borrow_mut().set_attrs(
                "length",
                crate::value::PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        let key = super::well_known("species").expect("Symbol.species");
        super::object::define_property(
            &constructor,
            &match &key {
                Value::Symbol(symbol) => crate::interpreter::symbol_slot_key(symbol),
                _ => unreachable!(),
            },
            &Value::descriptor_record(vec![
                (
                    "get".into(),
                    super::native_method(
                        "get [Symbol.species]",
                        0,
                        buffer_species,
                        e.get("Function").and_then(|f| f.get_prop("prototype")),
                    ),
                ),
                ("configurable".into(), Value::Bool(true)),
            ]),
        )
        .expect("buffer species getter");
        if let (Value::Object { props }, Value::Symbol(symbol)) = (&constructor, &key) {
            let slot = crate::interpreter::symbol_slot_key(symbol);
            props
                .meta
                .borrow_mut()
                .set_symbol_key(&slot, symbol.clone());
        }
    }
    if let Some(namespace) = e.get("ArrayBuffer") {
        namespace
            .set_prop(
                "isView".to_string(),
                super::native_method(
                    "isView",
                    1,
                    array_buffer_is_view,
                    e.get("Function").and_then(|f| f.get_prop("prototype")),
                ),
            )
            .expect("built-in ArrayBuffer property");
        super::make_callable(&namespace, super::require_new, Some(new_array_buffer));
        let prototype = make_prototype(
            object_prototype.clone(),
            namespace.clone(),
            [
                (
                    "slice",
                    super::native_method(
                        "slice",
                        2,
                        array_buffer_slice,
                        e.get("Function").and_then(|f| f.get_prop("prototype")),
                    ),
                ),
                (
                    "resize",
                    super::native_method(
                        "resize",
                        1,
                        array_buffer_resize,
                        e.get("Function")
                            .and_then(|function| function.get_prop("prototype")),
                    ),
                ),
            ],
        );
        for (name, function) in [
            ("transfer", array_buffer_transfer as super::NativeFn),
            ("transferToFixedLength", array_buffer_transfer_fixed),
            ("transferToImmutable", array_buffer_transfer_immutable),
            ("sliceToImmutable", array_buffer_slice_immutable),
        ] {
            prototype
                .set_prop(
                    name.into(),
                    super::native_method(
                        name,
                        if name == "sliceToImmutable" { 2 } else { 0 },
                        function,
                        e.get("Function").and_then(|f| f.get_prop("prototype")),
                    ),
                )
                .unwrap();
            prototype
                .property_cell()
                .unwrap()
                .meta
                .borrow_mut()
                .set_attrs(
                    name,
                    crate::value::PropAttrs {
                        enumerable: false,
                        ..Default::default()
                    },
                );
        }
        if let Some(Value::Symbol(ref symbol)) = super::well_known("toStringTag") {
            let key = crate::interpreter::symbol_slot_key(symbol);
            prototype
                .set_prop(key.clone(), Value::String("ArrayBuffer".into()))
                .unwrap();
            let cell = prototype.property_cell().unwrap();
            let mut meta = cell.meta.borrow_mut();
            meta.set_symbol_key(&key, symbol.clone());
            meta.set_attrs(
                &key,
                crate::value::PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        namespace
            .property_cell()
            .unwrap()
            .meta
            .borrow_mut()
            .set_attrs(
                "isView",
                crate::value::PropAttrs {
                    enumerable: false,
                    ..Default::default()
                },
            );
        for (name, getter) in [
            ("byteLength", array_byte_length as super::NativeFn),
            ("maxByteLength", array_maximum_length),
            ("resizable", array_resizable),
            ("detached", array_detached),
            ("immutable", array_immutable),
        ] {
            prototype
                .set_prop(
                    name.into(),
                    super::native_method(
                        &format!("get {name}"),
                        0,
                        getter,
                        e.get("Function")
                            .and_then(|function| function.get_prop("prototype")),
                    ),
                )
                .expect("buffer getter");
            if let Value::Object { props } = &prototype {
                let mut meta = props.meta.borrow_mut();
                meta.has_accessors = true;
                meta.set_attrs(
                    name,
                    crate::value::PropAttrs {
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
        }
        super::set_builtin_constructor_prototype(e, &namespace, prototype);
    }
    if let Some(namespace) = e.get("SharedArrayBuffer") {
        super::make_callable(
            &namespace,
            super::require_new,
            Some(new_shared_array_buffer),
        );
        let prototype = make_prototype(
            object_prototype.clone(),
            namespace.clone(),
            [
                (
                    "slice",
                    super::native_method(
                        "slice",
                        2,
                        shared_array_buffer_slice,
                        e.get("Function").and_then(|f| f.get_prop("prototype")),
                    ),
                ),
                (
                    "grow",
                    super::native_method(
                        "grow",
                        1,
                        shared_array_buffer_grow,
                        e.get("Function")
                            .and_then(|function| function.get_prop("prototype")),
                    ),
                ),
            ],
        );
        for (name, getter) in [
            ("byteLength", shared_byte_length as super::NativeFn),
            ("maxByteLength", shared_maximum_length),
            ("growable", shared_growable),
        ] {
            prototype
                .set_prop(
                    name.into(),
                    super::native_method(
                        &format!("get {name}"),
                        0,
                        getter,
                        e.get("Function")
                            .and_then(|function| function.get_prop("prototype")),
                    ),
                )
                .expect("shared buffer getter");
            if let Value::Object { props } = &prototype {
                let mut meta = props.meta.borrow_mut();
                meta.has_accessors = true;
                meta.set_attrs(
                    name,
                    crate::value::PropAttrs {
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
        }
        if let Some(Value::Symbol(ref symbol)) = super::well_known("toStringTag") {
            let slot = crate::interpreter::symbol_slot_key(symbol);
            prototype
                .set_prop(slot.clone(), Value::String("SharedArrayBuffer".into()))
                .expect("buffer tag");
            if let Value::Object { props } = &prototype {
                let mut meta = props.meta.borrow_mut();
                meta.set_symbol_key(&slot, symbol.clone());
                meta.set_attrs(
                    &slot,
                    crate::value::PropAttrs {
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
        }
        super::set_builtin_constructor_prototype(e, &namespace, prototype);
    }
    if let Some(namespace) = e.get("Atomics") {
        let function_prototype = e
            .get("Function")
            .and_then(|constructor| constructor.get_prop("prototype"));
        for (name, length, method) in [
            ("isLockFree", 1, atomics_is_lock_free as _),
            ("pause", 0, atomics_pause as _),
            ("load", 2, atomics_load as _),
            ("store", 3, atomics_store as _),
            ("add", 3, atomics_add as _),
            ("sub", 3, atomics_sub as _),
            ("and", 3, atomics_and as _),
            ("or", 3, atomics_or as _),
            ("xor", 3, atomics_xor as _),
            ("exchange", 3, atomics_exchange as _),
            ("compareExchange", 4, atomics_compare_exchange as _),
            ("wait", 4, atomics_wait as _),
            ("waitAsync", 4, atomics_wait_async as _),
            ("notify", 3, atomics_notify as _),
        ] {
            namespace
                .set_prop(
                    name.to_string(),
                    super::native_method(name, length, method, function_prototype.clone()),
                )
                .expect("built-in Atomics property");
            if let Value::Object { props } = &namespace {
                props.meta.borrow_mut().set_attrs(
                    name,
                    crate::value::PropAttrs {
                        enumerable: false,
                        ..Default::default()
                    },
                );
            }
        }
    }
    if let Some(Value::Object { props }) = &e.get("Atomics")
        && let Some(Value::Symbol(symbol)) = &super::well_known("toStringTag")
    {
        let key = crate::interpreter::symbol_slot_key(symbol);
        props
            .borrow_mut()
            .push((key.clone(), Value::String("Atomics".into())));
        props.meta.borrow_mut().set_attrs(
            &key,
            crate::value::PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        );
    }
    if let Some(namespace) = e.get("DataView") {
        super::make_callable(&namespace, super::require_new, Some(new_data_view));
        namespace
            .set_prop("length".into(), Value::Number(1.))
            .unwrap();
        namespace
            .set_prop("name".into(), Value::String("DataView".into()))
            .unwrap();
        for key in ["name", "length"] {
            namespace
                .property_cell()
                .unwrap()
                .meta
                .borrow_mut()
                .set_attrs(
                    key,
                    crate::value::PropAttrs {
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    },
                );
        }
        let prototype = data_view_prototype(object_prototype.clone(), namespace.clone());
        super::set_builtin_constructor_prototype(e, &namespace, prototype);
    }

    let typed_array_prototype = typed_array_prototype(object_prototype.clone());
    let abstract_constructor =
        Value::object(vec![("prototype".into(), typed_array_prototype.clone())]);
    super::make_callable(
        &abstract_constructor,
        abstract_typed_array_constructor,
        None,
    );
    super::set_builtin_constructor_prototype(
        e,
        &abstract_constructor,
        typed_array_prototype.clone(),
    );
    abstract_constructor
        .set_prop("name".into(), Value::String("TypedArray".into()))
        .expect("typed-array name");
    abstract_constructor
        .set_prop("length".into(), Value::Number(0.0))
        .expect("typed-array length");
    if let Value::Object { props } = &abstract_constructor {
        for name in ["name", "length"] {
            props.meta.borrow_mut().set_attrs(
                name,
                crate::value::PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
    }
    // Concrete constructors and user subclasses inherit the same methods
    // from their realm's %TypedArray%; their receiver selects construction.
    for (name, length, callable) in [
        ("from", 3, typed_from as super::NativeFn),
        ("of", 0, typed_of as super::NativeFn),
    ] {
        abstract_constructor
            .set_prop(
                name.into(),
                super::native_method(
                    name,
                    length,
                    callable,
                    e.get("Function")
                        .and_then(|function| function.get_prop("prototype")),
                ),
            )
            .expect("TypedArray static method");
        if let Value::Object { props } = &abstract_constructor {
            props.meta.borrow_mut().set_attrs(
                name,
                crate::value::PropAttrs {
                    enumerable: false,
                    ..Default::default()
                },
            );
        }
    }
    let species = super::well_known("species").expect("Symbol.species");
    if let Value::Symbol(symbol) = &species {
        let key = crate::interpreter::symbol_slot_key(symbol);
        super::object::define_property(
            &abstract_constructor,
            &key,
            &Value::descriptor_record(vec![
                (
                    "get".into(),
                    super::native_method(
                        "get [Symbol.species]",
                        0,
                        buffer_species,
                        e.get("Function")
                            .and_then(|function| function.get_prop("prototype")),
                    ),
                ),
                ("configurable".into(), Value::Bool(true)),
            ]),
        )
        .expect("TypedArray species getter");
        if let Value::Object { props } = &abstract_constructor {
            props.meta.borrow_mut().set_symbol_key(&key, symbol.clone());
        }
    }
    typed_array_prototype
        .set_prop("constructor".into(), abstract_constructor.clone())
        .expect("typed-array prototype constructor");
    if let Value::Object { props } = &typed_array_prototype {
        props.meta.borrow_mut().set_attrs(
            "constructor",
            crate::value::PropAttrs {
                enumerable: false,
                ..Default::default()
            },
        );
    }
    for (name, kind) in KINDS {
        // The constructors are not in the pre-seeded global list, so declare
        // them here with their element size as a static.
        let namespace = Value::object(vec![
            ("name".into(), Value::String((*name).into())),
            ("length".into(), Value::Number(3.0)),
            (
                "BYTES_PER_ELEMENT".into(),
                Value::Number(kind.size() as f64),
            ),
        ]);
        if let Value::Object { props } = &namespace {
            for property in ["name", "length"] {
                props.meta.borrow_mut().set_attrs(
                    property,
                    crate::value::PropAttrs {
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
        }
        namespace
            .set_prop(
                KIND_SLOT.to_string(),
                Value::String((name.to_string()).into()),
            )
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
        super::make_callable(&namespace, super::require_new, Some(new_typed_array));
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
        if let Value::Object { props } = &namespace {
            props.set_proto(Some(Rc::new(abstract_constructor.clone())));
        }
        e.set(name, namespace);
    }
    encoding::install(e);
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
        "copyWithin",
        "with",
        "toReversed",
        "toSorted",
        "subarray",
        "slice",
        "fill",
        "at",
        "toString",
        "join",
        "toLocaleString",
        "map",
        "filter",
        "forEach",
        "reduce",
        "reduceRight",
        "findLast",
        "findLastIndex",
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
            .set_prop(
                slot.clone(),
                prototype.get_prop("values").expect("typed-array values"),
            )
            .expect("typed-array iterator method");
        if let Value::Object { props } = &prototype {
            props
                .meta
                .borrow_mut()
                .set_symbol_key(&slot, symbol.clone());
        }
    }
    for (name, getter) in [
        ("buffer", typed_buffer_getter as super::NativeFn),
        ("byteLength", typed_byte_length_getter),
        ("byteOffset", typed_byte_offset_getter),
        ("length", typed_length_getter),
    ] {
        super::object::define_property(
            &prototype,
            name,
            &Value::descriptor_record(vec![
                (
                    "get".into(),
                    super::native_method(&format!("get {name}"), 0, getter, None),
                ),
                ("configurable".into(), Value::Bool(true)),
            ]),
        )
        .unwrap();
    }
    if let Some(Value::Symbol(ref symbol)) = super::well_known("toStringTag") {
        let key = crate::interpreter::symbol_slot_key(symbol);
        super::object::define_property(
            &prototype,
            &key,
            &Value::descriptor_record(vec![
                (
                    "get".into(),
                    super::native_method("get [Symbol.toStringTag]", 0, typed_tag_getter, None),
                ),
                ("configurable".into(), Value::Bool(true)),
            ]),
        )
        .unwrap();
        prototype
            .property_cell()
            .unwrap()
            .meta
            .borrow_mut()
            .set_symbol_key(&key, symbol.clone());
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
        ("getFloat16", data_view_get_float16),
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
        ("setFloat16", data_view_set_float16),
        ("setFloat32", data_view_set_float32),
        ("setFloat64", data_view_set_float64),
        ("setBigInt64", data_view_set_bigint64),
        ("setBigUint64", data_view_set_biguint64),
    ];
    let mut properties = vec![("constructor".to_string(), constructor)];
    properties.extend(methods.iter().map(|(name, function)| {
        (
            name.to_string(),
            super::native_method(
                name,
                if name.starts_with("get") { 1 } else { 2 },
                *function,
                None,
            ),
        )
    }));
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
    for (key, f) in [
        ("buffer", data_view_buffer as super::NativeFn),
        ("byteLength", data_view_byte_length),
        ("byteOffset", data_view_byte_offset),
    ] {
        super::object::define_property(
            &prototype,
            key,
            &Value::descriptor_record(vec![
                (
                    "get".into(),
                    super::native_method(&format!("get {key}"), 0, f, None),
                ),
                ("configurable".into(), Value::Bool(true)),
            ]),
        )
        .unwrap();
    }
    if let Some(Value::Symbol(ref symbol)) = super::well_known("toStringTag") {
        let key = crate::interpreter::symbol_slot_key(symbol);
        prototype
            .set_prop(key.clone(), Value::String("DataView".into()))
            .unwrap();
        let cell = prototype.property_cell().unwrap();
        let mut meta = cell.meta.borrow_mut();
        meta.set_symbol_key(&key, symbol.clone());
        meta.set_attrs(
            &key,
            crate::value::PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        );
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

fn constructor_index(interp: &mut Interpreter, value: Option<&Value>) -> Result<usize, VmErr> {
    let number = interp.ecmascript_to_number(value.unwrap_or(&Value::Undefined))?;
    let integer = if number.is_nan() { 0.0 } else { number.trunc() };
    if !integer.is_finite()
        || integer < 0.0
        || integer > 9_007_199_254_740_991.0
        || integer > usize::MAX as f64
    {
        return Err(range_err("Invalid buffer index"));
    }
    Ok(integer as usize)
}

fn new_array_buffer(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let length = constructor_index(interp, a.first())?;
    let maximum = buffer_maximum_option(interp, a.get(1))?;
    if maximum.is_some_and(|maximum| maximum < length) {
        return Err(range_err("Invalid ArrayBuffer maximum length"));
    }
    let prototype = buffer_constructor_prototype(interp, "ArrayBuffer")?;
    if maximum.is_some_and(|maximum| maximum > crate::value::MAX_ARRAY_LEN * 8) {
        return Err(range_err("Invalid buffer maximum length"));
    }
    let buffer = match maximum {
        Some(maximum) => Buffer::resizable(length, maximum)
            .ok_or_else(|| range_err("Invalid ArrayBuffer length"))?,
        None => new_buffer(length)?,
    };
    let value = Value::ArrayBuffer(buffer);
    value
        .property_cell()
        .expect("ArrayBuffer properties")
        .set_proto(prototype);
    Ok(value)
}

fn buffer_constructor_prototype(
    interp: &mut Interpreter,
    name: &str,
) -> Result<Option<Rc<Value>>, VmErr> {
    let constructor = interp
        .new_target_stack
        .last()
        .cloned()
        .or_else(|| interp.persistent_global.borrow().intrinsic(name))
        .ok_or_else(|| VmErr::Msg("TypeError: buffer constructor is unavailable".into()))?;
    interp.constructor_prototype(&constructor, name)
}

fn buffer_maximum_option(
    interp: &mut Interpreter,
    options: Option<&Value>,
) -> Result<Option<usize>, VmErr> {
    let Some(options) = options.filter(|value| crate::interpreter::call::is_js_object(value))
    else {
        return Ok(None);
    };
    let maximum = interp.get_prop_value_str(options, "maxByteLength")?;
    if matches!(maximum, Value::Undefined) {
        return Ok(None);
    }
    let maximum = constructor_index(interp, Some(&maximum))?;
    Ok(Some(maximum))
}

fn require_array_buffer(this: &Value) -> Result<&Buffer, VmErr> {
    match this {
        Value::ArrayBuffer(buffer) => Ok(buffer),
        _ => Err(VmErr::Msg(
            "TypeError: incompatible ArrayBuffer receiver".into(),
        )),
    }
}
fn array_byte_length(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(
        require_array_buffer(&this)?.borrow().len() as f64
    ))
}
fn array_maximum_length(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(
        require_array_buffer(&this)?.maximum_length() as f64
    ))
}
fn array_resizable(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(require_array_buffer(&this)?.is_resizable()))
}
fn array_detached(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(require_array_buffer(&this)?.is_detached()))
}
fn array_buffer_resize(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let buffer = require_array_buffer(&this)?;
    if !buffer.is_resizable() {
        return Err(VmErr::Msg("TypeError: ArrayBuffer is not resizable".into()));
    }
    let length = constructor_index(interp, args.first())?;
    buffer.resize(length)?;
    Ok(Value::Undefined)
}

fn new_shared_array_buffer(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let length = constructor_index(interp, a.first())?;
    let maximum = buffer_maximum_option(interp, a.get(1))?;
    if maximum.is_some_and(|maximum| maximum < length) {
        return Err(range_err("Invalid shared array buffer maximum length"));
    }
    let prototype = buffer_constructor_prototype(interp, "SharedArrayBuffer")?;
    if length > crate::value::MAX_ARRAY_LEN * 8 {
        return Err(range_err("Invalid shared array buffer length"));
    }
    if maximum.is_some_and(|maximum| maximum > crate::value::MAX_ARRAY_LEN * 8) {
        return Err(range_err("Invalid shared array buffer maximum length"));
    }
    let buffer = SharedBuffer::zeroed_with_maximum(length, maximum)
        .ok_or_else(|| range_err("Invalid shared array buffer length"))?;
    let value = Value::SharedArrayBuffer(buffer);
    value
        .property_cell()
        .expect("SharedArrayBuffer properties")
        .set_proto(prototype);
    Ok(value)
}

fn require_shared(this: &Value) -> Result<&SharedBuffer, VmErr> {
    match this {
        Value::SharedArrayBuffer(buffer) => Ok(buffer),
        _ => Err(VmErr::Msg(
            "TypeError: incompatible SharedArrayBuffer receiver".into(),
        )),
    }
}
fn shared_byte_length(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(require_shared(&this)?.len() as f64))
}
fn shared_maximum_length(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(require_shared(&this)?.maximum_length() as f64))
}
fn shared_growable(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(require_shared(&this)?.is_growable()))
}

fn shared_array_buffer_grow(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let Value::SharedArrayBuffer(buffer) = &this else {
        return Err(VmErr::Msg(
            "TypeError: incompatible SharedArrayBuffer grow receiver".into(),
        ));
    };
    if !buffer.is_growable() {
        return Err(VmErr::Msg(
            "TypeError: SharedArrayBuffer is not growable".into(),
        ));
    }
    let length = constructor_index(interp, args.first())?;
    buffer.grow(length).map_err(|error| match error {
        crate::value::SharedGrowError::NotGrowable => {
            VmErr::Msg("TypeError: SharedArrayBuffer is not growable".into())
        }
        crate::value::SharedGrowError::InvalidLength => {
            range_err("Invalid shared array buffer growth length")
        }
    })?;
    Ok(Value::Undefined)
}

/// A scheduling hint, never a wait or a guest-budget refill. Validation does
/// not coerce the optional iteration number or invoke guest callbacks.
fn atomics_pause(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    if let Some(value) = args.first()
        && !matches!(value, Value::Undefined)
        && !matches!(value, Value::Number(number) if number.is_finite() && *number >= 0.0 && number.fract() == 0.0)
    {
        return Err(VmErr::Msg(
            "TypeError: Atomics.pause iteration number must be a nonnegative integer Number".into(),
        ));
    }
    interp.check_execution_interrupt()?;
    std::hint::spin_loop();
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::yield_now();
    Ok(Value::Undefined)
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
    let size = interp.ecmascript_to_number(value)?.trunc();
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

fn atomics_wait(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let (kind, shared, offset) = atomics_wait_location(interp, &args, true)?;
    let shared = shared.expect("shared wait location");
    let expected = atomics_argument_bits(interp, kind, args.get(2).unwrap_or(&Value::Undefined))?;
    let timeout = atomics_timeout(interp, &args, 3)?;
    if !interp.can_block() {
        return Err(VmErr::Msg("TypeError: This agent cannot suspend".into()));
    }
    let result = shared
        .wait_interruptible(offset, kind.size(), expected, timeout, || {
            interp.check_execution_interrupt().is_err()
        })
        .ok_or_else(|| {
            VmErr::Msg("TypeError: blocking wait is unavailable for this backing store".into())
        })?;
    Ok(Value::String(
        match result {
            crate::value::SharedWaitResult::Cancelled => {
                interp.check_execution_interrupt()?;
                return Err(VmErr::Msg("Error: Guest execution interrupted".into()));
            }
            crate::value::SharedWaitResult::NotEqual => "not-equal",
            crate::value::SharedWaitResult::TimedOut => "timed-out",
            crate::value::SharedWaitResult::Ok => "ok",
        }
        .into(),
    ))
}

fn atomics_wait_async(
    interp: &mut Interpreter,
    _: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let (kind, shared, offset) = atomics_wait_location(interp, &args, true)?;
    let shared = shared.expect("shared wait location");
    let expected = atomics_argument_bits(interp, kind, args.get(2).unwrap_or(&Value::Undefined))?;
    let timeout = atomics_timeout(interp, &args, 3)?;
    let native = match shared.register_wait(offset, kind.size(), expected, timeout) {
        Some(Ok(native)) => {
            native.set_owner_wake(&interp.owner_wake_signal());
            Some(native)
        }
        Some(Err(result)) => {
            return Ok(atomics_wait_result(
                false,
                Value::String(
                    match result {
                        crate::value::SharedWaitResult::NotEqual => "not-equal",
                        _ => "timed-out",
                    }
                    .into(),
                ),
            ));
        }
        None => {
            let observed = shared.atomic_load(offset, kind.size()).ok_or_else(|| {
                VmErr::Msg(
                    "TypeError: atomic access is unaligned or unavailable on this target".into(),
                )
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
            None
        }
    };

    if timeout.is_finite() {
        interp.jobs.borrow().check_timer_capacity()?;
    }

    let promise = Value::pending_promise();
    let key = (shared.wait_identity(), offset);
    let waiter_id = interp.jobs.borrow_mut().register_shared_atomics_waiter(
        key,
        promise.clone(),
        shared.clone(),
        native,
    );
    if timeout.is_finite() {
        interp.jobs.borrow_mut().push_timer_job(
            timeout,
            crate::interpreter::Job::AtomicsWaitTimeout { key, waiter_id },
        );
    }
    Ok(atomics_wait_result(true, Value::Promise(promise)))
}

fn atomics_notify(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let (kind, shared, offset) = atomics_wait_location(interp, &args, false)?;
    let count = match args.get(2) {
        None | Some(Value::Undefined) => usize::MAX,
        Some(Value::BigInt(_) | Value::Symbol(_)) => {
            return Err(VmErr::Msg(
                "TypeError: cannot convert value to an Atomics notify count".into(),
            ));
        }
        Some(value) => {
            let count = interp.ecmascript_to_number(value)?;
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
    let Some(shared) = shared else {
        return Ok(Value::Number(0.0));
    };
    let waiters = interp
        .jobs
        .borrow_mut()
        .take_atomics_waiters((shared.wait_identity(), offset), count);
    let notified = waiters.len() + shared.notify(offset, count.saturating_sub(waiters.len()));
    for promise in waiters {
        crate::interpreter::jobs::settle(
            &interp.jobs,
            &promise,
            PromiseState::Fulfilled,
            Value::String("ok".into()),
        );
    }
    crate::interpreter::jobs::settle_notified_atomics_waiters(&interp.jobs);
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
    require_shared: bool,
) -> Result<(TypedKind, Option<SharedBuffer>, usize), VmErr> {
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
    let shared = match &view.buffer {
        BufferBacking::Shared(shared) => Some(shared.clone()),
        _ if require_shared || view.is_out_of_bounds() => {
            return Err(VmErr::Msg(
                "TypeError: Atomics wait requires a SharedArrayBuffer".into(),
            ));
        }
        _ => None,
    };
    let length = view.effective_length();
    let index = constructor_index(interp, args.get(1))?;
    if index >= length {
        return Err(range_err("Atomics index is outside the typed array"));
    }

    let offset = view
        .effective_byte_offset()
        .checked_add(index * view.kind.size())
        .ok_or_else(|| range_err("Atomics index is outside the typed array"))?;
    Ok((view.kind, shared, offset))
}

fn atomics_timeout(interp: &mut Interpreter, args: &[Value], index: usize) -> Result<f64, VmErr> {
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
    let timeout = interp.ecmascript_to_number(value)?;
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
    if !matches!(method, AtomicsMethod::Load) {
        require_writable(&view.buffer)?;
    }
    if view.is_out_of_bounds() {
        return Err(VmErr::Msg("TypeError: Detached buffer".into()));
    }
    let length = view.effective_length();
    let index = constructor_index(interp, args.get(1))?;
    if index >= length {
        return Err(range_err("Atomics index is outside the typed array"));
    }
    let width = view.kind.size();
    let offset = view.effective_byte_offset() + index * width;
    let result = match method {
        AtomicsMethod::Load => atomics_load_bits(&view.buffer, offset, width),
        AtomicsMethod::Store => {
            let converted = atomics_coerce_argument(
                interp,
                view.kind,
                args.get(2).unwrap_or(&Value::Undefined),
            )?;
            let value = atomics_argument_bits(interp, view.kind, &converted)?;
            if !atomics_store_bits(&view.buffer, offset, width, value) {
                return Err(VmErr::Msg(
                    "TypeError: atomic access is unaligned or unavailable on this target".into(),
                ));
            }
            return Ok(converted);
        }
        operation => {
            let value =
                atomics_argument_bits(interp, view.kind, args.get(2).unwrap_or(&Value::Undefined))?;
            let replacement = if matches!(operation, AtomicsMethod::CompareExchange) {
                atomics_argument_bits(interp, view.kind, args.get(3).unwrap_or(&Value::Undefined))?
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

fn atomics_coerce_argument(
    interp: &mut Interpreter,
    kind: TypedKind,
    value: &Value,
) -> Result<Value, VmErr> {
    if matches!(kind, TypedKind::BigInt64 | TypedKind::BigUint64) {
        interp.ecmascript_to_bigint(value)
    } else {
        Ok(Value::Number(crate::value::to_integer_or_infinity(
            interp.ecmascript_to_number(value)?,
        )))
    }
}

fn atomics_argument_bits(
    interp: &mut Interpreter,
    kind: TypedKind,
    value: &Value,
) -> Result<u64, VmErr> {
    let converted = atomics_coerce_argument(interp, kind, value)?;
    if let Value::BigInt(bigint) = &converted {
        return bigint
            .as_n_bit(64, false)
            .map_err(VmErr::Msg)?
            .to_decimal()
            .parse()
            .map_err(|_| VmErr::Msg("RangeError: invalid Atomics BigInt value".into()));
    }
    let bits = to_int(converted.to_number()) as u32 as u64;
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
            let length = constructor_index(interp, Some(&Value::Number(*n)))?;
            allocate_typed(kind, length)
        }
        Some(Value::ArrayBuffer(buffer)) => {
            let byte_offset = constructor_index(interp, a.get(1))?;
            if !byte_offset.is_multiple_of(size) {
                return Err(range_err("Start offset is not aligned"));
            }
            let explicit_length = match a.get(2) {
                Some(Value::Undefined) | None => None,
                Some(value) => Some(constructor_index(interp, Some(value))?),
            };
            if buffer.is_detached() {
                return Err(VmErr::Msg(
                    "TypeError: Cannot construct a typed array from a detached ArrayBuffer".into(),
                ));
            }
            let available = buffer.borrow().len();
            if byte_offset > available {
                return Err(range_err("Start offset is outside the buffer"));
            }
            let length = explicit_length.unwrap_or((available - byte_offset) / size);
            if length
                .checked_mul(size)
                .and_then(|length| byte_offset.checked_add(length))
                .is_none_or(|end| end > available)
            {
                return Err(range_err("Invalid typed array length"));
            }
            let length_tracking = buffer.is_resizable()
                && a.get(2)
                    .is_none_or(|length| matches!(length, Value::Undefined));
            if !length_tracking
                && a.get(2)
                    .is_none_or(|length| matches!(length, Value::Undefined))
                && !(available - byte_offset).is_multiple_of(size)
            {
                return Err(range_err("Invalid typed array buffer length"));
            }
            Ok(typed_with_tracking(
                kind,
                buffer.clone().into(),
                byte_offset,
                length,
                false,
                length_tracking,
            ))
        }
        Some(Value::SharedArrayBuffer(buffer)) => {
            let byte_offset = constructor_index(interp, a.get(1))?;
            if !byte_offset.is_multiple_of(size) {
                return Err(range_err("Start offset is not aligned"));
            }
            let explicit_length = match a.get(2) {
                Some(Value::Undefined) | None => None,
                Some(value) => Some(constructor_index(interp, Some(value))?),
            };
            let available = buffer.len();
            if byte_offset > available {
                return Err(range_err("Start offset is outside the buffer"));
            }
            let length = explicit_length.unwrap_or((available - byte_offset) / size);
            if length
                .checked_mul(size)
                .and_then(|length| byte_offset.checked_add(length))
                .is_none_or(|end| end > available)
            {
                return Err(range_err("Invalid typed array length"));
            }
            let length_tracking = buffer.is_growable()
                && a.get(2)
                    .is_none_or(|length| matches!(length, Value::Undefined));
            if !length_tracking
                && a.get(2)
                    .is_none_or(|length| matches!(length, Value::Undefined))
                && !(available - byte_offset).is_multiple_of(size)
            {
                return Err(range_err("Invalid typed array buffer length"));
            }
            Ok(typed_with_tracking(
                kind,
                buffer.clone().into(),
                byte_offset,
                length,
                false,
                length_tracking,
            ))
        }
        Some(source) if !crate::interpreter::call::is_js_object(source) => {
            let length = constructor_index(interp, Some(source))?;
            allocate_typed(kind, length)
        }
        Some(Value::TypedArray(source)) => {
            if source.is_out_of_bounds() || is_bigint_kind(source.kind) != is_bigint_kind(kind) {
                return Err(VmErr::Msg(
                    "TypeError: incompatible typed array source".into(),
                ));
            }
            typed_from_items(interp, kind, read_all(source))
        }
        Some(source) => {
            let key = super::well_known("iterator").expect("Symbol.iterator");
            if let Some(method) = interp.get_method(source, &key)? {
                let iterator = interp.iterator_from_method(source, &method)?;
                let items = interp.drain_iterator(&iterator)?;
                return typed_from_items(interp, kind, items);
            }
            let length = interp.member(source, "length")?;
            let number = interp.ecmascript_to_number(&length)?;
            let length = if number.is_nan() || number <= 0.0 {
                0.0
            } else {
                number.floor()
            };
            if length > crate::value::MAX_ARRAY_LEN as f64 {
                return Err(range_err("Invalid typed array length"));
            }
            let result = allocate_typed(kind, length as usize)?;
            let Value::TypedArray(view) = &result else {
                unreachable!()
            };
            for index in 0..length as usize {
                interp.check_execution()?;
                let value = interp.member(source, &index.to_string())?;
                write_element_in(interp, view, index, &value)?;
            }
            Ok(result)
        }
    }
}

fn is_bigint_kind(kind: TypedKind) -> bool {
    matches!(kind, TypedKind::BigInt64 | TypedKind::BigUint64)
}

fn allocate_typed(kind: TypedKind, length: usize) -> Result<Value, VmErr> {
    let bytes = length
        .checked_mul(kind.size())
        .ok_or_else(|| range_err("Invalid typed array length"))?;
    Ok(typed(kind, new_buffer(bytes)?, 0, length))
}

fn typed_from_items(
    interp: &mut Interpreter,
    kind: TypedKind,
    items: Vec<Value>,
) -> Result<Value, VmErr> {
    let result = allocate_typed(kind, items.len())?;
    let Value::TypedArray(view) = &result else {
        unreachable!()
    };
    for (index, value) in items.iter().enumerate() {
        write_element_in(interp, view, index, value)?;
    }
    Ok(result)
}

pub(crate) fn write_element_in(
    interp: &mut Interpreter,
    view: &Rc<TypedArrayData>,
    index: usize,
    value: &Value,
) -> Result<(), VmErr> {
    let converted = if is_bigint_kind(view.kind) {
        interp.ecmascript_to_bigint(value)?
    } else {
        Value::Number(interp.ecmascript_to_number(value)?)
    };
    write_element(view, index, &converted)
}

/// CanonicalNumericIndexString excludes ordinary numeric-looking names.
pub(crate) fn canonical_numeric_index(key: &str) -> Option<f64> {
    if key == "-0" {
        return Some(-0.0);
    }
    let number = key.parse::<f64>().ok()?;
    (crate::format::ecmascript_number_string(number) == key).then_some(number)
}

pub(crate) fn valid_integer_index(view: &TypedArrayData, index: f64) -> bool {
    index.is_finite()
        && index >= 0.0
        && !(index == 0.0 && index.is_sign_negative())
        && index.fract() == 0.0
        && index < view.effective_length() as f64
}

/// Conversion precedes validation when the receiver is the typed array itself.
pub(crate) fn set_integer_index_in(
    interp: &mut Interpreter,
    view: &Rc<TypedArrayData>,
    index: f64,
    value: &Value,
) -> Result<(), VmErr> {
    require_writable(&view.buffer)?;
    let converted = if is_bigint_kind(view.kind) {
        interp.ecmascript_to_bigint(value)?
    } else {
        Value::Number(interp.ecmascript_to_number(value)?)
    };
    if valid_integer_index(view, index) {
        write_element(view, index as usize, &converted)?;
    }
    Ok(())
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
    typed_with_tracking(kind, buffer.into(), byte_offset, length, is_buffer, false)
}

fn typed_with_tracking(
    kind: TypedKind,
    buffer: BufferBacking,
    byte_offset: usize,
    length: usize,
    is_buffer: bool,
    length_tracking: bool,
) -> Value {
    Value::TypedArray(Rc::new(TypedArrayData {
        length_tracking,
        properties: Value::instance_properties(),
        kind,
        buffer,
        byte_offset,
        length,
        is_buffer,
    }))
}

/// TypedArrayCreate validates the actual constructor result before any
/// element writes. Static factories and species allocation share this path.
fn typed_array_create(
    interp: &mut Interpreter,
    constructor: &Value,
    arguments: Vec<Value>,
    minimum_length: Option<usize>,
    writable: bool,
) -> Result<Value, VmErr> {
    if !super::is_constructor(constructor) {
        return Err(VmErr::Msg(
            "TypeError: TypedArray constructor must be constructible".into(),
        ));
    }
    let result = interp.ctor(constructor, arguments)?;
    let view = require(&result)?;
    if writable {
        require_writable(&view.buffer)?;
    }
    if view.is_out_of_bounds()
        || minimum_length.is_some_and(|minimum| view.effective_length() < minimum)
    {
        return Err(VmErr::Msg(
            "TypeError: TypedArray constructor returned an invalid or short view".into(),
        ));
    }
    Ok(result)
}

fn typed_species_create(
    interp: &mut Interpreter,
    original: &Value,
    arguments: Vec<Value>,
    minimum_length: Option<usize>,
    writable: bool,
) -> Result<Value, VmErr> {
    let source = require(original)?;
    let constructor = interp.species_constructor(original, source.kind.name())?;
    let result = typed_array_create(interp, &constructor, arguments, minimum_length, writable)?;
    if is_bigint_kind(source.kind) != is_bigint_kind(require(&result)?.kind) {
        return Err(VmErr::Msg(
            "TypeError: TypedArray species has a different content type".into(),
        ));
    }
    Ok(result)
}

fn typed_of(
    interp: &mut Interpreter,
    constructor: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let length = arguments.len();
    let result = typed_array_create(
        interp,
        &constructor,
        vec![Value::Number(length as f64)],
        Some(length),
        true,
    )?;
    let view = require(&result)?;
    for (index, value) in arguments.iter().enumerate() {
        write_element_in(interp, &view, index, value)?;
    }
    Ok(result)
}

fn typed_from(
    interp: &mut Interpreter,
    constructor: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    if !super::is_constructor(&constructor) {
        return Err(VmErr::Msg(
            "TypeError: TypedArray.from receiver must be a constructor".into(),
        ));
    }
    let mapper = arguments
        .get(1)
        .filter(|value| !matches!(value, Value::Undefined))
        .cloned();
    if mapper
        .as_ref()
        .is_some_and(|mapper| !crate::interpreter::call::is_callable_value(mapper))
    {
        return Err(VmErr::Msg(
            "TypeError: TypedArray.from mapper must be callable".into(),
        ));
    }
    let receiver = arguments.get(2).cloned().unwrap_or(Value::Undefined);
    let source = arguments.first().cloned().unwrap_or(Value::Undefined);
    let method = interp.get_method(
        &source,
        &super::well_known("iterator").expect("Symbol.iterator"),
    )?;
    let items = if let Some(method) = method {
        let iterator = interp.iterator_from_method(&source, &method)?;
        Some(interp.drain_iterator(&iterator)?)
    } else {
        None
    };
    let length = if let Some(items) = &items {
        items.len()
    } else {
        super::array::array_like_length(interp, &source)?
    };
    let result = typed_array_create(
        interp,
        &constructor,
        vec![Value::Number(length as f64)],
        Some(length),
        true,
    )?;
    let view = require(&result)?;
    for index in 0..length {
        interp.consume_loop()?;
        let value = if let Some(items) = &items {
            items[index].clone()
        } else {
            interp.get_prop_value_str(&source, &index.to_string())?
        };
        let value = if let Some(mapper) = &mapper {
            interp.call_this(
                mapper,
                receiver.clone(),
                vec![value, Value::Number(index as f64)],
            )?
        } else {
            value
        };
        write_element_in(interp, &view, index, &value)?;
    }
    Ok(result)
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
    require_writable(&view.buffer)?;
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
    crate::value::to_int32(value)
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

fn validated_typed_source(this: &Value) -> Result<Rc<TypedArrayData>, VmErr> {
    let view = require(this)?;
    if view.is_out_of_bounds() {
        return Err(VmErr::Msg(
            "TypeError: TypedArray is detached or out of bounds".into(),
        ));
    }
    Ok(view)
}

fn typed_map(interp: &mut Interpreter, this: Value, arguments: Vec<Value>) -> Result<Value, VmErr> {
    let source = validated_typed_source(&this)?;
    let length = source.effective_length();
    let callback = super::array::callback(&arguments)?;
    let receiver = arguments.get(1).cloned().unwrap_or(Value::Undefined);
    let result = typed_species_create(
        interp,
        &this,
        vec![Value::Number(length as f64)],
        Some(length),
        true,
    )?;
    let destination = require(&result)?;
    for index in 0..length {
        interp.consume_loop()?;
        let value = read_element(&source, index).unwrap_or(Value::Undefined);
        let mapped = interp.call_this(
            &callback,
            receiver.clone(),
            vec![value, Value::Number(index as f64), this.clone()],
        )?;
        write_element_in(interp, &destination, index, &mapped)?;
    }
    Ok(result)
}

fn typed_filter(
    interp: &mut Interpreter,
    this: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let source = validated_typed_source(&this)?;
    let length = source.effective_length();
    let callback = super::array::callback(&arguments)?;
    let receiver = arguments.get(1).cloned().unwrap_or(Value::Undefined);
    let mut kept = Vec::new();
    for index in 0..length {
        interp.consume_loop()?;
        let value = read_element(&source, index).unwrap_or(Value::Undefined);
        if interp
            .call_this(
                &callback,
                receiver.clone(),
                vec![value.clone(), Value::Number(index as f64), this.clone()],
            )?
            .is_truthy()
        {
            kept.push(value);
        }
    }
    let result = typed_species_create(
        interp,
        &this,
        vec![Value::Number(kept.len() as f64)],
        Some(kept.len()),
        true,
    )?;
    let destination = require(&result)?;
    for (index, value) in kept.iter().enumerate() {
        write_element_in(interp, &destination, index, value)?;
    }
    Ok(result)
}
fn typed_for_each(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_callback(i, t, a, "forEach")
}
fn typed_reduce(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_callback(i, t, a, "reduce")
}
fn typed_some(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_callback(i, t, a, "some")
}
fn typed_every(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_callback(i, t, a, "every")
}
fn typed_find(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_callback(i, t, a, "find")
}
fn typed_find_index(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_callback(i, t, a, "findIndex")
}
fn typed_index_of(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_search(i, t, a, "indexOf")
}
fn typed_last_index_of(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_search(i, t, a, "lastIndexOf")
}
fn typed_includes(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_search(i, t, a, "includes")
}

fn typed_iterator(this: Value, kind: &str) -> Result<Value, VmErr> {
    let view = require(&this)?;
    if view.is_out_of_bounds() {
        return Err(VmErr::Msg(
            "TypeError: Cannot iterate a detached typed array".into(),
        ));
    }
    crate::interpreter::array_iter_with_kind(this, kind)
}
fn typed_keys(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    typed_iterator(this, "keys")
}
fn typed_values(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    typed_iterator(this, "values")
}
fn typed_entries(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    typed_iterator(this, "entries")
}

fn typed_prototype_method(name: &str) -> Value {
    let callable: super::NativeFn = match name {
        "set" => typed_set,
        "copyWithin" => typed_copy_within,
        "with" => typed_with,
        "toReversed" => typed_to_reversed,
        "toSorted" => typed_to_sorted,
        "subarray" => typed_subarray,
        "slice" => typed_slice,
        "fill" => typed_fill,
        "at" => typed_at,
        "toString" | "join" => typed_join,
        "toLocaleString" => typed_to_locale_string,
        "map" => typed_map,
        "filter" => typed_filter,
        "forEach" => typed_for_each,
        "reduce" => typed_reduce,
        "reduceRight" => typed_reduce_right,
        "findLast" => typed_find_last,
        "findLastIndex" => typed_find_last_index,
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
    super::native_method(
        name,
        match name {
            "keys" | "values" | "entries" | "reverse" | "toString" | "toReversed"
            | "toLocaleString" => 0,
            "subarray" | "slice" | "copyWithin" | "with" => 2,
            _ => 1,
        },
        callable,
        None,
    )
}

fn typed_at(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = validated_typed_source(&this)?;
    let length = view.effective_length();
    let index = crate::value::to_integer_or_infinity(
        i.ecmascript_to_number(a.first().unwrap_or(&Value::Undefined))?,
    );
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

fn typed_to_locale_string(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = validated_typed_source(&this)?;
    let length = v.effective_length();
    let mut out = crate::JsString::default();
    for k in 0..length {
        i.consume_loop()?;
        if k > 0 {
            out.push_str(",");
        }
        let value = read_element(&v, k).unwrap_or(Value::Undefined);
        if !matches!(value, Value::Undefined | Value::Null) {
            let method = i.get_prop_value_str(&value, "toLocaleString")?;
            let result = i.call_this(
                &method,
                value,
                vec![
                    a.first().cloned().unwrap_or(Value::Undefined),
                    a.get(1).cloned().unwrap_or(Value::Undefined),
                ],
            )?;
            out.push_str(i.ecmascript_to_string(&result)?);
        }
        if out.len() > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
    }
    Value::checked_string(out)
}
fn typed_join(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = validated_typed_source(&this)?;
    let length = view.effective_length();
    let separator = match a.first() {
        Some(Value::Undefined) | None => crate::JsString::from(","),
        Some(v) => interp.ecmascript_to_string(v)?,
    };
    let mut result = crate::JsString::default();
    for index in 0..length {
        interp.check_execution()?;
        let value = read_element(&view, index).unwrap_or(Value::Undefined);
        let part = if matches!(value, Value::Undefined | Value::Null) {
            crate::JsString::default()
        } else {
            interp.ecmascript_to_string(&value)?
        };
        let added = part
            .len()
            .saturating_add(if index > 0 { separator.len() } else { 0 });
        if result.len().saturating_add(added) > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
        if index > 0 {
            result.push_str(&separator);
        }
        result.push_str(&part);
    }
    Value::checked_string(result)
}

/// `set(source, offset)`: copy elements in, converting as needed.
fn typed_set(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = require(&t)?;
    require_writable(&v.buffer)?;
    let offset = crate::value::to_integer_or_infinity(
        i.ecmascript_to_number(a.get(1).unwrap_or(&Value::Undefined))?,
    );
    if offset < 0. {
        return Err(range_err("Invalid offset"));
    }
    if v.is_out_of_bounds() {
        return Err(VmErr::Msg("TypeError: TypedArray out of bounds".into()));
    }
    let len = v.effective_length();
    let source = a.first().cloned().unwrap_or(Value::Undefined);
    if let Value::TypedArray(ref other) = source {
        if other.is_out_of_bounds() {
            return Err(VmErr::Msg("TypeError: source out of bounds".into()));
        }
        let count = other.effective_length();
        if offset + count as f64 > len as f64 {
            return Err(range_err("Source is too large"));
        }
        if is_bigint_kind(v.kind) != is_bigint_kind(other.kind) {
            return Err(VmErr::Msg(
                "TypeError: incompatible typed array content types".into(),
            ));
        }
        if v.kind == other.kind {
            let bytes = other
                .buffer
                .read(other.byte_offset, count * other.kind.size())
                .unwrap();
            v.buffer
                .write(v.byte_offset + offset as usize * v.kind.size(), &bytes);
        } else {
            let values = read_all(other);
            for (k, x) in values.iter().enumerate() {
                i.consume_loop()?;
                write_element_in(i, &v, offset as usize + k, x)?;
            }
        }
        return Ok(Value::Undefined);
    }
    let object = super::object::to_object_receiver(&source)?;
    let l = i.get_prop_value_str(&object, "length")?;
    let count = crate::value::to_integer_or_infinity(i.ecmascript_to_number(&l)?)
        .clamp(0., 9_007_199_254_740_991.);
    if offset + count > len as f64 {
        return Err(range_err("Source is too large"));
    }
    for k in 0..count as usize {
        i.consume_loop()?;
        let x = i.get_prop_value_str(&object, &k.to_string())?;
        write_element_in(i, &v, offset as usize + k, &x)?;
    }
    Ok(Value::Undefined)
}

fn window_in(
    interp: &mut Interpreter,
    length: usize,
    arguments: &[Value],
) -> Result<(usize, usize), VmErr> {
    let start = super::array::relative_index(
        interp.ecmascript_to_number(arguments.first().unwrap_or(&Value::Undefined))?,
        length,
    );
    let end = match arguments.get(1) {
        None | Some(Value::Undefined) => length,
        Some(value) => super::array::relative_index(interp.ecmascript_to_number(value)?, length),
    };
    Ok((start, end.max(start)))
}

/// subarray passes the original backing wrapper and offset to the species
/// constructor, omitting length when the source's view tracks buffer growth.
fn typed_subarray(
    interp: &mut Interpreter,
    this: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let source = require(&this)?;
    let (start, end) = window_in(interp, source.effective_length(), &arguments)?;
    let mut constructor_arguments = vec![
        source.buffer.to_value(),
        Value::Number((source.byte_offset + start * source.kind.size()) as f64),
    ];
    if !source.length_tracking
        || arguments
            .get(1)
            .is_some_and(|end| !matches!(end, Value::Undefined))
    {
        constructor_arguments.push(Value::Number((end - start) as f64));
    }
    typed_species_create(interp, &this, constructor_arguments, None, false)
}

fn typed_slice(
    interp: &mut Interpreter,
    this: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let source = validated_typed_source(&this)?;
    let (start, end) = window_in(interp, source.effective_length(), &arguments)?;
    let count = end - start;
    let result = typed_species_create(
        interp,
        &this,
        vec![Value::Number(count as f64)],
        Some(count),
        true,
    )?;
    if count == 0 {
        return Ok(result);
    }
    if source.is_out_of_bounds() {
        return Err(VmErr::Msg(
            "TypeError: TypedArray source became detached or out of bounds".into(),
        ));
    }
    let destination = require(&result)?;
    let count = count.min(source.effective_length().saturating_sub(start));
    if source.kind == destination.kind {
        let bytes = source
            .buffer
            .read(
                source.byte_offset + start * source.kind.size(),
                count * source.kind.size(),
            )
            .ok_or_else(|| VmErr::Msg("TypeError: TypedArray source became detached".into()))?;
        destination.buffer.write(destination.byte_offset, &bytes);
    } else {
        let values = (start..start + count)
            .map(|index| read_element(&source, index).unwrap_or(Value::Undefined))
            .collect::<Vec<_>>();
        for (index, value) in values.iter().enumerate() {
            write_element_in(interp, &destination, index, value)?;
        }
    }
    Ok(result)
}

fn typed_fill(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = validated_typed_source(&this)?;
    require_writable(&view.buffer)?;
    let len = view.effective_length();
    let input = a.first().cloned().unwrap_or(Value::Undefined);
    let value = if is_bigint_kind(view.kind) {
        interp.ecmascript_to_bigint(&input)?
    } else {
        Value::Number(interp.ecmascript_to_number(&input)?)
    };
    let (start, end) = window_in(interp, len, &a[1.min(a.len())..])?;
    if view.is_out_of_bounds() {
        return Err(VmErr::Msg("TypeError: TypedArray out of bounds".into()));
    }
    for index in start..end {
        interp.consume_loop()?;
        write_element(&view, index, &value)?;
    }
    Ok(this)
}

// --- ArrayBuffer members ----------------------------------------------------

pub fn array_buffer_member(buffer: &Buffer, key: &str) -> Option<Value> {
    Some(match key {
        "byteLength" => Value::Number(buffer.borrow().len() as f64),
        "maxByteLength" => Value::Number(buffer.maximum_length() as f64),
        "resizable" => Value::Bool(buffer.is_resizable()),
        "detached" => Value::Bool(buffer.is_detached()),
        _ => return None,
    })
}

pub fn shared_array_buffer_member(buffer: &SharedBuffer, key: &str) -> Option<Value> {
    Some(match key {
        "byteLength" => Value::Number(buffer.len() as f64),
        "maxByteLength" => Value::Number(buffer.maximum_length() as f64),
        "growable" => Value::Bool(buffer.is_growable()),
        _ => return None,
    })
}

fn buffer_species(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(this)
}

fn array_buffer_slice(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    buffer_slice(interp, this, args, false)
}

fn shared_array_buffer_slice(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    buffer_slice(interp, this, args, true)
}

fn buffer_slice(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
    shared: bool,
) -> Result<Value, VmErr> {
    let source: BufferBacking = match (&this, shared) {
        (Value::ArrayBuffer(buffer), false) => buffer.clone().into(),
        (Value::SharedArrayBuffer(buffer), true) => buffer.clone().into(),
        _ => {
            return Err(VmErr::Msg(
                "TypeError: incompatible buffer slice receiver".into(),
            ));
        }
    };
    if source.is_detached() {
        return Err(VmErr::Msg("TypeError: detached buffer".into()));
    }
    let length = source.len();
    let relative = |number: f64| {
        let integer = crate::value::to_integer_or_infinity(number);
        if integer < 0.0 {
            (length as f64 + integer).max(0.0) as usize
        } else {
            integer.min(length as f64) as usize
        }
    };
    let start = relative(interp.ecmascript_to_number(args.first().unwrap_or(&Value::Undefined))?);
    let end = match args.get(1) {
        None | Some(Value::Undefined) => length,
        Some(value) => relative(interp.ecmascript_to_number(value)?),
    };
    let count = end.saturating_sub(start);
    let name = if shared {
        "SharedArrayBuffer"
    } else {
        "ArrayBuffer"
    };
    let constructor = interp.species_constructor(&this, name)?;
    let result = interp.ctor(&constructor, vec![Value::Number(count as f64)])?;
    let destination: BufferBacking = match (&result, shared) {
        (Value::ArrayBuffer(buffer), false) => buffer.clone().into(),
        (Value::SharedArrayBuffer(buffer), true) => buffer.clone().into(),
        _ => {
            return Err(VmErr::Msg(
                "TypeError: species returned an incompatible buffer".into(),
            ));
        }
    };
    let same = match (&source, &destination) {
        (BufferBacking::Shared(a), BufferBacking::Shared(b)) => {
            a.wait_identity() == b.wait_identity()
        }
        _ => source.identity() == destination.identity(),
    };
    if same || destination.is_detached() || destination.is_immutable() || destination.len() < count
    {
        return Err(VmErr::Msg("TypeError: invalid species buffer".into()));
    }
    if source.is_detached() {
        return Err(VmErr::Msg(
            "TypeError: source buffer detached during slice".into(),
        ));
    }
    let count = count.min(source.len().saturating_sub(start));
    if count > 0 {
        let bytes = source.read(start, count).expect("validated source range");
        if !destination.write(0, &bytes) {
            return Err(VmErr::Msg(
                "TypeError: species buffer changed during slice".into(),
            ));
        }
    }
    Ok(result)
}

fn new_data_view(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let Some(value @ (Value::ArrayBuffer(_) | Value::SharedArrayBuffer(_))) = a.first() else {
        return Err(VmErr::Msg(
            "TypeError: First argument to DataView constructor must be an ArrayBuffer".to_string(),
        ));
    };
    let byte_offset = constructor_index(interp, a.get(1))?;
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
    if byte_offset > available {
        return Err(range_err("Start offset is outside the buffer"));
    }
    let byte_length = match a.get(2) {
        Some(Value::Undefined) | None => available - byte_offset,
        Some(v) => constructor_index(interp, Some(v))?,
    };
    if byte_offset
        .checked_add(byte_length)
        .is_none_or(|end| end > available)
    {
        return Err(range_err("Invalid DataView length"));
    }
    let prototype = buffer_constructor_prototype(interp, "DataView")?;
    if backing.is_detached() {
        return Err(VmErr::Msg("TypeError: detached DataView buffer".into()));
    }
    if byte_offset > backing.len()
        || (a.get(2).is_some_and(|v| !matches!(v, Value::Undefined))
            && byte_offset
                .checked_add(byte_length)
                .is_none_or(|end| end > backing.len()))
    {
        return Err(range_err("Invalid DataView length"));
    }
    let properties = Value::instance_properties();
    properties.set_proto(prototype);
    let length_tracking = match &backing {
        BufferBacking::Array(buffer) => buffer.is_resizable(),
        BufferBacking::Shared(buffer) => buffer.is_growable(),
    } && a
        .get(2)
        .is_none_or(|length| matches!(length, Value::Undefined));
    Ok(Value::DataView(Rc::new(TypedArrayData {
        length_tracking,
        properties,
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
fn require_data_view(this: &Value) -> Result<Rc<TypedArrayData>, VmErr> {
    let Value::DataView(view) = this else {
        return Err(VmErr::Msg("TypeError: not a DataView".into()));
    };
    Ok(view.clone())
}
fn data_view_buffer(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(require_data_view(&this)?.buffer.to_value())
}
fn data_view_byte_length(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let v = require_data_view(&this)?;
    if v.is_out_of_bounds() {
        return Err(VmErr::Msg("TypeError: DataView out of bounds".into()));
    }
    Ok(Value::Number(v.effective_length() as f64))
}
fn data_view_byte_offset(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let v = require_data_view(&this)?;
    if v.is_out_of_bounds() {
        return Err(VmErr::Msg("TypeError: DataView out of bounds".into()));
    }
    Ok(Value::Number(v.byte_offset as f64))
}
fn data_view_slot(
    view: &Rc<TypedArrayData>,
    offset: usize,
    kind: TypedKind,
) -> Result<Rc<TypedArrayData>, VmErr> {
    if view.is_out_of_bounds() {
        return Err(VmErr::Msg("TypeError: DataView out of bounds".into()));
    }
    if offset
        .checked_add(kind.size())
        .is_none_or(|end| end > view.effective_length())
    {
        return Err(range_err("Offset is outside the DataView"));
    }
    Ok(Rc::new(TypedArrayData {
        length_tracking: false,
        properties: Value::instance_properties(),
        kind,
        buffer: view.buffer.clone(),
        byte_offset: view.byte_offset + offset,
        length: 1,
        is_buffer: false,
    }))
}

fn data_view_write(slot: &Rc<TypedArrayData>, value: &Value, little: bool) -> Result<(), VmErr> {
    require_writable(&slot.buffer)?;
    let buffer = BufferBacking::Array(Buffer::zeroed(slot.kind.size()));
    let local = Rc::new(TypedArrayData {
        length_tracking: false,
        properties: Value::instance_properties(),
        kind: slot.kind,
        buffer,
        byte_offset: 0,
        length: 1,
        is_buffer: false,
    });
    write_element(&local, 0, value)?;
    let mut bytes = local.buffer.read(0, slot.kind.size()).unwrap();
    if !little {
        bytes.reverse();
    }
    slot.buffer.write(slot.byte_offset, &bytes);
    Ok(())
}

fn data_view_get_kind(
    i: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
    kind: TypedKind,
) -> Result<Value, VmErr> {
    let view = require_data_view(&this)?;
    let offset = constructor_index(i, a.first())?;
    let slot = data_view_slot(&view, offset, kind)?;
    let little_endian = a.get(1).map(|v| v.is_truthy()).unwrap_or(false);
    let mut bytes = slot
        .buffer
        .read(slot.byte_offset, kind.size())
        .ok_or_else(|| range_err("Invalid DataView read"))?;
    if !little_endian {
        bytes.reverse();
    }
    let local = Rc::new(TypedArrayData {
        length_tracking: false,
        properties: Value::instance_properties(),
        kind,
        buffer: BufferBacking::Array(Buffer::owned(bytes)),
        byte_offset: 0,
        length: 1,
        is_buffer: false,
    });
    Ok(read_element(&local, 0).unwrap_or(Value::Undefined))
}

fn data_view_set_kind(
    i: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
    kind: TypedKind,
) -> Result<Value, VmErr> {
    let view = require_data_view(&this)?;
    require_writable(&view.buffer)?;
    let offset = constructor_index(i, a.first())?;
    let undefined = Value::Undefined;
    let value = a.get(1).unwrap_or(&undefined);
    let value = if matches!(kind, TypedKind::BigInt64 | TypedKind::BigUint64) {
        i.ecmascript_to_bigint(value)?
    } else {
        Value::Number(i.ecmascript_to_number(value)?)
    };
    let little = a.get(2).is_some_and(Value::is_truthy);
    let slot = data_view_slot(&view, offset, kind)?;
    data_view_write(&slot, &value, little)?;
    Ok(Value::Undefined)
}

macro_rules! data_view_accessors {
    ($(($get_fn:ident, $set_fn:ident, $kind:ident, $get_name:literal, $set_name:literal)),* $(,)?) => {
        $(
            fn $get_fn(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
                data_view_get_kind(i, this, args, TypedKind::$kind)
            }

            fn $set_fn(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
                data_view_set_kind(i, this, args, TypedKind::$kind)
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

fn typed_buffer_getter(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(require(&this)?.buffer.to_value())
}
fn typed_byte_length_getter(
    _: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    let view = require(&this)?;
    Ok(Value::Number(
        (view.effective_length() * view.kind.size()) as f64,
    ))
}
fn typed_byte_offset_getter(
    _: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    Ok(Value::Number(require(&this)?.effective_byte_offset() as f64))
}
fn typed_length_getter(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(require(&this)?.effective_length() as f64))
}

fn abstract_typed_array_constructor(
    _: &mut Interpreter,
    _: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    Err(VmErr::Msg(
        "TypeError: Abstract TypedArray constructor cannot be invoked".into(),
    ))
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
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
        let mut interpreter = Interpreter::with_runtime_builtins();
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

    #[cfg(feature = "node-api-host")]
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
        let mut interpreter = Interpreter::with_runtime_builtins();
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

pub(crate) fn float16_bits(n: f64) -> u16 {
    let sign = if n.is_sign_negative() { 0x8000 } else { 0 };
    let n = n.abs();
    if n.is_nan() {
        return sign | 0x7e00;
    }
    if n >= 65520. {
        return sign | 0x7c00;
    }
    if n < 2f64.powi(-14) {
        return sign | (n * 2f64.powi(24)).round_ties_even() as u16;
    }
    let exponent = ((n.to_bits() >> 52) & 0x7ff) as i32 - 1023;
    let mantissa = (n / 2f64.powi(exponent - 10)).round_ties_even() as u16;
    sign | (((exponent + 14) as u16) << 10).wrapping_add(mantissa)
}
pub(crate) fn float16_number(bits: u16) -> f64 {
    let exp = (bits >> 10) & 31;
    let mantissa = bits & 1023;
    let n = match exp {
        0 => (mantissa as f64) * 2f64.powi(-24),
        31 => {
            if mantissa == 0 {
                f64::INFINITY
            } else {
                f64::NAN
            }
        }
        _ => ((1024 + mantissa) as f64) * 2f64.powi(exp as i32 - 25),
    };
    if bits & 0x8000 != 0 { -n } else { n }
}
fn data_view_get_float16(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = data_view_get_kind(i, this, a, TypedKind::Uint16)?;
    Ok(Value::Number(float16_number(v.to_number() as u16)))
}
fn data_view_set_float16(i: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let view = require_data_view(&this)?;
    require_writable(&view.buffer)?;
    let offset = constructor_index(i, a.first())?;
    let n = i.ecmascript_to_number(a.get(1).unwrap_or(&Value::Undefined))?;
    let little = a.get(2).is_some_and(Value::is_truthy);
    let slot = data_view_slot(&view, offset, TypedKind::Uint16)?;
    data_view_write(&slot, &Value::Number(float16_bits(n) as f64), little)?;
    Ok(Value::Undefined)
}

fn typed_find_last(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_callback(i, t, a, "findLast")
}
fn typed_find_last_index(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_callback(i, t, a, "findLastIndex")
}
fn typed_reduce_right(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_callback(i, t, a, "reduceRight")
}
fn typed_callback(
    i: &mut Interpreter,
    t: Value,
    a: Vec<Value>,
    name: &str,
) -> Result<Value, VmErr> {
    let view = validated_typed_source(&t)?;
    let len = view.effective_length();
    let callback = super::array::callback(&a)?;
    let reduce = matches!(name, "reduce" | "reduceRight");
    let reverse = matches!(name, "reduceRight" | "findLast" | "findLastIndex");
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    let mut acc = receiver.clone();
    let mut first = 0;
    if reduce && a.len() < 2 {
        if len == 0 {
            return Err(VmErr::Msg("TypeError: Reduce of empty typed array".into()));
        }
        acc = read_element(&view, if reverse { len - 1 } else { 0 }).unwrap_or(Value::Undefined);
        first = 1;
    }
    for step in first..len {
        i.consume_loop()?;
        let k = if reverse { len - 1 - step } else { step };
        let v = read_element(&view, k).unwrap_or(Value::Undefined);
        if reduce {
            acc = i.call_this(
                &callback,
                Value::Undefined,
                vec![acc, v, Value::Number(k as f64), t.clone()],
            )?;
            continue;
        }
        let result = i
            .call_this(
                &callback,
                receiver.clone(),
                vec![v.clone(), Value::Number(k as f64), t.clone()],
            )?
            .is_truthy();
        match name {
            "every" if !result => return Ok(Value::Bool(false)),
            "some" if result => return Ok(Value::Bool(true)),
            "find" | "findLast" if result => return Ok(v),
            "findIndex" | "findLastIndex" if result => return Ok(Value::Number(k as f64)),
            _ => {}
        }
    }
    Ok(match name {
        "reduce" | "reduceRight" => acc,
        "every" => Value::Bool(true),
        "some" => Value::Bool(false),
        "findIndex" | "findLastIndex" => Value::Number(-1.),
        _ => Value::Undefined,
    })
}

fn typed_tag_getter(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(match t {
        Value::TypedArray(ref v) => Value::String(v.kind.name().into()),
        _ => Value::Undefined,
    })
}

fn typed_relative(
    i: &mut Interpreter,
    v: Option<&Value>,
    len: usize,
    default: usize,
) -> Result<usize, VmErr> {
    let Some(v) = v else { return Ok(default) };
    if matches!(v, Value::Undefined) {
        return Ok(default);
    }
    let n = i.ecmascript_to_number(v)?;
    let n = if n.is_nan() { 0. } else { n.trunc() };
    Ok(if n < 0. {
        (len as f64 + n).clamp(0., len as f64) as usize
    } else {
        n.min(len as f64) as usize
    })
}
fn typed_copy_within(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = validated_typed_source(&t)?;
    require_writable(&v.buffer)?;
    let len = v.effective_length();
    let to = typed_relative(i, a.first(), len, 0)?;
    let from = typed_relative(i, a.get(1), len, 0)?;
    let end = typed_relative(i, a.get(2), len, len)?;
    let count = end.saturating_sub(from).min(len - to);
    if count == 0 {
        return Ok(t);
    }
    if v.is_out_of_bounds() {
        return Err(VmErr::Msg("TypeError: TypedArray out of bounds".into()));
    }
    let current = v.effective_length();
    let count = count
        .min(current.saturating_sub(to))
        .min(current.saturating_sub(from));
    if count > 0 {
        i.consume_loop()?;
        let size = v.kind.size();
        let bytes = v
            .buffer
            .read(v.byte_offset + from * size, count * size)
            .unwrap();
        v.buffer.write(v.byte_offset + to * size, &bytes);
    }
    Ok(t)
}
fn typed_reverse(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let v = validated_typed_source(&t)?;
    require_writable(&v.buffer)?;
    let len = v.effective_length();
    let size = v.kind.size();
    for k in 0..len / 2 {
        i.consume_loop()?;
        let a = v.byte_offset + k * size;
        let b = v.byte_offset + (len - 1 - k) * size;
        let first = v.buffer.read(a, size).unwrap();
        let last = v.buffer.read(b, size).unwrap();
        v.buffer.write(a, &last);
        v.buffer.write(b, &first);
    }
    Ok(t)
}

fn typed_with(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = validated_typed_source(&t)?;
    let len = v.effective_length();
    let n = crate::value::to_integer_or_infinity(
        i.ecmascript_to_number(a.first().unwrap_or(&Value::Undefined))?,
    );
    let k = if n < 0. { len as f64 + n } else { n };
    let x = a.get(1).cloned().unwrap_or(Value::Undefined);
    let x = if is_bigint_kind(v.kind) {
        i.ecmascript_to_bigint(&x)?
    } else {
        Value::Number(i.ecmascript_to_number(&x)?)
    };
    if !k.is_finite() || k < 0. || k >= v.effective_length() as f64 {
        return Err(range_err("Invalid typed array index"));
    }
    let result = allocate_typed(v.kind, len)?;
    let dest = require(&result)?;
    for j in 0..len {
        i.consume_loop()?;
        let value = if j == k as usize {
            x.clone()
        } else {
            read_element(&v, j).unwrap_or(Value::Undefined)
        };
        write_element_in(i, &dest, j, &value)?;
    }
    Ok(result)
}
fn typed_to_reversed(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let v = validated_typed_source(&t)?;
    let len = v.effective_length();
    let result = allocate_typed(v.kind, len)?;
    let dest = require(&result)?;
    let size = v.kind.size();
    for k in 0..len {
        i.consume_loop()?;
        let bytes = v
            .buffer
            .read(v.byte_offset + (len - 1 - k) * size, size)
            .unwrap();
        dest.buffer.write(k * size, &bytes);
    }
    Ok(result)
}
fn typed_search(i: &mut Interpreter, t: Value, a: Vec<Value>, name: &str) -> Result<Value, VmErr> {
    let v = validated_typed_source(&t)?;
    let len = v.effective_length();
    let found = |k: Option<usize>| {
        if name == "includes" {
            Value::Bool(k.is_some())
        } else {
            Value::Number(k.map(|k| k as f64).unwrap_or(-1.))
        }
    };
    if len == 0 {
        return Ok(found(None));
    }
    let needle = a.first().cloned().unwrap_or(Value::Undefined);
    let last = name == "lastIndexOf";
    let n = if last && a.len() < 2 {
        len as f64 - 1.
    } else {
        crate::value::to_integer_or_infinity(
            i.ecmascript_to_number(a.get(1).unwrap_or(&Value::Undefined))?,
        )
    };
    let k = if n < 0. { len as f64 + n } else { n };
    let mut k = if last {
        k.min(len as f64 - 1.)
    } else {
        k.max(0.)
    };
    while k >= 0. && k < len as f64 {
        i.consume_loop()?;
        let value = read_element(&v, k as usize).unwrap_or(Value::Undefined);
        let equal = (name == "includes" || !matches!(value, Value::Undefined))
            && i.seq(&value, &needle)
            || name == "includes"
                && matches!((&value,&needle),(Value::Number(a),Value::Number(b)) if a.is_nan()&&b.is_nan());
        if equal {
            return Ok(found(Some(k as usize)));
        }
        k += if last { -1. } else { 1. };
    }
    Ok(found(None))
}
fn typed_sort_compare(
    i: &mut Interpreter,
    a: &Value,
    b: &Value,
    cmp: &Value,
) -> Result<std::cmp::Ordering, VmErr> {
    if !matches!(cmp, Value::Undefined) {
        let r = i.call_this(cmp, Value::Undefined, vec![a.clone(), b.clone()])?;
        let n = i.ecmascript_to_number(&r)?;
        return Ok(if n < 0. {
            std::cmp::Ordering::Less
        } else if n > 0. {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        });
    }
    if let (Value::BigInt(a), Value::BigInt(b)) = (a, b) {
        return Ok(a.compare(b));
    }
    let a = a.to_number();
    let b = b.to_number();
    Ok(if a.is_nan() {
        if b.is_nan() {
            std::cmp::Ordering::Equal
        } else {
            std::cmp::Ordering::Greater
        }
    } else if b.is_nan() {
        std::cmp::Ordering::Less
    } else if a == 0. && b == 0. {
        b.is_sign_negative().cmp(&a.is_sign_negative())
    } else {
        a.partial_cmp(&b).unwrap()
    })
}
fn typed_sort_values(
    i: &mut Interpreter,
    values: &mut Vec<Value>,
    cmp: &Value,
) -> Result<(), VmErr> {
    let len = values.len();
    // Sorted numeric inputs need no merge passes. This keeps zero-filled and
    // already ordered buffers linear without adding observable comparator calls.
    if matches!(cmp, Value::Undefined) {
        let mut ordered = true;
        for pair in values.windows(2) {
            i.consume_loop()?;
            if typed_sort_compare(i, &pair[0], &pair[1], cmp)? == std::cmp::Ordering::Greater {
                ordered = false;
                break;
            }
        }
        if ordered {
            return Ok(());
        }
    }
    let mut width = 1;
    while width < len {
        let mut out = Vec::with_capacity(len);
        for start in (0..len).step_by(width * 2) {
            let mid = (start + width).min(len);
            let end = (start + 2 * width).min(len);
            let (mut a, mut b) = (start, mid);
            while a < mid && b < end {
                i.consume_loop()?;
                if typed_sort_compare(i, &values[a], &values[b], cmp)?
                    != std::cmp::Ordering::Greater
                {
                    out.push(values[a].clone());
                    a += 1;
                } else {
                    out.push(values[b].clone());
                    b += 1;
                }
            }
            out.extend_from_slice(&values[a..mid]);
            out.extend_from_slice(&values[b..end]);
        }
        *values = out;
        width *= 2;
    }
    Ok(())
}
fn typed_sort(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_sort_impl(i, t, a, false)
}
fn typed_to_sorted(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    typed_sort_impl(i, t, a, true)
}
fn typed_sort_impl(
    i: &mut Interpreter,
    t: Value,
    a: Vec<Value>,
    copy: bool,
) -> Result<Value, VmErr> {
    let cmp = a.first().cloned().unwrap_or(Value::Undefined);
    if !matches!(cmp, Value::Undefined) && !crate::interpreter::call::is_callable_value(&cmp) {
        return Err(VmErr::Msg("TypeError: comparator must be callable".into()));
    }
    let v = validated_typed_source(&t)?;
    if !copy {
        require_writable(&v.buffer)?;
    }
    let len = v.effective_length();
    let result = if copy {
        allocate_typed(v.kind, len)?
    } else {
        t
    };
    let dest = require(&result)?;
    let mut values = read_all(&v);
    typed_sort_values(i, &mut values, &cmp)?;
    for (k, x) in values.iter().enumerate() {
        i.consume_loop()?;
        write_element(&dest, k, x)?;
    }
    Ok(result)
}

fn array_buffer_transfer(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    transfer_buffer(i, this, args, BufferTransferMode::Preserve)
}
fn array_buffer_transfer_fixed(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    transfer_buffer(i, this, args, BufferTransferMode::Fixed)
}
fn transfer_buffer(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
    mode: BufferTransferMode,
) -> Result<Value, VmErr> {
    let source = require_array_buffer(&this)?;
    let length = match args.first() {
        None | Some(Value::Undefined) => source.borrow().len(),
        Some(value) => constructor_index(i, Some(value))?,
    };
    if source.is_detached() {
        return Err(VmErr::Msg(
            "TypeError: Cannot transfer a detached ArrayBuffer".into(),
        ));
    }
    if source.is_immutable() {
        return Err(VmErr::Msg(
            "TypeError: Cannot transfer an immutable ArrayBuffer".into(),
        ));
    }
    if matches!(mode, BufferTransferMode::Immutable) {
        let count = length.min(source.borrow().len());
        let result = immutable_buffer(i, source, 0, length, count)?;
        source.try_detach()?;
        return Ok(result);
    }
    let destination = if matches!(mode, BufferTransferMode::Preserve) && source.is_resizable() {
        Buffer::resizable(length, source.maximum_length())
            .ok_or_else(|| range_err("Invalid transferred ArrayBuffer length"))?
    } else {
        new_buffer(length)?
    };
    {
        let bytes = source.borrow();
        let count = bytes.len().min(length);
        destination.borrow_mut()[..count].copy_from_slice(&bytes[..count]);
    }
    let result = Value::ArrayBuffer(destination);
    let prototype = i
        .persistent_global
        .borrow()
        .intrinsic("ArrayBuffer")
        .and_then(|c| c.get_prop("prototype"));
    result
        .property_cell()
        .unwrap()
        .set_proto(prototype.map(Rc::new));
    source.try_detach()?;
    Ok(result)
}

enum BufferTransferMode {
    Preserve,
    Fixed,
    Immutable,
}
fn array_immutable(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(require_array_buffer(&this)?.is_immutable()))
}
fn array_buffer_transfer_immutable(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    transfer_buffer(i, this, args, BufferTransferMode::Immutable)
}
fn immutable_buffer(
    i: &mut Interpreter,
    source: &Buffer,
    start: usize,
    length: usize,
    count: usize,
) -> Result<Value, VmErr> {
    if length > crate::value::MAX_ARRAY_LEN * 8 {
        return Err(range_err("Invalid array buffer length"));
    }
    let mut bytes = vec![0; length];
    if count > 0 {
        bytes[..count].copy_from_slice(&source.borrow()[start..start + count]);
    }
    let result = Value::ArrayBuffer(Buffer::immutable(bytes));
    let prototype = i
        .persistent_global
        .borrow()
        .intrinsic("ArrayBuffer")
        .and_then(|c| c.get_prop("prototype"));
    result
        .property_cell()
        .unwrap()
        .set_proto(prototype.map(Rc::new));
    Ok(result)
}
fn array_buffer_slice_immutable(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let source = require_array_buffer(&this)?;
    if source.is_detached() {
        return Err(VmErr::Msg("TypeError: Detached ArrayBuffer".into()));
    }
    let length = source.borrow().len();
    let relative = |number: f64| {
        let integer = crate::value::to_integer_or_infinity(number);
        if integer < 0. {
            (length as f64 + integer).max(0.) as usize
        } else {
            integer.min(length as f64) as usize
        }
    };
    let start = relative(i.ecmascript_to_number(args.first().unwrap_or(&Value::Undefined))?);
    let end = match args.get(1) {
        None | Some(Value::Undefined) => length,
        Some(value) => relative(i.ecmascript_to_number(value)?),
    };
    if source.is_detached() {
        return Err(VmErr::Msg("TypeError: Detached ArrayBuffer".into()));
    }
    let length = end.saturating_sub(start);
    if source.borrow().len() < end {
        return Err(range_err("ArrayBuffer shrank below the resolved slice end"));
    }
    immutable_buffer(i, source, start, length, length)
}
fn require_writable(buffer: &BufferBacking) -> Result<(), VmErr> {
    if buffer.is_immutable() {
        return Err(VmErr::Msg(
            "TypeError: Cannot write an immutable ArrayBuffer".into(),
        ));
    }
    Ok(())
}
