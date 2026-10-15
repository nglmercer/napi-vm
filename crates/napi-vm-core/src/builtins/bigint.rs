//! The `BigInt` global: conversion and the fixed-width wrapping helpers.

use std::rc::Rc;

use crate::bigint::BigInt;
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{BoxedPrimitive, Value};

pub(super) fn install(e: &mut Environment) {
    let Some(namespace) = e.get("BigInt") else {
        return;
    };
    namespace
        .set_prop("asIntN".to_string(), super::nf("asIntN", as_int_n))
        .expect("built-in BigInt property");
    namespace
        .set_prop("asUintN".to_string(), super::nf("asUintN", as_uint_n))
        .expect("built-in BigInt property");
    super::install_method_metadata(e, &namespace, &[("asIntN", 2), ("asUintN", 2)]);
    super::make_callable(&namespace, bigint_convert, Some(bigint_construct));
    for (key, value) in [
        ("name", Value::String("BigInt".into())),
        ("length", Value::Number(1.)),
    ] {
        namespace.set_prop(key.into(), value).unwrap();
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

    let methods = ["toString", "toLocaleString", "valueOf"]
        .into_iter()
        .filter_map(|name| bigint_method(name).map(|method| (name, method)))
        .collect();
    super::install_primitive_prototype(
        e,
        &namespace,
        Value::BigInt(Rc::new(BigInt::from_i64(0))),
        methods,
    );
    let proto = namespace.get_prop("prototype").unwrap();
    proto
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .boxed_primitive = None;
    if let Some(Value::Symbol(ref symbol)) = super::well_known("toStringTag") {
        let key = crate::interpreter::symbol_slot_key(symbol);
        proto
            .set_prop(key.clone(), Value::String("BigInt".into()))
            .unwrap();
        let cell = proto.property_cell().unwrap();
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
    super::install_method_metadata(
        e,
        &proto,
        &[("toString", 0), ("toLocaleString", 0), ("valueOf", 0)],
    );
}

/// `BigInt(value)`. A number must be an exact integer — there is no rounding,
/// since a silent one would defeat the point of the type.
fn bigint_convert(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let primitive =
        interp.coerce_object_to_primitive(a.first().unwrap_or(&Value::Undefined), "number")?;
    if let Value::Number(number) = primitive {
        return BigInt::from_f64(number)
            .map(|value| Value::BigInt(Rc::new(value)))
            .map_err(VmErr::Msg);
    }
    interp.ecmascript_to_bigint(&primitive)
}

fn bigint_construct(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Err(VmErr::Msg("TypeError: BigInt is not a constructor".into()))
}

fn wrap(i: &mut Interpreter, a: &[Value], signed: bool) -> Result<Value, VmErr> {
    let bits = i.ecmascript_to_number(a.first().unwrap_or(&Value::Undefined))?;
    let bits = crate::value::to_integer_or_infinity(bits);
    if !(0.0..=9_007_199_254_740_991.0).contains(&bits) {
        return Err(VmErr::Msg("RangeError: Invalid bit width".into()));
    }
    let value = i.ecmascript_to_bigint(a.get(1).unwrap_or(&Value::Undefined))?;
    let value = value.as_bigint().unwrap();
    value
        .as_n_bit(bits as usize, signed)
        .map(|v| Value::BigInt(Rc::new(v)))
        .map_err(VmErr::Msg)
}
fn as_int_n(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    wrap(i, &a, true)
}
fn as_uint_n(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    wrap(i, &a, false)
}

/// Methods readable on a `BigInt` value.
pub fn bigint_method(key: &str) -> Option<Value> {
    match key {
        "toString" | "toLocaleString" => Some(super::nf(key, bigint_to_string)),
        "valueOf" => Some(super::nf("valueOf", bigint_value_of)),
        _ => None,
    }
}

fn bigint_to_string(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let value = bigint_receiver(&this)?;
    let radix = match args.first() {
        None | Some(Value::Undefined) => 10.,
        Some(v) => crate::value::to_integer_or_infinity(i.ecmascript_to_number(v)?),
    };
    if !(2.0..=36.0).contains(&radix) {
        return Err(VmErr::Msg("RangeError: Invalid BigInt radix".into()));
    }
    Value::checked_string(value.to_radix(radix as u32))
}

fn bigint_value_of(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::BigInt(bigint_receiver(&this)?))
}

fn bigint_receiver(this: &Value) -> Result<Rc<BigInt>, VmErr> {
    match this {
        Value::BigInt(value) => Ok(value.clone()),
        Value::Object { props } => match props.meta.borrow().boxed_primitive.as_ref() {
            Some(BoxedPrimitive::BigInt(value)) => Ok(value.clone()),
            _ => Err(VmErr::Msg(
                "TypeError: BigInt method called on an incompatible receiver".into(),
            )),
        },
        _ => Err(VmErr::Msg(
            "TypeError: BigInt method called on an incompatible receiver".into(),
        )),
    }
}
