//! `String.prototype` methods.

use super::{NativeFn, nf, str_this};
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::Value;

fn bounded_string(value: String) -> Result<Value, VmErr> {
    Value::checked_string(value)
}

pub(super) fn install(e: &mut Environment) {
    if let Some(s) = e.get("String") {
        s.set_prop(
            "fromCharCode".to_string(),
            nf("fromCharCode", string_from_char_code),
        )
        .expect("built-in String property");
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
        super::install_primitive_prototype(e, &s, Value::String(String::new()), methods);
    }
}

fn string_construct(
    interpreter: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let primitive = string_ctor(interpreter, this, args)?;
    Ok(Value::boxed_primitive(primitive).expect("String constructor produces a string"))
}

fn string_value_of(
    interpreter: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    bounded_string(str_this(interpreter, &this)?)
}

/// `String(v)`: the string coercion of `v`.
fn string_ctor(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    match a.first() {
        None => Ok(Value::String(String::new())),
        // `String(sym)` is the one coercion the specification allows for a
        // symbol; template literals and `+` still reject it.
        Some(Value::Symbol(s)) => Ok(Value::String(s.to_display())),
        Some(v) => {
            let rendered = interp.display_string(v)?;
            Value::checked_string(rendered)
        }
    }
}

/// `String.raw(strings, ...subs)`: the cooked strings joined with the
/// substitutions, without escape processing.
fn string_raw(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let template = a.first().cloned().unwrap_or(Value::Undefined);
    let raw = match interp.member(&template, "raw")? {
        Value::Undefined => template,
        other => other,
    };
    let parts = match &raw {
        Value::Array(items) => items.borrow().clone(),
        _ => Vec::new(),
    };
    let mut out = String::new();
    for (index, part) in parts.iter().enumerate() {
        out.push_str(&super::join_str(interp, part)?);
        if index + 1 < parts.len()
            && let Some(sub) = a.get(index + 1)
        {
            out.push_str(&super::join_str(interp, sub)?);
        }
        if out.len() > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
    }
    Value::checked_string(out)
}

/// `at(index)`: a negative index counts back from the end.
fn string_at(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let chars: Vec<char> = str_this(interp, &this)?.chars().collect();
    let index = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    let index = if index < 0.0 {
        chars.len() as f64 + index
    } else {
        index
    };
    if !index.is_finite() || index < 0.0 || index >= chars.len() as f64 {
        return Ok(Value::Undefined);
    }
    Ok(Value::String(chars[index as usize].to_string()))
}

/// Shared implementation of `padStart` and `padEnd`.
fn pad(
    interp: &mut Interpreter,
    this: &Value,
    a: &[Value],
    at_start: bool,
) -> Result<Value, VmErr> {
    let text = str_this(interp, this)?;
    let current = text.chars().count();
    let target = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    if !target.is_finite() || target <= current as f64 {
        return Ok(Value::String(text));
    }
    let target = target as usize;
    if target > crate::value::MAX_STRING_LEN {
        return Err(crate::value::limit_err("Maximum string length exceeded"));
    }
    let filler = match a.get(1) {
        Some(Value::Undefined) | None => " ".to_string(),
        Some(v) => interp.vs(v)?,
    };
    if filler.is_empty() {
        return Ok(Value::String(text));
    }
    // Repeat the filler and cut it to the exact width needed.
    let needed = target - current;
    let padding: String = filler.chars().cycle().take(needed).collect();
    bounded_string(if at_start {
        format!("{}{}", padding, text)
    } else {
        format!("{}{}", text, padding)
    })
}

fn string_pad_start(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    pad(interp, &this, &a, true)
}
fn string_pad_end(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    pad(interp, &this, &a, false)
}

fn string_trim_start(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::String(
        str_this(interp, &this)?.trim_start().to_string(),
    ))
}
fn string_trim_end(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::String(
        str_this(interp, &this)?.trim_end().to_string(),
    ))
}

fn string_last_index_of(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let text = str_this(interp, &this)?;
    let needle = match a.first() {
        Some(Value::String(n)) => n.clone(),
        Some(v) => interp.vs(v)?,
        None => String::new(),
    };
    // Reported in characters, not bytes, like every other string index here.
    Ok(Value::Number(match text.rfind(&needle) {
        Some(byte) => text[..byte].chars().count() as f64,
        None => -1.0,
    }))
}

fn string_code_point_at(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let text = str_this(interp, &this)?;
    let index = a.first().map(|v| v.to_number()).unwrap_or(0.0);
    if !index.is_finite() || index < 0.0 {
        return Ok(Value::Undefined);
    }
    Ok(text
        .chars()
        .nth(index as usize)
        .map(|c| Value::Number(c as u32 as f64))
        .unwrap_or(Value::Undefined))
}

fn string_concat(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let mut out = str_this(interp, &this)?;
    for value in &a {
        out.push_str(&interp.vs(value)?);
        if out.len() > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
    }
    bounded_string(out)
}

/// `localeCompare`: the sandbox has no locale data, so this is an ordinary
/// code-point comparison — which is what it degrades to for ASCII anyway.
fn string_locale_compare(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let left = str_this(interp, &this)?;
    let right = match a.first() {
        Some(Value::String(s)) => s.clone(),
        Some(v) => interp.vs(v)?,
        None => String::new(),
    };
    Ok(Value::Number(match left.cmp(&right) {
        std::cmp::Ordering::Less => -1.0,
        std::cmp::Ordering::Equal => 0.0,
        std::cmp::Ordering::Greater => 1.0,
    }))
}

fn string_from_char_code(_: &mut Interpreter, _this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let mut s = String::new();
    for v in a {
        let n = v.to_number() as u32;
        let Some(ch) = char::from_u32(n) else {
            continue;
        };
        if s.len().saturating_add(ch.len_utf8()) > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
        s.push(ch);
    }
    Value::checked_string(s)
}

/// Dispatch table for `String.prototype` methods, looked up by `prop()`.
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

fn string_to_upper(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    bounded_string(str_this(interp, &this)?.to_uppercase())
}
fn string_to_lower(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    bounded_string(str_this(interp, &this)?.to_lowercase())
}
fn string_trim(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    bounded_string(str_this(interp, &this)?.trim().to_string())
}
fn string_slice(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let norm = |v: f64| -> i64 {
        if v.is_nan() {
            return 0;
        }
        let i = v as i64;
        if i < 0 { (len + i).max(0) } else { i.min(len) }
    };
    let start = norm(a.first().map(|v| v.to_number()).unwrap_or(0.0));
    let end = match a.get(1) {
        Some(v) => norm(v.to_number()),
        None => len,
    };
    if start >= end {
        return Ok(Value::String(String::new()));
    }
    bounded_string(chars[start as usize..end as usize].iter().collect())
}
fn string_split(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    // A regular-expression separator splices its capture groups into the
    // result, so it goes through the pattern path rather than `str::split`.
    if let Some(pattern) = a.first().and_then(|v| v.as_regexp()) {
        let limit = match a.get(1) {
            Some(Value::Undefined) | None => crate::value::MAX_ARRAY_LEN,
            Some(l) => (l.to_number().max(0.0) as usize).min(crate::value::MAX_ARRAY_LEN),
        };
        let input: Vec<char> = s.chars().collect();
        return super::regexp::split_with_pattern(&input, &pattern, limit);
    }
    match a.first() {
        Some(Value::String(sep)) => {
            let limit = match a.get(1) {
                Some(l) => (l.to_number().max(0.0) as usize).min(crate::value::MAX_ARRAY_LEN),
                None => crate::value::MAX_ARRAY_LEN,
            };
            let mut parts: Vec<Value> = Vec::new();
            // An empty separator splits into characters. Rust's `split("")`
            // instead yields a leading and trailing empty string, which is not
            // what JavaScript does.
            if sep.is_empty() {
                for c in s.chars() {
                    if parts.len() >= limit {
                        break;
                    }
                    parts.push(Value::String(c.to_string()));
                }
                return Value::checked_array(parts);
            }
            for p in s.split(sep.as_str()) {
                if parts.len() >= limit {
                    break;
                }
                if parts.len() >= crate::value::MAX_ARRAY_LEN {
                    return Err(crate::value::limit_err("Maximum array length exceeded"));
                }
                parts.push(Value::String(p.to_string()));
            }
            Value::checked_array(parts)
        }
        _ => Value::checked_array(vec![Value::String(s)]),
    }
}
fn string_includes(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let needle = match a.first() {
        Some(Value::String(n)) => n.clone(),
        Some(v) => interp.vs(v)?,
        None => String::new(),
    };
    Ok(Value::Bool(s.contains(&needle)))
}
fn string_index_of(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let needle = match a.first() {
        Some(Value::String(n)) => n.clone(),
        Some(v) => interp.vs(v)?,
        None => String::new(),
    };
    Ok(Value::Number(
        s.find(&needle).map(|i| i as f64).unwrap_or(-1.0),
    ))
}
fn string_char_at(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let idx = a.first().map(|v| v.to_number() as usize).unwrap_or(0);
    Ok(Value::String(
        s.chars()
            .nth(idx)
            .map(|c| c.to_string())
            .unwrap_or_default(),
    ))
}
fn string_starts_with(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let needle = match a.first() {
        Some(Value::String(n)) => n.clone(),
        Some(v) => interp.vs(v)?,
        None => String::new(),
    };
    Ok(Value::Bool(s.starts_with(&needle)))
}
fn string_ends_with(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let needle = match a.first() {
        Some(Value::String(n)) => n.clone(),
        Some(v) => interp.vs(v)?,
        None => String::new(),
    };
    Ok(Value::Bool(s.ends_with(&needle)))
}
fn string_repeat(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let n = a.first().map(|v| v.to_number() as usize).unwrap_or(0);
    if s.len().saturating_mul(n) > crate::value::MAX_STRING_LEN {
        return Err(crate::value::limit_err("Maximum string length exceeded"));
    }
    Value::checked_string(s.repeat(n))
}
/// Handle `replace`/`replaceAll` when the pattern is a regular expression, or
/// when the replacement is a function (which the plain-string path cannot
/// call). Returns `None` when neither applies, so the caller keeps its fast
/// literal path.
fn replace_via_pattern(
    interp: &mut Interpreter,
    subject: &str,
    a: &[Value],
    all: bool,
) -> Result<Option<Value>, VmErr> {
    let replacement = a.get(1).cloned().unwrap_or(Value::Undefined);
    let callable = matches!(
        replacement,
        Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
    );
    let pattern = match a.first() {
        Some(value) if value.as_regexp().is_some() => {
            value.as_regexp().expect("checked immediately above")
        }
        Some(value) if callable => {
            // A literal needle with a function replacement: compile the
            // literal so both go through one implementation.
            let needle = interp.vs(value)?;
            let flags = if all { "g" } else { "" };
            let compiled = super::regexp::compile(&super::regexp::escape_pattern(&needle), flags)?;
            compiled.as_regexp().expect("compile returns a RegExp")
        }
        _ => return Ok(None),
    };
    let input: Vec<char> = subject.chars().collect();
    let replaced =
        super::regexp::replace_with_pattern(interp, &input, &pattern, &replacement, all)?;
    Ok(Some(replaced))
}

fn string_replace(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    if let Some(replaced) = replace_via_pattern(interp, &s, &a, false)? {
        return Ok(replaced);
    }
    let from = match a.first() {
        Some(Value::String(n)) => n.clone(),
        Some(v) => interp.vs(v)?,
        None => return Ok(Value::String(s)),
    };
    let to = match a.get(1) {
        Some(Value::String(n)) => n.clone(),
        Some(v) => interp.vs(v)?,
        None => String::new(),
    };
    let replaces: usize = if from.is_empty() || s.contains(&from) {
        1
    } else {
        0
    };
    let result_len = if to.len() >= from.len() {
        s.len()
            .saturating_add(replaces.saturating_mul(to.len().saturating_sub(from.len())))
    } else {
        s.len()
            .saturating_sub(replaces.saturating_mul(from.len().saturating_sub(to.len())))
    };
    if result_len > crate::value::MAX_STRING_LEN {
        return Err(crate::value::limit_err("Maximum string length exceeded"));
    }
    bounded_string(s.replacen(&from, &to, 1))
}
fn string_replace_all(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    if let Some(replaced) = replace_via_pattern(interp, &s, &a, true)? {
        return Ok(replaced);
    }
    let from = match a.first() {
        Some(Value::String(n)) => n.clone(),
        Some(v) => interp.vs(v)?,
        None => return Ok(Value::String(s)),
    };
    let to = match a.get(1) {
        Some(Value::String(n)) => n.clone(),
        Some(v) => interp.vs(v)?,
        None => String::new(),
    };
    // Estimate the result size before allocating: replacing millions of
    // matches with a long string could otherwise exhaust host memory. An
    // empty pattern matches at every char boundary (len+1 insertions).
    let matches = if from.is_empty() {
        s.chars().count() as i128 + 1
    } else {
        s.matches(&from).count() as i128
    };
    let delta_per = to.len() as i128 - from.len() as i128;
    if matches > 0 && delta_per > 0 {
        let estimate = s.len() as i128 + matches * delta_per;
        if estimate > crate::value::MAX_STRING_LEN as i128 {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
    }
    bounded_string(s.replace(&from, &to))
}
fn string_char_code_at(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let idx = a.first().map(|v| v.to_number() as usize).unwrap_or(0);
    match s.chars().nth(idx) {
        Some(ch) => Ok(Value::Number(ch as u32 as f64)),
        None => Ok(Value::Number(f64::NAN)),
    }
}
fn string_substring(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = str_this(interp, &this)?;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len() as i64;
    let norm = |v: f64| -> i64 {
        if v.is_nan() {
            return 0;
        }
        let i = v as i64;
        if i < 0 { 0 } else { i.min(len) }
    };
    let mut start = norm(a.first().map(|v| v.to_number()).unwrap_or(0.0));
    let mut end = match a.get(1) {
        Some(v) => norm(v.to_number()),
        None => len,
    };
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    bounded_string(chars[start as usize..end as usize].iter().collect())
}
