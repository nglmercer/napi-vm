//! `Array` statics and `Array.prototype` methods.

use std::cmp::Ordering;
use std::rc::Rc;

use super::{NativeFn, arr_items, join_str, nf};
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{Value, to_integer_or_infinity};

/// ArraySetLength converts the original value twice: ToUint32, then ToNumber.
/// Both observable conversions precede descriptor compatibility checks.
pub(super) fn array_length_value(interp: &mut Interpreter, value: &Value) -> Result<Value, VmErr> {
    let unsigned = crate::value::to_int32(interp.ecmascript_to_number(value)?) as u32;
    let number = interp.ecmascript_to_number(value)?;
    if number != unsigned as f64 {
        return Err(VmErr::Msg("RangeError: Invalid array length".into()));
    }
    if unsigned as usize > crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
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
    let key = Value::String((index.to_string()).into());
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

fn arr_presence(this: &Value) -> Vec<bool> {
    match this {
        Value::Array(array) => array.presence_snapshot(),
        _ => vec![true; arr_items(this).len()],
    }
}

pub(super) fn install(e: &mut Environment) {
    if let Some(a) = e.get("Array") {
        let statics: &[(&str, NativeFn)] = &[
            ("isArray", array_is_array),
            ("from", array_from),
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
    let mut value = a.first().cloned().unwrap_or(Value::Undefined);
    // ECMAScript's IsArray operation follows Proxy [[ProxyTarget]] links.
    // Iterate so deeply nested proxies do not consume the Rust call stack.
    while let Value::Proxy(proxy) = &value {
        value = proxy.snapshot()?.0;
    }
    Ok(Value::Bool(matches!(value, Value::Array(_))))
}

/// `Array(n)` allocates `n` holes; `Array(a, b, …)` collects its arguments.
fn array_ctor(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if let [Value::Number(n)] = a.as_slice() {
        if !n.is_finite() || *n < 0.0 || n.fract() != 0.0 {
            return Err(crate::value::limit_err("Invalid array length"));
        }
        if *n > crate::value::MAX_ARRAY_LEN as f64 {
            return Err(crate::value::limit_err("Maximum array length exceeded"));
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
    let join = interp.prop_str(&this, "join")?;
    if matches!(
        join,
        Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
    ) {
        return interp.call_this(&join, this, vec![]);
    }
    let object_prototype = interp
        .global
        .borrow()
        .get("Object")
        .and_then(|object| object.get_prop("prototype"))
        .ok_or_else(|| VmErr::Msg("TypeError: Object.prototype is unavailable".into()))?;
    let method = interp.prop_str(&object_prototype, "toString")?;
    interp.call_this(&method, this, vec![])
}

/// `splice(start, deleteCount, ...items)`: remove a range in place and insert
/// replacements, returning what was removed.
fn delete_array_property_or_throw(
    interp: &mut Interpreter,
    object: &Value,
    index: usize,
) -> Result<(), VmErr> {
    if !interp
        .delete_member(object, &Value::String(index.to_string().into()))?
        .is_truthy()
    {
        return Err(VmErr::Msg("TypeError: Cannot delete array property".into()));
    }
    Ok(())
}

fn array_splice(
    interp: &mut Interpreter,
    this: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length(interp, &this)?;
    let start = relative_index(
        interp.ecmascript_to_number(arguments.first().unwrap_or(&Value::Undefined))?,
        length,
    );
    let remove = if arguments.is_empty() {
        0
    } else if arguments.len() == 1 {
        length - start
    } else {
        to_integer_or_infinity(interp.ecmascript_to_number(&arguments[1])?)
            .max(0.0)
            .min((length - start) as f64) as usize
    };
    let inserted = arguments.len().saturating_sub(2);
    let new_length = length - remove + inserted;
    if new_length > crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    let result = array_species_create(interp, &this, remove)?;
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
fn array_at(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let index = to_integer_or_infinity(a.first().map(|v| v.to_number()).unwrap_or(0.0));
    let index = if index < 0.0 {
        items.len() as f64 + index
    } else {
        index
    };
    if !index.is_finite() || index < 0.0 || index >= items.len() as f64 {
        return Ok(Value::Undefined);
    }
    Ok(items[index as usize].clone())
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
    let length = array_like_length(interp, this)?;
    let predicate = callback(a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    for step in 0..length {
        let index = if reverse { length - 1 - step } else { step };
        let value = array_element(interp, this, index, false)?.expect("holes are visited");
        let hit = interp.call_this(
            &predicate,
            receiver.clone(),
            vec![value.clone(), Value::Number(index as f64), this.clone()],
        )?;
        if hit.is_truthy() {
            return Ok(if want_index {
                Value::Number(index as f64)
            } else {
                value
            });
        }
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
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let needle = a.first().cloned().unwrap_or(Value::Undefined);
    if items.is_empty() {
        return Ok(Value::Number(-1.0));
    }
    // Unlike the string version, an explicit `undefined` converts to 0 (only
    // a missing argument means "the whole array"), and negatives wrap.
    let start = match a.get(1) {
        None => items.len() - 1,
        Some(v) => {
            let n = to_integer_or_infinity(v.to_number());
            if n >= 0.0 {
                (n as usize).min(items.len() - 1)
            } else {
                let k = (items.len() as i64).saturating_add(n as i64);
                if k < 0 {
                    return Ok(Value::Number(-1.0));
                }
                k as usize
            }
        }
    };
    for index in (0..=start).rev() {
        if presence.get(index).copied().unwrap_or(true) && interp.seq(&items[index], &needle) {
            return Ok(Value::Number(index as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

/// `shift()`: remove and return the first element.
fn array_shift(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let Value::Array(cell) = &this else {
        return Ok(Value::Undefined);
    };
    reject_frozen_array_mutation(cell)?;
    let mut items = cell.borrow_mut();
    if items.is_empty() {
        return Ok(Value::Undefined);
    }
    let removed = items.remove(0);
    let length = items.len();
    drop(items);
    cell.remove_presence(0);
    cell.truncate_presence(length);
    Ok(removed)
}

/// `unshift(...values)`: prepend, returning the new length.
fn array_unshift(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let Value::Array(cell) = &this else {
        return Ok(Value::Number(0.0));
    };
    reject_frozen_array_mutation(cell)?;
    let added = a.len();
    let mut items = cell.borrow_mut();
    if items.len().saturating_add(a.len()) > crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    for (offset, value) in a.into_iter().enumerate() {
        items.insert(offset, value);
    }
    let length = items.len();
    drop(items);
    cell.insert_present(0, added);
    Ok(Value::Number(length as f64))
}

/// `fill(value, start, end)`.
fn array_fill(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let Value::Array(cell) = &this else {
        return Ok(this.clone());
    };
    let length = cell.borrow().len();
    let resolve = |value: Option<&Value>, default: usize| -> usize {
        match value {
            Some(Value::Undefined) | None => default,
            Some(v) => relative_index(v.to_number(), length),
        }
    };
    let start = resolve(a.get(1), 0);
    let end = resolve(a.get(2), length).max(start);
    if start < end {
        reject_frozen_array_mutation(cell)?;
    }
    let value = a.first().cloned().unwrap_or(Value::Undefined);
    {
        let mut items = cell.borrow_mut();
        for slot in items.iter_mut().take(end).skip(start) {
            *slot = value.clone();
        }
    }
    cell.fill_presence(start, end);
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
    length: usize,
) -> Result<Value, VmErr> {
    let default =
        || Value::checked_array_with_presence(vec![Value::Undefined; length], vec![false; length]);
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
    interp.ctor(&constructor, vec![Value::Number(length as f64)])
}

fn create_array_result_element(
    interp: &mut Interpreter,
    result: &Value,
    index: usize,
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
        &Value::String(index.to_string().into()),
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
    let length = array_like_length(interp, &this)?;
    let cb = callback(&a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    let result = array_species_create(interp, &this, length)?;
    for index in 0..length {
        if let Some(value) = array_element(interp, &this, index, true)? {
            let mapped = interp.call_this(
                &cb,
                receiver.clone(),
                vec![value, Value::Number(index as f64), this.clone()],
            )?;
            create_array_result_element(interp, &result, index, mapped)?;
        }
    }
    Ok(result)
}

fn array_filter(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length(interp, &this)?;
    let cb = callback(&a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    let result = array_species_create(interp, &this, 0)?;
    let mut next = 0;
    for index in 0..length {
        let Some(value) = array_element(interp, &this, index, true)? else {
            continue;
        };
        if interp
            .call_this(
                &cb,
                receiver.clone(),
                vec![value.clone(), Value::Number(index as f64), this.clone()],
            )?
            .is_truthy()
        {
            create_array_result_element(interp, &result, next, value)?;
            next += 1;
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
    let length = array_like_length(interp, &this)?;
    let cb = callback(&a)?;
    let mut acc = a.get(1).cloned();
    for step in 0..length {
        let index = if reverse { length - 1 - step } else { step };
        let Some(value) = array_element(interp, &this, index, true)? else {
            continue;
        };
        acc = Some(match acc {
            None => value,
            Some(previous) => interp.call_this(
                &cb,
                Value::Undefined,
                vec![previous, value, Value::Number(index as f64), this.clone()],
            )?,
        });
    }
    acc.ok_or_else(|| VmErr::Msg("TypeError: Reduce of empty array with no initial value".into()))
}

fn array_reduce(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    reduce_direction(interp, this, a, false)
}

fn array_for_each(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let length = array_like_length(interp, &this)?;
    let cb = callback(&a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    for index in 0..length {
        if let Some(value) = array_element(interp, &this, index, true)? {
            interp.call_this(
                &cb,
                receiver.clone(),
                vec![value, Value::Number(index as f64), this.clone()],
            )?;
        }
    }
    Ok(Value::Undefined)
}

fn array_find(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    search(interp, &this, &a, false, false)
}

fn array_some(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let length = array_like_length(interp, &this)?;
    let cb = callback(&a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    for index in 0..length {
        let Some(value) = array_element(interp, &this, index, true)? else {
            continue;
        };
        let hit = interp.call_this(
            &cb,
            receiver.clone(),
            vec![value, Value::Number(index as f64), this.clone()],
        )?;
        if hit.is_truthy() {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn array_every(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let length = array_like_length(interp, &this)?;
    let cb = callback(&a)?;
    let receiver = a.get(1).cloned().unwrap_or(Value::Undefined);
    for index in 0..length {
        let Some(value) = array_element(interp, &this, index, true)? else {
            continue;
        };
        let hit = interp.call_this(
            &cb,
            receiver.clone(),
            vec![value, Value::Number(index as f64), this.clone()],
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

fn array_push(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if let Value::Array(items) = &this {
        reject_frozen_array_mutation(items)?;
        let added = a.len();
        let mut b = items.borrow_mut();
        if b.len().saturating_add(a.len()) > crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum array length exceeded"));
        }
        for x in a {
            b.push(x);
        }
        let length = b.len();
        drop(b);
        items.append_present(added);
        return Ok(Value::Number(length as f64));
    }
    // `push` is generic: `class NodeList extends Array` instances (plain
    // objects in this VM) and other array-likes push through `length`.
    if matches!(this, Value::Null | Value::Undefined) {
        return Err(VmErr::Msg(
            "TypeError: Cannot convert undefined or null to object".to_string(),
        ));
    }
    let mut len = array_like_length(interp, &this)?;
    if len.saturating_add(a.len()) > crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    for x in a {
        interp.assign_member(&this, &Value::String((len.to_string()).into()), x)?;
        len += 1;
    }
    interp.assign_member_str(&this, "length", Value::Number(len as f64))?;
    Ok(Value::Number(len as f64))
}

fn array_pop(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    if let Value::Array(items) = &this {
        reject_frozen_array_mutation(items)?;
        let result = items.borrow_mut().pop().unwrap_or(Value::Undefined);
        let length = items.borrow().len();
        items.truncate_presence(length);
        return Ok(result);
    }
    if matches!(this, Value::Null | Value::Undefined) {
        return Err(VmErr::Msg(
            "TypeError: Cannot convert undefined or null to object".to_string(),
        ));
    }
    let len = array_like_length(interp, &this)?;
    if len == 0 {
        interp.assign_member_str(&this, "length", Value::Number(0.0))?;
        return Ok(Value::Undefined);
    }
    let new_len = len - 1;
    let key = Value::String((new_len.to_string()).into());
    let result = interp.get_prop_value(&this, &key)?;
    interp.delete_member(&this, &key)?;
    interp.assign_member_str(&this, "length", Value::Number(new_len as f64))?;
    Ok(result)
}

fn array_join(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let sep = match a.first() {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Undefined) | None => crate::JsString::from(","),
        Some(v) => interp.to_js_string(v)?,
    };
    let mut out = crate::JsString::default();
    // This renderer does not execute guest code. Shared child arrays have an
    // identical rendering during one join; cache it to keep bounded joins fast.
    let mut shared = std::collections::HashMap::new();
    for (index, value) in items.iter().enumerate() {
        if index > 0 {
            if out.len().saturating_add(sep.len()) > crate::value::MAX_STRING_LEN {
                return Err(crate::value::limit_err("Maximum string length exceeded"));
            }
            out.push_str(&sep);
        }
        let part = if let Value::Array(array) = value {
            let key = std::rc::Rc::as_ptr(array);
            if let Some(part) = shared.get(&key) {
                crate::JsString::clone(part)
            } else {
                let part = join_str(interp, value)?;
                shared.insert(key, part.clone());
                part
            }
        } else {
            join_str(interp, value)?
        };
        if out.len().saturating_add(part.len()) > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
        out.push_str(&part);
    }
    Value::checked_string(out)
}

/// `ToIntegerOrInfinity`, clamped into `[0, len]`, for the forward searches
/// (`indexOf`, `includes`): negatives count back from the end, `NaN` and
/// missing mean `0`.
fn forward_from_index(raw: Option<f64>, len: usize) -> usize {
    let n = raw.unwrap_or(0.0);
    if n.is_nan() {
        return 0;
    }
    // `as` saturates infinities and truncates toward zero, matching
    // `ToIntegerOrInfinity`; `saturating_add` keeps `-Infinity` from
    // overflowing.
    let i = n as i64;
    if i < 0 {
        (len as i64).saturating_add(i).max(0) as usize
    } else {
        (i as usize).min(len)
    }
}

fn array_index_of(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let from = forward_from_index(a.get(1).map(|v| v.to_number()), items.len());
    for (i, it) in items.iter().enumerate().skip(from) {
        if presence.get(i).copied().unwrap_or(true) && interp.seq(it, &target) {
            return Ok(Value::Number(i as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

fn array_includes(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let from = forward_from_index(a.get(1).map(|v| v.to_number()), items.len());
    // Holes read as `undefined` here (no presence check), and comparison is
    // `SameValueZero`: unlike `indexOf`, `NaN` matches itself.
    let target_is_nan = matches!(&target, Value::Number(n) if n.is_nan());
    for it in items.iter().skip(from) {
        if interp.seq(it, &target)
            || (target_is_nan && matches!(it, Value::Number(n) if n.is_nan()))
        {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn array_slice(
    interp: &mut Interpreter,
    this: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = super::object::to_object_receiver(&this)?;
    let length = array_like_length(interp, &this)?;
    let start = relative_index(
        interp.ecmascript_to_number(arguments.first().unwrap_or(&Value::Undefined))?,
        length,
    );
    let end = match arguments.get(1) {
        None | Some(Value::Undefined) => length,
        Some(value) => relative_index(interp.ecmascript_to_number(value)?, length),
    };
    let count = end.saturating_sub(start);
    let result = array_species_create(interp, &this, count)?;
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
    let result = array_species_create(interp, &this, 0)?;
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

fn array_reverse(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    if let Value::Array(items) = &this {
        if items.borrow().len() > 1 {
            reject_frozen_array_mutation(items)?;
        }
        items.borrow_mut().reverse();
        items.reverse_presence();
        return Ok(this);
    }
    Ok(Value::Undefined)
}

fn compare_sort_values(
    interp: &mut Interpreter,
    comparator: &Value,
    left: &Value,
    right: &Value,
) -> Result<Ordering, VmErr> {
    let number = interp
        .call_this(
            comparator,
            Value::Undefined,
            vec![left.clone(), right.clone()],
        )?
        .to_number();
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
    let cmp = a.first().cloned().unwrap_or(Value::Undefined);
    if !matches!(cmp, Value::Undefined) && !crate::interpreter::call::is_callable_value(&cmp) {
        return Err(VmErr::Msg("TypeError: comparator is not a function".into()));
    }
    if let Value::Array(items) = &this {
        // Never hold the array's RefCell borrow while invoking guest code.
        // Comparators are allowed to re-enter the array (for example by
        // calling `push`), and keeping the RefMut across the callback used to
        // turn that normal JavaScript re-entry into a Rust RefCell panic.
        let original_values = items.borrow().clone();
        let original_presence = items.presence_snapshot();
        let original_len = original_values.len();
        if original_len > 1 {
            reject_frozen_array_mutation(items)?;
        }
        let present_undefined = original_values
            .iter()
            .zip(&original_presence)
            .filter(|(value, present)| **present && matches!(value, Value::Undefined))
            .count();
        let hole_count = original_presence
            .iter()
            .filter(|present| !**present)
            .count();
        let mut sorted: Vec<Value> = original_values
            .into_iter()
            .zip(original_presence.iter().copied())
            .filter_map(|(value, present)| {
                (present && !matches!(value, Value::Undefined)).then_some(value)
            })
            .collect();
        if !matches!(cmp, Value::Undefined) {
            // Comparator errors must escape sort. Converting them to zero
            // silently changes program behavior and leaves callers unable to
            // catch the original exception.
            merge_sort_values(interp, &mut sorted, &cmp)?;
        } else {
            // Default: lexicographic comparison of the stringified elements.
            let mut format_error = None;
            sorted.sort_by(|left, right| {
                if format_error.is_some() {
                    return Ordering::Equal;
                }
                match (interp.to_js_string(left), interp.to_js_string(right)) {
                    (Ok(left), Ok(right)) => left.cmp(&right),
                    (Err(error), _) | (_, Err(error)) => {
                        format_error = Some(error);
                        Ordering::Equal
                    }
                }
            });
            if let Some(error) = format_error {
                return Err(error);
            }
        }
        let defined_len = sorted.len();
        sorted.resize(original_len - hole_count, Value::Undefined);
        let mut sorted_presence = vec![true; original_len - hole_count];
        sorted_presence.resize(original_len, false);
        debug_assert_eq!(defined_len + present_undefined + hole_count, original_len);

        // Comparators may mutate the array. Keep values appended beyond the
        // captured sort length, while the sorted prefix follows the captured
        // values and preserves holes at the end.
        let mut presence_after_compare = items.presence_snapshot();
        let mut current = items.borrow_mut();
        if current.len() < original_len {
            current.resize(original_len, Value::Undefined);
            presence_after_compare.resize(original_len, false);
        }
        for (index, value) in sorted.into_iter().enumerate() {
            current[index] = value;
        }
        if presence_after_compare.len() > original_len {
            sorted_presence.extend_from_slice(&presence_after_compare[original_len..]);
        }
        drop(current);
        items.replace_presence(sorted_presence);
    }
    Ok(this)
}

fn reject_frozen_array_mutation(array: &crate::value::ArrayCell) -> Result<(), VmErr> {
    if array.is_integrity_locked(true) {
        Err(VmErr::Msg(
            "TypeError: Cannot modify a frozen array".to_owned(),
        ))
    } else {
        Ok(())
    }
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
    let result = array_species_create(interp, &source, 0)?;
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
