//! `Number` statics, `Number.prototype` methods, and the global
//! `parseInt` / `parseFloat` implementations (shared with the `Number` statics).

use super::{NativeFn, nf};
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{BoxedPrimitive, Value};

pub(super) fn install(e: &mut Environment) {
    if let Some(n) = e.get("Number") {
        n.set_prop("isNaN".to_string(), nf("isNaN", number_is_nan))
            .expect("built-in Number property");
        n.set_prop("isFinite".to_string(), nf("isFinite", number_is_finite))
            .expect("built-in Number property");
        n.set_prop("parseInt".to_string(), nf("parseInt", parse_int))
            .expect("built-in Number property");
        n.set_prop("parseFloat".to_string(), nf("parseFloat", parse_float))
            .expect("built-in Number property");
        let constants: &[(&str, f64)] = &[
            ("MAX_SAFE_INTEGER", 9_007_199_254_740_991.0),
            ("MIN_SAFE_INTEGER", -9_007_199_254_740_991.0),
            ("MAX_VALUE", f64::MAX),
            ("MIN_VALUE", f64::MIN_POSITIVE * f64::EPSILON),
            ("EPSILON", f64::EPSILON),
            ("POSITIVE_INFINITY", f64::INFINITY),
            ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
            ("NaN", f64::NAN),
        ];
        for (name, value) in constants {
            n.set_prop(name.to_string(), Value::Number(*value))
                .expect("built-in Number property");
            n.property_cell().unwrap().meta.borrow_mut().set_attrs(
                name,
                crate::value::PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: false,
                },
            );
        }
        n.set_prop("isInteger".to_string(), nf("isInteger", number_is_integer))
            .expect("built-in Number property");
        n.set_prop(
            "isSafeInteger".to_string(),
            nf("isSafeInteger", number_is_safe_integer),
        )
        .expect("built-in Number property");
        super::make_callable(&n, number_ctor, Some(number_construct));
        for (key, value) in [
            ("name", Value::String("Number".into())),
            ("length", Value::Number(1.0)),
        ] {
            n.set_prop(key.into(), value)
                .expect("Number constructor metadata");
            n.property_cell()
                .expect("Number constructor")
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
        let methods = [
            "toString",
            "toLocaleString",
            "toFixed",
            "toPrecision",
            "valueOf",
        ]
        .into_iter()
        .filter_map(|name| number_method(name).map(|method| (name, method)))
        .collect();
        super::install_primitive_prototype(e, &n, Value::Number(0.0), methods);
        super::install_method_metadata(
            e,
            &n,
            &[
                ("isNaN", 1),
                ("isFinite", 1),
                ("isInteger", 1),
                ("isSafeInteger", 1),
                ("parseInt", 2),
                ("parseFloat", 1),
            ],
        );
        let prototype = n.get_prop("prototype").expect("Number.prototype");
        super::install_method_metadata(
            e,
            &prototype,
            &[
                ("toString", 1),
                ("toLocaleString", 0),
                ("toFixed", 1),
                ("toPrecision", 1),
                ("valueOf", 0),
            ],
        );
    }
    if let Some(b) = e.get("Boolean") {
        super::make_callable(&b, boolean_ctor, Some(boolean_construct));
        super::install_primitive_prototype(
            e,
            &b,
            Value::Bool(false),
            vec![
                ("toString", nf("toString", boolean_to_string)),
                ("valueOf", nf("valueOf", boolean_value_of)),
            ],
        );
    }
    if let Some(o) = e.get("Object") {
        super::make_callable(&o, object_ctor, None);
    }
}

/// `Number(v)`: the numeric coercion of `v`.
fn number_ctor(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(match a.first() {
        None => 0.0,
        Some(v) => match interp.ecmascript_to_numeric(v)? {
            Value::BigInt(ref n) => n.to_f64(),
            Value::Number(n) => n,
            _ => unreachable!("ToNumeric produces Number or BigInt"),
        },
    }))
}

fn number_construct(
    interpreter: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let number = number_ctor(interpreter, this, args)?;
    Ok(Value::boxed_primitive(number).expect("Number constructor produces a number"))
}

fn boolean_ctor(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(
        a.first().map(|v| v.is_truthy()).unwrap_or(false),
    ))
}

fn boolean_construct(_: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let value = Value::Bool(args.first().is_some_and(Value::is_truthy));
    Ok(Value::boxed_primitive(value).expect("Boolean constructor produces a boolean"))
}

fn boolean_primitive(this: &Value) -> Result<bool, VmErr> {
    match this {
        Value::Bool(value) => Ok(*value),
        Value::Object { props } => match props.meta.borrow().boxed_primitive.as_ref() {
            Some(BoxedPrimitive::Bool(value)) => Ok(*value),
            _ => Err(VmErr::Msg(
                "TypeError: Boolean method called on an incompatible receiver".into(),
            )),
        },
        _ => Err(VmErr::Msg(
            "TypeError: Boolean method called on an incompatible receiver".into(),
        )),
    }
}

fn boolean_to_string(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::String(
        (boolean_primitive(&this)?.to_string()).into(),
    ))
}

fn boolean_value_of(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(boolean_primitive(&this)?))
}

/// `Object(v)`: `v` itself when it is already an object, a fresh object when
/// it is nullish, and a boxed primitive otherwise.
fn object_ctor(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(match a.first() {
        None | Some(Value::Undefined) | Some(Value::Null) => Value::object(vec![]),
        Some(
            v @ (Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Symbol(_)
            | Value::BigInt(_)),
        ) => Value::boxed_primitive(v.clone()).expect("primitive values have wrapper objects"),
        Some(v) => v.clone(),
    })
}

fn number_is_integer(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(match a.first() {
        Some(Value::Number(n)) => n.is_finite() && n.fract() == 0.0,
        _ => false,
    }))
}

fn number_is_safe_integer(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(match a.first() {
        Some(Value::Number(n)) => {
            n.is_finite() && n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0
        }
        _ => false,
    }))
}

// --- Number prototype -------------------------------------------------------

fn number_primitive(this: &Value) -> Result<f64, VmErr> {
    match this {
        Value::Number(value) => Ok(*value),
        Value::Object { props } => match props.meta.borrow().boxed_primitive.as_ref() {
            Some(BoxedPrimitive::Number(value)) => Ok(*value),
            _ => Err(VmErr::Msg("TypeError: incompatible Number receiver".into())),
        },
        _ => Err(VmErr::Msg("TypeError: incompatible Number receiver".into())),
    }
}

pub fn number_method(name: &str) -> Option<Value> {
    let f: NativeFn = match name {
        "toFixed" => number_to_fixed,
        "toString" | "toLocaleString" => number_to_string,
        "toPrecision" => number_to_precision,
        "valueOf" => number_value_of,
        _ => return None,
    };
    Some(nf(name, f))
}

/// `toString(radix)`. Base 10 renders like every other number here; another
/// base renders the integer part in that base, as the specification does for
/// an integral value.
fn number_to_string(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let value = number_primitive(&this)?;
    let radix = match a.first() {
        None | Some(Value::Undefined) => 10.0,
        Some(value) => crate::value::to_integer_or_infinity(interp.ecmascript_to_number(value)?),
    };
    if !(2.0..=36.0).contains(&radix) {
        return Err(VmErr::Msg(
            "RangeError: toString() radix must be between 2 and 36".to_string(),
        ));
    }
    let radix = radix as u32;
    if radix == 10 {
        return Ok(Value::String(
            (crate::format::ecmascript_number_string(value)).into(),
        ));
    }
    if !value.is_finite() {
        return Ok(Value::String((crate::format::number_string(value)).into()));
    }
    let negative = value < 0.0;
    let mut magnitude = value.abs().trunc() as u128;
    let mut digits = Vec::new();
    if magnitude == 0 {
        digits.push(b'0');
    }
    while magnitude > 0 {
        let digit = (magnitude % radix as u128) as u32;
        digits.push(std::char::from_digit(digit, radix).unwrap_or('0') as u8);
        magnitude /= radix as u128;
    }
    if negative {
        digits.push(b'-');
    }
    digits.reverse();
    Ok(Value::String(
        (String::from_utf8(digits).unwrap_or_else(|_| "0".to_string())).into(),
    ))
}

/// `toPrecision(digits)`: `digits` significant figures.
fn number_to_precision(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let value = number_primitive(&this)?;
    let digits = match a.first() {
        None | Some(Value::Undefined) => {
            return Ok(Value::String((crate::format::number_string(value)).into()));
        }
        Some(value) => crate::value::to_integer_or_infinity(interp.ecmascript_to_number(value)?),
    };
    if !value.is_finite() {
        return Ok(Value::String(
            crate::format::ecmascript_number_string(value).into(),
        ));
    }
    if !(1.0..=100.0).contains(&digits) {
        return Err(VmErr::Msg(
            "RangeError: toPrecision() argument must be between 1 and 100".to_string(),
        ));
    }
    let digits = digits as usize;
    if value == 0.0 {
        return Ok(Value::String((format!("{:.*}", digits - 1, 0.0)).into()));
    }
    // Significant figures = decimal places, shifted by the exponent.
    let exponent = value.abs().log10().floor() as i32;
    let decimals = (digits as i32 - 1 - exponent).max(0) as usize;
    Ok(Value::String((format!("{:.*}", decimals, value)).into()))
}

fn number_value_of(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(number_primitive(&this)?))
}

fn number_to_fixed(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let n = number_primitive(&this)?;
    let digits = crate::value::to_integer_or_infinity(
        interp.ecmascript_to_number(a.first().unwrap_or(&Value::Undefined))?,
    );
    if !(0.0..=100.0).contains(&digits) {
        return Err(crate::value::limit_err(
            "toFixed digits must be between 0 and 100",
        ));
    }
    if !n.is_finite() || n.abs() >= 1e21 {
        return Value::checked_string(crate::format::ecmascript_number_string(n));
    }
    let digits = digits as usize;
    // Scale the exact binary64 value with the existing integer operations.
    // Rust's formatter rounds ties to even; toFixed chooses the larger n.
    use crate::bigint::BigInt;
    let bits = n.abs().to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i64;
    let mantissa = (bits & ((1 << 52) - 1)) | if exponent == 0 { 0 } else { 1 << 52 };
    let shift = if exponent == 0 {
        -1074
    } else {
        exponent - 1075
    } + digits as i64;
    let integer = (|| -> crate::bigint::BigResult<BigInt> {
        let scaled = BigInt::from_words(false, &[mantissa])
            .mul(&BigInt::from_i64(5).pow(&BigInt::from_i64(digits as i64))?)?;
        if shift >= 0 {
            scaled.shl(&BigInt::from_i64(shift))
        } else {
            let half = BigInt::from_i64(1).shl(&BigInt::from_i64(-shift - 1))?;
            scaled.add(&half)?.shr(&BigInt::from_i64(-shift))
        }
    })()
    .map_err(VmErr::Msg)?;
    let mut text = integer.to_decimal();
    if digits > 0 {
        if text.len() <= digits {
            text = format!("{}{}", "0".repeat(digits + 1 - text.len()), text);
        }
        text.insert(text.len() - digits, '.');
    }
    if n < 0.0 {
        text.insert(0, '-');
    }
    Value::checked_string(text)
}

// --- Number statics ---------------------------------------------------------

fn number_is_nan(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(
        matches!(a.first(), Some(Value::Number(n)) if n.is_nan()),
    ))
}
fn number_is_finite(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(
        matches!(a.first(), Some(Value::Number(n)) if n.is_finite()),
    ))
}

// --- parseInt / parseFloat (global and Number.* share these) ----------------

pub(super) fn parse_int(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = interp.ecmascript_to_string(a.first().unwrap_or(&Value::Undefined))?;
    let s = s.trim_units(true, false).to_string();
    let mut radix = match a.get(1) {
        Some(v) => crate::value::to_int32(interp.ecmascript_to_number(v)?),
        None => 0,
    };
    if radix != 0 && !(2..=36).contains(&radix) {
        return Ok(Value::Number(f64::NAN));
    }
    let mut digits = s.as_str();
    let neg = digits.starts_with('-');
    if digits.starts_with(['+', '-']) {
        digits = &digits[1..];
    }
    let strip_prefix = radix == 0 || radix == 16;
    if radix == 0 {
        radix = 10;
    }
    if strip_prefix && (digits.starts_with("0x") || digits.starts_with("0X")) {
        radix = 16;
        digits = &digits[2..];
    }
    let end = digits
        .find(|c: char| !c.is_ascii() || c.to_digit(radix as u32).is_none())
        .unwrap_or(digits.len());
    if end == 0 {
        return Ok(Value::Number(f64::NAN));
    }
    let digits = &digits[..end];
    // Decimal parsing rounds the full integer once, rather than overflowing
    // an i64 or accumulating rounding error one digit at a time.
    let val = if radix == 10 {
        digits.parse::<f64>().unwrap_or(f64::INFINITY)
    } else {
        digits.chars().fold(0.0, |value, c| {
            value * radix as f64 + c.to_digit(radix as u32).expect("validated digit") as f64
        })
    };
    Ok(Value::Number(if neg { -val } else { val }))
}

/// Longest initial decimal literal, including an optional signed exponent.
fn decimal_prefix(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut index = usize::from(matches!(bytes.first(), Some(b'+') | Some(b'-')));
    let mut digits = 0;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
        digits += 1;
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return 0;
    }
    let end = index;
    if matches!(bytes.get(index), Some(b'e') | Some(b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+') | Some(b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            return end;
        }
    }
    index
}

pub(super) fn parse_float(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = interp.ecmascript_to_string(a.first().unwrap_or(&Value::Undefined))?;
    let s = s.trim_units(true, false).to_string();
    let t = s.as_str();
    let unsigned = t.strip_prefix(['+', '-']).unwrap_or(t);
    if unsigned.starts_with("Infinity") {
        return Ok(Value::Number(if t.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        }));
    }
    let end = decimal_prefix(t);
    match t[..end].parse::<f64>() {
        Ok(n) => Ok(Value::Number(n)),
        Err(_) => Ok(Value::Number(f64::NAN)),
    }
}
