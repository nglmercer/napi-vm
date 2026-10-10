//! `Reflect`: the function form of the object-model operations.
//!
//! Each method mirrors the corresponding `Object` static or interpreter
//! primitive, differing where the specification does: `Reflect.defineProperty`
//! reports failure with `false` instead of throwing, and `Reflect.ownKeys`
//! lists non-enumerable properties.

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
    let function_prototype = e.get("Function").and_then(|f| f.get_prop("prototype"));
    for (name, callable) in methods {
        let length = match *name {
            "get" | "set" | "defineProperty" | "apply" => 3,
            "has"
            | "deleteProperty"
            | "getOwnPropertyDescriptor"
            | "setPrototypeOf"
            | "construct" => 2,
            _ => 1,
        };
        r.set_prop(
            name.to_string(),
            super::native_method(name, length, *callable, function_prototype.clone()),
        )
        .expect("built-in Reflect property");
        if let Value::Object { props } = &r {
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
    let target = arg(&a, 0);
    if !crate::interpreter::call::is_js_object(&target) {
        return Err(VmErr::Msg(
            "TypeError: Reflect.get target must be an object".into(),
        ));
    }
    let key = interp.to_property_key(&arg(&a, 1))?;
    let receiver = a.get(2).cloned().unwrap_or_else(|| target.clone());
    interp.get_prop_value_with_receiver(&target, &key, &receiver)
}

fn reflect_set(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let target = reflect_object_target(&a)?;
    let receiver = a.get(3).cloned().unwrap_or_else(|| target.clone());
    interp
        .set_member_with_receiver(&target, &arg(&a, 1), arg(&a, 2), &receiver)
        .map(Value::Bool)
}

fn reflect_has(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let target = reflect_object_target(&a)?;
    interp.has_property(&target, &arg(&a, 1)).map(Value::Bool)
}

fn reflect_object_target(arguments: &[Value]) -> Result<Value, VmErr> {
    let target = arg(arguments, 0);
    if !crate::interpreter::call::is_js_object(&target) {
        return Err(VmErr::Msg(
            "TypeError: Reflect target must be an object".into(),
        ));
    }
    Ok(target)
}

fn reflect_delete(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let target = reflect_object_target(&a)?;
    interp.delete_member(&target, &arg(&a, 1))
}

fn reflect_own_keys(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let target = reflect_object_target(&a)?;
    Value::checked_array(interp.own_property_keys(&target)?)
}

fn reflect_define_property(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = reflect_object_target(&a)?;
    let key = interp.to_property_key(&arg(&a, 1))?;
    let descriptor = super::object::to_property_descriptor(interp, &arg(&a, 2))?;
    interp
        .define_own_property(&target, &key, &descriptor)
        .map(Value::Bool)
}

fn reflect_get_own_descriptor(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = reflect_object_target(&a)?;
    let key = interp.to_property_key(&arg(&a, 1))?;
    super::object::descriptor_for_key_in(interp, &target, &key)
        .map(super::object::from_property_descriptor)
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
    let target = reflect_object_target(&a)?;
    let prototype = arg(&a, 1);
    if !matches!(prototype, Value::Null) && !crate::interpreter::call::is_js_object(&prototype) {
        return Err(VmErr::Msg(
            "TypeError: Reflect prototype must be an object or null".into(),
        ));
    }
    interp
        .set_prototype_of(&target, &prototype)
        .map(Value::Bool)
}

fn reflect_is_extensible(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = reflect_object_target(&a)?;
    interp.is_extensible(&target).map(Value::Bool)
}

fn reflect_prevent_extensions(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = reflect_object_target(&a)?;
    interp.prevent_extensions(&target).map(Value::Bool)
}

fn reflect_apply(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let target = arg(&a, 0);
    if !crate::interpreter::is_callable_value(&target) {
        return Err(VmErr::Msg(
            "TypeError: Reflect.apply target must be callable".into(),
        ));
    }
    let args = interp.argument_list_from_array_like(&arg(&a, 2))?;
    interp.call_this(&target, arg(&a, 1), args)
}

fn reflect_construct(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let target = arg(&a, 0);
    let new_target = a.get(2).cloned().unwrap_or_else(|| target.clone());
    if !super::is_constructor(&target) || !super::is_constructor(&new_target) {
        return Err(VmErr::Msg(
            "TypeError: Target and newTarget must be constructors".into(),
        ));
    }
    let args = interp.argument_list_from_array_like(&arg(&a, 1))?;
    interp.reflect_constructor(&target, args, new_target)
}
