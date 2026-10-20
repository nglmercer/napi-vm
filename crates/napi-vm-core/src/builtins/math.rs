//! `Math` methods. The constants (`PI`, `E`, ...) are installed as plain
//! properties by `setup_builtins`; this module supplies the callable methods.

use super::NativeFn;
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{PropAttrs, Value, to_int32};

pub(super) fn install(e: &mut Environment) {
    if let Some(math) = e.get("Math") {
        let prototype = e.get("Function").and_then(|v| v.get_prop("prototype"));
        for (name, f) in math_methods(prototype) {
            math.set_prop(name, f).expect("built-in Math property");
        }
        let properties = math.property_cell().expect("Math object");
        let keys: Vec<String> = properties
            .borrow()
            .iter()
            .map(|(key, _)| key.clone())
            .collect();
        let mut metadata = properties.meta.borrow_mut();
        for key in keys {
            let constant = matches!(
                key.as_str(),
                "PI" | "E" | "LN2" | "LN10" | "LOG2E" | "LOG10E" | "SQRT1_2" | "SQRT2"
            );
            metadata.set_attrs(
                &key,
                PropAttrs {
                    writable: !constant,
                    enumerable: false,
                    configurable: !constant,
                },
            );
        }
        if let Some(Value::Symbol(ref symbol)) = super::well_known("toStringTag") {
            let key = crate::interpreter::symbol_slot_key(symbol);
            // Release the metadata borrow before inserting the property.
            drop(metadata);
            math.set_prop(key.clone(), Value::String("Math".into()))
                .expect("Math toStringTag");
            let mut metadata = properties.meta.borrow_mut();
            metadata.set_symbol_key(&key, symbol.clone());
            metadata.set_attrs(
                &key,
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
    }
}

fn math_methods(prototype: Option<Value>) -> Vec<(String, Value)> {
    let table: Vec<(&str, NativeFn)> = vec![
        ("abs", math_abs),
        ("sumPrecise", math_sum_precise),
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
        ("clz32", math_clz32),
        ("imul", math_imul),
        ("fround", math_fround),
        ("f16round", math_f16round),
        ("acos", math_acos),
        ("asin", math_asin),
        ("atan", math_atan),
        ("atan2", math_atan2),
        ("acosh", math_acosh),
        ("asinh", math_asinh),
        ("atanh", math_atanh),
        ("cosh", math_cosh),
        ("sinh", math_sinh),
        ("tanh", math_tanh),
        ("expm1", math_expm1),
        ("log1p", math_log1p),
    ];
    table
        .into_iter()
        .map(|(n, f)| {
            let length = match n {
                "random" => 0,
                "pow" | "min" | "max" | "hypot" | "imul" | "atan2" => 2,
                _ => 1,
            };
            (
                n.to_string(),
                super::native_method(n, length, f, prototype.clone()),
            )
        })
        .collect()
}

fn math_arg(interp: &mut Interpreter, args: &[Value], index: usize) -> Result<f64, VmErr> {
    interp.ecmascript_to_number(args.get(index).unwrap_or(&Value::Undefined))
}

macro_rules! unary_math {
    ($name:ident, $operation:ident) => {
        fn $name(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
            Ok(Value::Number(math_arg(interp, &args, 0)?.$operation()))
        }
    };
}
unary_math!(math_acos, acos);
unary_math!(math_asin, asin);
unary_math!(math_atan, atan);
unary_math!(math_acosh, acosh);
unary_math!(math_asinh, asinh);
unary_math!(math_atanh, atanh);
unary_math!(math_cosh, cosh);
unary_math!(math_sinh, sinh);
unary_math!(math_tanh, tanh);
unary_math!(math_expm1, exp_m1);
unary_math!(math_log1p, ln_1p);

fn math_atan2(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let y = math_arg(interp, &args, 0)?;
    let x = math_arg(interp, &args, 1)?;
    Ok(Value::Number(y.atan2(x)))
}

fn math_clz32(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let bits = to_int32(math_arg(interp, &a, 0)?) as u32;
    Ok(Value::Number(bits.leading_zeros() as f64))
}

fn math_imul(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let left = to_int32(math_arg(interp, &a, 0)?);
    let right = to_int32(math_arg(interp, &a, 1)?);
    Ok(Value::Number(left.wrapping_mul(right) as f64))
}

fn math_fround(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number((math_arg(interp, &a, 0)? as f32) as f64))
}

fn math_f16round(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    // Share DataView's conversion: the `half` crate mis-rounds values just
    // above the subnormal midpoint (e.g. 2^-25 + 1ulp rounds to zero).
    let bits = super::typedarray::float16_bits(math_arg(interp, &a, 0)?);
    Ok(Value::Number(super::typedarray::float16_number(bits)))
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
    Ok(Value::Number(crate::interpreter::number_pow(
        math_arg(interp, &a, 0)?,
        math_arg(interp, &a, 1)?,
    )))
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
        // All arguments must be coerced, even after encountering NaN.
        if n.is_nan() || n < m || (n == 0.0 && m == 0.0 && n.is_sign_negative()) {
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
        if n.is_nan() || n > m || (n == 0.0 && m == 0.0 && n.is_sign_positive()) {
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

/// Accumulate finite binary64 values exactly in units of 2^-1074. The largest
/// finite input needs only 2098 bits, so cancellation cannot overflow f64.
fn math_sum_precise(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    use crate::bigint::BigInt;
    let source = args.first().cloned().unwrap_or(Value::Undefined);
    let iterator = interp.iterator_for(&source)?;
    let next = interp.get_prop_value_str(&iterator, "next")?;
    let mut sum = BigInt::zero();
    let mut negative_zero = true;
    let mut positive_infinity = false;
    let mut negative_infinity = false;
    let mut nan = false;
    loop {
        interp.consume_loop()?;
        let result = interp.call_this(&next, iterator.clone(), vec![])?;
        let (done, value) = interp.iterator_result_fields(&result)?;
        if done {
            break;
        }
        let Value::Number(number) = value else {
            let error = VmErr::Msg("TypeError: Math.sumPrecise requires Number elements".into());
            interp.close_guest_iterator_for_abrupt(&iterator, false, &error)?;
            return Err(error);
        };
        if number != 0.0 || !number.is_sign_negative() {
            negative_zero = false;
        }
        if number.is_nan() {
            nan = true;
            continue;
        }
        if number.is_infinite() {
            if number.is_sign_negative() {
                negative_infinity = true;
            } else {
                positive_infinity = true;
            }
            continue;
        }
        let bits = number.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i64;
        let fraction = bits & ((1_u64 << 52) - 1);
        let significand = if exponent == 0 {
            fraction
        } else {
            fraction | (1_u64 << 52)
        };
        let term = BigInt::from_words(number.is_sign_negative(), &[significand])
            .shl(&BigInt::from_i64((exponent - 1).max(0)))
            .map_err(VmErr::Msg)?;
        sum = sum.add(&term).map_err(VmErr::Msg)?;
    }
    let number = if nan || (positive_infinity && negative_infinity) {
        f64::NAN
    } else if positive_infinity {
        f64::INFINITY
    } else if negative_infinity {
        f64::NEG_INFINITY
    } else if sum.is_zero() {
        if negative_zero { -0.0 } else { 0.0 }
    } else {
        rounded_binary64_sum(&sum)
    };
    Ok(Value::Number(number))
}

/// Round the magnitude once to nearest, ties to even, then restore its sign.
fn rounded_binary64_sum(sum: &crate::bigint::BigInt) -> f64 {
    let (negative, words) = sum.to_words();
    let mut highest = (words.len() - 1) * 64 + 63 - words.last().unwrap().leading_zeros() as usize;
    let bit = |index: usize| (words[index / 64] >> (index % 64)) & 1;
    let sign = if negative { 1_u64 << 63 } else { 0 };
    if highest < 52 {
        return f64::from_bits(sign | words[0]);
    }
    let shift = highest - 52;
    let mut significand = 0_u64;
    for index in (shift..=highest).rev() {
        significand = (significand << 1) | bit(index);
    }
    if shift > 0
        && bit(shift - 1) != 0
        && (significand & 1 != 0 || (0..shift - 1).any(|index| bit(index) != 0))
    {
        significand += 1;
        if significand == 1_u64 << 53 {
            significand >>= 1;
            highest += 1;
        }
    }
    let exponent = highest - 51;
    if exponent >= 0x7ff {
        return f64::from_bits(sign | (0x7ff_u64 << 52));
    }
    f64::from_bits(sign | ((exponent as u64) << 52) | (significand & ((1_u64 << 52) - 1)))
}
