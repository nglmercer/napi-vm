//! `Reflect`: the function form of the object-model operations.
//!
//! Each method mirrors the corresponding `Object` static or interpreter
//! primitive, differing where the specification does: `Reflect.defineProperty`
//! reports failure with `false` instead of throwing, and `Reflect.ownKeys`
//! lists non-enumerable properties.

use super::nf;
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::Value;

pub(super) fn install(e: &mut Environment) {
    let Some(r) = e.get("Reflect") else { return };
    let methods: &[(&str, super::NativeFn)] = &[
        ("get", reflect_get),
        ("set", reflect_set),
        ("has", reflect_has),
        ("deleteProperty", reflect_delete),
        ("ownKeys", reflect_own_keys),
        ("defineProperty", reflect_define_property),
        ("getOwnPropertyDescriptor", reflect_get_own_descriptor),
        ("getPrototypeOf", reflect_get_prototype_of),
        ("setPrototypeOf", reflect_set_prototype_of),
        ("isExtensible", reflect_is_extensible),
        ("preventExtensions", reflect_prevent_extensions),
        ("apply", reflect_apply),
        ("construct", reflect_construct),
    ];
    for (name, callable) in methods {
        r.set_prop(name.to_string(), nf(name, *callable))
            .expect("built-in Reflect property");
    }
}

/// Delegate to the `Object` static of the same name, which already implements
/// the shared behaviour.
fn via_object(interp: &mut Interpreter, method: &str, args: Vec<Value>) -> Result<Value, VmErr> {
    let object = interp
        .global_value("Object")
        .ok_or_else(|| VmErr::Msg("ReferenceError: Object is not defined".to_string()))?;
    let f = interp.member(&object, method)?;
    interp.call_this(&f, Value::Undefined, args)
}

fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or(Value::Undefined)
}

fn reflect_get(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    interp.get_prop_value(&arg(&a, 0), &arg(&a, 1))
}

fn reflect_set(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    interp.assign_member(&arg(&a, 0), &arg(&a, 1), arg(&a, 2))?;
    Ok(Value::Bool(true))
}

fn reflect_has(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let key = interp.property_key(&arg(&a, 1))?;
    Ok(Value::Bool(arg(&a, 0).has_prop(&key)))
}

fn reflect_delete(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    interp.delete_member(&arg(&a, 0), &arg(&a, 1))
}

fn reflect_own_keys(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    via_object(interp, "getOwnPropertyNames", a)
}

fn reflect_define_property(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    // `Reflect.defineProperty` reports failure rather than throwing.
    let target = arg(&a, 0);
    let key = interp.property_key(&arg(&a, 1))?;
    match super::object::define_property(&target, &key, &arg(&a, 2)) {
        Ok(()) => Ok(Value::Bool(true)),
        Err(VmErr::Msg(_)) => Ok(Value::Bool(false)),
        Err(other) => Err(other),
    }
}

fn reflect_get_own_descriptor(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    via_object(interp, "getOwnPropertyDescriptor", a)
}

fn reflect_get_prototype_of(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    via_object(interp, "getPrototypeOf", a)
}

fn reflect_set_prototype_of(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    via_object(interp, "setPrototypeOf", a)?;
    Ok(Value::Bool(true))
}

fn reflect_is_extensible(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    via_object(interp, "isExtensible", a)
}

fn reflect_prevent_extensions(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    via_object(interp, "preventExtensions", a)?;
    Ok(Value::Bool(true))
}

fn reflect_apply(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let args = match &arg(&a, 2) {
        Value::Array(items) => items.borrow().clone(),
        Value::Undefined | Value::Null => Vec::new(),
        other => interp.iterate(other)?,
    };
    interp.call_this(&arg(&a, 0), arg(&a, 1), args)
}

fn reflect_construct(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let args = match &arg(&a, 1) {
        Value::Array(items) => items.borrow().clone(),
        Value::Undefined | Value::Null => Vec::new(),
        other => interp.iterate(other)?,
    };
    interp.ctor(&arg(&a, 0), args)
}
