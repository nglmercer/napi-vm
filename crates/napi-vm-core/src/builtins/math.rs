//! `Math` methods. The constants (`PI`, `E`, ...) are installed as plain
//! properties by `setup_builtins`; this module supplies the callable methods.

use super::{NativeFn, nf};
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::Value;

pub(super) fn install(e: &mut Environment) {
    if let Some(math) = e.get("Math") {
        for (name, f) in math_methods() {
            math.set_prop(name, f).expect("built-in Math property");
        }
    }
}

fn math_methods() -> Vec<(String, Value)> {
    let table: Vec<(&str, NativeFn)> = vec![
        ("abs", math_abs),
        ("floor", math_floor),
        ("ceil", math_ceil),
        ("round", math_round),
        ("sqrt", math_sqrt),
        ("cbrt", math_cbrt),
        ("pow", math_pow),
        ("min", math_min),
        ("max", math_max),
        ("random", math_random),
        ("trunc", math_trunc),
        ("sign", math_sign),
        ("log", math_log),
        ("log2", math_log2),
        ("log10", math_log10),
        ("exp", math_exp),
        ("sin", math_sin),
        ("cos", math_cos),
        ("tan", math_tan),
        ("hypot", math_hypot),
    ];
    table
        .into_iter()
        .map(|(n, f)| (n.to_string(), nf(n, f)))
        .collect()
}

fn math_arg(interp: &mut Interpreter, args: &[Value], index: usize) -> Result<f64, VmErr> {
    interp.ecmascript_to_number(args.get(index).unwrap_or(&Value::Undefined))
}

fn math_abs(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.abs()))
}
fn math_floor(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.floor()))
}
fn math_ceil(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.ceil()))
}
fn math_round(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let x = math_arg(interp, &a, 0)?;
    // JS rounds halves toward +Infinity.
    let rounded = if x == 0.0 || !x.is_finite() {
        x
    } else if (-0.5..0.0).contains(&x) || x == -0.5 {
        -0.0
    } else {
        let floor = x.floor();
        if x - floor < 0.5 { floor } else { floor + 1.0 }
    };
    Ok(Value::Number(rounded))
}
fn math_sqrt(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.sqrt()))
}
fn math_cbrt(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.cbrt()))
}
fn math_pow(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(
        math_arg(interp, &a, 0)?.powf(math_arg(interp, &a, 1)?),
    ))
}
fn math_trunc(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.trunc()))
}
fn math_sign(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let x = math_arg(interp, &a, 0)?;
    let r = if x.is_nan() {
        f64::NAN
    } else if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        x
    };
    Ok(Value::Number(r))
}
fn math_log(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.ln()))
}
fn math_log2(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.log2()))
}
fn math_log10(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.log10()))
}
fn math_exp(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.exp()))
}
fn math_sin(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.sin()))
}
fn math_cos(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.cos()))
}
fn math_tan(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(math_arg(interp, &a, 0)?.tan()))
}
fn math_min(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if a.is_empty() {
        return Ok(Value::Number(f64::INFINITY));
    }
    let mut m = f64::INFINITY;
    for v in &a {
        let n = interp.ecmascript_to_number(v)?;
        if n.is_nan() {
            return Ok(Value::Number(f64::NAN));
        }
        if n < m || (n == 0.0 && m == 0.0 && n.is_sign_negative()) {
            m = n;
        }
    }
    Ok(Value::Number(m))
}
fn math_max(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    if a.is_empty() {
        return Ok(Value::Number(f64::NEG_INFINITY));
    }
    let mut m = f64::NEG_INFINITY;
    for v in &a {
        let n = interp.ecmascript_to_number(v)?;
        if n.is_nan() {
            return Ok(Value::Number(f64::NAN));
        }
        if n > m || (n == 0.0 && m == 0.0 && n.is_sign_positive()) {
            m = n;
        }
    }
    Ok(Value::Number(m))
}
fn math_hypot(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    // Scale before squaring so large and tiny finite inputs stay finite and
    // nonzero. Infinity takes precedence over NaN regardless of input order.
    let values = a
        .iter()
        .map(|v| interp.ecmascript_to_number(v).map(f64::abs))
        .collect::<Result<Vec<_>, _>>()?;
    if values.iter().any(|n| n.is_infinite()) {
        return Ok(Value::Number(f64::INFINITY));
    }
    if values.iter().any(|n| n.is_nan()) {
        return Ok(Value::Number(f64::NAN));
    }
    let scale = values.iter().copied().fold(0.0, f64::max);
    if scale == 0.0 {
        return Ok(Value::Number(0.0));
    }
    let sum: f64 = values.iter().map(|n| (n / scale).powi(2)).sum();
    Ok(Value::Number(scale * sum.sqrt()))
}
fn math_random(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let mut x = SEED.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    SEED.store(x, Ordering::Relaxed);
    Ok(Value::Number((x >> 11) as f64 / (1u64 << 53) as f64))
}
