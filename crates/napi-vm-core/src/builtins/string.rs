//! ECMAScript string operations use UTF-16 code-unit indices.
use super::{NativeFn, nf, str_this};
use crate::{
    JsString,
    error::VmErr,
    interpreter::{Environment, Interpreter},
    value::{Value, to_integer_or_infinity},
};
fn bounded_string(value: impl Into<JsString>) -> Result<Value, VmErr> {
    Value::checked_string(value)
}
fn arg_string(interp: &mut Interpreter, args: &[Value], index: usize) -> Result<JsString, VmErr> {
    interp.ecmascript_to_string(args.get(index).unwrap_or(&Value::Undefined))
}
fn position(
    interp: &mut Interpreter,
    args: &[Value],
    index: usize,
    default: f64,
) -> Result<f64, VmErr> {
    Ok(to_integer_or_infinity(match args.get(index) {
        Some(value) => interp.ecmascript_to_number(value)?,
        None => default,
    }))
}
fn clamp(n: f64, len: usize) -> usize {
    if n.is_nan() || n <= 0.0 {
        0
    } else {
        (n as usize).min(len)
    }
}
fn string_construct(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    Ok(Value::boxed_primitive(string_ctor(interp, this, args)?).unwrap())
}
fn string_ctor(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    match args.first() {
        None => bounded_string(JsString::default()),
        Some(Value::Symbol(s)) => bounded_string(s.to_display()),
        Some(v) => bounded_string(interp.display_string(v)?),
    }
}
fn string_value_of(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    match &this {
        Value::String(value) => bounded_string(value.clone()),
        Value::Object { props } => match &props.meta.borrow().boxed_primitive {
            Some(crate::value::BoxedPrimitive::String(value)) => bounded_string(value.clone()),
            _ => Err(VmErr::Msg("TypeError: incompatible String receiver".into())),
        },
        _ => Err(VmErr::Msg("TypeError: incompatible String receiver".into())),
    }
}
fn string_raw(interp: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let template = args.first().cloned().unwrap_or(Value::Undefined);
    let raw = interp.member(&template, "raw")?;
    let length = interp.member(&raw, "length")?.to_number().max(0.0) as usize;
    if length > crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    let mut out = JsString::default();
    for i in 0..length {
        let part = interp.member(&raw, &i.to_string())?;
        out.push_str(interp.display_string(&part)?);
        if i + 1 < length
            && let Some(v) = args.get(i + 1)
        {
            out.push_str(interp.display_string(v)?);
        }
        if out.len() > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
    }
    bounded_string(out)
}
fn string_from_char_code(
    interp: &mut Interpreter,
    _: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    bounded_string(JsString::from_units(
        args.iter()
            .map(|v| {
                interp
                    .ecmascript_to_number(v)
                    .map(|n| crate::value::to_int32(n) as u16)
            })
            .collect::<Result<Vec<_>, _>>()?,
    ))
}
fn string_from_code_point(
    interp: &mut Interpreter,
    _: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let mut out = Vec::new();
    for v in args {
        let n = interp.ecmascript_to_number(&v)?;
        if !n.is_finite() || n.fract() != 0.0 || !(0.0..=0x10FFFF as f64).contains(&n) {
            return Err(crate::value::limit_err("Invalid code point"));
        }
        let cp = n as u32;
        if cp <= 0xFFFF {
            out.push(cp as u16);
        } else {
            out.push((0xD800 + ((cp - 0x10000) >> 10)) as u16);
            out.push((0xDC00 + ((cp - 0x10000) & 1023)) as u16);
        }
    }
    bounded_string(JsString::from_units(out))
}
fn string_char_at(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let n = position(interp, &args, 0, 0.0)?;
    if !n.is_finite() || n < 0.0 || n >= s.len() as f64 {
        return bounded_string("");
    }
    bounded_string(s.slice(n as usize, n as usize + 1))
}
fn string_at(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let mut n = position(interp, &args, 0, 0.0)?;
    if n < 0.0 {
        n += s.len() as f64;
    }
    if !n.is_finite() || n < 0.0 || n >= s.len() as f64 {
        return Ok(Value::Undefined);
    }
    bounded_string(s.slice(n as usize, n as usize + 1))
}
fn string_char_code_at(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let n = position(interp, &args, 0, 0.0)?;
    Ok(Value::Number(if n < 0.0 || !n.is_finite() {
        f64::NAN
    } else {
        s.units()
            .get(n as usize)
            .map(|u| *u as f64)
            .unwrap_or(f64::NAN)
    }))
}
fn string_code_point_at(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let n = position(interp, &args, 0, 0.0)?;
    Ok(if n < 0.0 || !n.is_finite() {
        Value::Undefined
    } else {
        s.code_point_at(n as usize)
            .map(|cp| Value::Number(cp as f64))
            .unwrap_or(Value::Undefined)
    })
}
fn string_slice(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let norm = |n: f64| {
        if n < 0.0 {
            clamp(s.len() as f64 + n, s.len())
        } else {
            clamp(n, s.len())
        }
    };
    let a = norm(position(interp, &args, 0, 0.0)?);
    let b = if matches!(args.get(1), None | Some(Value::Undefined)) {
        s.len()
    } else {
        norm(position(interp, &args, 1, 0.0)?)
    };
    bounded_string(s.slice(a, b.max(a)))
}
fn string_substring(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let a = clamp(position(interp, &args, 0, 0.0)?, s.len());
    let b = if matches!(args.get(1), None | Some(Value::Undefined)) {
        s.len()
    } else {
        clamp(position(interp, &args, 1, 0.0)?, s.len())
    };
    bounded_string(s.slice(a.min(b), a.max(b)))
}
fn reject_regexp(args: &[Value]) -> Result<(), VmErr> {
    if args.first().is_some_and(|v| v.as_regexp().is_some()) {
        Err(VmErr::Msg(
            "TypeError: search string cannot be a RegExp".into(),
        ))
    } else {
        Ok(())
    }
}
fn string_index_of(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let n = arg_string(interp, &args, 0)?;
    let from = clamp(position(interp, &args, 1, 0.0)?, s.len());
    Ok(Value::Number(
        s.find_from(&n, from).map(|i| i as f64).unwrap_or(-1.0),
    ))
}
fn string_includes(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    reject_regexp(&args)?;
    let result = string_index_of(interp, this, args)?;
    Ok(Value::Bool(result.to_number() >= 0.0))
}
fn string_last_index_of(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let n = arg_string(interp, &args, 0)?;
    let raw = args.get(1).map(Value::to_number).unwrap_or(f64::INFINITY);
    let pos = if raw.is_nan() {
        s.len()
    } else {
        clamp(raw, s.len())
    };
    if n.is_empty() {
        return Ok(Value::Number(pos as f64));
    }
    if n.len() > s.len() {
        return Ok(Value::Number(-1.0));
    }
    Ok(Value::Number(
        (0..=pos.min(s.len() - n.len()))
            .rev()
            .find(|i| &s.units()[*i..*i + n.len()] == n.units())
            .map(|i| i as f64)
            .unwrap_or(-1.0),
    ))
}
fn string_starts_with(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    reject_regexp(&args)?;
    let s = str_this(interp, &this)?;
    let n = arg_string(interp, &args, 0)?;
    let pos = clamp(position(interp, &args, 1, 0.0)?, s.len());
    Ok(Value::Bool(s.units()[pos..].starts_with(n.units())))
}
fn string_ends_with(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    reject_regexp(&args)?;
    let s = str_this(interp, &this)?;
    let n = arg_string(interp, &args, 0)?;
    let pos = if matches!(args.get(1), None | Some(Value::Undefined)) {
        s.len()
    } else {
        clamp(position(interp, &args, 1, 0.0)?, s.len())
    };
    Ok(Value::Bool(s.units()[..pos].ends_with(n.units())))
}
fn string_concat(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let mut s = str_this(interp, &this)?;
    for a in args {
        let part = interp.display_string(&a)?;
        if s.len().saturating_add(part.len()) > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
        s.push_str(part);
    }
    bounded_string(s)
}
fn string_repeat(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let n = position(interp, &args, 0, 0.0)?;
    if !n.is_finite() || n < 0.0 {
        return Err(crate::value::limit_err("Invalid repeat count"));
    }
    if s.len().saturating_mul(n as usize) > crate::value::MAX_STRING_LEN {
        return Err(crate::value::limit_err("Maximum string length exceeded"));
    }
    bounded_string(s.repeat(n as usize))
}
fn pad(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
    start: bool,
) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let len = clamp(position(interp, &args, 0, 0.0)?, usize::MAX);
    if len <= s.len() {
        return bounded_string(s);
    }
    if len > crate::value::MAX_STRING_LEN {
        return Err(crate::value::limit_err("Maximum string length exceeded"));
    }
    let fill = if matches!(args.get(1), None | Some(Value::Undefined)) {
        JsString::from(" ")
    } else {
        arg_string(interp, &args, 1)?
    };
    if fill.is_empty() {
        return bounded_string(s);
    }
    let pad = JsString::from_units(
        fill.units()
            .iter()
            .copied()
            .cycle()
            .take(len - s.len())
            .collect::<Vec<_>>(),
    );
    bounded_string(if start {
        pad.concat(&s)
    } else {
        s.concat(&pad)
    })
}
fn string_pad_start(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    pad(i, t, a, true)
}
fn string_pad_end(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    pad(i, t, a, false)
}
fn string_trim(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    bounded_string(str_this(i, &t)?.trim_units(true, true))
}
fn string_trim_start(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    bounded_string(str_this(i, &t)?.trim_units(true, false))
}
fn string_trim_end(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    bounded_string(str_this(i, &t)?.trim_units(false, true))
}
fn string_to_upper(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    bounded_string(str_this(i, &t)?.map_case(true))
}
fn string_to_lower(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    bounded_string(str_this(i, &t)?.map_case(false))
}
fn string_locale_compare(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let l = str_this(i, &t)?;
    let r = arg_string(i, &a, 0)?;
    Ok(Value::Number(match l.cmp(&r) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}
fn string_split(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(i, &t)?;
    let limit = match a.get(1) {
        None | Some(Value::Undefined) => crate::value::MAX_ARRAY_LEN,
        Some(v) => (crate::value::to_int32(i.ecmascript_to_number(v)?) as u32 as usize)
            .min(crate::value::MAX_ARRAY_LEN),
    };
    if limit == 0 {
        return Value::checked_array(vec![]);
    }
    if let Some(re) = a.first().and_then(Value::as_regexp) {
        return super::regexp::split_with_pattern(i, &s, &re, limit);
    }
    if matches!(a.first(), None | Some(Value::Undefined)) {
        return Value::checked_array(vec![Value::String(s)]);
    }
    let needle = arg_string(i, &a, 0)?;
    let mut parts = Vec::new();
    if needle.is_empty() {
        for &u in s.units().iter().take(limit) {
            parts.push(Value::String(JsString::from_units(vec![u])));
        }
    } else {
        let mut from = 0;
        while let Some(at) = s.find_from(&needle, from) {
            parts.push(Value::String(s.slice(from, at)));
            if parts.len() == limit {
                return Value::checked_array(parts);
            }
            from = at + needle.len();
        }
        parts.push(Value::String(s.slice(from, s.len())));
        parts.truncate(limit);
    }
    Value::checked_array(parts)
}
fn replace(i: &mut Interpreter, t: Value, a: Vec<Value>, all: bool) -> Result<Value, VmErr> {
    let s = str_this(i, &t)?;
    let replacement = a.get(1).cloned().unwrap_or(Value::Undefined);
    if let Some(re) = a.first().and_then(Value::as_regexp) {
        if all && !re.regex.borrow().global {
            return Err(VmErr::Msg(
                "TypeError: replaceAll requires a global RegExp".into(),
            ));
        }
        return super::regexp::replace_with_pattern(i, &s, &re, &replacement, all);
    }
    let needle = arg_string(i, &a, 0)?;
    let callable = crate::interpreter::call::is_callable_value(&replacement);
    let text = if callable {
        JsString::default()
    } else {
        i.display_string(&replacement)?
    };
    let mut out = JsString::default();
    let mut consumed = 0;
    let mut search = 0;
    while let Some(at) = s.find_from(&needle, search) {
        out.push_str(s.slice(consumed, at));
        let matched = s.slice(at, at + needle.len());
        if callable {
            let result = i.call_this(
                &replacement,
                Value::Undefined,
                vec![
                    Value::String(matched),
                    Value::Number(at as f64),
                    Value::String(s.clone()),
                ],
            )?;
            out.push_str(i.display_string(&result)?);
        } else {
            out.push_str(substitution(&text, &matched, &s, at, at + needle.len()));
        }
        if out.len() > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
        consumed = at + needle.len();
        if !all {
            break;
        }
        search = consumed + usize::from(needle.is_empty());
        if search > s.len() {
            break;
        }
    }
    out.push_str(s.slice(consumed, s.len()));
    bounded_string(out)
}
pub(crate) fn substitution(
    replacement: &JsString,
    matched: &JsString,
    subject: &JsString,
    start: usize,
    end: usize,
) -> JsString {
    let mut out = JsString::default();
    let mut pos = 0;
    while pos < replacement.len() {
        let unit = replacement.units()[pos];
        if unit == 36 && pos + 1 < replacement.len() {
            match replacement.units()[pos + 1] {
                36 => out.push_str("$"),
                38 => out.push_str(matched),
                96 => out.push_str(subject.slice(0, start)),
                39 => out.push_str(subject.slice(end, subject.len())),
                _ => {
                    out.push_str(replacement.slice(pos, pos + 1));
                    pos += 1;
                    continue;
                }
            }
            pos += 2;
        } else {
            out.push_str(replacement.slice(pos, pos + 1));
            pos += 1;
        }
    }
    out
}
fn string_replace(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    replace(i, t, a, false)
}
fn string_replace_all(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    replace(i, t, a, true)
}
pub(super) fn install(e: &mut Environment) {
    if let Some(s) = e.get("String") {
        s.set_prop(
            "fromCharCode".to_string(),
            nf("fromCharCode", string_from_char_code),
        )
        .expect("built-in String property");
        s.set_prop(
            "fromCodePoint".into(),
            nf("fromCodePoint", string_from_code_point),
        )
        .expect("String.fromCodePoint");
        s.set_prop("raw".to_string(), nf("raw", string_raw))
            .expect("built-in String property");
        super::make_callable(&s, string_ctor, Some(string_construct));
        let mut methods: Vec<(&str, Value)> = [
            "toUpperCase",
            "toLowerCase",
            "trim",
            "slice",
            "substring",
            "split",
            "match",
            "matchAll",
            "search",
            "includes",
            "indexOf",
            "charAt",
            "startsWith",
            "endsWith",
            "repeat",
            "replace",
            "replaceAll",
            "charCodeAt",
            "at",
            "padStart",
            "padEnd",
            "trimStart",
            "trimEnd",
            "lastIndexOf",
            "codePointAt",
            "concat",
            "localeCompare",
        ]
        .into_iter()
        .filter_map(|name| super::string_method(name).map(|method| (name, method)))
        .collect();
        methods.push(("toString", nf("toString", string_value_of)));
        methods.push(("valueOf", nf("valueOf", string_value_of)));
        super::install_primitive_prototype(e, &s, Value::String((String::new()).into()), methods);
    }
}

pub fn string_method(name: &str) -> Option<Value> {
    let f: NativeFn = match name {
        "toUpperCase" => string_to_upper,
        "toLowerCase" => string_to_lower,
        "trim" => string_trim,
        "slice" => string_slice,
        "substring" => string_substring,
        "split" => string_split,
        "match" => super::regexp::string_match,
        "matchAll" => super::regexp::string_match_all,
        "search" => super::regexp::string_search,
        "includes" => string_includes,
        "indexOf" => string_index_of,
        "charAt" => string_char_at,
        "startsWith" => string_starts_with,
        "endsWith" => string_ends_with,
        "repeat" => string_repeat,
        "replace" => string_replace,
        "replaceAll" => string_replace_all,
        "charCodeAt" => string_char_code_at,
        "at" => string_at,
        "padStart" => string_pad_start,
        "padEnd" => string_pad_end,
        "trimStart" => string_trim_start,
        "trimEnd" => string_trim_end,
        "lastIndexOf" => string_last_index_of,
        "codePointAt" => string_code_point_at,
        "concat" => string_concat,
        "localeCompare" => string_locale_compare,
        _ => return None,
    };
    Some(nf(name, f))
}
