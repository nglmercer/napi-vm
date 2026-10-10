//! The `RegExp` constructor, its instance methods, and the string methods that
//! take a pattern (`match`, `matchAll`, `replace`, `replaceAll`, `search`,
//! `split`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::regex::{Captures, Regex};
use crate::value::{RegExpData, Value};

#[derive(Default)]
pub(crate) struct LegacyState {
    input: crate::JsString,
    matched_input: crate::JsString,
    last_match: Option<(usize, usize)>,
    last_paren: Option<(usize, usize)>,
    left_context: Option<(usize, usize)>,
    right_context: Option<(usize, usize)>,
    captures: [Option<(usize, usize)>; 9],
    invalidated: bool,
    input_unavailable: bool,
}

fn legacy_state(interp: &Interpreter, receiver: &Value) -> Result<Rc<RefCell<LegacyState>>, VmErr> {
    let realm = interp.persistent_global.borrow();
    let constructor = realm
        .intrinsic("RegExp")
        .ok_or_else(|| VmErr::Msg("TypeError: RegExp intrinsic is unavailable".into()))?;
    if !crate::interpreter::strict_equals(receiver, &constructor) {
        return Err(VmErr::Msg(
            "TypeError: incompatible legacy RegExp receiver".into(),
        ));
    }
    realm
        .regexp_legacy_state()
        .ok_or_else(|| VmErr::Msg("TypeError: RegExp match state is unavailable".into()))
}

fn legacy_get(interp: &Interpreter, receiver: &Value, name: &str) -> Result<Value, VmErr> {
    let state = legacy_state(interp, receiver)?;
    let state = state.borrow();
    if name == "input" && !state.input_unavailable {
        return Ok(Value::String(state.input.clone()));
    }
    if state.invalidated {
        return Err(VmErr::Msg(
            "TypeError: legacy RegExp match state is invalidated".into(),
        ));
    }
    let range = match name {
        "lastMatch" => state.last_match,
        "lastParen" => state.last_paren,
        "leftContext" => state.left_context,
        "rightContext" => state.right_context,
        _ => state.captures[name.as_bytes()[1] as usize - b'1' as usize],
    };
    Ok(Value::String(
        range
            .map(|(start, end)| state.matched_input.slice(start, end))
            .unwrap_or_default(),
    ))
}

macro_rules! legacy_getters {
    ($(($function:ident, $name:literal)),* $(,)?) => {
        $(fn $function(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
            legacy_get(interp, &this, $name)
        })*
    };
}
legacy_getters!(
    (legacy_input, "input"),
    (legacy_last_match, "lastMatch"),
    (legacy_last_paren, "lastParen"),
    (legacy_left_context, "leftContext"),
    (legacy_right_context, "rightContext"),
    (legacy_capture_1, "$1"),
    (legacy_capture_2, "$2"),
    (legacy_capture_3, "$3"),
    (legacy_capture_4, "$4"),
    (legacy_capture_5, "$5"),
    (legacy_capture_6, "$6"),
    (legacy_capture_7, "$7"),
    (legacy_capture_8, "$8"),
    (legacy_capture_9, "$9"),
);

fn legacy_set_input(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let state = legacy_state(interp, &this)?;
    let input = interp.ecmascript_to_string(args.first().unwrap_or(&Value::Undefined))?;
    let mut state = state.borrow_mut();
    state.input = input;
    state.input_unavailable = false;
    Ok(Value::Undefined)
}

/// Only the executing realm's own regular expressions update its legacy state.
/// The state contains strings, so it adds no guest object or cross-thread roots.
fn update_legacy(
    interp: &Interpreter,
    data: &RegExpData,
    input: &crate::JsString,
    caps: &Captures,
) {
    if data
        .properties
        .meta
        .borrow()
        .realm_global
        .as_ref()
        .is_some_and(|realm| !Rc::ptr_eq(realm, &interp.persistent_global))
    {
        return;
    }
    let Some(state) = interp.persistent_global.borrow().regexp_legacy_state() else {
        return;
    };
    let mut state = state.borrow_mut();
    if !data.legacy_enabled.get() {
        *state = LegacyState {
            invalidated: true,
            input_unavailable: true,
            ..LegacyState::default()
        };
        return;
    }
    state.invalidated = false;
    state.input_unavailable = false;
    let (start, end) = caps[0].unwrap_or((0, 0));
    state.input = input.clone();
    state.matched_input = input.clone();
    state.last_match = caps[0];
    state.last_paren = caps.last().copied().flatten().filter(|_| caps.len() > 1);
    state.left_context = Some((0, start));
    state.right_context = Some((end, input.len()));
    for (index, value) in state.captures.iter_mut().enumerate() {
        *value = caps.get(index + 1).copied().flatten();
    }
}

pub(super) fn install(e: &mut Environment) {
    if let Some(namespace) = e.get("RegExp") {
        e.regexp_legacy = Some(Rc::new(RefCell::new(LegacyState::default())));
        super::make_callable(&namespace, regexp_construct, None);
        for (names, getter) in [
            (&["input", "$_"][..], legacy_input as super::NativeFn),
            (&["lastMatch", "$&"][..], legacy_last_match),
            (&["lastParen", "$+"][..], legacy_last_paren),
            (&["leftContext", "$`"][..], legacy_left_context),
            (&["rightContext", "$'"][..], legacy_right_context),
            (&["$1"][..], legacy_capture_1),
            (&["$2"][..], legacy_capture_2),
            (&["$3"][..], legacy_capture_3),
            (&["$4"][..], legacy_capture_4),
            (&["$5"][..], legacy_capture_5),
            (&["$6"][..], legacy_capture_6),
            (&["$7"][..], legacy_capture_7),
            (&["$8"][..], legacy_capture_8),
            (&["$9"][..], legacy_capture_9),
        ] {
            for name in names {
                let mut descriptor = vec![
                    (
                        "get".into(),
                        super::native_method(
                            &format!("get {name}"),
                            0,
                            getter,
                            e.get("Function").and_then(|f| f.get_prop("prototype")),
                        ),
                    ),
                    ("configurable".into(), Value::Bool(true)),
                ];
                if names[0] == "input" {
                    descriptor.push((
                        "set".into(),
                        super::native_method(
                            &format!("set {name}"),
                            1,
                            legacy_set_input,
                            e.get("Function").and_then(|f| f.get_prop("prototype")),
                        ),
                    ));
                }
                super::object::define_property(
                    &namespace,
                    name,
                    &Value::descriptor_record(descriptor),
                )
                .expect("RegExp legacy accessor");
            }
        }

        let object_prototype = e
            .get("Object")
            .and_then(|object| object.get_prop("prototype"));
        let prototype = Value::object_with_proto(
            vec![
                ("constructor".into(), namespace.clone()),
                ("exec".into(), super::nf("exec", regexp_exec)),
                ("compile".into(), super::nf("compile", regexp_compile)),
                ("test".into(), super::nf("test", regexp_test)),
                ("toString".into(), super::nf("toString", regexp_to_string)),
            ],
            object_prototype.map(Rc::new),
        );
        if let Value::Object { props } = &prototype {
            let mut metadata = props.meta.borrow_mut();
            for name in ["constructor", "exec", "compile", "test", "toString"] {
                metadata.set_attrs(
                    name,
                    crate::value::PropAttrs {
                        enumerable: false,
                        ..crate::value::PropAttrs::default()
                    },
                );
            }
        }
        for (name, getter) in [
            ("source", regexp_source as super::NativeFn),
            ("global", regexp_global),
            ("ignoreCase", regexp_ignore_case),
            ("multiline", regexp_multiline),
            ("dotAll", regexp_dot_all),
            ("sticky", regexp_sticky),
            ("unicode", regexp_unicode),
            ("unicodeSets", regexp_unicode_sets),
            ("hasIndices", regexp_has_indices),
        ] {
            super::object::define_property(
                &prototype,
                name,
                &Value::descriptor_record(vec![
                    (
                        "get".into(),
                        super::native_method(
                            &format!("get {name}"),
                            0,
                            getter,
                            e.get("Function").and_then(|f| f.get_prop("prototype")),
                        ),
                    ),
                    ("configurable".into(), Value::Bool(true)),
                ]),
            )
            .expect("RegExp intrinsic getter");
        }
        super::set_builtin_constructor_prototype(e, &namespace, prototype);
    }
}

fn regexp_attribute(interp: &mut Interpreter, this: Value, name: &str) -> Result<Value, VmErr> {
    if let Value::RegExp(data) = &this {
        return Ok(match name {
            "hasIndices" => Value::Bool(data.regex.borrow().flags.contains('d')),
            "unicodeSets" => Value::Bool(data.regex.borrow().flags.contains('v')),
            _ => regexp_member(data, name).expect("RegExp attribute"),
        });
    }
    let constructor = interp
        .persistent_global
        .borrow()
        .intrinsic("RegExp")
        .ok_or_else(|| VmErr::Msg("TypeError: RegExp intrinsic is unavailable".into()))?;
    let prototype = interp.member(&constructor, "prototype")?;
    if crate::interpreter::strict_equals(&this, &prototype) {
        return Ok(if name == "source" {
            Value::String("(?:)".into())
        } else {
            Value::Undefined
        });
    }
    Err(VmErr::Msg("TypeError: incompatible RegExp receiver".into()))
}

macro_rules! regexp_attribute_getters {
    ($(($function:ident, $name:literal)),* $(,)?) => {
        $(fn $function(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
            regexp_attribute(interp, this, $name)
        })*
    };
}
regexp_attribute_getters!(
    (regexp_source, "source"),
    (regexp_global, "global"),
    (regexp_ignore_case, "ignoreCase"),
    (regexp_multiline, "multiline"),
    (regexp_dot_all, "dotAll"),
    (regexp_sticky, "sticky"),
    (regexp_unicode, "unicode"),
    (regexp_unicode_sets, "unicodeSets"),
    (regexp_has_indices, "hasIndices"),
);

fn type_err(message: String) -> VmErr {
    VmErr::Msg(format!("SyntaxError: {}", message))
}

pub(crate) fn compile(source: impl Into<crate::JsString>, flags: &str) -> Result<Value, VmErr> {
    let regex = Regex::new(source, flags).map_err(type_err)?;
    Ok(Value::RegExp(Rc::new(RegExpData {
        properties: Value::instance_properties(),
        regex: RefCell::new(regex),
        legacy_enabled: Cell::new(true),
        last_index: Cell::new(0),
    })))
}

/// `RegExp(pattern, flags)`. A `RegExp` argument is re-compiled, taking its
/// own flags unless new ones are given.
fn regexp_construct(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let (source, own_flags) = match a.first() {
        Some(Value::RegExp(data)) => (
            data.regex.borrow().source.clone(),
            data.regex.borrow().flags.clone(),
        ),
        Some(Value::Undefined) | None => (crate::JsString::default(), String::new()),
        Some(other) => (interp.to_js_string(other)?, String::new()),
    };
    let flags = match a.get(1) {
        Some(Value::Undefined) | None => own_flags,
        Some(other) => interp.vs(other)?,
    };
    // `RegExp(/(?:)/)` round-trips through the canonical empty source.
    let source = if source == "(?:)" {
        crate::JsString::default()
    } else {
        source
    };
    compile(source, &flags)
}

/// Properties and methods readable on a regular expression.
pub fn regexp_member(data: &Rc<RegExpData>, key: &str) -> Option<Value> {
    let regex = data.regex.borrow();
    Some(match key {
        "source" => Value::String(regex.source.clone()),
        "flags" => Value::String((regex.flags.clone()).into()),
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
        return Ok(Value::String(("/(?:)/".to_string()).into()));
    };
    Ok(Value::String(
        crate::JsString::from("/")
            .concat(&data.regex.borrow().source)
            .concat(&crate::JsString::from("/"))
            .concat(&crate::JsString::from(&data.regex.borrow().flags)),
    ))
}

/// Build the array `exec` returns: the whole match, then each group, with
/// `index`, `input` and `groups` as named properties.
fn match_result(
    data: &Rc<RegExpData>,
    input: &crate::JsString,
    caps: &Captures,
) -> Result<Value, VmErr> {
    let slice = |range: Option<(usize, usize)>| match range {
        Some((start, end)) => Value::String(crate::JsString::from_units(
            input.units()[start..end].to_vec(),
        )),
        None => Value::Undefined,
    };
    let items: Vec<Value> = caps.iter().map(|c| slice(*c)).collect();
    let result = Value::checked_array(items)?;
    let start = caps[0].map(|(s, _)| s).unwrap_or(0);
    result.set_prop("index".to_string(), Value::Number(start as f64))?;
    result.set_prop("input".to_string(), Value::String(input.clone()))?;
    let groups = if data.regex.borrow().names.is_empty() {
        Value::Undefined
    } else {
        let mut named: Vec<(String, Value)> = data
            .regex
            .borrow()
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
fn exec(
    interp: &Interpreter,
    data: &Rc<RegExpData>,
    input: &crate::JsString,
) -> Result<Option<Captures>, VmErr> {
    let stateful = data.regex.borrow().global || data.regex.borrow().sticky;
    let start = if stateful { data.last_index.get() } else { 0 };
    if start > input.len() {
        data.last_index.set(0);
        return Ok(None);
    }
    let found = data
        .regex
        .borrow()
        .find_at(input.units(), start)
        .map_err(|e| VmErr::Msg(e.to_string()))?;
    match &found {
        Some(caps) => {
            update_legacy(interp, data, input, caps);
            if stateful {
                let end = caps[0].map(|(_, e)| e).unwrap_or(start);
                // An empty match must still advance, or a `g` loop never ends.
                data.last_index.set(if end == start {
                    advance(input, end, data.regex.borrow().unicode)
                } else {
                    end
                });
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
    match exec(interp, &data, &subject)? {
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
    Ok(Value::Bool(exec(interp, &data, &subject)?.is_some()))
}

fn subject_chars(interp: &Interpreter, value: Option<&Value>) -> Result<crate::JsString, VmErr> {
    interp.to_js_string(value.unwrap_or(&Value::Undefined))
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
fn all_matches(
    interp: &Interpreter,
    data: &Rc<RegExpData>,
    input: &crate::JsString,
) -> Result<Vec<Captures>, VmErr> {
    let mut out = Vec::new();
    let mut at = 0usize;
    loop {
        let found = data
            .regex
            .borrow()
            .find_at(input.units(), at)
            .map_err(|e| VmErr::Msg(e.to_string()))?;
        let Some(caps) = found else { break };
        update_legacy(interp, data, input, &caps);
        let (start, end) = caps[0].unwrap_or((at, at));
        out.push(caps);
        if !data.regex.borrow().global {
            break;
        }
        // An empty match advances by one so the scan terminates.
        at = if end == start {
            advance(input, end, data.regex.borrow().unicode)
        } else {
            end
        };
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
    let input = super::str_this(interp, &this)?;
    let Some(data) = as_pattern(a.first()) else {
        let source = if matches!(a.first(), None | Some(Value::Undefined)) {
            crate::JsString::default()
        } else {
            interp.to_js_string(&a[0])?
        };
        let compiled = compile(source, "")?;
        return string_match(interp, this, vec![compiled]);
    };
    if data.regex.borrow().global {
        data.last_index.set(0);
        let matches = all_matches(interp, &data, &input)?;
        if matches.is_empty() {
            return Ok(Value::Null);
        }
        let items = matches
            .iter()
            .map(|caps| match caps[0] {
                Some((start, end)) => Value::String(crate::JsString::from_units(
                    input.units()[start..end].to_vec(),
                )),
                None => Value::Undefined,
            })
            .collect();
        return Value::checked_array(items);
    }
    match exec(interp, &data, &input)? {
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
    let input = super::str_this(interp, &this)?;
    let Some(data) = as_pattern(a.first()) else {
        return Err(VmErr::Msg(
            "TypeError: matchAll requires a global RegExp".to_string(),
        ));
    };
    if !data.regex.borrow().global {
        return Err(VmErr::Msg(
            "TypeError: matchAll must be called with a global RegExp".to_string(),
        ));
    }
    let matches = all_matches(interp, &data, &input)?;
    let items = matches
        .iter()
        .map(|caps| match_result(&data, &input, caps))
        .collect::<Result<Vec<_>, _>>()?;
    Value::checked_array(items)
}

/// `str.search(pattern)`: the index of the first match, or `-1`.
pub fn string_search(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let input = super::str_this(interp, &this)?;
    let data = match as_pattern(a.first()) {
        Some(data) => data,
        None => {
            let source = if matches!(a.first(), None | Some(Value::Undefined)) {
                crate::JsString::default()
            } else {
                interp.to_js_string(&a[0])?
            };
            let compiled = compile(source, "")?;
            compiled.as_regexp().expect("compile returns a RegExp")
        }
    };
    let found = data
        .regex
        .borrow()
        .find_at(input.units(), 0)
        .map_err(|e| VmErr::Msg(e.to_string()))?;
    if let Some(caps) = &found {
        update_legacy(interp, &data, &input, caps);
    }
    Ok(Value::Number(match found {
        Some(caps) => caps[0].map(|(start, _)| start as f64).unwrap_or(-1.0),
        None => -1.0,
    }))
}

/// Expand `$&`, `$1`, `$<name>` and friends in a replacement template.
fn expand(
    template: &crate::JsString,
    input: &crate::JsString,
    caps: &Captures,
    data: &Rc<RegExpData>,
) -> Result<crate::JsString, VmErr> {
    let slice = |range: Option<(usize, usize)>| -> crate::JsString {
        match range {
            Some((start, end)) => input.slice(start, end),
            None => crate::JsString::default(),
        }
    };
    let chars = template.units();
    let mut out = crate::JsString::default();
    let mut index = 0;
    let (whole_start, whole_end) = caps[0].unwrap_or((0, 0));
    while index < chars.len() {
        if chars[index] != 36 || index + 1 >= chars.len() {
            out.push_str(crate::JsString::from_units(vec![chars[index]]));
            index += 1;
            continue;
        }
        match chars[index + 1] {
            36 => {
                out.push('$');
                index += 2;
            }
            38 => {
                out.push_str(slice(caps[0]));
                index += 2;
            }
            96 => {
                out.push_str(input.slice(0, whole_start));
                index += 2;
            }
            39 => {
                out.push_str(input.slice(whole_end, input.len()));
                index += 2;
            }
            60 => {
                let mut name = String::new();
                let mut cursor = index + 2;
                while cursor < chars.len() && chars[cursor] != 62 {
                    name.push(char::from_u32(chars[cursor] as u32).unwrap_or('\u{FFFD}'));
                    cursor += 1;
                }
                if cursor >= chars.len() {
                    out.push('$');
                    index += 1;
                    continue;
                }
                if let Some(group) = data.regex.borrow().names.get(&name) {
                    out.push_str(slice(caps.get(*group).copied().flatten()));
                }
                index = cursor + 1;
            }
            c if (48..=57).contains(&c) => {
                // Prefer the two-digit group when it exists, as specified.
                let mut group = (c - 48) as u32 as usize;
                let mut width = 2;
                if index + 2 < chars.len()
                    && let Some(second) =
                        char::from_u32(chars[index + 2] as u32).and_then(|c| c.to_digit(10))
                {
                    let two = group * 10 + second as usize;
                    if two <= data.regex.borrow().group_count && two > 0 {
                        group = two;
                        width = 3;
                    }
                }
                if group > 0 && group <= data.regex.borrow().group_count {
                    out.push_str(slice(caps.get(group).copied().flatten()));
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
    input: &crate::JsString,
    data: &Rc<RegExpData>,
    replacement: &Value,
    all: bool,
) -> Result<Value, VmErr> {
    let matches = if all || data.regex.borrow().global {
        all_matches(interp, data, input)?
    } else {
        data.regex
            .borrow()
            .find_at(input.units(), 0)
            .map_err(|e| VmErr::Msg(e.to_string()))?
            .into_iter()
            .collect()
    };
    if !(all || data.regex.borrow().global)
        && let Some(caps) = matches.first()
    {
        update_legacy(interp, data, input, caps);
    }
    let callable = matches!(
        replacement,
        Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
    );
    let template = if callable {
        crate::JsString::default()
    } else {
        interp.to_js_string(replacement)?
    };

    let mut out = crate::JsString::default();
    let mut cursor = 0usize;
    for caps in &matches {
        let Some((start, end)) = caps[0] else {
            continue;
        };
        out.push_str(input.slice(cursor, start));
        let piece = if callable {
            // The callback receives (match, ...groups, index, input).
            let mut args: Vec<Value> = caps
                .iter()
                .map(|range| match range {
                    Some((s, e)) => {
                        Value::String(crate::JsString::from_units(input.units()[*s..*e].to_vec()))
                    }
                    None => Value::Undefined,
                })
                .collect();
            args.push(Value::Number(start as f64));
            args.push(Value::String(input.clone()));
            let produced = interp.call_this(replacement, Value::Undefined, args)?;
            interp.to_js_string(&produced)?
        } else {
            expand(&template, input, caps, data)?
        };
        out.push_str(&piece);
        cursor = end;
        if out.len() > crate::value::MAX_STRING_LEN {
            return Err(crate::value::limit_err("Maximum string length exceeded"));
        }
    }
    out.push_str(input.slice(cursor.min(input.len()), input.len()));
    Value::checked_string(out)
}

/// `str.split(pattern, limit)` where the separator is a regular expression.
/// Capture groups in the separator are spliced into the result.
pub fn split_with_pattern(
    interp: &Interpreter,
    input: &crate::JsString,
    data: &Rc<RegExpData>,
    limit: usize,
) -> Result<Value, VmErr> {
    let mut out: Vec<Value> = Vec::new();
    let mut cursor = 0usize;
    let mut at = 0usize;
    while at <= input.len() && out.len() < limit {
        let found = data
            .regex
            .borrow()
            .find_at(input.units(), at)
            .map_err(|e| VmErr::Msg(e.to_string()))?;
        let Some(caps) = found else { break };
        update_legacy(interp, data, input, &caps);
        let (start, end) = caps[0].unwrap_or((at, at));
        // An empty match at the cursor would split into empty strings forever.
        if end == start && start == cursor {
            at = advance(input, start, data.regex.borrow().unicode);
            continue;
        }
        out.push(Value::String(crate::JsString::from_units(
            input.units()[cursor..start].to_vec(),
        )));
        for group in caps.iter().skip(1) {
            if out.len() >= limit {
                break;
            }
            out.push(match group {
                Some((s, e)) => {
                    Value::String(crate::JsString::from_units(input.units()[*s..*e].to_vec()))
                }
                None => Value::Undefined,
            });
        }
        cursor = end;
        at = if end == start {
            advance(input, end, data.regex.borrow().unicode)
        } else {
            end
        };
    }
    if out.len() < limit {
        out.push(Value::String(crate::JsString::from_units(
            input.units()[cursor.min(input.len())..].to_vec(),
        )));
    }
    Value::checked_array(out)
}

fn advance(input: &crate::JsString, pos: usize, unicode: bool) -> usize {
    pos + if unicode && input.code_point_at(pos).is_some_and(|cp| cp > 0xFFFF) {
        2
    } else {
        1
    }
}

fn regexp_compile(interp: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let Value::RegExp(data) = &this else {
        return Err(VmErr::Msg(
            "TypeError: RegExp.compile requires a RegExp receiver".into(),
        ));
    };
    let foreign_realm = data
        .properties
        .meta
        .borrow()
        .realm_global
        .as_ref()
        .is_some_and(|realm| !Rc::ptr_eq(realm, &interp.persistent_global));
    if !data.legacy_enabled.get() || foreign_realm {
        return Err(VmErr::Msg(
            "TypeError: Legacy RegExp methods require an enabled same-realm receiver".into(),
        ));
    }
    if matches!(args.first(), Some(Value::RegExp(_)))
        && args
            .get(1)
            .is_some_and(|value| !matches!(value, Value::Undefined))
    {
        return Err(VmErr::Msg(
            "TypeError: Flags must be undefined for a RegExp pattern".into(),
        ));
    }
    let Value::RegExp(ref replacement) = regexp_construct(interp, Value::Undefined, args)? else {
        unreachable!()
    };
    let replacement = replacement.regex.borrow();
    let compiled = Regex::new(replacement.source.clone(), &replacement.flags).map_err(type_err)?;
    *data.regex.borrow_mut() = compiled;
    data.last_index.set(0);
    Ok(this)
}
