//! WeakRef and FinalizationRegistry backed by the cycle collector. Cleanup is
//! queued at a host checkpoint, never invoked while heap edges are being swept.
use super::*;
use crate::value::weak::{FinalizationRecord, WeakStorage, WeakTarget};
fn incompatible(name: &str) -> VmErr {
    VmErr::Msg(format!(
        "TypeError: {name} called on an incompatible receiver"
    ))
}
pub(super) fn install(env: &mut Environment) {
    let function_prototype = env
        .get("Function")
        .and_then(|value| value.get_prop("prototype"));
    let object_prototype = env
        .get("Object")
        .and_then(|value| value.get_prop("prototype"));
    for (name, constructor, methods) in [
        (
            "WeakRef",
            new_weak_ref as NativeFn,
            vec![("deref", 0, deref as NativeFn)],
        ),
        (
            "FinalizationRegistry",
            new_registry as NativeFn,
            vec![
                ("register", 2, register as NativeFn),
                ("unregister", 1, unregister as NativeFn),
            ],
        ),
    ] {
        let namespace = env.get(name).expect("weak builtin namespace");
        make_callable(&namespace, super::require_new, Some(constructor));
        let prototype = Value::object_with_proto(vec![], object_prototype.clone().map(Rc::new));
        for (method, length, callable) in methods {
            prototype
                .set_prop(
                    method.into(),
                    native_method(method, length, callable, function_prototype.clone()),
                )
                .expect("weak prototype");
            if let Value::Object { props } = &prototype {
                props.meta.borrow_mut().set_attrs(
                    method,
                    PropAttrs {
                        enumerable: false,
                        ..Default::default()
                    },
                );
            }
        }
        prototype
            .set_prop("constructor".into(), namespace.clone())
            .expect("weak constructor");
        if let Value::Object { props } = &prototype {
            props.meta.borrow_mut().set_attrs(
                "constructor",
                PropAttrs {
                    enumerable: false,
                    ..Default::default()
                },
            );
            if let Some(Value::Symbol(ref symbol)) = well_known("toStringTag") {
                let key = crate::interpreter::symbol_slot_key(symbol);
                prototype
                    .set_prop(key.clone(), Value::String(name.into()))
                    .expect("weak toStringTag");
                props.meta.borrow_mut().set_symbol_key(&key, symbol.clone());
                props.meta.borrow_mut().set_attrs(
                    &key,
                    PropAttrs {
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
        }
        set_builtin_constructor_prototype(env, &namespace, prototype);
        namespace
            .set_prop("name".into(), Value::String(name.into()))
            .expect("weak constructor name");
        namespace
            .set_prop("length".into(), Value::Number(1.0))
            .expect("weak constructor length");
        if let Value::Object { props } = &namespace {
            for key in ["name", "length"] {
                props.meta.borrow_mut().set_attrs(
                    key,
                    PropAttrs {
                        writable: false,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
        }
    }
}
fn instance(interp: &Interpreter, name: &str, storage: WeakStorage) -> Value {
    let prototype = interp
        .persistent_global
        .borrow()
        .intrinsic(name)
        .and_then(|constructor| constructor.get_prop("prototype"));
    let object = Value::object_with_proto(vec![], prototype.map(Rc::new));
    if let Value::Object { props } = &object {
        *props.weak.borrow_mut() = storage;
    }
    object
}
fn new_weak_ref(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let undefined = Value::Undefined;
    let value = args.first().unwrap_or(&undefined);
    let target = WeakTarget::new(value, &interp.persistent_global).ok_or_else(|| {
        VmErr::Msg("TypeError: WeakRef target must be an object or a non-registered symbol".into())
    })?;
    interp.jobs.borrow_mut().keep_alive(value.clone())?;
    Ok(instance(interp, "WeakRef", WeakStorage::Ref(Some(target))))
}
fn deref(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let Value::Object { props } = &this else {
        return Err(incompatible("WeakRef.prototype.deref"));
    };
    let WeakStorage::Ref(target) = &*props.weak.borrow() else {
        return Err(incompatible("WeakRef.prototype.deref"));
    };
    let result = target
        .as_ref()
        .and_then(WeakTarget::upgrade)
        .unwrap_or(Value::Undefined);
    if !matches!(result, Value::Undefined) {
        interp.jobs.borrow_mut().keep_alive(result.clone())?;
    }
    Ok(result)
}
fn new_registry(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let callback = args.first().cloned().unwrap_or(Value::Undefined);
    if !crate::interpreter::is_callable_value(&callback) {
        return Err(VmErr::Msg(
            "TypeError: FinalizationRegistry cleanup callback must be callable".into(),
        ));
    }
    Ok(instance(
        interp,
        "FinalizationRegistry",
        WeakStorage::Registry {
            callback,
            records: Vec::new(),
            jobs: Rc::downgrade(&interp.jobs),
        },
    ))
}
fn register(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let Value::Object { props } = &this else {
        return Err(incompatible("FinalizationRegistry.prototype.register"));
    };
    let mut storage = props.weak.borrow_mut();
    let WeakStorage::Registry { records, .. } = &mut *storage else {
        return Err(incompatible("FinalizationRegistry.prototype.register"));
    };
    let undefined = Value::Undefined;
    let value = args.first().unwrap_or(&undefined);
    let target = WeakTarget::new(value, &interp.persistent_global).ok_or_else(|| {
        VmErr::Msg("TypeError: FinalizationRegistry target must be weakly holdable".into())
    })?;
    let held = args.get(1).cloned().unwrap_or(Value::Undefined);
    if crate::interpreter::strict_equals(value, &held) {
        return Err(VmErr::Msg(
            "TypeError: FinalizationRegistry target and holdings must differ".into(),
        ));
    }
    let token = match args.get(2) {
        None | Some(Value::Undefined) => None,
        Some(value) => Some(
            WeakTarget::new(value, &interp.persistent_global).ok_or_else(|| {
                VmErr::Msg(
                    "TypeError: FinalizationRegistry unregister token must be weakly holdable"
                        .into(),
                )
            })?,
        ),
    };
    if records.len() >= crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err(
            "Maximum finalization registration count exceeded",
        ));
    }
    records.push(FinalizationRecord {
        target,
        held,
        token,
    });
    Ok(Value::Undefined)
}
fn unregister(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let undefined = Value::Undefined;
    let value = args.first().unwrap_or(&undefined);
    if WeakTarget::new(value, &interp.persistent_global).is_none() {
        return Err(VmErr::Msg(
            "TypeError: FinalizationRegistry unregister token must be weakly holdable".into(),
        ));
    }
    let Value::Object { props } = &this else {
        return Err(incompatible("FinalizationRegistry.prototype.unregister"));
    };
    let mut storage = props.weak.borrow_mut();
    let WeakStorage::Registry { records, .. } = &mut *storage else {
        return Err(incompatible("FinalizationRegistry.prototype.unregister"));
    };
    let prior = records.len();
    records.retain(|record| {
        !record
            .token
            .as_ref()
            .is_some_and(|token| token.matches(value))
    });
    Ok(Value::Bool(records.len() != prior))
}
