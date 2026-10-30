//! ShadowRealm construction, evaluation, and callable boundary wrappers.

use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Env, Interpreter};
use crate::value::{PropAttrs, ShadowRealmWrappedSlots, Value};

pub(super) fn install(e: &mut crate::interpreter::Environment) {
    let Some(constructor) = e.get("ShadowRealm") else {
        return;
    };
    let object_prototype = e
        .get("Object")
        .and_then(|object| object.get_prop("prototype"));
    let function_prototype = e
        .get("Function")
        .and_then(|function| function.get_prop("prototype"));

    constructor
        .set_prop("name".into(), Value::String("ShadowRealm".into()))
        .expect("ShadowRealm name");
    constructor
        .set_prop("length".into(), Value::Number(0.0))
        .expect("ShadowRealm length");
    super::make_callable(
        &constructor,
        super::require_new,
        Some(shadow_realm_construct),
    );
    if let Some(properties) = constructor.property_cell() {
        let mut meta = properties.meta.borrow_mut();
        for key in ["name", "length"] {
            meta.set_attrs(
                key,
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
    }

    let prototype = Value::object_with_proto(
        vec![("constructor".into(), constructor.clone())],
        object_prototype.map(Rc::new),
    );
    prototype
        .set_prop(
            "evaluate".into(),
            super::native_method("evaluate", 1, evaluate, function_prototype.clone()),
        )
        .expect("ShadowRealm.prototype.evaluate");
    prototype
        .set_prop(
            "importValue".into(),
            super::native_method("importValue", 2, import_value, function_prototype),
        )
        .expect("ShadowRealm.prototype.importValue");
    if let Some(properties) = prototype.property_cell() {
        let mut meta = properties.meta.borrow_mut();
        for key in ["constructor", "evaluate", "importValue"] {
            meta.set_attrs(
                key,
                PropAttrs {
                    writable: true,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
    }
    if let Some(Value::Symbol(symbol)) = super::well_known("toStringTag").as_ref() {
        let key = crate::interpreter::symbol_slot_key(symbol);
        prototype
            .set_prop(key.clone(), Value::String("ShadowRealm".into()))
            .expect("ShadowRealm toStringTag");
        if let Some(properties) = prototype.property_cell() {
            let mut meta = properties.meta.borrow_mut();
            meta.set_symbol_key(&key, symbol.clone());
            meta.set_attrs(
                &key,
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
    }
    super::set_builtin_constructor_prototype(e, &constructor, prototype);
    e.set("ShadowRealm", constructor);
}

fn shadow_realm_construct(
    interp: &mut Interpreter,
    _: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    let child = interp.create_realm();
    let realm_global = child.realm_global_object();
    let Value::RealmGlobal(global) = &realm_global else {
        unreachable!("realm global wrapper")
    };
    let instance = Value::object(vec![]);
    instance
        .property_cell()
        .expect("ShadowRealm instance object")
        .meta
        .borrow_mut()
        .shadow_realm_global = Some(global.clone());
    Ok(instance)
}

fn evaluate(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let global = this
        .property_cell()
        .and_then(|properties| properties.meta.borrow().shadow_realm_global.clone())
        .ok_or_else(|| {
            type_error("ShadowRealm.prototype.evaluate called on incompatible receiver")
        })?;
    let Some(Value::String(source)) = args.first() else {
        return Err(type_error(
            "ShadowRealm.prototype.evaluate requires a string",
        ));
    };

    // Parse as a Script first so syntax errors remain distinguishable from
    // exceptions raised during evaluation, which cross the boundary as a
    // caller-realm TypeError.
    Interpreter::compile_utf16_with_goal(source, crate::parser::ParseGoal::Script)?;
    let eval = global
        .borrow()
        .get("eval")
        .ok_or_else(|| type_error("ShadowRealm intrinsic eval is unavailable"))?;
    let result = interp
        .call_this(&eval, Value::Undefined, vec![Value::String(source.clone())])
        .map_err(|_| type_error("An exception was thrown while evaluating in a ShadowRealm"))?;
    if is_wrap_callable(&result) {
        let caller_realm = interp.persistent_global.clone();
        return wrap_callable(interp, result, global, caller_realm)
            .map_err(|_| type_error("Unable to wrap a ShadowRealm callable"));
    }
    if is_primitive(&result) {
        return Ok(result);
    }
    Err(type_error(
        "ShadowRealm evaluation returned a non-primitive value",
    ))
}

fn import_value(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let inner_realm = this
        .property_cell()
        .and_then(|properties| properties.meta.borrow().shadow_realm_global.clone())
        .ok_or_else(|| {
            type_error("ShadowRealm.prototype.importValue called on incompatible receiver")
        })?;
    let specifier = interp.ecmascript_to_string(args.first().unwrap_or(&Value::Undefined))?;
    let Some(Value::String(export_name)) = args.get(1) else {
        return Err(type_error("ShadowRealm export name must be a string"));
    };
    let export_name = export_name.clone();
    let caller_realm = interp.persistent_global.clone();
    let module_promise = interp.with_global_storage(inner_realm.clone(), |vm| {
        let _allocation_realm =
            crate::interpreter::realm::AllocationRealm::enter(Some(inner_realm.clone()));
        vm.import_module(specifier.as_str())
    })?;
    let _allocation_realm =
        crate::interpreter::realm::AllocationRealm::enter(Some(caller_realm.clone()));
    let state = Value::object(vec![
        ("exportName".into(), Value::String(export_name)),
        ("innerRealm".into(), Value::RealmGlobal(inner_realm)),
        (
            "callerRealm".into(),
            Value::RealmGlobal(caller_realm.clone()),
        ),
    ]);
    let function_prototype = caller_realm
        .borrow()
        .intrinsic("Function")
        .and_then(|function| function.get_prop("prototype"));
    let on_fulfilled = super::bound_native_method(
        "getExport",
        1,
        import_value_fulfilled,
        function_prototype.clone(),
        state.clone(),
    );
    let on_rejected = super::bound_native_method(
        "throwTypeError",
        1,
        import_value_rejected,
        function_prototype,
        state,
    );
    let then = interp.member(&module_promise, "then")?;
    interp.call_this(&then, module_promise, vec![on_fulfilled, on_rejected])
}

fn import_value_fulfilled(
    interp: &mut Interpreter,
    state: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let namespace = args.first().cloned().unwrap_or(Value::Undefined);
    let export_name = state
        .get_prop("exportName")
        .and_then(|value| match &value {
            Value::String(value) => Some(value.clone()),
            _ => None,
        })
        .ok_or_else(|| type_error("Invalid ShadowRealm import state"))?;
    let export_name_key = export_name.as_str();
    let keys = interp.own_property_keys(&namespace)?;
    if !keys
        .iter()
        .any(|value| matches!(value, Value::String(name) if name == &export_name))
    {
        return Err(type_error("Requested ShadowRealm export does not exist"));
    }
    let value = interp.get_prop_value_str(&namespace, export_name_key)?;
    if is_primitive(&value) {
        return Ok(value);
    }
    if is_wrap_callable(&value) {
        let inner_realm = state
            .get_prop("innerRealm")
            .and_then(|value| match &value {
                Value::RealmGlobal(global) => Some(global.clone()),
                _ => None,
            })
            .ok_or_else(|| type_error("Invalid ShadowRealm import realm"))?;
        let caller_realm = state
            .get_prop("callerRealm")
            .and_then(|value| match &value {
                Value::RealmGlobal(global) => Some(global.clone()),
                _ => None,
            })
            .ok_or_else(|| type_error("Invalid ShadowRealm caller realm"))?;
        return wrap_callable(interp, value, inner_realm, caller_realm);
    }
    Err(type_error("Requested ShadowRealm export is not wrappable"))
}

fn import_value_rejected(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Err(type_error("ShadowRealm module import failed"))
}

fn is_primitive(value: &Value) -> bool {
    matches!(
        value,
        Value::Undefined
            | Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Symbol(_)
            | Value::BigInt(_)
    )
}

fn is_wrap_callable(value: &Value) -> bool {
    !matches!(value, Value::Class(_)) && crate::interpreter::is_callable_value(value)
}

fn wrap_callable(
    interp: &mut Interpreter,
    target: Value,
    target_realm: Env,
    caller_realm: Env,
) -> Result<Value, VmErr> {
    let (name, length) = copy_name_and_length(interp, &target)?;
    let _allocation_realm =
        crate::interpreter::realm::AllocationRealm::enter(Some(caller_realm.clone()));
    let state = Value::object(vec![]);
    state
        .property_cell()
        .expect("wrapped function state")
        .meta
        .borrow_mut()
        .shadow_realm_wrapped = Some(Box::new(ShadowRealmWrappedSlots {
        target,
        target_realm,
        caller_realm: caller_realm.clone(),
    }));
    let function_prototype = caller_realm
        .borrow()
        .intrinsic("Function")
        .and_then(|function| function.get_prop("prototype"));
    let wrapper = super::bound_native_method(
        "wrapped",
        0,
        wrapped_function_marker,
        function_prototype,
        state,
    );
    if let Value::Function(function) = &wrapper {
        let mut properties = function.properties.borrow_mut();
        for (key, value) in [
            ("name", Value::String(name)),
            ("length", Value::Number(length)),
        ] {
            if let Some((_, slot)) = properties.iter_mut().find(|(name, _)| name == key) {
                *slot = value;
            }
        }
    }
    Ok(wrapper)
}

fn copy_name_and_length(
    interp: &mut Interpreter,
    target: &Value,
) -> Result<(crate::JsString, f64), VmErr> {
    let target_name = interp.get_prop_value_str(target, "name")?;
    let name = match &target_name {
        Value::String(name) => name.clone(),
        _ => crate::JsString::default(),
    };
    let length_descriptor =
        super::object::descriptor_for_key_in(interp, target, &Value::String("length".into()))?;
    if matches!(length_descriptor, Value::Undefined) {
        return Ok((name, 0.0));
    }
    let length = match interp.get_prop_value_str(target, "length")? {
        Value::Number(length) if length == f64::INFINITY => f64::INFINITY,
        Value::Number(length) if length.is_finite() => length.trunc().max(0.0),
        _ => 0.0,
    };
    Ok((name, length))
}

pub(crate) fn is_wrapped_state(value: &Value) -> bool {
    value
        .property_cell()
        .is_some_and(|properties| properties.meta.borrow().shadow_realm_wrapped.is_some())
}

pub(crate) fn call_wrapped(
    interp: &mut Interpreter,
    state: &Value,
    this_value: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let slots = state
        .property_cell()
        .and_then(|properties| {
            properties
                .meta
                .borrow()
                .shadow_realm_wrapped
                .as_deref()
                .cloned()
        })
        .ok_or_else(|| type_error("Invalid ShadowRealm wrapped function"))?;
    let this_value =
        translate_into_realm(interp, this_value, &slots.caller_realm, &slots.target_realm)?;
    let mut translated = Vec::with_capacity(args.len());
    for argument in args {
        translated.push(translate_into_realm(
            interp,
            argument,
            &slots.caller_realm,
            &slots.target_realm,
        )?);
    }
    let result = interp
        .call_this(&slots.target, this_value, translated)
        .map_err(|_| type_error("A ShadowRealm wrapped function threw an exception"))?;
    if is_primitive(&result) {
        return Ok(result);
    }
    if is_wrap_callable(&result) {
        return wrap_callable(interp, result, slots.target_realm, slots.caller_realm)
            .map_err(|_| type_error("Unable to wrap a returned ShadowRealm callable"));
    }
    Err(type_error(
        "ShadowRealm wrapped function returned a non-primitive value",
    ))
}

fn translate_into_realm(
    interp: &mut Interpreter,
    value: Value,
    from_realm: &Env,
    target_realm: &Env,
) -> Result<Value, VmErr> {
    if is_primitive(&value) {
        return Ok(value);
    }
    if is_wrap_callable(&value) {
        let source_realm =
            crate::interpreter::realm::value_realm(&value).unwrap_or_else(|| from_realm.clone());
        return wrap_callable(interp, value, source_realm, target_realm.clone())
            .map_err(|_| type_error("Unable to wrap a ShadowRealm callable argument"));
    }
    Err(type_error(
        "ShadowRealm arguments must be primitive or callable",
    ))
}

fn wrapped_function_marker(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Err(type_error("Internal ShadowRealm wrapper dispatch failed"))
}

fn type_error(message: &str) -> VmErr {
    VmErr::Msg(format!("TypeError: {message}"))
}
