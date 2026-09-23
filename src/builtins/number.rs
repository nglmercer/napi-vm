//! `Number` statics, `Number.prototype` methods, and the global
//! `parseInt` / `parseFloat` implementations (shared with the `Number` statics).

use super::{NativeFn, nf};
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::Value;

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
        }
        n.set_prop("isInteger".to_string(), nf("isInteger", number_is_integer))
            .expect("built-in Number property");
        n.set_prop(
            "isSafeInteger".to_string(),
            nf("isSafeInteger", number_is_safe_integer),
        )
        .expect("built-in Number property");
        super::make_callable(&n, number_ctor, None);
    }
    if let Some(b) = e.get("Boolean") {
        super::make_callable(&b, boolean_ctor, None);
    }
    if let Some(o) = e.get("Object") {
        super::make_callable(&o, object_ctor, None);
    }
}

/// `Number(v)`: the numeric coercion of `v`.
fn number_ctor(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(match a.first() {
        None => 0.0,
        Some(v) => v.to_number(),
    }))
}

fn boolean_ctor(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(
        a.first().map(|v| v.is_truthy()).unwrap_or(false),
    ))
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
fn number_to_string(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let value = this.to_number();
    let radix = a.first().map(|v| v.to_number()).unwrap_or(10.0);
    if !(2.0..=36.0).contains(&radix) {
        return Err(VmErr::Msg(
            "RangeError: toString() radix must be between 2 and 36".to_string(),
        ));
    }
    let radix = radix as u32;
    if radix == 10 {
        return Ok(Value::String(crate::format::number_string(value)));
    }
    if !value.is_finite() {
        return Ok(Value::String(crate::format::number_string(value)));
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
        String::from_utf8(digits).unwrap_or_else(|_| "0".to_string()),
    ))
}

/// `toPrecision(digits)`: `digits` significant figures.
fn number_to_precision(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let value = this.to_number();
    let Some(digits) = a.first().map(|v| v.to_number()) else {
        return Ok(Value::String(crate::format::number_string(value)));
    };
    if !(1.0..=100.0).contains(&digits) {
        return Err(VmErr::Msg(
            "RangeError: toPrecision() argument must be between 1 and 100".to_string(),
        ));
    }
    let digits = digits as usize;
    if value == 0.0 {
        return Ok(Value::String(format!("{:.*}", digits - 1, 0.0)));
    }
    // Significant figures = decimal places, shifted by the exponent.
    let exponent = value.abs().log10().floor() as i32;
    let decimals = (digits as i32 - 1 - exponent).max(0) as usize;
    Ok(Value::String(format!("{:.*}", decimals, value)))
}

fn number_value_of(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(this.to_number()))
}

fn number_to_fixed(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let n = this.to_number();
    let digits = a.first().map(|v| v.to_number() as usize).unwrap_or(0);
    if digits > crate::value::MAX_STRING_LEN {
        return Err(crate::value::limit_err("Maximum string length exceeded"));
    }
    Value::checked_string(format!("{:.*}", digits, n))
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
    let s = match a.first() {
        Some(Value::String(s)) => s.clone(),
        Some(v) => interp.vs(v)?,
        None => return Ok(Value::Number(f64::NAN)),
    };
    let mut radix = match a.get(1) {
        Some(v) => v.to_number() as u32,
        None => 0,
    };
    let t = s.trim();
    let mut chars = t.chars();
    let mut first = chars.next();
    let mut neg = false;
    if first == Some('+') {
        first = chars.next();
    } else if first == Some('-') {
        neg = true;
        first = chars.next();
    }
    // Infer radix from a 0x/0X prefix when unspecified.
    if radix == 0 {
        if first == Some('0') {
            let mut peek = chars.clone();
            if matches!(peek.next(), Some('x') | Some('X')) {
                radix = 16;
                chars.next();
                first = chars.next();
            } else {
                radix = 10;
            }
        } else {
            radix = 10;
        }
    }
    let mut val: i64 = 0;
    let mut any = false;
    let mut cur = first;
    while let Some(c) = cur {
        match c.to_digit(radix) {
            Some(d) => {
                val = val.saturating_mul(radix as i64).saturating_add(d as i64);
                any = true;
                cur = chars.next();
            }
            None => break,
        }
    }
    if !any {
        return Ok(Value::Number(f64::NAN));
    }
    Ok(Value::Number((if neg { -val } else { val }) as f64))
}

pub(super) fn parse_float(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = match a.first() {
        Some(Value::String(s)) => s.clone(),
        Some(v) => interp.vs(v)?,
        None => return Ok(Value::Number(f64::NAN)),
    };
    let t = s.trim();
    let mut end = 0usize;
    let mut seen_digit = false;
    for (i, c) in t.char_indices() {
        let ok = c.is_ascii_digit()
            || (c == '.' && seen_digit)
            || ((c == '+' || c == '-') && i == 0)
            || ((c == 'e' || c == 'E') && seen_digit);
        if ok {
            if c.is_ascii_digit() {
                seen_digit = true;
            }
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    match t[..end].parse::<f64>() {
        Ok(n) => Ok(Value::Number(n)),
        Err(_) => Ok(Value::Number(f64::NAN)),
    }
}
