//! `Array` statics and `Array.prototype` methods.

use std::cmp::Ordering;
use std::rc::Rc;

use super::{NativeFn, arr_items, join_str, nf};
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::Value;

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
                    array_method("values").expect("Array.prototype.values"),
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
    Ok(Value::Bool(matches!(a.first(), Some(Value::Array(_)))))
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

fn array_of(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_array(a)
}

/// `Array.from(source, mapFn?)`: drains an iterable, or reads `length` and the
/// index properties of an array-like.
fn array_from(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let source = a.first().cloned().unwrap_or(Value::Undefined);
    let mapper = a.get(1).cloned();
    let items = match &source {
        Value::Array(items) => items.borrow().clone(),
        Value::Undefined | Value::Null => {
            return Err(VmErr::Msg(
                "TypeError: Array.from requires an array-like or iterable".to_string(),
            ));
        }
        // An array-like has a `length` but no iterator; the iterator protocol
        // takes precedence when both are present, as in the specification.
        Value::Object { .. }
            if matches!(
                interp.prop(
                    &source,
                    &Value::String(crate::interpreter::SYMBOL_ITERATOR_SLOT.to_string())
                )?,
                Value::Undefined
            ) =>
        {
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
    let Some(mapper) = mapper.filter(|m| !matches!(m, Value::Undefined | Value::Null)) else {
        return Value::checked_array(items);
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        out.push(interp.call_this(
            &mapper,
            Value::Undefined,
            vec![item, Value::Number(index as f64)],
        )?);
    }
    Value::checked_array(out)
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

/// `splice(start, deleteCount, ...items)`: remove a range in place and insert
/// replacements, returning what was removed.
fn array_splice(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let Value::Array(cell) = &this else {
        return Value::checked_array(vec![]);
    };
    let length = cell.borrow().len();
    let start = match a.first() {
        Some(v) => {
            let n = v.to_number();
            if !n.is_finite() {
                if n > 0.0 { length } else { 0 }
            } else if n < 0.0 {
                ((length as f64 + n).max(0.0)) as usize
            } else {
                (n as usize).min(length)
            }
        }
        None => 0,
    };
    let remove = match a.get(1) {
        // No `deleteCount` removes everything from `start` onwards.
        None => length - start,
        Some(v) => {
            let n = v.to_number();
            if !n.is_finite() || n <= 0.0 {
                0
            } else {
                (n as usize).min(length - start)
            }
        }
    };
    let inserted: Vec<Value> = a.iter().skip(2).cloned().collect();
    if length - remove + inserted.len() > crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    let mut presence = cell.presence_snapshot();
    let removed_presence: Vec<bool> = presence
        .splice(start..start + remove, vec![true; inserted.len()])
        .collect();
    let removed: Vec<Value> = cell
        .borrow_mut()
        .splice(start..start + remove, inserted)
        .collect();
    cell.replace_presence(presence);
    Value::checked_array_with_presence(removed, removed_presence)
}

/// `at(index)`: a negative index counts back from the end.
fn array_at(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let index = a.first().map(|v| v.to_number()).unwrap_or(0.0);
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
    let items = arr_items(this);
    let predicate = a.first().cloned().unwrap_or(Value::Undefined);
    let positions: Vec<usize> = if reverse {
        (0..items.len()).rev().collect()
    } else {
        (0..items.len()).collect()
    };
    for index in positions {
        let hit = interp.call_this(
            &predicate,
            Value::Undefined,
            vec![
                items[index].clone(),
                Value::Number(index as f64),
                this.clone(),
            ],
        )?;
        if hit.is_truthy() {
            return Ok(if want_index {
                Value::Number(index as f64)
            } else {
                items[index].clone()
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
    for index in (0..items.len()).rev() {
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
    let start = resolve(a.get(1), 0);
    let end = resolve(a.get(2), length).max(start);
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

/// Build an iterator over a projection of the elements. `keys()`, `values()`
/// and `entries()` differ only in what each step produces.
fn iterate_projection(
    interp: &mut Interpreter,
    this: &Value,
    project: impl Fn(usize, &Value) -> Value,
) -> Result<Value, VmErr> {
    let projected: Vec<Value> = arr_items(this)
        .iter()
        .enumerate()
        .map(|(index, value)| project(index, value))
        .collect();
    let array = Value::checked_array(projected)?;
    let iterator = interp.prop(
        &array,
        &Value::String(crate::interpreter::SYMBOL_ITERATOR_SLOT.to_string()),
    )?;
    interp.call_this(&iterator, array, vec![])
}

fn array_keys(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    iterate_projection(interp, &this, |index, _| Value::Number(index as f64))
}

fn array_values(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    crate::interpreter::array_iter(interp, this, Vec::new())
}

fn array_entries(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    iterate_projection(interp, &this, |index, value| {
        Value::array(vec![Value::Number(index as f64), value.clone()])
    })
}

fn array_map(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let mut out = Vec::with_capacity(items.len());
    let mut out_presence = Vec::with_capacity(items.len());
    for (i, it) in items.iter().enumerate() {
        if !presence.get(i).copied().unwrap_or(true) {
            out.push(Value::Undefined);
            out_presence.push(false);
            continue;
        }
        let r = interp.call_this(
            &cb,
            Value::Undefined,
            vec![it.clone(), Value::Number(i as f64), this.clone()],
        )?;
        out.push(r);
        out_presence.push(true);
    }
    Value::checked_array_with_presence(out, out_presence)
}

fn array_filter(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let mut out = Vec::new();
    for (i, it) in items.iter().enumerate() {
        if !presence.get(i).copied().unwrap_or(true) {
            continue;
        }
        let keep = interp.call_this(
            &cb,
            Value::Undefined,
            vec![it.clone(), Value::Number(i as f64), this.clone()],
        )?;
        if keep.is_truthy() {
            out.push(it.clone());
        }
    }
    Value::checked_array(out)
}

fn array_reduce(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let (mut acc, start) = if a.len() >= 2 {
        (a[1].clone(), 0)
    } else {
        let Some(first) = presence.iter().position(|present| *present) else {
            return Err(VmErr::Msg(
                "TypeError: Reduce of empty array with no initial value".into(),
            ));
        };
        (items[first].clone(), first + 1)
    };
    for (i, item) in items.iter().enumerate().skip(start) {
        if !presence.get(i).copied().unwrap_or(true) {
            continue;
        }
        acc = interp.call_this(
            &cb,
            Value::Undefined,
            vec![acc, item.clone(), Value::Number(i as f64), this.clone()],
        )?;
    }
    Ok(acc)
}

fn array_for_each(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    for (i, it) in items.iter().enumerate() {
        if !presence.get(i).copied().unwrap_or(true) {
            continue;
        }
        interp.call_this(
            &cb,
            Value::Undefined,
            vec![it.clone(), Value::Number(i as f64), this.clone()],
        )?;
    }
    Ok(Value::Undefined)
}

fn array_find(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    for (i, it) in items.iter().enumerate() {
        let hit = interp.call_this(
            &cb,
            Value::Undefined,
            vec![it.clone(), Value::Number(i as f64), this.clone()],
        )?;
        if hit.is_truthy() {
            return Ok(it.clone());
        }
    }
    Ok(Value::Undefined)
}

fn array_some(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    for (i, it) in items.iter().enumerate() {
        if !presence.get(i).copied().unwrap_or(true) {
            continue;
        }
        let hit = interp.call_this(
            &cb,
            Value::Undefined,
            vec![it.clone(), Value::Number(i as f64), this.clone()],
        )?;
        if hit.is_truthy() {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn array_every(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    for (i, it) in items.iter().enumerate() {
        if !presence.get(i).copied().unwrap_or(true) {
            continue;
        }
        let hit = interp.call_this(
            &cb,
            Value::Undefined,
            vec![it.clone(), Value::Number(i as f64), this.clone()],
        )?;
        if !hit.is_truthy() {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

fn array_push(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if let Value::Array(items) = &this {
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
    Ok(Value::Undefined)
}

fn array_pop(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    if let Value::Array(items) = &this {
        let result = items.borrow_mut().pop().unwrap_or(Value::Undefined);
        let length = items.borrow().len();
        items.truncate_presence(length);
        return Ok(result);
    }
    Ok(Value::Undefined)
}

fn array_join(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let sep = match a.first() {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Undefined) | None => ",".to_string(),
        Some(v) => interp.vs(v)?,
    };
    let mut out = String::new();
    for (index, value) in items.iter().enumerate() {
        if index > 0 {
            if out.len().saturating_add(sep.len()) > crate::value::MAX_STRING_LEN {
                return Err(crate::value::limit_err("Maximum string length exceeded"));
            }
            out.push_str(&sep);
        }
        let part = join_str(interp, value)?;
        if out.len().saturating_add(part.len()) > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
        out.push_str(&part);
    }
    Value::checked_string(out)
}

fn array_index_of(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    for (i, it) in items.iter().enumerate() {
        if presence.get(i).copied().unwrap_or(true) && interp.seq(it, &target) {
            return Ok(Value::Number(i as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

fn array_includes(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    for it in &items {
        if interp.seq(it, &target) {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn array_slice(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let len = items.len() as i64;
    let norm = |v: f64| -> i64 {
        if v.is_nan() {
            return 0;
        }
        let i = v as i64;
        if i < 0 { (len + i).max(0) } else { i.min(len) }
    };
    let start = norm(a.first().map(|v| v.to_number()).unwrap_or(0.0));
    let end = match a.get(1) {
        Some(v) => norm(v.to_number()),
        None => len,
    };
    if start >= end {
        return Ok(Value::array(vec![]));
    }
    Value::checked_array_with_presence(
        items[start as usize..end as usize].to_vec(),
        presence[start as usize..end as usize].to_vec(),
    )
}

fn array_concat(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let mut out = arr_items(&this);
    let mut presence = arr_presence(&this);
    for v in a {
        match &v {
            Value::Array(items) => {
                let source = items.borrow();
                if out.len().saturating_add(source.len()) > crate::value::MAX_ARRAY_LEN {
                    return Err(crate::value::limit_err("Maximum array length exceeded"));
                }
                out.extend(source.iter().cloned());
                presence.extend(
                    source
                        .iter()
                        .enumerate()
                        .map(|(index, _)| items.has_index(index)),
                );
            }
            _ => {
                if out.len() >= crate::value::MAX_ARRAY_LEN {
                    return Err(crate::value::limit_err("Maximum array length exceeded"));
                }
                out.push(v);
                presence.push(true);
            }
        }
        if out.len() > crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum array length exceeded"));
        }
    }
    Value::checked_array_with_presence(out, presence)
}

fn array_reverse(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    if let Value::Array(items) = &this {
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
    if let Value::Array(items) = &this {
        let cmp = a.first().cloned().unwrap_or(Value::Undefined);
        // Never hold the array's RefCell borrow while invoking guest code.
        // Comparators are allowed to re-enter the array (for example by
        // calling `push`), and keeping the RefMut across the callback used to
        // turn that normal JavaScript re-entry into a Rust RefCell panic.
        let original_values = items.borrow().clone();
        let original_presence = items.presence_snapshot();
        let original_len = original_values.len();
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
        if matches!(
            cmp,
            Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
        ) {
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
                match (interp.vs(left), interp.vs(right)) {
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

fn array_flat(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let depth = a.first().map(|v| v.to_number()).unwrap_or(1.0);
    let depth = if depth.is_nan() { 0.0 } else { depth };

    // Use an explicit work stack. Guest code can construct deeply nested
    // arrays dynamically, so parser and call-depth limits do not protect the
    // recursive implementation that used to live here.
    enum Work {
        Value(Value, f64),
        Leave(usize),
    }

    let mut work = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate().rev() {
        if presence.get(index).copied().unwrap_or(true) {
            work.push(Work::Value(item, depth));
        }
    }
    let mut active = std::collections::HashSet::new();
    let mut out = Vec::new();
    while let Some(entry) = work.pop() {
        match entry {
            Work::Leave(identity) => {
                active.remove(&identity);
            }
            Work::Value(value, remaining) => {
                if remaining > 0.0
                    && let Value::Array(inner) = &value
                {
                    let identity = std::rc::Rc::as_ptr(inner) as usize;
                    // A cyclic array cannot be expanded forever. Treat the
                    // back-edge as a leaf, matching the boundary's other
                    // cycle-safe representations.
                    if active.insert(identity) {
                        work.push(Work::Leave(identity));
                        let children = inner.borrow().clone();
                        let child_presence = inner.presence_snapshot();
                        for (index, child) in children.into_iter().enumerate().rev() {
                            if child_presence.get(index).copied().unwrap_or(true) {
                                work.push(Work::Value(child, remaining - 1.0));
                            }
                        }
                        continue;
                    }
                }
                out.push(value);
                if out.len() > crate::value::MAX_ARRAY_LEN {
                    return Err(crate::value::limit_err("Maximum array length exceeded"));
                }
            }
        }
    }
    Value::checked_array(out)
}

fn array_flat_map(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let mut out = Vec::new();
    for (i, it) in items.iter().enumerate() {
        if !presence.get(i).copied().unwrap_or(true) {
            continue;
        }
        let r = interp.call_this(
            &cb,
            Value::Undefined,
            vec![it.clone(), Value::Number(i as f64), this.clone()],
        )?;
        match &r {
            Value::Array(inner) => {
                let values = inner.borrow().clone();
                let inner_presence = inner.presence_snapshot();
                let added = inner_presence.iter().filter(|present| **present).count();
                if out.len().saturating_add(added) > crate::value::MAX_ARRAY_LEN {
                    return Err(crate::value::limit_err("Maximum array length exceeded"));
                }
                out.extend(
                    values
                        .into_iter()
                        .zip(inner_presence)
                        .filter_map(|(value, present)| present.then_some(value)),
                );
            }
            _ => {
                if out.len() >= crate::value::MAX_ARRAY_LEN {
                    return Err(crate::value::limit_err("Maximum array length exceeded"));
                }
                out.push(r);
            }
        }
        if out.len() > crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum array length exceeded"));
        }
    }
    Value::checked_array(out)
}

fn array_reduce_right(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let items = arr_items(&this);
    let presence = arr_presence(&this);
    let cb = a.first().cloned().unwrap_or(Value::Undefined);
    let len = items.len();
    let (mut acc, mut i) = if a.len() >= 2 {
        (a[1].clone(), len as i64 - 1)
    } else {
        let Some(last) = presence.iter().rposition(|present| *present) else {
            return Err(VmErr::Msg(
                "TypeError: Reduce of empty array with no initial value".into(),
            ));
        };
        (items[last].clone(), last as i64 - 1)
    };
    while i >= 0 {
        let index = i as usize;
        if presence.get(index).copied().unwrap_or(true) {
            acc = interp.call_this(
                &cb,
                Value::Undefined,
                vec![
                    acc,
                    items[index].clone(),
                    Value::Number(i as f64),
                    this.clone(),
                ],
            )?;
        }
        i -= 1;
    }
    Ok(acc)
}
