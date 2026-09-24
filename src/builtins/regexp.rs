//! The `RegExp` constructor, its instance methods, and the string methods that
//! take a pattern (`match`, `matchAll`, `replace`, `replaceAll`, `search`,
//! `split`).

use std::cell::Cell;
use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::regex::{Captures, Regex};
use crate::value::{RegExpData, Value};

pub(super) fn install(e: &mut Environment) {
    if let Some(namespace) = e.get("RegExp") {
        super::make_callable(&namespace, regexp_construct, None);

        let object_prototype = e
            .get("Object")
            .and_then(|object| object.get_prop("prototype"));
        let prototype = Value::object_with_proto(
            vec![
                ("constructor".into(), namespace.clone()),
                ("exec".into(), super::nf("exec", regexp_exec)),
                ("test".into(), super::nf("test", regexp_test)),
                ("toString".into(), super::nf("toString", regexp_to_string)),
            ],
            object_prototype.map(Rc::new),
        );
        if let Value::Object { props } = &prototype {
            let mut metadata = props.meta.borrow_mut();
            for name in ["constructor", "exec", "test", "toString"] {
                metadata.set_attrs(
                    name,
                    crate::value::PropAttrs {
                        enumerable: false,
                        ..crate::value::PropAttrs::default()
                    },
                );
            }
        }
        super::set_builtin_constructor_prototype(e, &namespace, prototype);
    }
}

fn type_err(message: String) -> VmErr {
    VmErr::Msg(format!("SyntaxError: {}", message))
}

pub(crate) fn compile(source: &str, flags: &str) -> Result<Value, VmErr> {
    let regex = Regex::new(source, flags).map_err(type_err)?;
    Ok(Value::RegExp(Rc::new(RegExpData {
        regex,
        last_index: Cell::new(0),
    })))
}

/// `RegExp(pattern, flags)`. A `RegExp` argument is re-compiled, taking its
/// own flags unless new ones are given.
fn regexp_construct(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let (source, own_flags) = match a.first() {
        Some(Value::RegExp(data)) => (data.regex.source.clone(), data.regex.flags.clone()),
        Some(Value::Undefined) | None => (String::new(), String::new()),
        Some(other) => (interp.vs(other)?, String::new()),
    };
    let flags = match a.get(1) {
        Some(Value::Undefined) | None => own_flags,
        Some(other) => interp.vs(other)?,
    };
    // `RegExp(/(?:)/)` round-trips through the canonical empty source.
    let source = if source == "(?:)" { "" } else { &source };
    compile(source, &flags)
}

/// Properties and methods readable on a regular expression.
pub fn regexp_member(data: &Rc<RegExpData>, key: &str) -> Option<Value> {
    let regex = &data.regex;
    Some(match key {
        "source" => Value::String(regex.source.clone()),
        "flags" => Value::String(regex.flags.clone()),
        "global" => Value::Bool(regex.global),
        "ignoreCase" => Value::Bool(regex.ignore_case),
        "multiline" => Value::Bool(regex.multiline),
        "dotAll" => Value::Bool(regex.dot_all),
        "sticky" => Value::Bool(regex.sticky),
        "unicode" => Value::Bool(regex.unicode),
        "lastIndex" => Value::Number(data.last_index.get() as f64),
        _ => return None,
    })
}

fn regexp_to_string(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let Some(data) = this.as_regexp() else {
        return Ok(Value::String("/(?:)/".to_string()));
    };
    Ok(Value::String(format!(
        "/{}/{}",
        data.regex.source, data.regex.flags
    )))
}

/// Build the array `exec` returns: the whole match, then each group, with
/// `index`, `input` and `groups` as named properties.
fn match_result(data: &Rc<RegExpData>, input: &[char], caps: &Captures) -> Result<Value, VmErr> {
    let slice = |range: Option<(usize, usize)>| match range {
        Some((start, end)) => Value::String(input[start..end].iter().collect()),
        None => Value::Undefined,
    };
    let items: Vec<Value> = caps.iter().map(|c| slice(*c)).collect();
    let result = Value::checked_array(items)?;
    let start = caps[0].map(|(s, _)| s).unwrap_or(0);
    result.set_prop("index".to_string(), Value::Number(start as f64))?;
    result.set_prop(
        "input".to_string(),
        Value::String(input.iter().collect::<String>()),
    )?;
    let groups = if data.regex.names.is_empty() {
        Value::Undefined
    } else {
        let mut named: Vec<(String, Value)> = data
            .regex
            .names
            .iter()
            .map(|(name, index)| (name.clone(), slice(caps.get(*index).copied().flatten())))
            .collect();
        named.sort_by(|a, b| a.0.cmp(&b.0));
        Value::checked_object(named)?
    };
    result.set_prop("groups".to_string(), groups)?;
    Ok(result)
}

/// Run one search, honouring and updating `lastIndex` for a `g`/`y` pattern.
fn exec(data: &Rc<RegExpData>, input: &[char]) -> Result<Option<Captures>, VmErr> {
    let stateful = data.regex.global || data.regex.sticky;
    let start = if stateful { data.last_index.get() } else { 0 };
    if start > input.len() {
        data.last_index.set(0);
        return Ok(None);
    }
    let found = data
        .regex
        .find_at(input, start)
        .map_err(|e| VmErr::Msg(e.to_string()))?;
    match &found {
        Some(caps) => {
            if stateful {
                let end = caps[0].map(|(_, e)| e).unwrap_or(start);
                // An empty match must still advance, or a `g` loop never ends.
                data.last_index
                    .set(if end == start { end + 1 } else { end });
            }
        }
        None => {
            if stateful {
                data.last_index.set(0);
            }
        }
    }
    Ok(found)
}

fn regexp_exec(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let Some(data) = this.as_regexp() else {
        return Err(VmErr::Msg(
            "TypeError: RegExp.prototype.exec called on a non-RegExp".to_string(),
        ));
    };
    let subject = subject_chars(interp, a.first())?;
    match exec(&data, &subject)? {
        Some(caps) => match_result(&data, &subject, &caps),
        None => Ok(Value::Null),
    }
}

fn regexp_test(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let Some(data) = this.as_regexp() else {
        return Err(VmErr::Msg(
            "TypeError: RegExp.prototype.test called on a non-RegExp".to_string(),
        ));
    };
    let subject = subject_chars(interp, a.first())?;
    Ok(Value::Bool(exec(&data, &subject)?.is_some()))
}

fn subject_chars(interp: &Interpreter, value: Option<&Value>) -> Result<Vec<char>, VmErr> {
    Ok(match value {
        Some(Value::String(s)) => s.chars().collect(),
        Some(other) => interp.vs(other)?.chars().collect(),
        None => "undefined".chars().collect(),
    })
}

// ---------------------------------------------------------------------------
// String methods that take a pattern.
// ---------------------------------------------------------------------------

/// Coerce the pattern argument of a string method: a `RegExp` is used as-is, a
/// string is a literal to find (not a pattern to compile).
fn as_pattern(value: Option<&Value>) -> Option<Rc<RegExpData>> {
    value?.as_regexp()
}

/// Every match of a global pattern, or just the first for a non-global one.
fn all_matches(data: &Rc<RegExpData>, input: &[char]) -> Result<Vec<Captures>, VmErr> {
    let mut out = Vec::new();
    let mut at = 0usize;
    loop {
        let found = data
            .regex
            .find_at(input, at)
            .map_err(|e| VmErr::Msg(e.to_string()))?;
        let Some(caps) = found else { break };
        let (start, end) = caps[0].unwrap_or((at, at));
        out.push(caps);
        if !data.regex.global {
            break;
        }
        // An empty match advances by one so the scan terminates.
        at = if end == start { end + 1 } else { end };
        if at > input.len() || out.len() > crate::value::MAX_ARRAY_LEN {
            break;
        }
    }
    Ok(out)
}

/// `str.match(pattern)`.
///
/// A global pattern returns every matched substring; a non-global one returns
/// the full `exec` result, groups and all.
pub fn string_match(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let input: Vec<char> = super::str_this(interp, &this)?.chars().collect();
    let Some(data) = as_pattern(a.first()) else {
        let source = interp.vs(a.first().unwrap_or(&Value::Undefined))?;
        let compiled = compile(&escape_pattern(&source), "")?;
        return string_match(interp, this, vec![compiled]);
    };
    if data.regex.global {
        data.last_index.set(0);
        let matches = all_matches(&data, &input)?;
        if matches.is_empty() {
            return Ok(Value::Null);
        }
        let items = matches
            .iter()
            .map(|caps| match caps[0] {
                Some((start, end)) => Value::String(input[start..end].iter().collect()),
                None => Value::Undefined,
            })
            .collect();
        return Value::checked_array(items);
    }
    match exec(&data, &input)? {
        Some(caps) => match_result(&data, &input, &caps),
        None => Ok(Value::Null),
    }
}

/// `str.matchAll(pattern)`: an array of full match results.
///
/// The specification returns an iterator; an array is iterable in every way
/// guest code uses one here (`for…of`, spread, `Array.from`).
pub fn string_match_all(
    interp: &mut Interpreter,
    this: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let input: Vec<char> = super::str_this(interp, &this)?.chars().collect();
    let Some(data) = as_pattern(a.first()) else {
        return Err(VmErr::Msg(
            "TypeError: matchAll requires a global RegExp".to_string(),
        ));
    };
    if !data.regex.global {
        return Err(VmErr::Msg(
            "TypeError: matchAll must be called with a global RegExp".to_string(),
        ));
    }
    let matches = all_matches(&data, &input)?;
    let items = matches
        .iter()
        .map(|caps| match_result(&data, &input, caps))
        .collect::<Result<Vec<_>, _>>()?;
    Value::checked_array(items)
}

/// `str.search(pattern)`: the index of the first match, or `-1`.
pub fn string_search(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let input: Vec<char> = super::str_this(interp, &this)?.chars().collect();
    let data = match as_pattern(a.first()) {
        Some(data) => data,
        None => {
            let source = interp.vs(a.first().unwrap_or(&Value::Undefined))?;
            let compiled = compile(&escape_pattern(&source), "")?;
            compiled.as_regexp().expect("compile returns a RegExp")
        }
    };
    let found = data
        .regex
        .find_at(&input, 0)
        .map_err(|e| VmErr::Msg(e.to_string()))?;
    Ok(Value::Number(match found {
        Some(caps) => caps[0].map(|(start, _)| start as f64).unwrap_or(-1.0),
        None => -1.0,
    }))
}

/// Escape a string so it matches itself when compiled as a pattern.
pub(crate) fn escape_pattern(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\^$.|?*+()[]{}/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Expand `$&`, `$1`, `$<name>` and friends in a replacement template.
fn expand(
    template: &str,
    input: &[char],
    caps: &Captures,
    data: &Rc<RegExpData>,
) -> Result<String, VmErr> {
    let slice = |range: Option<(usize, usize)>| -> String {
        match range {
            Some((start, end)) => input[start..end].iter().collect(),
            None => String::new(),
        }
    };
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    let (whole_start, whole_end) = caps[0].unwrap_or((0, 0));
    while index < chars.len() {
        if chars[index] != '$' || index + 1 >= chars.len() {
            out.push(chars[index]);
            index += 1;
            continue;
        }
        match chars[index + 1] {
            '$' => {
                out.push('$');
                index += 2;
            }
            '&' => {
                out.push_str(&slice(caps[0]));
                index += 2;
            }
            '`' => {
                out.extend(&input[..whole_start]);
                index += 2;
            }
            '\'' => {
                out.extend(&input[whole_end..]);
                index += 2;
            }
            '<' => {
                let mut name = String::new();
                let mut cursor = index + 2;
                while cursor < chars.len() && chars[cursor] != '>' {
                    name.push(chars[cursor]);
                    cursor += 1;
                }
                if cursor >= chars.len() {
                    out.push('$');
                    index += 1;
                    continue;
                }
                if let Some(group) = data.regex.names.get(&name) {
                    out.push_str(&slice(caps.get(*group).copied().flatten()));
                }
                index = cursor + 1;
            }
            c if c.is_ascii_digit() => {
                // Prefer the two-digit group when it exists, as specified.
                let mut group = c.to_digit(10).unwrap_or(0) as usize;
                let mut width = 2;
                if index + 2 < chars.len()
                    && let Some(second) = chars[index + 2].to_digit(10)
                {
                    let two = group * 10 + second as usize;
                    if two <= data.regex.group_count && two > 0 {
                        group = two;
                        width = 3;
                    }
                }
                if group > 0 && group <= data.regex.group_count {
                    out.push_str(&slice(caps.get(group).copied().flatten()));
                    index += width;
                } else {
                    out.push('$');
                    index += 1;
                }
            }
            _ => {
                out.push('$');
                index += 1;
            }
        }
        if out.len() > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
    }
    Ok(out)
}

/// Shared implementation of `replace` and `replaceAll`.
pub fn replace_with_pattern(
    interp: &mut Interpreter,
    input: &[char],
    data: &Rc<RegExpData>,
    replacement: &Value,
    all: bool,
) -> Result<Value, VmErr> {
    let matches = if all || data.regex.global {
        all_matches(data, input)?
    } else {
        data.regex
            .find_at(input, 0)
            .map_err(|e| VmErr::Msg(e.to_string()))?
            .into_iter()
            .collect()
    };
    let callable = matches!(
        replacement,
        Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
    );
    let template = if callable {
        String::new()
    } else {
        interp.vs(replacement)?
    };

    let mut out = String::new();
    let mut cursor = 0usize;
    for caps in &matches {
        let Some((start, end)) = caps[0] else {
            continue;
        };
        out.extend(&input[cursor..start]);
        let piece = if callable {
            // The callback receives (match, ...groups, index, input).
            let mut args: Vec<Value> = caps
                .iter()
                .map(|range| match range {
                    Some((s, e)) => Value::String(input[*s..*e].iter().collect()),
                    None => Value::Undefined,
                })
                .collect();
            args.push(Value::Number(start as f64));
            args.push(Value::String(input.iter().collect::<String>()));
            let produced = interp.call_this(replacement, Value::Undefined, args)?;
            interp.vs(&produced)?
        } else {
            expand(&template, input, caps, data)?
        };
        out.push_str(&piece);
        cursor = end;
        if out.len() > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
    }
    out.extend(&input[cursor.min(input.len())..]);
    Value::checked_string(out)
}

/// `str.split(pattern, limit)` where the separator is a regular expression.
/// Capture groups in the separator are spliced into the result.
pub fn split_with_pattern(
    input: &[char],
    data: &Rc<RegExpData>,
    limit: usize,
) -> Result<Value, VmErr> {
    let mut out: Vec<Value> = Vec::new();
    let mut cursor = 0usize;
    let mut at = 0usize;
    while at <= input.len() && out.len() < limit {
        let found = data
            .regex
            .find_at(input, at)
            .map_err(|e| VmErr::Msg(e.to_string()))?;
        let Some(caps) = found else { break };
        let (start, end) = caps[0].unwrap_or((at, at));
        // An empty match at the cursor would split into empty strings forever.
        if end == start && start == cursor {
            at = start + 1;
            continue;
        }
        out.push(Value::String(input[cursor..start].iter().collect()));
        for group in caps.iter().skip(1) {
            if out.len() >= limit {
                break;
            }
            out.push(match group {
                Some((s, e)) => Value::String(input[*s..*e].iter().collect()),
                None => Value::Undefined,
            });
        }
        cursor = end;
        at = if end == start { end + 1 } else { end };
    }
    if out.len() < limit {
        out.push(Value::String(
            input[cursor.min(input.len())..].iter().collect(),
        ));
    }
    Value::checked_array(out)
}
