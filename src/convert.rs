//! Direct `serde_json` ↔ VM [`Value`] conversion for host calls.
//!
//! Steady-state plugin calls must not route values through generated
//! JavaScript source or `JSON.stringify` trampolines. These functions move
//! plain JSON data across the boundary with one recursive Rust walk each
//! way, enforcing the same depth, size, and cycle rules as the guest
//! `JSON.parse` / `JSON.stringify` builtins so behavior stays identical.
//!
//! [`value_to_json`] needs the interpreter because faithful serialization
//! executes guest code: getters run, `toJSON` methods run, and proxy
//! targets resolve exactly like [`builtins::json`] would. Anything that
//! cannot survive that treatment (cycles, `BigInt`, excessive depth)
//! errors the same way instead of silently degrading.

use std::collections::HashSet;
use std::rc::Rc;

use serde_json::Value as JsonValue;

use crate::VmErr;
use crate::builtins::json::MAX_JSON_DEPTH;
use crate::interpreter::Interpreter;
use crate::value::Value;
use crate::value::{ArrayCell, BoxedPrimitive, MAX_ARRAY_LEN, MAX_OBJECT_PROPS, ObjectCell};

/// Build a guest value from plain JSON data. Plain objects become
/// ordinary objects with default (writable/enumerable/configurable)
/// attributes, matching what the guest `JSON.parse` builtin produces.
pub fn value_from_json(value: &JsonValue) -> Result<Value, VmErr> {
    value_from_json_depth(value, 0)
}

fn value_from_json_depth(value: &JsonValue, depth: usize) -> Result<Value, VmErr> {
    if depth > MAX_JSON_DEPTH {
        return Err(VmErr::Msg(
            "RangeError: Maximum JSON depth exceeded".to_string(),
        ));
    }
    match value {
        JsonValue::Null => Ok(Value::Null),
        JsonValue::Bool(flag) => Ok(Value::Bool(*flag)),
        JsonValue::Number(number) => Ok(Value::Number(number.as_f64().unwrap_or(f64::NAN))),
        JsonValue::String(text) => Value::checked_string(text.clone()),
        JsonValue::Array(items) => {
            if items.len() > MAX_ARRAY_LEN {
                return Err(crate::value::limit_err("Maximum array length exceeded"));
            }
            let mut elements = Vec::with_capacity(items.len());
            for item in items {
                elements.push(value_from_json_depth(item, depth + 1)?);
            }
            Ok(Value::Array(Rc::new(ArrayCell::new(elements))))
        }
        JsonValue::Object(map) => {
            if map.len() > MAX_OBJECT_PROPS {
                return Err(crate::value::limit_err(
                    "Maximum object property count exceeded",
                ));
            }
            let mut props = Vec::with_capacity(map.len());
            for (key, item) in map {
                if key.len() > crate::value::MAX_STRING_LEN {
                    return Err(crate::value::limit_err("Maximum string length exceeded"));
                }
                props.push((key.clone(), value_from_json_depth(item, depth + 1)?));
            }
            Ok(Value::Object {
                props: Rc::new(ObjectCell::new_with_default_proto(props)),
            })
        }
    }
}

/// Serialize a guest value to plain JSON data, mirroring the guest
/// `JSON.stringify` builtin arm-for-arm: `toJSON` methods and getters run,
/// dates render as ISO strings, proxies resolve to their target, and
/// functions, symbols, bigints, promises, and other non-JSON values become
/// `null` exactly like the serializer's catch-all does.
///
/// Cycles, boxed `BigInt` values, and excessive depth raise the same
/// catchable errors `JSON.stringify` raises instead of recursing forever.
pub fn value_to_json(interp: &mut Interpreter, value: &Value) -> Result<JsonValue, VmErr> {
    let mut visited: HashSet<*const ()> = HashSet::new();
    value_to_json_depth(interp, value, &mut visited, 0)
}

fn value_to_json_depth(
    interp: &mut Interpreter,
    value: &Value,
    visited: &mut HashSet<*const ()>,
    depth: usize,
) -> Result<JsonValue, VmErr> {
    if depth > MAX_JSON_DEPTH {
        return Err(VmErr::Msg(
            "RangeError: Maximum JSON depth exceeded".to_string(),
        ));
    }
    match value {
        Value::Null | Value::Undefined => Ok(JsonValue::Null),
        Value::Bool(flag) => Ok(JsonValue::Bool(*flag)),
        Value::Number(number) => Ok(number_to_json(*number)),
        Value::String(text) => Ok(JsonValue::String(text.clone())),
        Value::TypedArray(view) if view.is_buffer => {
            let to_json = interp.member(value, "toJSON")?;
            if crate::interpreter::call::is_callable_value(&to_json) {
                let converted = interp.call_this(&to_json, value.clone(), Vec::new())?;
                return value_to_json_depth(interp, &converted, visited, depth + 1);
            }
            typed_array_to_json(interp, view, visited, depth)
        }
        Value::TypedArray(view) => typed_array_to_json(interp, view, visited, depth),
        Value::ArrayBuffer(_) | Value::SharedArrayBuffer(_) | Value::DataView(_) => {
            Ok(JsonValue::Object(serde_json::Map::new()))
        }
        Value::Array(items) => {
            let ptr = Rc::as_ptr(items) as *const ();
            if !visited.insert(ptr) {
                return Err(VmErr::Msg(
                    "TypeError: Converting circular structure to JSON".to_string(),
                ));
            }
            let mut out = Vec::with_capacity(items.borrow().len());
            for item in items.borrow().iter() {
                out.push(value_to_json_depth(interp, item, visited, depth + 1)?);
            }
            visited.remove(&ptr);
            Ok(JsonValue::Array(out))
        }
        Value::Date(ms) => Ok(JsonValue::String(crate::builtins::iso_string(ms.get()))),
        Value::Proxy(proxy) => value_to_json_depth(interp, &proxy.target, visited, depth),
        Value::Object { props, .. } => {
            let to_json = interp.member(value, "toJSON")?;
            if crate::interpreter::call::is_callable_value(&to_json) {
                let converted = interp.call_this(&to_json, value.clone(), Vec::new())?;
                return value_to_json_depth(interp, &converted, visited, depth + 1);
            }
            match props.meta.borrow().boxed_primitive.clone() {
                Some(BoxedPrimitive::Bool(flag)) => {
                    return value_to_json_depth(interp, &Value::Bool(flag), visited, depth + 1);
                }
                Some(BoxedPrimitive::Number(number)) => {
                    return value_to_json_depth(interp, &Value::Number(number), visited, depth + 1);
                }
                Some(BoxedPrimitive::String(text)) => {
                    return value_to_json_depth(interp, &Value::String(text), visited, depth + 1);
                }
                Some(BoxedPrimitive::BigInt(_)) => {
                    return Err(VmErr::Msg(
                        "TypeError: Do not know how to serialize a BigInt".into(),
                    ));
                }
                Some(BoxedPrimitive::Symbol(_)) | None => {}
            }
            object_to_json(interp, value, props, visited, depth)
        }
        _ => Ok(JsonValue::Null),
    }
}

/// Render a guest number the way the JSON serializer does: integers stay
/// integers (so `3` round-trips as `3`, not `3.0`), non-finite values
/// become `null`, and everything else keeps its shortest form.
fn number_to_json(number: f64) -> JsonValue {
    if number.is_nan() || number.is_infinite() {
        return JsonValue::Null;
    }
    if number.fract() == 0.0 && number.abs() < 1e15 {
        // `as i64` truncates toward zero, which is exact here; `-0.0`
        // becomes `0`, matching `JSON.stringify(-0)`.
        return JsonValue::Number(serde_json::Number::from(number as i64));
    }
    JsonValue::Number(serde_json::Number::from_f64(number).unwrap_or_else(|| 0.into()))
}

/// Own enumerable string keys only, skipping VM-internal slots — the same
/// snapshot-then-getters discipline as the JSON serializer.
fn object_to_json(
    interp: &mut Interpreter,
    value: &Value,
    props: &Rc<ObjectCell>,
    visited: &mut HashSet<*const ()>,
    depth: usize,
) -> Result<JsonValue, VmErr> {
    let ptr = Rc::as_ptr(props) as *const ();
    if !visited.insert(ptr) {
        return Err(VmErr::Msg(
            "TypeError: Converting circular structure to JSON".to_string(),
        ));
    }
    let meta = props.meta.borrow();
    let entries: Vec<(String, Value)> = props
        .borrow()
        .iter()
        .filter(|(key, _)| {
            !crate::interpreter::is_internal_key(key) && meta.attrs_of(key).enumerable
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    drop(meta);
    let mut out = serde_json::Map::with_capacity(entries.len());
    for (key, slot) in entries {
        let is_getter = matches!(&slot, Value::Function(function)
        if function.name.as_ref().is_some_and(|name| {
            name.strip_prefix("get ").is_some_and(|rest| rest == key)
        }));
        let property = if is_getter {
            interp.member(value, &key)?
        } else {
            slot.deref_binding()
        };
        if matches!(property, Value::Undefined) {
            continue;
        }
        out.insert(
            key,
            value_to_json_depth(interp, &property, visited, depth + 1)?,
        );
    }
    visited.remove(&ptr);
    Ok(JsonValue::Object(out))
}

/// Typed arrays serialize index-by-index like the JSON serializer does.
fn typed_array_to_json(
    interp: &mut Interpreter,
    view: &Rc<crate::value::TypedArrayData>,
    visited: &mut HashSet<*const ()>,
    depth: usize,
) -> Result<JsonValue, VmErr> {
    let mut out = serde_json::Map::new();
    for index in 0..view.effective_length() {
        let element = crate::builtins::read_element(view, index).unwrap_or(Value::Undefined);
        out.insert(
            index.to_string(),
            value_to_json_depth(interp, &element, visited, depth + 1)?,
        );
    }
    Ok(JsonValue::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interpreter() -> Interpreter {
        Interpreter::with_builtins()
    }

    #[test]
    fn json_round_trip_preserves_shape_and_numbers() {
        let mut interp = interpreter();
        let input: JsonValue = serde_json::json!({
            "null": null,
            "bool": true,
            "int": 3,
            "float": 2.5,
            "string": "héllo",
            "array": [1, "two", null, [3]],
            "nested": {"a": {"b": [true]}},
        });
        let value = value_from_json(&input).unwrap();
        let output = value_to_json(&mut interp, &value).unwrap();
        assert_eq!(output, input);
        // Integers stay integers rather than degrading to floats.
        assert_eq!(output.get("int"), Some(&JsonValue::Number(3.into())));
    }

    #[test]
    fn from_json_rejects_excessive_depth() {
        let mut nested = JsonValue::Null;
        for _ in 0..=MAX_JSON_DEPTH + 4 {
            nested = JsonValue::Array(vec![nested]);
        }
        let error = value_from_json(&nested).unwrap_err().to_string();
        assert!(error.contains("Maximum JSON depth"), "{error}");
    }

    #[test]
    fn to_json_matches_stringify_edge_values() {
        let mut interp = interpreter();
        // NaN/Infinity become null; functions, symbols, and promises in
        // nested positions become null via the catch-all.
        let value = interp
            .eval_source("({nan: NaN, inf: Infinity, f: () => 1, s: Symbol('x'), p: Promise.resolve(1), u: undefined})")
            .unwrap();
        let output = value_to_json(&mut interp, &value).unwrap();
        assert_eq!(
            output,
            serde_json::json!({"nan": null, "inf": null, "f": null, "s": null, "p": null}),
        );
    }

    #[test]
    fn to_json_runs_getters_and_to_json() {
        let mut interp = interpreter();
        let value = interp
            .eval_source("({get answer() { return 42; }, nested: { toJSON() { return 'flat'; } }, d: new Date(0) })")
            .unwrap();
        let output = value_to_json(&mut interp, &value).unwrap();
        assert_eq!(output.get("answer"), Some(&serde_json::json!(42)));
        assert_eq!(output.get("nested"), Some(&serde_json::json!("flat")));
        assert_eq!(
            output.get("d"),
            Some(&serde_json::json!("1970-01-01T00:00:00.000Z")),
        );
    }

    #[test]
    fn to_json_rejects_cycles_like_stringify() {
        let mut interp = interpreter();
        let value = interp.eval_source("const o = {}; o.self = o; o").unwrap();
        let error = value_to_json(&mut interp, &value).unwrap_err().to_string();
        assert!(error.contains("circular"), "{error}");
    }

    #[test]
    fn to_json_matches_stringify_bigint_behavior() {
        let mut interp = interpreter();
        // Bare BigInt hits the serializer catch-all and becomes null.
        let bare = interp.eval_source("10n").unwrap();
        assert_eq!(value_to_json(&mut interp, &bare).unwrap(), JsonValue::Null);
        let wrapped = interp.eval_source("({n: 10n})").unwrap();
        assert_eq!(
            value_to_json(&mut interp, &wrapped).unwrap(),
            serde_json::json!({"n": null}),
        );
        // Boxed BigInt throws like JSON.stringify does.
        let boxed = interp.eval_source("Object(10n)").unwrap();
        let error = value_to_json(&mut interp, &boxed).unwrap_err().to_string();
        assert!(error.contains("BigInt"), "{error}");
    }
}
