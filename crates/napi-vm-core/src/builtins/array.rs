//! `Array` statics and `Array.prototype` methods.

mod from_async;
use std::cmp::Ordering;
use std::rc::Rc;

use super::{NativeFn, nf};
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{Value, to_integer_or_infinity};

/// ArraySetLength converts the original value twice: ToUint32, then ToNumber.
/// Both observable conversions precede descriptor compatibility checks.
pub(super) fn array_length_value(interp: &mut Interpreter, value: &Value) -> Result<Value, VmErr> {
    // ArraySetLength accepts any Uint32; huge lengths stay sparse downstream.
    let unsigned = crate::value::to_uint32(interp.ecmascript_to_number(value)?);
    let number = interp.ecmascript_to_number(value)?;
    if number != unsigned as f64 {
        return Err(VmErr::Msg("RangeError: Invalid array length".into()));
    }
    Ok(Value::Number(unsigned as f64))
}

pub(super) fn callback(a: &[Value]) -> Result<Value, VmErr> {
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    if !crate::interpreter::call::is_callable_value(&cb) {
        return Err(VmErr::Msg("TypeError: callback is not a function".into()));
    }
    Ok(cb)
}

fn array_element(
    interp: &mut Interpreter,
    this: &Value,
    index: usize,
    skip_holes: bool,
) -> Result<Option<Value>, VmErr> {
    array_element_number(interp, this, index as f64, skip_holes)
}

fn array_element_number(
    interp: &mut Interpreter,
    this: &Value,
    index: f64,
    skip_holes: bool,
) -> Result<Option<Value>, VmErr> {
    let key = Value::String(crate::format::ecmascript_number_string(index).into());
    if skip_holes && !interp.has_property(this, &key)? {
        return Ok(None);
    }
    interp.get_prop_value(this, &key).map(Some)
}

pub(super) fn relative_index(number: f64, length: usize) -> usize {
    let integer = to_integer_or_infinity(number);
    if integer < 0.0 {
        (length as f64 + integer).max(0.0) as usize
    } else {
        (integer as usize).min(length)
    }
}

pub(super) fn install(e: &mut Environment) {
    if let Some(a) = e.get("Array") {
        let statics: &[(&str, NativeFn)] = &[
            ("isArray", array_is_array),
            ("from", array_from),
            ("fromAsync", from_async::from_async),
            ("of", array_of),
        ];
        for (name, callable) in statics {
            a.set_prop(name.to_string(), nf(name, *callable))
                .expect("built-in Array property");
        }
        super::make_callable(&a, array_ctor, None);

        let object_prototype = e
            .get("Object")
            .and_then(|object| object.get_prop("prototype"));
        let function_prototype = e
            .get("Function")
            .and_then(|function| function.get_prop("prototype"));
        let species = super::well_known("species").expect("Symbol.species");
        if let Value::Symbol(symbol) = &species {
            let slot = crate::interpreter::symbol_slot_key(symbol);
            super::object::define_property(
                &a,
                &slot,
                &Value::descriptor_record(vec![
                    (
                        "get".into(),
                        super::native_method(
                            "get [Symbol.species]",
                            0,
                            array_species,
                            function_prototype.clone(),
                        ),
                    ),
                    ("configurable".into(), Value::Bool(true)),
                ]),
            )
            .expect("Array species getter");
            if let Value::Object { props } = &a {
                props
                    .meta
                    .borrow_mut()
                    .set_symbol_key(&slot, symbol.clone());
            }
        }
        let prototype = Value::array(Vec::new());
        prototype
            .set_prop("constructor".into(), a.clone())
            .expect("Array.prototype constructor");
        for name in ARRAY_PROTOTYPE_METHODS {
            prototype
                .set_prop(
                    (*name).into(),
                    array_method(name).expect("listed Array.prototype method"),
                )
                .expect("Array.prototype method");
        }
        super::install_method_metadata(
            e,
            &a,
            &[("isArray", 1), ("from", 1), ("fromAsync", 1), ("of", 0)],
        );
        super::install_method_metadata(
            e,
            &prototype,
            &[
                ("map", 1),
                ("filter", 1),
                ("reduce", 1),
                ("reduceRight", 1),
                ("forEach", 1),
                ("find", 1),
                ("findIndex", 1),
                ("findLast", 1),
                ("findLastIndex", 1),
                ("some", 1),
                ("every", 1),
                ("push", 1),
                ("pop", 0),
                ("toString", 0),
                ("toLocaleString", 0),
                ("join", 1),
                ("indexOf", 1),
                ("lastIndexOf", 1),
                ("includes", 1),
                ("slice", 2),
                ("concat", 1),
                ("reverse", 0),
                ("sort", 1),
                ("flat", 0),
                ("flatMap", 1),
                ("at", 1),
                ("with", 2),
                ("toReversed", 0),
                ("toSorted", 1),
                ("copyWithin", 2),
                ("toSpliced", 2),
                ("splice", 2),
                ("shift", 0),
                ("unshift", 1),
                ("fill", 1),
                ("keys", 0),
                ("values", 0),
                ("entries", 0),
            ],
        );
        if let Value::Array(array_prototype) = &prototype {
            array_prototype.set_proto(object_prototype.map(Rc::new));
            let mut metadata = array_prototype.meta.borrow_mut();
            metadata.set_attrs(
                "constructor",
                crate::value::PropAttrs {
                    enumerable: false,
                    ..crate::value::PropAttrs::default()
                },
            );
            for name in ARRAY_PROTOTYPE_METHODS {
                metadata.set_attrs(
                    name,
                    crate::value::PropAttrs {
                        enumerable: false,
                        ..crate::value::PropAttrs::default()
                    },
                );
            }
        }
        if let Value::Symbol(symbol) =
            &crate::builtins::well_known("iterator").expect("Symbol.iterator is well-known")
        {
            let slot = crate::interpreter::symbol_slot_key(symbol);
            prototype
                .set_prop(
                    slot.clone(),
                    prototype
                        .get_prop("values")
                        .expect("Array.prototype.values"),
                )
                .expect("Array.prototype[Symbol.iterator]");
            if let Value::Array(array_prototype) = &prototype {
                array_prototype.set_symbol_key(&slot, symbol.clone());
                array_prototype.meta.borrow_mut().set_attrs(
                    &slot,
                    crate::value::PropAttrs {
                        enumerable: false,
                        ..crate::value::PropAttrs::default()
                    },
                );
            }
        }
        if let Some(Value::Symbol(ref symbol)) = super::well_known("unscopables") {
            let excluded = Value::object_with_proto(
                [
                    "at",
                    "copyWithin",
                    "entries",
                    "fill",
                    "find",
                    "findIndex",
                    "findLast",
                    "findLastIndex",
                    "flat",
                    "flatMap",
                    "includes",
                    "keys",
                    "toReversed",
                    "toSorted",
                    "toSpliced",
                    "values",
                    "with",
                ]
                .into_iter()
                .map(|name| (name.into(), Value::Bool(true)))
                .collect(),
                None,
            );
            let slot = crate::interpreter::symbol_slot_key(symbol);
            prototype
                .set_prop(slot.clone(), excluded)
                .expect("Array unscopables");
            if let Value::Array(array) = &prototype {
                array.set_symbol_key(&slot, symbol.clone());
                array.meta.borrow_mut().set_attrs(
                    &slot,
                    crate::value::PropAttrs {
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
        }
        a.set_prop("prototype".into(), prototype)
            .expect("Array.prototype");
        if let Value::Object { props } = &a {
            props.meta.borrow_mut().set_attrs(
                "prototype",
                crate::value::PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: false,
                },
            );
            if let Some(function_prototype) = function_prototype {
                props.set_proto(Some(Rc::new(function_prototype)));
            }
        }
    }
}

const ARRAY_PROTOTYPE_METHODS: &[&str] = &[
    "map",
    "filter",
    "reduce",
    "forEach",
    "find",
    "some",
    "every",
    "push",
    "pop",
    "toString",
    "toLocaleString",
    "join",
    "indexOf",
    "includes",
    "slice",
    "concat",
    "reverse",
    "sort",
    "flat",
    "flatMap",
    "reduceRight",
    "at",
    "with",
    "toReversed",
    "toSorted",
    "copyWithin",
    "toSpliced",
    "splice",
    "findIndex",
    "findLast",
    "findLastIndex",
    "lastIndexOf",
    "shift",
    "unshift",
    "fill",
    "keys",
    "values",
    "entries",
];

// --- Array statics ----------------------------------------------------------

fn array_is_array(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    is_array(a.first().unwrap_or(&Value::Undefined)).map(Value::Bool)
}

pub(super) fn is_array(value: &Value) -> Result<bool, VmErr> {
    let mut value = value.clone();
    // ECMAScript's IsArray operation follows Proxy [[ProxyTarget]] links.
    // Iterate so deeply nested proxies do not consume the Rust call stack.
    while let Value::Proxy(proxy) = &value {
        value = proxy.snapshot()?.0;
    }
    Ok(matches!(value, Value::Array(_)))
}

/// `Array(n)` allocates `n` holes; `Array(a, b, …)` collects its arguments.
fn array_ctor(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if let [Value::Number(n)] = a.as_slice() {
        if !n.is_finite() || *n < 0.0 || n.fract() != 0.0 || *n > u32::MAX as f64 {
            return Err(crate::value::limit_err("Invalid array length"));
        }
        if *n > crate::value::MAX_ARRAY_LEN as f64 {
            // Huge single-argument lengths stay sparse: no allocation.
            let empty = Value::array(vec![]);
            if let Value::Array(cell) = &empty {
                cell.set_length(*n as usize);
            }
            return Ok(empty);
        }
        let length = *n as usize;
        return Ok(Value::array_with_presence(
            vec![Value::Undefined; length],
            vec![false; length],
        ));
    }
    Value::checked_array(a)
}

fn array_result_create(
    interp: &mut Interpreter,
    constructor: &Value,
    length: usize,
    iterable: bool,
) -> Result<Value, VmErr> {
    if super::is_constructor(constructor) {
        return interp.ctor(
            constructor,
            if iterable {
                vec![]
            } else {
                vec![Value::Number(length as f64)]
            },
        );
    }
    if length > crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    Value::checked_array_with_presence(vec![Value::Undefined; length], vec![false; length])
}

fn array_of(
    interp: &mut Interpreter,
    constructor: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let length = arguments.len();
    let result = array_result_create(interp, &constructor, length, false)?;
    for (index, value) in arguments.into_iter().enumerate() {
        create_array_result_element(interp, &result, index, value)?;
    }
    interp.set_member_str_or_throw(&result, "length", Value::Number(length as f64))?;
    Ok(result)
}

/// Array.from creates its result before calling the iterator or reading
/// indexed values. Only mapping/definition failures close an active iterator.
fn array_from(
    interp: &mut Interpreter,
    constructor: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let source = arguments.first().cloned().unwrap_or(Value::Undefined);
    let mapper = arguments
        .get(1)
        .filter(|value| !matches!(value, Value::Undefined))
        .cloned();
    if mapper
        .as_ref()
        .is_some_and(|mapper| !crate::interpreter::call::is_callable_value(mapper))
    {
        return Err(VmErr::Msg(
            "TypeError: Array.from mapper is not a function".into(),
        ));
    }
    let receiver = arguments.get(2).cloned().unwrap_or(Value::Undefined);
    let method = interp.get_method(
        &source,
        &super::well_known("iterator").expect("Symbol.iterator"),
    )?;
    if let Some(method) = method {
        let result = array_result_create(interp, &constructor, 0, true)?;
        let iterator = interp.iterator_from_method(&source, &method)?;
        let next = interp.get_prop_value_str(&iterator, "next")?;
        let mut index = 0;
        loop {
            interp.consume_loop()?;
            let step = interp.call_this(&next, iterator.clone(), vec![])?;
            let (done, value) = interp.iterator_result_fields(&step)?;
            if done {
                interp.set_member_str_or_throw(&result, "length", Value::Number(index as f64))?;
                return Ok(result);
            }
            let definition = (|| {
                if index >= crate::value::MAX_ARRAY_LEN {
                    return Err(crate::value::limit_err("Maximum array length exceeded"));
                }
                let value = if let Some(mapper) = &mapper {
                    interp.call_this(
                        mapper,
                        receiver.clone(),
                        vec![value, Value::Number(index as f64)],
                    )?
                } else {
                    value
                };
                create_array_result_element(interp, &result, index, value)
            })();
            if let Err(error) = definition {
                interp.close_guest_iterator_for_abrupt(&iterator, false, &error)?;
                return Err(error);
            }
            index += 1;
        }
    }
    let length = array_like_length(interp, &source)?;
    let result = array_result_create(interp, &constructor, length, false)?;
    for index in 0..length {
        interp.consume_loop()?;
        let value = interp.get_prop_value_str(&source, &index.to_string())?;
        let value = if let Some(mapper) = &mapper {
            interp.call_this(
                mapper,
                receiver.clone(),
                vec![value, Value::Number(index as f64)],
            )?
        } else {
            value
        };
        create_array_result_element(interp, &result, index, value)?;
    }
    interp.set_member_str_or_throw(&result, "length", Value::Number(length as f64))?;
    Ok(result)
}

// --- Array prototype --------------------------------------------------------

/// Dispatch table for `Array.prototype` methods, looked up by `prop()`.
pub fn array_method(name: &str) -> Option<Value> {
    let f: NativeFn = match name {
        "map" => array_map,
        "filter" => array_filter,
        "reduce" => array_reduce,
        "forEach" => array_for_each,
        "find" => array_find,
        "some" => array_some,
        "every" => array_every,
        "push" => array_push,
        "pop" => array_pop,
        "toString" => array_to_string,
        "toLocaleString" => array_to_locale_string,
        "join" => array_join,
        "indexOf" => array_index_of,
        "includes" => array_includes,
        "slice" => array_slice,
        "concat" => array_concat,
        "reverse" => array_reverse,
        "sort" => array_sort,
        "flat" => array_flat,
        "flatMap" => array_flat_map,
        "reduceRight" => array_reduce_right,
        "at" => array_at,
        "with" => array_with,
        "toReversed" => array_to_reversed,
        "toSorted" => array_to_sorted,
        "copyWithin" => array_copy_within,
        "toSpliced" => array_to_spliced,
        "splice" => array_splice,
        "findIndex" => array_find_index,
        "findLast" => array_find_last,
        "findLastIndex" => array_find_last_index,
        "lastIndexOf" => array_last_index_of,
        "shift" => array_shift,
        "unshift" => array_unshift,
        "fill" => array_fill,
        "keys" => array_keys,
        "values" => array_values,
        "entries" => array_entries,
        _ => return None,
    };
    Some(nf(name, f))
}

fn array_to_string(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let object = super::object::to_object_receiver(&this)?;
    let join = interp.get_prop_value_str(&object, "join")?;
    if crate::interpreter::is_callable_value(&join) {
        return interp.call_this(&join, object, vec![]);
    }
    // The fallback is the intrinsic, independent of mutable global bindings
    // and the public Object.prototype.toString property.
    super::object::object_to_string(interp, object, vec![])
}

/// `splice(start, deleteCount, ...items)`: remove a range in place and insert
/// replacements, returning what was removed.
fn delete_array_property_or_throw(
    interp: &mut Interpreter,
    object: &Value,
    index: usize,
) -> Result<(), VmErr> {
    delete_numeric_property_or_throw(interp, object, index.to_string())
}

fn array_splice(
    interp: &mut Interpreter,
    this: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    // Read `length` once: observable getters run a single time.
    let length_value = interp.get_prop_value_str(&this, "length")?;
    let length_number = interp.ecmascript_to_number(&length_value)?;
    let logical = to_integer_or_infinity(length_number).clamp(0.0, 9_007_199_254_740_991.0);
    let length = if !length_number.is_finite() || length_number <= 0.0 {
        0
    } else {
        (length_number.trunc() as usize).min(crate::value::MAX_ARRAY_LEN)
    };
    let start_number =
        interp.ecmascript_to_number(arguments.first().unwrap_or(&Value::Undefined))?;
    let delete_number = arguments
        .get(1)
        .map(|value| interp.ecmascript_to_number(value))
        .transpose()?;
    // ArraySpeciesCreate throws for result lengths above 2^32-1. Check the
    // logical remove count first; the clamped path below still bounds the
    // actual allocation.
    let logical_start = relative_number_index(start_number, logical);
    let logical_remove = if arguments.is_empty() {
        0.0
    } else if let Some(delete) = delete_number {
        to_integer_or_infinity(delete)
            .max(0.0)
            .min(logical - logical_start)
    } else {
        logical - logical_start
    };
    if logical_remove > 4294967295.0 {
        return Err(VmErr::Msg("RangeError: Invalid array length".into()));
    }
    let start = relative_index(start_number, length);
    let remove = if arguments.is_empty() {
        0
    } else if let Some(delete) = delete_number {
        to_integer_or_infinity(delete)
            .max(0.0)
            .min((length - start) as f64) as usize
    } else {
        length - start
    };
    let inserted = arguments.len().saturating_sub(2);
    let new_length = length - remove + inserted;
    if new_length > crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    let result = array_species_create(interp, &this, remove as f64)?;
    for offset in 0..remove {
        interp.consume_loop()?;
        if let Some(value) = array_element(interp, &this, start + offset, true)? {
            create_array_result_element(interp, &result, offset, value)?;
        }
    }
    interp.set_member_str_or_throw(&result, "length", Value::Number(remove as f64))?;
    if inserted < remove {
        for index in start..length - remove {
            interp.consume_loop()?;
            let destination = index + inserted;
            if let Some(value) = array_element(interp, &this, index + remove, true)? {
                interp.set_member_str_or_throw(&this, &destination.to_string(), value)?;
            } else {
                delete_array_property_or_throw(interp, &this, destination)?;
            }
        }
        for index in (new_length..length).rev() {
            delete_array_property_or_throw(interp, &this, index)?;
        }
    } else if inserted > remove {
        for index in (start..length - remove).rev() {
            interp.consume_loop()?;
            let destination = index + inserted;
            if let Some(value) = array_element(interp, &this, index + remove, true)? {
                interp.set_member_str_or_throw(&this, &destination.to_string(), value)?;
            } else {
                delete_array_property_or_throw(interp, &this, destination)?;
            }
        }
    }
    for (offset, value) in arguments.into_iter().skip(2).enumerate() {
        interp.set_member_str_or_throw(&this, &(start + offset).to_string(), value)?;
    }
    interp.set_member_str_or_throw(&this, "length", Value::Number(new_length as f64))?;
    Ok(result)
}

/// `at(index)`: a negative index counts back from the end.
fn array_at(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let index = to_integer_or_infinity(
        interp.ecmascript_to_number(a.first().unwrap_or(&Value::Undefined))?,
    );
    let index = if index < 0.0 { length + index } else { index };
    if !index.is_finite() || index < 0.0 || index >= length {
        return Ok(Value::Undefined);
    }
    interp.get_prop_value_str(&this, &crate::format::ecmascript_number_string(index))
}

fn array_to_reversed(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    if length > crate::value::MAX_ARRAY_LEN as f64 {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    let mut result = Vec::with_capacity(length as usize);
    for index in (0..length as usize).rev() {
        interp.consume_loop()?;
        result.push(interp.get_prop_value_str(&this, &index.to_string())?);
    }
    Value::checked_array(result)
}

fn relative_number_index(number: f64, length: f64) -> f64 {
    let integer = to_integer_or_infinity(number);
    if integer < 0.0 {
        (length + integer).max(0.0)
    } else {
        integer.min(length)
    }
}

fn array_copy_within(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let mut to = relative_number_index(
        interp.ecmascript_to_number(args.first().unwrap_or(&Value::Undefined))?,
        length,
    );
    let mut from = relative_number_index(
        interp.ecmascript_to_number(args.get(1).unwrap_or(&Value::Undefined))?,
        length,
    );
    let end = match args.get(2) {
        None | Some(Value::Undefined) => length,
        Some(value) => relative_number_index(interp.ecmascript_to_number(value)?, length),
    };
    let mut count = (end - from).min(length - to).max(0.0);
    let direction = if from < to && to < from + count {
        from += count - 1.0;
        to += count - 1.0;
        -1.0
    } else {
        1.0
    };
    while count > 0.0 {
        interp.consume_loop()?;
        let from_key = Value::String(crate::format::ecmascript_number_string(from).into());
        let to_key = crate::format::ecmascript_number_string(to);
        if interp.has_property(&this, &from_key)? {
            let value = interp.get_prop_value(&this, &from_key)?;
            interp.set_member_str_or_throw(&this, &to_key, value)?;
        } else {
            delete_numeric_property_or_throw(interp, &this, to_key)?;
        }
        from += direction;
        to += direction;
        count -= 1.0;
    }
    Ok(this)
}

fn array_to_spliced(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let start = relative_number_index(
        interp.ecmascript_to_number(args.first().unwrap_or(&Value::Undefined))?,
        length,
    );
    let inserted = args.len().saturating_sub(2);
    let remove = match args.len() {
        0 => 0.0,
        1 => length - start,
        _ => to_integer_or_infinity(interp.ecmascript_to_number(&args[1])?)
            .clamp(0.0, length - start),
    };
    // Subtract first so cancellation stays exact near MAX_SAFE_INTEGER.
    let new_length = (length - remove) + inserted as f64;
    if new_length > 9_007_199_254_740_991.0 {
        return Err(VmErr::Msg("TypeError: Invalid array-like length".into()));
    }
    if new_length > crate::value::MAX_ARRAY_LEN as f64 {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    let mut result = Vec::with_capacity(new_length as usize);
    // Only indexes surviving the deletion are read, and holes become values.
    let mut index = 0.0;
    while index < start {
        interp.consume_loop()?;
        result.push(
            interp.get_prop_value_str(&this, &crate::format::ecmascript_number_string(index))?,
        );
        index += 1.0;
    }
    result.extend(args.into_iter().skip(2));
    index = start + remove;
    while index < length {
        interp.consume_loop()?;
        result.push(
            interp.get_prop_value_str(&this, &crate::format::ecmascript_number_string(index))?,
        );
        index += 1.0;
    }
    Value::checked_array(result)
}

fn array_with(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let relative = to_integer_or_infinity(
        interp.ecmascript_to_number(a.first().unwrap_or(&Value::Undefined))?,
    );
    let index = if relative < 0.0 {
        length + relative
    } else {
        relative
    };
    if index < 0.0 || index >= length {
        return Err(crate::value::limit_err("Array.with index out of range"));
    }
    if length > crate::value::MAX_ARRAY_LEN as f64 {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    let replacement = a.get(1).cloned().unwrap_or(Value::Undefined);
    let mut result = Vec::with_capacity(length as usize);
    for k in 0..length as usize {
        interp.consume_loop()?;
        result.push(if k as f64 == index {
            replacement.clone()
        } else {
            interp.get_prop_value_str(&this, &k.to_string())?
        });
    }
    Value::checked_array(result)
}

/// Walk the elements, returning the first that satisfies `predicate`.
/// `reverse` searches from the end; `want_index` returns the position rather
/// than the element.
fn search(
    interp: &mut Interpreter,
    this: &Value,
    a: &[Value],
    reverse: bool,
    want_index: bool,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(this)?;
    let length = array_like_length_number(interp, &this)?;
    let predicate = callback(a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    let mut step = 0.0;
    while step < length {
        interp.consume_loop()?;
        let index = if reverse { length - 1.0 - step } else { step };
        let value =
            interp.get_prop_value_str(&this, &crate::format::ecmascript_number_string(index))?;
        let hit = interp.call_this(
            &predicate,
            receiver.clone(),
            vec![value.clone(), Value::Number(index), this.clone()],
        )?;
        if hit.is_truthy() {
            return Ok(if want_index {
                Value::Number(index)
            } else {
                value
            });
        }
        step += 1.0;
    }
    Ok(if want_index {
        Value::Number(-1.0)
    } else {
        Value::Undefined
    })
}

fn array_find_index(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    search(interp, &this, &a, false, true)
}
fn array_find_last(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    search(interp, &this, &a, true, false)
}
fn array_find_last_index(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    search(interp, &this, &a, true, true)
}

fn array_last_index_of(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    if length == 0.0 {
        return Ok(Value::Number(-1.0));
    }
    let needle = a.first().cloned().unwrap_or(Value::Undefined);
    let from = match a.get(1) {
        None => length - 1.0,
        Some(value) => to_integer_or_infinity(interp.ecmascript_to_number(value)?),
    };
    let mut index = if from >= 0.0 {
        from.min(length - 1.0)
    } else {
        length + from
    };
    while index >= 0.0 {
        // Preserve the existing bounded-search policy. Logical lengths beyond
        // the allocation bound additionally need a loop limit for sparse scans.
        if length > crate::value::MAX_ARRAY_LEN as f64 {
            interp.consume_loop()?;
        } else {
            interp.check_execution()?;
        }
        if let Some(value) = array_element_number(interp, &this, index, true)?
            && interp.seq(&value, &needle)
        {
            return Ok(Value::Number(if index == 0.0 { 0.0 } else { index }));
        }
        index -= 1.0;
    }
    Ok(Value::Number(-1.0))
}

fn array_shift(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    if length == 0.0 {
        interp.set_member_str_or_throw(&this, "length", Value::Number(0.0))?;
        return Ok(Value::Undefined);
    }
    let first = interp.get_prop_value_str(&this, "0")?;
    let mut index = 1.0;
    while index < length {
        interp.consume_loop()?;
        let destination = crate::format::ecmascript_number_string(index - 1.0);
        if let Some(value) = array_element_number(interp, &this, index, true)? {
            interp.set_member_str_or_throw(&this, &destination, value)?;
        } else {
            delete_numeric_property_or_throw(interp, &this, destination)?;
        }
        index += 1.0;
    }
    delete_numeric_property_or_throw(
        interp,
        &this,
        crate::format::ecmascript_number_string(length - 1.0),
    )?;
    interp.set_member_str_or_throw(&this, "length", Value::Number(length - 1.0))?;
    Ok(first)
}

/// Check logical growth independently of the existing array allocation guard.
fn array_growth_length(this: &Value, length: f64, added: usize) -> Result<f64, VmErr> {
    if added as f64 > 9_007_199_254_740_991.0 - length {
        return Err(VmErr::Msg(
            "TypeError: Array-like length exceeds MAX_SAFE_INTEGER".into(),
        ));
    }
    let result = length + added as f64;
    if matches!(this, Value::Array(_)) && result > crate::value::MAX_ARRAY_LEN as f64 {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    Ok(result)
}

fn array_unshift(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let result_length = array_growth_length(&this, length, a.len())?;
    if !a.is_empty() {
        let mut index = length;
        while index > 0.0 {
            interp.consume_loop()?;
            let destination = crate::format::ecmascript_number_string(index + a.len() as f64 - 1.0);
            if let Some(value) = array_element_number(interp, &this, index - 1.0, true)? {
                interp.set_member_str_or_throw(&this, &destination, value)?;
            } else {
                delete_numeric_property_or_throw(interp, &this, destination)?;
            }
            index -= 1.0;
        }
        for (index, value) in a.into_iter().enumerate() {
            interp.consume_loop()?;
            interp.set_member_str_or_throw(&this, &index.to_string(), value)?;
        }
    }
    interp.set_member_str_or_throw(&this, "length", Value::Number(result_length))?;
    Ok(Value::Number(result_length))
}

/// `fill(value, start, end)`.
fn array_fill(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let mut index = relative_number_index(
        interp.ecmascript_to_number(a.get(1).unwrap_or(&Value::Undefined))?,
        length,
    );
    let end = match a.get(2) {
        None | Some(Value::Undefined) => length,
        Some(value) => relative_number_index(interp.ecmascript_to_number(value)?, length),
    };
    let value = a.first().cloned().unwrap_or(Value::Undefined);
    while index < end {
        interp.consume_loop()?;
        interp.set_member_str_or_throw(
            &this,
            &crate::format::ecmascript_number_string(index),
            value.clone(),
        )?;
        index += 1.0;
    }
    Ok(this)
}

fn array_keys(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    crate::interpreter::array_iter_with_kind(this, "keys")
}

fn array_values(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    crate::interpreter::array_iter(interp, this, Vec::new())
}

fn array_entries(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    crate::interpreter::array_iter_with_kind(this, "entries")
}

fn array_species(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(this)
}

/// ArraySpeciesCreate uses the executing method's intrinsic Array, and ignores
/// another realm's intrinsic Array before observing its species property.
fn array_species_create(
    interp: &mut Interpreter,
    original: &Value,
    length: f64,
) -> Result<Value, VmErr> {
    let default = || {
        if length > u32::MAX as f64 {
            return Err(crate::value::limit_err("Invalid array length"));
        }
        let result = Value::array(vec![]);
        if let Value::Array(array) = &result {
            array.set_length(length as usize);
        }
        Ok(result)
    };
    if !array_is_array(interp, Value::Undefined, vec![original.clone()])?.is_truthy() {
        return default();
    }
    let mut constructor = interp.get_prop_value_str(original, "constructor")?;
    if super::is_constructor(&constructor)
        && let Some(realm) = crate::interpreter::realm::function_realm(&constructor)?
        && let Some(current) = crate::interpreter::realm::allocation_global()
        && !Rc::ptr_eq(&realm, &current)
        && realm
            .borrow()
            .intrinsic("Array")
            .is_some_and(|array| crate::interpreter::strict_equals(&array, &constructor))
    {
        constructor = Value::Undefined;
    }
    if crate::interpreter::call::is_js_object(&constructor) {
        constructor = interp.get_prop_value(
            &constructor,
            &super::well_known("species").expect("Symbol.species"),
        )?;
        if matches!(constructor, Value::Null) {
            constructor = Value::Undefined;
        }
    }
    if matches!(constructor, Value::Undefined) {
        return default();
    }
    if !super::is_constructor(&constructor) {
        return Err(VmErr::Msg(
            "TypeError: Array species must be a constructor".into(),
        ));
    }
    interp.ctor(&constructor, vec![Value::Number(length)])
}

fn create_array_result_element(
    interp: &mut Interpreter,
    result: &Value,
    index: usize,
    value: Value,
) -> Result<(), VmErr> {
    create_array_result_element_number(interp, result, index as f64, value)
}

fn create_array_result_element_number(
    interp: &mut Interpreter,
    result: &Value,
    index: f64,
    value: Value,
) -> Result<(), VmErr> {
    let descriptor = Value::descriptor_record(vec![
        ("value".into(), value),
        ("writable".into(), Value::Bool(true)),
        ("enumerable".into(), Value::Bool(true)),
        ("configurable".into(), Value::Bool(true)),
    ]);
    if interp.define_own_property(
        result,
        &Value::String(crate::format::ecmascript_number_string(index).into()),
        &descriptor,
    )? {
        Ok(())
    } else {
        Err(VmErr::Msg(
            "TypeError: Cannot define array result element".into(),
        ))
    }
}

fn array_map(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let cb = callback(&a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    let result = array_species_create(interp, &this, length)?;
    let mut cursor = 0.0;
    while cursor < length {
        interp.consume_loop()?;
        let index = cursor;
        cursor += 1.0;
        if let Some(value) = array_element_number(interp, &this, index, true)? {
            let mapped = interp.call_this(
                &cb,
                receiver.clone(),
                vec![value, Value::Number(index), this.clone()],
            )?;
            create_array_result_element_number(interp, &result, index, mapped)?;
        }
    }
    Ok(result)
}

fn array_filter(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let cb = callback(&a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    let result = array_species_create(interp, &this, 0.0)?;
    let mut next = 0.0;
    let mut cursor = 0.0;
    while cursor < length {
        interp.consume_loop()?;
        let index = cursor;
        cursor += 1.0;
        let Some(value) = array_element_number(interp, &this, index, true)? else {
            continue;
        };
        if interp
            .call_this(
                &cb,
                receiver.clone(),
                vec![value.clone(), Value::Number(index), this.clone()],
            )?
            .is_truthy()
        {
            create_array_result_element_number(interp, &result, next, value)?;
            next += 1.0;
        }
    }
    Ok(result)
}

fn reduce_direction(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
    reverse: bool,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let cb = callback(&a)?;
    let mut acc = a.get(1).cloned();
    let mut step = 0.0;
    while step < length {
        interp.consume_loop()?;
        let index = if reverse { length - 1.0 - step } else { step };
        step += 1.0;
        let Some(value) = array_element_number(interp, &this, index, true)? else {
            continue;
        };
        acc = Some(match acc {
            None => value,
            Some(previous) => interp.call_this(
                &cb,
                Value::Undefined,
                vec![previous, value, Value::Number(index), this.clone()],
            )?,
        });
    }
    acc.ok_or_else(|| VmErr::Msg("TypeError: Reduce of empty array with no initial value".into()))
}

fn array_reduce(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    reduce_direction(interp, this, a, false)
}

fn array_for_each(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let cb = callback(&a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    let mut next = 0.0;
    while next < length {
        interp.consume_loop()?;
        let index = next;
        next += 1.0;
        if let Some(value) = array_element_number(interp, &this, index, true)? {
            interp.call_this(
                &cb,
                receiver.clone(),
                vec![value, Value::Number(index), this.clone()],
            )?;
        }
    }
    Ok(Value::Undefined)
}

fn array_find(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    search(interp, &this, &a, false, false)
}

fn array_some(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let cb = callback(&a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    let mut next = 0.0;
    while next < length {
        interp.consume_loop()?;
        let index = next;
        next += 1.0;
        let Some(value) = array_element_number(interp, &this, index, true)? else {
            continue;
        };
        let hit = interp.call_this(
            &cb,
            receiver.clone(),
            vec![value, Value::Number(index), this.clone()],
        )?;
        if hit.is_truthy() {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn array_every(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let cb = callback(&a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    let mut next = 0.0;
    while next < length {
        interp.consume_loop()?;
        let index = next;
        next += 1.0;
        let Some(value) = array_element_number(interp, &this, index, true)? else {
            continue;
        };
        let hit = interp.call_this(
            &cb,
            receiver.clone(),
            vec![value, Value::Number(index), this.clone()],
        )?;
        if !hit.is_truthy() {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

/// `LengthOfArrayLike`: `ToLength(Get(O, "length"))`, clamped to the VM's
/// array bound so a hostile `length` cannot drive an unbounded loop.
pub(super) fn array_like_length(interp: &mut Interpreter, this: &Value) -> Result<usize, VmErr> {
    let len_val = interp.get_prop_value_str(this, "length")?;
    let n = interp.ecmascript_to_number(&len_val)?;
    if !n.is_finite() || n <= 0.0 {
        return Ok(0);
    }
    Ok((n.trunc() as usize).min(crate::value::MAX_ARRAY_LEN))
}

/// Logical array-like length, independent of the cap on allocated arrays.
pub(super) fn array_like_length_number(
    interp: &mut Interpreter,
    this: &Value,
) -> Result<f64, VmErr> {
    let value = interp.get_prop_value_str(this, "length")?;
    Ok(to_integer_or_infinity(interp.ecmascript_to_number(&value)?)
        .clamp(0.0, 9_007_199_254_740_991.0))
}

fn array_push(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let mut length = array_like_length_number(interp, &this)?;
    let result_length = array_growth_length(&this, length, a.len())?;
    for value in a {
        interp.consume_loop()?;
        interp.set_member_str_or_throw(
            &this,
            &crate::format::ecmascript_number_string(length),
            value,
        )?;
        length += 1.0;
    }
    interp.set_member_str_or_throw(&this, "length", Value::Number(result_length))?;
    Ok(Value::Number(result_length))
}

fn array_pop(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    if length == 0.0 {
        interp.set_member_str_or_throw(&this, "length", Value::Number(0.0))?;
        return Ok(Value::Undefined);
    }
    let key = crate::format::ecmascript_number_string(length - 1.0);
    let result = interp.get_prop_value_str(&this, &key)?;
    delete_numeric_property_or_throw(interp, &this, key)?;
    interp.set_member_str_or_throw(&this, "length", Value::Number(length - 1.0))?;
    Ok(result)
}

thread_local! {
    static JOIN_STACK: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}
struct JoinGuard;
impl Drop for JoinGuard {
    fn drop(&mut self) {
        JOIN_STACK.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}
fn array_join(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    array_join_impl(interp, this, a, false)
}
fn array_to_locale_string(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    array_join_impl(interp, this, a, true)
}
fn array_join_impl(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
    locale: bool,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let separator = if locale || matches!(a.first(), None | Some(Value::Undefined)) {
        crate::JsString::from(",")
    } else {
        interp.ecmascript_to_string(&a[0])?
    };
    // Recursive array stringification renders an active array as an empty part.
    let _guard = if let Value::Array(array) = &this {
        let identity = Rc::as_ptr(array) as usize;
        if JOIN_STACK.with(|stack| stack.borrow().contains(&identity)) {
            return Ok(Value::String(crate::JsString::default()));
        }
        JOIN_STACK.with(|stack| stack.borrow_mut().push(identity));
        Some(JoinGuard)
    } else {
        None
    };
    let mut out = crate::JsString::default();
    let mut index = 0.0;
    while index < length {
        interp.check_execution()?;
        if length > crate::value::MAX_ARRAY_LEN as f64 {
            interp.consume_loop()?;
        }
        let value = array_element_number(interp, &this, index, false)?.unwrap_or(Value::Undefined);
        let part = if matches!(value, Value::Undefined | Value::Null) {
            crate::JsString::default()
        } else if locale {
            let method = interp.get_prop_value_str(&value, "toLocaleString")?;
            let result = interp.call_this(
                &method,
                value,
                vec![
                    a.first().cloned().unwrap_or(Value::Undefined),
                    a.get(1).cloned().unwrap_or(Value::Undefined),
                ],
            )?;
            interp.ecmascript_to_string(&result)?
        } else {
            interp.ecmascript_to_string(&value)?
        };
        let added = part
            .len()
            .saturating_add(if index > 0.0 { separator.len() } else { 0 });
        if out.len().saturating_add(added) > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
        if index > 0.0 {
            out.push_str(&separator);
        }
        out.push_str(&part);
        index += 1.0;
    }
    Value::checked_string(out)
}

/// `ToIntegerOrInfinity`, clamped into `[0, len]`, for the forward searches
/// (`indexOf`, `includes`): negatives count back from the end, `NaN` and
/// missing mean `0`.
fn array_index_of(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    if length == 0.0 {
        return Ok(Value::Number(-1.0));
    }
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let from =
        to_integer_or_infinity(interp.ecmascript_to_number(a.get(1).unwrap_or(&Value::Undefined))?);
    let mut index = if from < 0.0 {
        (length + from).max(0.0)
    } else {
        from
    };
    while index < length {
        // Preserve the existing bounded-search policy. Logical lengths beyond
        // the allocation bound additionally need a loop limit for sparse scans.
        if length > crate::value::MAX_ARRAY_LEN as f64 {
            interp.consume_loop()?;
        } else {
            interp.check_execution()?;
        }
        if let Some(value) = array_element_number(interp, &this, index, true)?
            && interp.seq(&value, &target)
        {
            return Ok(Value::Number(if index == 0.0 { 0.0 } else { index }));
        }
        index += 1.0;
    }
    Ok(Value::Number(-1.0))
}

fn array_includes(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    if length == 0.0 {
        return Ok(Value::Bool(false));
    }
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let raw = a
        .get(1)
        .map(|value| interp.ecmascript_to_number(value))
        .transpose()?;
    let from = to_integer_or_infinity(raw.unwrap_or(0.0));
    let mut index = if from < 0.0 {
        (length + from).max(0.0)
    } else {
        from
    };
    // Includes performs Get even for holes, and compares using SameValueZero.
    let target_is_nan = matches!(&target, Value::Number(n) if n.is_nan());
    while index < length {
        interp.consume_loop()?;
        let value =
            interp.get_prop_value_str(&this, &crate::format::ecmascript_number_string(index))?;
        if interp.seq(&value, &target)
            || (target_is_nan && matches!(value, Value::Number(n) if n.is_nan()))
        {
            return Ok(Value::Bool(true));
        }
        index += 1.0;
    }
    Ok(Value::Bool(false))
}

fn array_slice(
    interp: &mut Interpreter,
    this: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    // Read `length` once: the logical (spec) length drives the 2^32-1
    // ArrayCreate check while the clamped length bounds the allocation.
    let length_value = interp.get_prop_value_str(&this, "length")?;
    let length_number = interp.ecmascript_to_number(&length_value)?;
    let logical = to_integer_or_infinity(length_number).clamp(0.0, 9_007_199_254_740_991.0);
    let length = if !length_number.is_finite() || length_number <= 0.0 {
        0
    } else {
        (length_number.trunc() as usize).min(crate::value::MAX_ARRAY_LEN)
    };
    let start_number =
        interp.ecmascript_to_number(arguments.first().unwrap_or(&Value::Undefined))?;
    let end_number = match arguments.get(1) {
        None | Some(Value::Undefined) => None,
        Some(value) => Some(interp.ecmascript_to_number(value)?),
    };
    // ArrayCreate throws for lengths above 2^32-1. The clamped `length`
    // above would swallow that spec error, so validate the logical count
    // first; the clamped path below still bounds the actual allocation.
    let logical_start = relative_number_index(start_number, logical);
    let logical_end = end_number.map_or(logical, |end| relative_number_index(end, logical));
    if logical_end - logical_start > 4294967295.0 {
        return Err(VmErr::Msg("RangeError: Invalid array length".into()));
    }
    let start = relative_index(start_number, length);
    let end = match end_number {
        None => length,
        Some(value) => relative_index(value, length),
    };
    let count = end.saturating_sub(start);
    let result = array_species_create(interp, &this, count as f64)?;
    for offset in 0..count {
        interp.consume_loop()?;
        if let Some(value) = array_element(interp, &this, start + offset, true)? {
            create_array_result_element(interp, &result, offset, value)?;
        }
    }
    interp.set_member_str_or_throw(&result, "length", Value::Number(count as f64))?;
    Ok(result)
}

fn array_concat(
    interp: &mut Interpreter,
    this: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let result = array_species_create(interp, &this, 0.0)?;
    let spreadable = super::well_known("isConcatSpreadable").expect("Symbol.isConcatSpreadable");
    let mut next = 0;
    for item in std::iter::once(this).chain(arguments) {
        let spread = if crate::interpreter::call::is_js_object(&item) {
            let value = interp.get_prop_value(&item, &spreadable)?;
            if matches!(value, Value::Undefined) {
                array_is_array(interp, Value::Undefined, vec![item.clone()])?.is_truthy()
            } else {
                value.is_truthy()
            }
        } else {
            false
        };
        if spread {
            let length = array_like_length(interp, &item)?;
            if next + length > crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum array length exceeded"));
            }
            for index in 0..length {
                interp.consume_loop()?;
                if let Some(value) = array_element(interp, &item, index, true)? {
                    create_array_result_element(interp, &result, next, value)?;
                }
                next += 1;
            }
        } else {
            if next >= crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum array length exceeded"));
            }
            create_array_result_element(interp, &result, next, item)?;
            next += 1;
        }
    }
    interp.set_member_str_or_throw(&result, "length", Value::Number(next as f64))?;
    Ok(result)
}

fn array_reverse(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let mut lower = 0.0;
    while lower < (length / 2.0).floor() {
        interp.consume_loop()?;
        let upper = length - lower - 1.0;
        let low = array_element_number(interp, &this, lower, true)?;
        let high = array_element_number(interp, &this, upper, true)?;
        let low_key = crate::format::ecmascript_number_string(lower);
        let high_key = crate::format::ecmascript_number_string(upper);
        match (low, high) {
            (Some(low), Some(high)) => {
                interp.set_member_str_or_throw(&this, &low_key, high)?;
                interp.set_member_str_or_throw(&this, &high_key, low)?;
            }
            (None, Some(high)) => {
                interp.set_member_str_or_throw(&this, &low_key, high)?;
                delete_numeric_property_or_throw(interp, &this, high_key)?;
            }
            (Some(low), None) => {
                delete_numeric_property_or_throw(interp, &this, low_key)?;
                interp.set_member_str_or_throw(&this, &high_key, low)?;
            }
            (None, None) => {}
        }
        lower += 1.0;
    }
    Ok(this)
}

fn delete_numeric_property_or_throw(
    interp: &mut Interpreter,
    this: &Value,
    key: String,
) -> Result<(), VmErr> {
    if interp
        .delete_member(this, &Value::String(key.into()))?
        .is_truthy()
    {
        Ok(())
    } else {
        Err(VmErr::Msg("TypeError: Cannot delete array property".into()))
    }
}

fn array_to_sorted(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let comparator = args.first().cloned().unwrap_or(Value::Undefined);
    if !matches!(comparator, Value::Undefined)
        && !crate::interpreter::call::is_callable_value(&comparator)
    {
        return Err(VmErr::Msg("TypeError: comparator is not a function".into()));
    }
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    if length > crate::value::MAX_ARRAY_LEN as f64 {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    let mut values = Vec::with_capacity(length as usize);
    for index in 0..length as usize {
        interp.consume_loop()?;
        values.push(interp.get_prop_value_str(&this, &index.to_string())?);
    }
    merge_sort_values(interp, &mut values, &comparator)?;
    Value::checked_array(values)
}

fn compare_sort_values(
    interp: &mut Interpreter,
    comparator: &Value,
    left: &Value,
    right: &Value,
) -> Result<Ordering, VmErr> {
    if matches!(left, Value::Undefined) {
        return Ok(if matches!(right, Value::Undefined) {
            Ordering::Equal
        } else {
            Ordering::Greater
        });
    }
    if matches!(right, Value::Undefined) {
        return Ok(Ordering::Less);
    }
    if matches!(comparator, Value::Undefined) {
        let left = interp.ecmascript_to_string(left)?;
        let right = interp.ecmascript_to_string(right)?;
        return Ok(left.cmp(&right));
    }
    let result = interp.call_this(
        comparator,
        Value::Undefined,
        vec![left.clone(), right.clone()],
    )?;
    let number = interp.ecmascript_to_number(&result)?;
    Ok(number.partial_cmp(&0.0).unwrap_or(Ordering::Equal))
}

fn merge_sort_values(
    interp: &mut Interpreter,
    values: &mut [Value],
    comparator: &Value,
) -> Result<(), VmErr> {
    if values.len() < 2 {
        return Ok(());
    }
    let midpoint = values.len() / 2;
    merge_sort_values(interp, &mut values[..midpoint], comparator)?;
    merge_sort_values(interp, &mut values[midpoint..], comparator)?;

    // Clone the halves before invoking guest code. The comparator can mutate
    // the original array, but it must not encounter a Rust borrow of this
    // temporary sort buffer.
    let left = values[..midpoint].to_vec();
    let right = values[midpoint..].to_vec();
    let mut merged = Vec::with_capacity(values.len());
    let mut left_index = 0;
    let mut right_index = 0;
    while left_index < left.len() && right_index < right.len() {
        let order =
            compare_sort_values(interp, comparator, &left[left_index], &right[right_index])?;
        if order == Ordering::Greater {
            merged.push(right[right_index].clone());
            right_index += 1;
        } else {
            // Taking the left value for equality preserves sort stability.
            merged.push(left[left_index].clone());
            left_index += 1;
        }
    }
    merged.extend(left[left_index..].iter().cloned());
    merged.extend(right[right_index..].iter().cloned());
    values.clone_from_slice(&merged);
    Ok(())
}

fn array_sort(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let comparator = a.first().cloned().unwrap_or(Value::Undefined);
    if !matches!(comparator, Value::Undefined)
        && !crate::interpreter::is_callable_value(&comparator)
    {
        return Err(VmErr::Msg("TypeError: comparator is not a function".into()));
    }
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length_number(interp, &this)?;
    let mut values = Vec::new();
    let mut index = 0.0;
    while index < length {
        interp.check_execution()?;
        if length > crate::value::MAX_ARRAY_LEN as f64 {
            interp.consume_loop()?;
        }
        if let Some(value) = array_element_number(interp, &this, index, true)? {
            if values.len() >= crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum array length exceeded"));
            }
            values.push(value);
        }
        index += 1.0;
    }
    merge_sort_values(interp, &mut values, &comparator)?;
    for (index, value) in values.iter().enumerate() {
        interp.set_member_str_or_throw(&this, &index.to_string(), value.clone())?;
    }
    index = values.len() as f64;
    while index < length {
        interp.check_execution()?;
        if length > crate::value::MAX_ARRAY_LEN as f64 {
            interp.consume_loop()?;
        }
        delete_numeric_property_or_throw(
            interp,
            &this,
            crate::format::ecmascript_number_string(index),
        )?;
        index += 1.0;
    }
    Ok(this)
}

fn flatten_array_result(
    interp: &mut Interpreter,
    source: Value,
    length: usize,
    depth: f64,
    mapper: Option<(Value, Value)>,
) -> Result<Value, VmErr> {
    struct Frame {
        source: Value,
        length: usize,
        index: usize,
        depth: f64,
    }
    let result = array_species_create(interp, &source, 0.0)?;
    let mut frames = vec![Frame {
        source: source.clone(),
        length,
        index: 0,
        depth,
    }];
    let mut next = 0;
    while let Some(frame) = frames.last_mut() {
        if frame.index == frame.length {
            frames.pop();
            continue;
        }
        let index = frame.index;
        frame.index += 1;
        let current = frame.source.clone();
        let remaining = frame.depth;
        interp.consume_loop()?;
        let Some(mut value) = array_element(interp, &current, index, true)? else {
            continue;
        };
        if frames.len() == 1
            && let Some((callback, receiver)) = &mapper
        {
            value = interp.call_this(
                callback,
                receiver.clone(),
                vec![value, Value::Number(index as f64), source.clone()],
            )?;
        }
        if remaining > 0.0
            && array_is_array(interp, Value::Undefined, vec![value.clone()])?.is_truthy()
        {
            let length = array_like_length(interp, &value)?;
            frames.push(Frame {
                source: value,
                length,
                index: 0,
                depth: remaining - 1.0,
            });
        } else {
            if next >= crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum array length exceeded"));
            }
            create_array_result_element(interp, &result, next, value)?;
            next += 1;
        }
    }
    Ok(result)
}

fn array_flat(
    interp: &mut Interpreter,
    this: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length(interp, &this)?;
    let depth = match arguments.first() {
        None | Some(Value::Undefined) => 1.0,
        Some(value) => to_integer_or_infinity(interp.ecmascript_to_number(value)?).max(0.0),
    };
    flatten_array_result(interp, this, length, depth, None)
}

fn array_flat_map(
    interp: &mut Interpreter,
    this: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length(interp, &this)?;
    let callback = callback(&arguments)?;
    let receiver = arguments.get(1).cloned().unwrap_or(Value::Undefined);
    flatten_array_result(interp, this, length, 1.0, Some((callback, receiver)))
}

fn array_reduce_right(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    reduce_direction(interp, this, a, true)
}
