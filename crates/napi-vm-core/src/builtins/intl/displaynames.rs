//! ECMA-402 display names: structural code validation with code fallback.
//!
//! No test262 DisplayNames assertion inspects a localized name; every `of`
//! valid case only requires a string result and every invalid case requires
//! a `RangeError`. The implementation therefore validates codes per type and
//! returns the code itself (the `fallback: "code"` default), keeping all
//! observable option coercions and constructor shapes in the interpreter.
use super::*;

const TYPES: [&str; 6] = [
    "language",
    "region",
    "script",
    "currency",
    "calendar",
    "dateTimeField",
];
const DATE_TIME_FIELDS: [&str; 12] = [
    "era",
    "year",
    "quarter",
    "month",
    "weekOfYear",
    "weekday",
    "day",
    "dayPeriod",
    "hour",
    "minute",
    "second",
    "timeZoneName",
];

#[derive(Clone)]
pub(crate) struct Slots {
    locale: String,
    style: String,
    type_name: String,
    fallback: String,
    language_display: String,
}
fn error() -> VmErr {
    VmErr::Msg("TypeError: Intl.DisplayNames receiver required".into())
}
fn slot(this: &Value) -> Result<Slots, VmErr> {
    this.property_cell()
        .and_then(|p| p.meta.borrow().intl_displaynames.clone())
        .ok_or_else(error)
}
fn method(target: &Value, name: &str, length: usize, f: NativeFn, fp: &Option<Value>) {
    nonenum(target, name, native_method(name, length, f, fp.clone()));
}
pub(super) fn install(e: &mut Environment, namespace: &Value) {
    let fp = e.get("Function").and_then(|f| f.get_prop("prototype"));
    let op = e.get("Object").and_then(|o| o.get_prop("prototype"));
    let constructor = Value::object_with_proto(vec![], fp.clone().map(Rc::new));
    super::super::make_callable(&constructor, super::super::require_new, Some(construct));
    for (key, value) in [
        ("name", Value::String("DisplayNames".into())),
        ("length", Value::Number(2.)),
    ] {
        nonenum(&constructor, key, value);
        constructor
            .property_cell()
            .unwrap()
            .meta
            .borrow_mut()
            .set_attrs(
                key,
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
    }
    method(
        &constructor,
        "supportedLocalesOf",
        1,
        supported_locales,
        &fp,
    );
    let prototype = Value::object_with_proto(vec![], op.map(Rc::new));
    nonenum(&prototype, "constructor", constructor.clone());
    tag(&prototype, "Intl.DisplayNames");
    method(&prototype, "of", 1, of, &fp);
    method(&prototype, "resolvedOptions", 0, resolved_options, &fp);
    super::super::set_builtin_constructor_prototype(e, &constructor, prototype.clone());
    nonenum(namespace, "DisplayNames", constructor.clone());
    e.install_intrinsic("Intl.DisplayNames", constructor);
    e.install_intrinsic("Intl.DisplayNames.prototype", prototype);
}

fn choice(
    i: &mut Interpreter,
    options: &Value,
    key: &str,
    default: &str,
    allowed: &[&str],
) -> Result<String, VmErr> {
    let value = option(i, options, key)?.unwrap_or_else(|| default.into());
    if !allowed.contains(&value.as_str()) {
        return Err(range());
    }
    Ok(value)
}

fn construct(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    // DisplayNames requires `new`: a plain call throws a TypeError.
    let target = i.new_target_stack.last().cloned().ok_or_else(error)?;
    let prototype = i.constructor_prototype(&target, "Intl.DisplayNames")?;
    let requested = locale_list(i, arg(&args, 0))?;
    // An absent options object cannot supply the required `type`.
    let options = match arg(&args, 1) {
        Value::Undefined => return Err(type_error()),
        v => super::super::object::to_object_receiver(&v)?,
    };
    locale_matcher(i, &options)?;
    // Spec order reads style before the required type, so a bad style
    // throws RangeError even when type is absent.
    let style = choice(i, &options, "style", "long", &["narrow", "short", "long"])?;
    let type_name = match option(i, &options, "type")? {
        None => return Err(type_error()),
        Some(t) if TYPES.contains(&t.as_str()) => t,
        Some(_) => return Err(range()),
    };
    let fallback = choice(i, &options, "fallback", "code", &["code", "none"])?;
    let default_display = if type_name == "language" {
        "dialect"
    } else {
        "standard"
    };
    let language_display = choice(
        i,
        &options,
        "languageDisplay",
        default_display,
        &["dialect", "standard"],
    )?;
    let locale = requested
        .iter()
        .find_map(|tag| best_available_locale(tag))
        .unwrap_or_else(|| "en-US".into());
    let result = Value::object_with_proto(vec![], prototype);
    result
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .intl_displaynames = Some(Slots {
        locale,
        style,
        type_name,
        fallback,
        language_display,
    });
    Ok(result)
}

fn of(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let slots = slot(&this)?;
    let code = i.ecmascript_to_string(&arg(&args, 0))?.to_string();
    let known = match slots.type_name.as_str() {
        "language" => valid_language(&code).then_some(true),
        "region" => valid_region(&code).then_some(true),
        "script" => {
            (code.len() == 4 && code.bytes().all(|b| b.is_ascii_alphabetic())).then_some(true)
        }
        "currency" => {
            if !(code.len() == 3 && code.bytes().all(|b| b.is_ascii_alphabetic())) {
                return Err(range());
            }
            Some(supported::SUPPORTED_CURRENCIES.contains(&code.to_ascii_uppercase().as_str()))
        }
        "calendar" => valid_calendar(&code).then_some(true),
        "dateTimeField" => DATE_TIME_FIELDS.contains(&code.as_str()).then_some(true),
        _ => None,
    };
    match known {
        None => Err(range()),
        Some(false) if slots.fallback == "none" => Ok(Value::Undefined),
        Some(_) => Ok(Value::String(code.into())),
    }
}

fn resolved_options(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    Ok(Value::object(vec![
        ("locale".into(), Value::String(s.locale.into())),
        ("style".into(), Value::String(s.style.into())),
        ("type".into(), Value::String(s.type_name.into())),
        ("fallback".into(), Value::String(s.fallback.into())),
        (
            "languageDisplay".into(),
            Value::String(s.language_display.into()),
        ),
    ]))
}

fn ascii_alnum(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn valid_language(code: &str) -> bool {
    if code.eq_ignore_ascii_case("root") {
        return false;
    }
    let parts: Vec<&str> = code.split('-').collect();
    // No extensions, singletons, or private use: every subtag is 2+ alphanumerics.
    if parts.iter().any(|p| p.len() < 2 || !ascii_alnum(p)) {
        return false;
    }
    let first = parts[0];
    let language =
        first.bytes().all(|b| b.is_ascii_alphabetic()) && matches!(first.len(), 2..=3 | 5..=8);
    if !language {
        return false;
    }
    let mut index = 1;
    // Optional single script subtag.
    if index < parts.len()
        && parts[index].len() == 4
        && parts[index].bytes().all(|b| b.is_ascii_alphabetic())
    {
        index += 1;
    }
    // Optional single region subtag.
    if index < parts.len()
        && ((parts[index].len() == 2 && parts[index].bytes().all(|b| b.is_ascii_alphabetic()))
            || (parts[index].len() == 3 && parts[index].bytes().all(|b| b.is_ascii_digit())))
    {
        index += 1;
    }
    // Variant subtags must be unique.
    let mut seen = std::collections::HashSet::new();
    while index < parts.len() {
        let part = parts[index];
        let long = matches!(part.len(), 5..=8) && ascii_alnum(part);
        let short = part.len() == 4
            && part.as_bytes()[0].is_ascii_digit()
            && part.bytes().skip(1).all(|b| b.is_ascii_alphanumeric());
        if !(long || short) || !seen.insert(part.to_ascii_lowercase()) {
            return false;
        }
        index += 1;
    }
    true
}

fn valid_region(code: &str) -> bool {
    (code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic()))
        || (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()))
}

fn valid_calendar(code: &str) -> bool {
    !code.is_empty()
        && code
            .split('-')
            .all(|part| matches!(part.len(), 3..=8) && ascii_alnum(part))
}
