//! The abstract Iterator constructor shares the realm's existing iterator prototype.
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter, strict_equals};
use crate::value::{PropAttrs, Value};

pub(super) fn install(environment: &mut Environment) {
    let prototype = environment
        .intrinsic("%IteratorPrototype%")
        .expect("iterator prototype installed before constructor");
    let constructor = Value::object(vec![]);
    super::make_callable(&constructor, call, Some(construct));
    super::set_builtin_constructor_prototype(environment, &constructor, prototype.clone());
    constructor
        .set_prop("name".into(), Value::String("Iterator".into()))
        .expect("iterator constructor name");
    constructor
        .set_prop("length".into(), Value::Number(0.))
        .expect("iterator constructor length");
    if let Some(properties) = constructor.property_cell() {
        for key in ["name", "length"] {
            properties.meta.borrow_mut().set_attrs(
                key,
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
    }
    super::install_intrinsic_accessor(
        &prototype,
        "constructor",
        &super::native_method(
            "get constructor",
            0,
            get_constructor,
            environment
                .get("Function")
                .and_then(|value| value.get_prop("prototype")),
        ),
        &super::native_method(
            "set constructor",
            1,
            set_constructor,
            environment
                .get("Function")
                .and_then(|value| value.get_prop("prototype")),
        ),
        true,
    );
    let Value::Symbol(ref symbol) = super::well_known("toStringTag").expect("toStringTag") else {
        unreachable!()
    };
    let key = crate::interpreter::symbol_slot_key(symbol);
    prototype
        .property_cell()
        .expect("iterator prototype cell")
        .meta
        .borrow_mut()
        .set_symbol_key(&key, symbol.clone());
    super::install_intrinsic_accessor(
        &prototype,
        &key,
        &super::native_method(
            "get [Symbol.toStringTag]",
            0,
            get_tag,
            environment
                .get("Function")
                .and_then(|value| value.get_prop("prototype")),
        ),
        &super::native_method(
            "set [Symbol.toStringTag]",
            1,
            set_tag,
            environment
                .get("Function")
                .and_then(|value| value.get_prop("prototype")),
        ),
        true,
    );
    environment.set("Iterator", constructor);
}

fn call(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Err(VmErr::Msg(
        "TypeError: Iterator is an abstract constructor".into(),
    ))
}

fn construct(interpreter: &mut Interpreter, active: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let target = interpreter.new_target_stack.last().cloned();
    if target
        .as_ref()
        .is_none_or(|target| strict_equals(target, &active))
    {
        return call(interpreter, active, vec![]);
    }
    // The common constructor operation installs the newTarget prototype and
    // applies its realm fallback; no separate iterator allocation model.
    Value::checked_object(vec![])
}

fn get_constructor(interpreter: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(interpreter
        .persistent_global
        .borrow()
        .intrinsic("Iterator")
        .expect("realm iterator constructor"))
}

fn get_tag(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::String("Iterator".into()))
}

fn set_constructor(
    interpreter: &mut Interpreter,
    receiver: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    set_ignoring_prototype(
        interpreter,
        receiver,
        Value::String("constructor".into()),
        arguments,
    )
}

fn set_tag(
    interpreter: &mut Interpreter,
    receiver: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    set_ignoring_prototype(
        interpreter,
        receiver,
        super::well_known("toStringTag").expect("toStringTag"),
        arguments,
    )
}

fn set_ignoring_prototype(
    interpreter: &mut Interpreter,
    receiver: Value,
    key: Value,
    arguments: Vec<Value>,
) -> Result<Value, VmErr> {
    let home = interpreter
        .persistent_global
        .borrow()
        .intrinsic("%IteratorPrototype%")
        .expect("iterator prototype");
    if !crate::interpreter::call::is_js_object(&receiver) || strict_equals(&receiver, &home) {
        return Err(VmErr::Msg(
            "TypeError: Cannot replace an intrinsic iterator prototype property".into(),
        ));
    }
    let value = arguments.first().cloned().unwrap_or(Value::Undefined);
    if matches!(
        super::object::descriptor_for_key_in(interpreter, &receiver, &key)?,
        Value::Undefined
    ) {
        let descriptor = Value::checked_object(vec![
            ("value".into(), value),
            ("writable".into(), Value::Bool(true)),
            ("enumerable".into(), Value::Bool(true)),
            ("configurable".into(), Value::Bool(true)),
        ])?;
        if !interpreter.define_own_property(&receiver, &key, &descriptor)? {
            return Err(VmErr::Msg(
                "TypeError: Cannot define iterator property".into(),
            ));
        }
    } else {
        interpreter.assign_property_with_receiver(&receiver, &key, value, &receiver, true)?;
    }
    Ok(Value::Undefined)
}
