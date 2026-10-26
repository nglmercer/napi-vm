//! The `RegExp` constructor, its instance methods, and the string methods that
//! take a pattern (`match`, `matchAll`, `replace`, `replaceAll`, `search`,
//! `split`).

mod protocol;
pub(crate) use protocol::{IteratorSlots as RegExpIteratorSlots, install_iterator};

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
        super::install_constructor_metadata(&namespace, "RegExp", 2);
        e.regexp_legacy = Some(Rc::new(RefCell::new(LegacyState::default())));
        super::make_callable(&namespace, regexp_call, Some(regexp_construct));
        let fp = e.get("Function").and_then(|f| f.get_prop("prototype"));
        namespace
            .set_prop(
                "escape".into(),
                super::native_method("escape", 1, regexp_escape, fp),
            )
            .unwrap();
        namespace
            .property_cell()
            .unwrap()
            .meta
            .borrow_mut()
            .set_attrs(
                "escape",
                crate::value::PropAttrs {
                    enumerable: false,
                    ..Default::default()
                },
            );

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
                ("test".into(), super::nf("test", protocol::test)),
                (
                    "toString".into(),
                    super::nf("toString", protocol::to_string),
                ),
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
            ("flags", protocol::flags as super::NativeFn),
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
        for (name, function) in [
            ("match", protocol::matches as super::NativeFn),
            ("search", protocol::search),
            ("matchAll", protocol::match_all),
            ("split", protocol::split),
            ("replace", protocol::replace),
        ] {
            if let Some(Value::Symbol(ref symbol)) = super::well_known(name) {
                let key = crate::interpreter::symbol_slot_key(symbol);
                prototype
                    .set_prop(
                        key.clone(),
                        super::native_method(
                            &format!("[Symbol.{name}]"),
                            if name == "split" || name == "replace" {
                                2
                            } else {
                                1
                            },
                            function,
                            e.get("Function").and_then(|f| f.get_prop("prototype")),
                        ),
                    )
                    .unwrap();
                let cell = prototype.property_cell().unwrap();
                let mut meta = cell.meta.borrow_mut();
                meta.set_symbol_key(&key, symbol.clone());
                meta.set_attrs(
                    &key,
                    crate::value::PropAttrs {
                        enumerable: false,
                        ..Default::default()
                    },
                );
            }
        }
        if let Some(Value::Symbol(ref symbol)) = super::well_known("species") {
            let key = crate::interpreter::symbol_slot_key(symbol);
            let getter = super::native_method(
                "get [Symbol.species]",
                0,
                regexp_species,
                e.get("Function").and_then(|f| f.get_prop("prototype")),
            );
            super::object::define_property(
                &namespace,
                &key,
                &Value::descriptor_record(vec![
                    ("get".into(), getter),
                    ("configurable".into(), Value::Bool(true)),
                ]),
            )
            .unwrap();
            namespace
                .property_cell()
                .unwrap()
                .meta
                .borrow_mut()
                .set_symbol_key(&key, symbol.clone());
        }
        super::install_method_metadata(
            e,
            &prototype,
            &[("exec", 1), ("test", 1), ("compile", 2), ("toString", 0)],
        );
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
fn is_regexp(i: &mut Interpreter, pattern: &Value) -> Result<bool, VmErr> {
    if !crate::interpreter::call::is_js_object(pattern) {
        return Ok(false);
    }
    let key = super::well_known("match").unwrap();
    let matcher = i.get_prop_value(pattern, &key)?;
    Ok(if matches!(matcher, Value::Undefined) {
        pattern.as_regexp().is_some()
    } else {
        matcher.is_truthy()
    })
}
fn regexp_call(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let pattern = args.first().cloned().unwrap_or(Value::Undefined);
    let regex = is_regexp(i, &pattern)?;
    if regex && matches!(args.get(1), None | Some(Value::Undefined)) {
        let constructor = i.member(&pattern, "constructor")?;
        let intrinsic = i.persistent_global.borrow().intrinsic("RegExp").unwrap();
        if crate::interpreter::strict_equals(&constructor, &intrinsic) {
            return Ok(pattern);
        }
    }
    initialize_regexp(i, args, regex, false)
}
fn regexp_construct(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let regex = is_regexp(i, args.first().unwrap_or(&Value::Undefined))?;
    initialize_regexp(i, args, regex, true)
}
fn initialize_regexp(
    i: &mut Interpreter,
    args: Vec<Value>,
    regex: bool,
    constructing: bool,
) -> Result<Value, VmErr> {
    let pattern = args.first().cloned().unwrap_or(Value::Undefined);
    let mut flags = args.get(1).cloned().unwrap_or(Value::Undefined);
    let source = if let Value::RegExp(data) = &pattern {
        if matches!(flags, Value::Undefined) {
            flags = Value::String(data.regex.borrow().flags.clone().into());
        }
        Value::String(data.regex.borrow().source.clone())
    } else if regex {
        let source = i.member(&pattern, "source")?;
        if matches!(flags, Value::Undefined) {
            flags = i.member(&pattern, "flags")?;
        }
        source
    } else {
        pattern
    };
    // RegExpAlloc follows extraction of pattern/flags, but precedes their
    // string conversion in RegExpInitialize.
    let intrinsic = i.persistent_global.borrow().intrinsic("RegExp").unwrap();
    let new_target = if constructing {
        i.new_target_stack
            .last()
            .cloned()
            .unwrap_or_else(|| intrinsic.clone())
    } else {
        intrinsic.clone()
    };
    let prototype = i.constructor_prototype(&new_target, "RegExp")?;
    let source = if matches!(source, Value::Undefined) {
        crate::JsString::default()
    } else {
        i.ecmascript_to_string(&source)?
    };
    let flags = if matches!(flags, Value::Undefined) {
        String::new()
    } else {
        i.ecmascript_to_string(&flags)?.to_string()
    };
    let result = compile(source, &flags)?;
    if let Value::RegExp(data) = &result {
        data.properties.set_proto(prototype);
        data.legacy_enabled
            .set(crate::interpreter::strict_equals(&new_target, &intrinsic));
    }
    Ok(result)
}

/// Properties and methods readable on a regular expression.
pub fn regexp_member(data: &Rc<RegExpData>, key: &str) -> Option<Value> {
    let regex = data.regex.borrow();
    Some(match key {
        "source" => Value::String(escaped_source(&regex.source, regex.flags.contains('v'))),
        "flags" => Value::String((regex.flags.clone()).into()),
        "global" => Value::Bool(regex.global),
        "ignoreCase" => Value::Bool(regex.ignore_case),
        "multiline" => Value::Bool(regex.multiline),
        "dotAll" => Value::Bool(regex.dot_all),
        "sticky" => Value::Bool(regex.sticky),
        "unicode" => Value::Bool(regex.flags.contains('u')),
        "lastIndex" => Value::Number(data.last_index.get() as f64),
        _ => return None,
    })
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
            .map(|(name, indices)| {
                (
                    name.clone(),
                    slice(
                        indices
                            .iter()
                            .find_map(|index| caps.get(*index).copied().flatten()),
                    ),
                )
            })
            .collect();
        named.sort_by_key(|(name, _)| data.regex.borrow().names[name][0]);
        Value::object_with_proto(named, None)
    };
    result.set_prop("groups".to_string(), groups)?;
    if data.regex.borrow().flags.contains('d') {
        let indices = Value::checked_array(
            caps.iter()
                .map(|range| match range {
                    Some((start, end)) => Value::array(vec![
                        Value::Number(*start as f64),
                        Value::Number(*end as f64),
                    ]),
                    None => Value::Undefined,
                })
                .collect(),
        )?;
        let groups = if data.regex.borrow().names.is_empty() {
            Value::Undefined
        } else {
            let mut names: Vec<_> = data
                .regex
                .borrow()
                .names
                .iter()
                .map(|(name, indices)| (name.clone(), indices.clone()))
                .collect();
            names.sort_by_key(|(_, indices)| indices[0]);
            Value::object_with_proto(
                names
                    .into_iter()
                    .map(|(name, groups)| {
                        let group = groups
                            .iter()
                            .find(|index| caps.get(**index).is_some_and(Option::is_some));
                        (
                            name,
                            group
                                .and_then(|index| indices.get_prop(&index.to_string()))
                                .unwrap_or(Value::Undefined),
                        )
                    })
                    .collect(),
                None,
            )
        };
        indices.set_prop("groups".into(), groups)?;
        result.set_prop("indices".into(), indices)?;
    }

    Ok(result)
}

/// Run one search, honouring and updating `lastIndex` for a `g`/`y` pattern.
fn exec(
    interp: &mut Interpreter,
    data: &Rc<RegExpData>,
    input: &crate::JsString,
) -> Result<Option<Captures>, VmErr> {
    let receiver = Value::RegExp(data.clone());
    let last_index = interp.member(&receiver, "lastIndex")?;
    let last_index = interp.ecmascript_to_number(&last_index)?;
    let last_index = if last_index.is_nan() || last_index < 0. {
        0
    } else {
        last_index.trunc().min(9_007_199_254_740_991.) as usize
    };
    let stateful = data.regex.borrow().global || data.regex.borrow().sticky;
    let start = if stateful { last_index } else { 0 };
    if start > input.len() {
        interp.set_member_str_or_throw(&receiver, "lastIndex", Value::Number(0.))?;
        return Ok(None);
    }
    let found = data
        .regex
        .borrow()
        .find_at(input.units(), start)
        .map_err(VmErr::Msg)?;
    match &found {
        Some(caps) => {
            update_legacy(interp, data, input, caps);
            if stateful {
                let end = caps[0].map(|(_, e)| e).unwrap_or(start);
                interp.set_member_str_or_throw(
                    &receiver,
                    "lastIndex",
                    Value::Number(end as f64),
                )?;
            }
        }
        None if stateful => {
            interp.set_member_str_or_throw(&receiver, "lastIndex", Value::Number(0.))?;
        }
        None => {}
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

fn subject_chars(
    interp: &mut Interpreter,
    value: Option<&Value>,
) -> Result<crate::JsString, VmErr> {
    interp.ecmascript_to_string(value.unwrap_or(&Value::Undefined))
}

// ---------------------------------------------------------------------------
// String methods that take a pattern.
// ---------------------------------------------------------------------------

/// Coerce the pattern argument of a string method: a `RegExp` is used as-is, a
/// string is a literal to find (not a pattern to compile).
///
/// `str.match(pattern)`.
///
/// A global pattern returns every matched substring; a non-global one returns
/// the full `exec` result, groups and all.
pub fn string_match(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    string_protocol(interp, this, a, "match")
}
fn string_protocol(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
    name: &str,
) -> Result<Value, VmErr> {
    if matches!(this, Value::Undefined | Value::Null) {
        return Err(VmErr::Msg(
            "TypeError: Cannot convert undefined or null to object".into(),
        ));
    }
    let pattern = args.first().cloned().unwrap_or(Value::Undefined);
    let symbol = super::well_known(name).unwrap();
    if crate::interpreter::call::is_js_object(&pattern) {
        let method = i.get_prop_value(&pattern, &symbol)?;
        if !matches!(method, Value::Undefined | Value::Null) {
            return i.call_this(&method, pattern, vec![this]);
        }
    }
    let input = i.ecmascript_to_string(&this)?;
    let source = if matches!(pattern, Value::Undefined) {
        crate::JsString::default()
    } else {
        i.ecmascript_to_string(&pattern)?
    };
    let regex = compile(source, "")?;
    let method = i.get_prop_value(&regex, &symbol)?;
    i.call_this(&method, regex, vec![Value::String(input)])
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
    if matches!(this, Value::Undefined | Value::Null) {
        return Err(VmErr::Msg(
            "TypeError: Cannot convert undefined or null to object".into(),
        ));
    }
    let pattern = a.first().cloned().unwrap_or(Value::Undefined);
    let symbol = super::well_known("matchAll").unwrap();
    if crate::interpreter::call::is_js_object(&pattern) {
        let match_symbol = super::well_known("match").unwrap();
        let matcher = interp.get_prop_value(&pattern, &match_symbol)?;
        let is_regexp = if matches!(matcher, Value::Undefined) {
            pattern.as_regexp().is_some()
        } else {
            matcher.is_truthy()
        };
        if is_regexp {
            let flags = interp.member(&pattern, "flags")?;
            if matches!(flags, Value::Undefined | Value::Null) {
                return Err(VmErr::Msg(
                    "TypeError: RegExp flags cannot be nullish".into(),
                ));
            }
            if !interp.ecmascript_to_string(&flags)?.contains('g') {
                return Err(VmErr::Msg(
                    "TypeError: matchAll requires a global RegExp".into(),
                ));
            }
        }
        let method = interp.get_prop_value(&pattern, &symbol)?;
        if !matches!(method, Value::Undefined | Value::Null) {
            return interp.call_this(&method, pattern, vec![this]);
        }
    }
    let input = interp.ecmascript_to_string(&this)?;
    let source = if matches!(pattern, Value::Undefined) {
        crate::JsString::default()
    } else {
        interp.ecmascript_to_string(&pattern)?
    };
    let regex = compile(source, "g")?;
    let method = interp.get_prop_value(&regex, &symbol)?;
    interp.call_this(&method, regex, vec![Value::String(input)])
}

/// `str.search(pattern)`: the index of the first match, or `-1`.
pub fn string_search(interp: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    string_protocol(interp, this, a, "search")
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
    let regex = is_regexp(interp, args.first().unwrap_or(&Value::Undefined))?;
    let Value::RegExp(ref replacement) = initialize_regexp(interp, args, regex, false)? else {
        unreachable!()
    };
    let replacement = replacement.regex.borrow();
    let compiled = Regex::new(replacement.source.clone(), &replacement.flags).map_err(type_err)?;
    *data.regex.borrow_mut() = compiled;
    interp.set_member_str_or_throw(&this, "lastIndex", Value::Number(0.))?;
    Ok(this)
}

fn regexp_species(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(this)
}

fn escaped_source(source: &crate::JsString, unicode_sets: bool) -> crate::JsString {
    if source.is_empty() {
        return "(?:)".into();
    }
    let mut result = Vec::new();
    let mut backslashes = 0;
    let mut class_depth = 0usize;
    for &unit in source.units() {
        let escaped = backslashes % 2 != 0;
        if matches!(unit, 10 | 13 | 0x2028 | 0x2029) && escaped {
            result.pop();
        }
        match unit {
            47 if !escaped && class_depth == 0 => result.extend([92, 47]),
            10 => result.extend([92, 110]),
            13 => result.extend([92, 114]),
            0x2028 => result.extend([92, 117, 50, 48, 50, 56]),
            0x2029 => result.extend([92, 117, 50, 48, 50, 57]),
            _ => result.push(unit),
        }
        if !escaped {
            if unit == 91 && (class_depth == 0 || unicode_sets) {
                class_depth += 1;
            } else if unit == 93 {
                class_depth = class_depth.saturating_sub(1);
            }
        }
        backslashes = if unit == 92 { backslashes + 1 } else { 0 };
    }
    crate::JsString::from_units(result)
}

fn regexp_escape(_: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let Some(Value::String(text)) = args.first() else {
        return Err(VmErr::Msg(
            "TypeError: RegExp.escape requires a string".into(),
        ));
    };
    let mut out = Vec::new();
    let units = text.units();
    let mut index = 0;
    while index < units.len() {
        let u = units[index];
        let c = char::from_u32(u as u32);
        let escaped = if index == 0 && c.is_some_and(|c| c.is_ascii_alphanumeric()) {
            Some(format!("\\x{u:02x}"))
        } else if c.is_some_and(|c| "^$\\.*+?()[]{}|/".contains(c)) {
            Some(format!("\\{}", c.unwrap()))
        } else if let Some(letter) = match u {
            9 => Some('t'),
            10 => Some('n'),
            11 => Some('v'),
            12 => Some('f'),
            13 => Some('r'),
            _ => None,
        } {
            Some(format!("\\{letter}"))
        } else if u == 32 || c.is_some_and(|c| ",-=<>#&!%:;@~'`\"".contains(c)) {
            Some(format!("\\x{u:02x}"))
        } else if c.is_some_and(|c| c.is_whitespace() && c != '\u{85}') || u == 0xfeff {
            Some(if u <= 255 {
                format!("\\x{u:02x}")
            } else {
                format!("\\u{u:04x}")
            })
        } else if (0xd800..=0xdbff).contains(&u)
            && units
                .get(index + 1)
                .is_some_and(|lo| (0xdc00..=0xdfff).contains(lo))
        {
            out.push(u);
            index += 1;
            out.push(units[index]);
            index += 1;
            continue;
        } else if (0xd800..=0xdfff).contains(&u) {
            Some(format!("\\u{u:04x}"))
        } else {
            None
        };
        if let Some(s) = escaped {
            out.extend(s.encode_utf16());
        } else {
            out.push(u);
        }
        index += 1;
    }
    Value::checked_string(crate::JsString::from_units(out))
}
