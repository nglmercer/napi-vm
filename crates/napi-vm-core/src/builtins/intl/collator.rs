//! Locale-aware string comparison; ICU implements collation, VM owns coercions.
use super::*;
use icu_collator::{Collator, CollatorBorrowed, CollatorPreferences, options::*, preferences::*};
#[derive(Clone)]
pub(crate) struct Slots {
    formatter: Rc<CollatorBorrowed<'static>>,
    locale: String,
    usage: String,
    sensitivity: String,
    collation: String,
}
fn error() -> VmErr {
    VmErr::Msg("TypeError: Intl.Collator receiver required".into())
}
fn slot(this: &Value) -> Result<Slots, VmErr> {
    this.property_cell()
        .and_then(|p| p.meta.borrow().intl_collator.clone())
        .ok_or_else(error)
}
fn choice(
    i: &mut Interpreter,
    o: &Value,
    key: &str,
    default: &str,
    allowed: &[&str],
) -> Result<String, VmErr> {
    let value = option(i, o, key)?.unwrap_or_else(|| default.into());
    if !allowed.contains(&value.as_str()) {
        return Err(VmErr::Msg(format!("RangeError: Invalid {key}")));
    }
    Ok(value)
}
fn initialize(i: &mut Interpreter, args: &[Value]) -> Result<Slots, VmErr> {
    let requested = locale_list(i, arg(args, 0))?;
    let options = match arg(args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        Value::Null => return Err(error()),
        v => Value::boxed_primitive(v.clone()).unwrap_or(v),
    };
    let usage = choice(i, &options, "usage", "sort", &["sort", "search"])?;
    locale_matcher(i, &options)?;
    let collation = option(i, &options, "collation")?;
    if collation.as_ref().is_some_and(|c| {
        !c.split('-')
            .all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
    }) {
        return Err(range());
    }
    let numeric = i.member(&options, "numeric")?;
    let case_first = option(i, &options, "caseFirst")?;
    if case_first
        .as_ref()
        .is_some_and(|v| !["upper", "lower", "false"].contains(&v.as_str()))
    {
        return Err(range());
    }
    let matched = requested
        .iter()
        .find_map(|tag| best_available_locale(tag).map(|base| (tag.clone(), base)))
        .unwrap_or_else(|| ("en-US".into(), "en-US".into()));
    let mut resolved: Locale = matched.1.parse().unwrap();
    let original: Locale = matched.0.parse().unwrap_or_else(|_| resolved.clone());
    let key = |k: &str| k.parse::<icu_locale::extensions::unicode::Key>().unwrap();
    // Keep the ECMA-402 relevant Unicode extensions, allowing explicit options
    // to override them without retaining unsupported locale requests.
    for k in ["co", "kn", "kf"] {
        if let Some(v) = original.extensions.unicode.keywords.get(&key(k)) {
            resolved.extensions.unicode.keywords.set(key(k), v.clone());
        }
    }
    let mut prefs: CollatorPreferences = resolved.clone().into();
    if !matches!(numeric, Value::Undefined) {
        prefs.numeric_ordering = Some(if numeric.is_truthy() {
            CollationNumericOrdering::True
        } else {
            CollationNumericOrdering::False
        });
        if original
            .extensions
            .unicode
            .keywords
            .get(&key("kn"))
            .map(|v| v.to_string())
            .as_deref()
            != Some(if numeric.is_truthy() { "" } else { "false" })
        {
            resolved.extensions.unicode.keywords.remove(&key("kn"));
        }
    }
    if let Some(v) = case_first {
        prefs.case_first = Some(match v.as_str() {
            "upper" => CollationCaseFirst::Upper,
            "lower" => CollationCaseFirst::Lower,
            _ => CollationCaseFirst::False,
        });
        if original
            .extensions
            .unicode
            .keywords
            .get(&key("kf"))
            .map(|v| v.to_string())
            .as_deref()
            != Some(&v)
        {
            resolved.extensions.unicode.keywords.remove(&key("kf"));
        }
    }
    let mut collation = collation
        .or_else(|| {
            original
                .extensions
                .unicode
                .keywords
                .get(&key("co"))
                .map(|v| v.to_string())
        })
        .unwrap_or_else(|| "default".into());
    if ["standard", "search"].contains(&collation.as_str()) {
        collation = "default".into();
        resolved.extensions.unicode.keywords.remove(&key("co"));
    }
    if collation != "default" {
        let mut with = resolved.clone();
        with.extensions
            .unicode
            .keywords
            .set(key("co"), collation.parse().unwrap());
        let try_prefs: CollatorPreferences = with.into();
        prefs.collation_type = try_prefs.collation_type;
        if prefs.collation_type.is_none() {
            collation = "default".into();
            resolved.extensions.unicode.keywords.remove(&key("co"));
        }
    }
    let sensitivity = choice(
        i,
        &options,
        "sensitivity",
        "variant",
        &["base", "accent", "case", "variant"],
    )?;
    let ignore = i.member(&options, "ignorePunctuation")?;
    let mut icu = CollatorOptions::default();
    icu.strength = Some(match sensitivity.as_str() {
        "base" | "case" => Strength::Primary,
        "accent" => Strength::Secondary,
        _ => Strength::Tertiary,
    });
    icu.case_level = Some(if sensitivity == "case" {
        CaseLevel::On
    } else {
        CaseLevel::Off
    });
    if !matches!(ignore, Value::Undefined) {
        icu.alternate_handling = Some(if ignore.is_truthy() {
            AlternateHandling::Shifted
        } else {
            AlternateHandling::NonIgnorable
        });
    }
    // Search uses the locale's collation with the requested strength. ICU's
    // compiled data determines the language-specific tailoring.
    let formatter = match Collator::try_new(prefs, icu) {
        Ok(c) => c,
        Err(_) => {
            collation = "default".into();
            resolved.extensions.unicode.keywords.remove(&key("co"));
            Collator::try_new(resolved.clone().into(), icu).map_err(|_| range())?
        }
    };
    Ok(Slots {
        formatter: Rc::new(formatter),
        locale: resolved.to_string(),
        usage,
        sensitivity,
        collation,
    })
}
fn construct(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let target = i.new_target_stack.last().cloned().unwrap_or_else(|| {
        i.persistent_global
            .borrow()
            .intrinsic("Intl.Collator")
            .unwrap()
    });
    let prototype = i.constructor_prototype(&target, "Intl.Collator")?;
    let slots = initialize(i, &args)?;
    let object = Value::object_with_proto(vec![], prototype);
    object
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .intl_collator = Some(slots);
    Ok(object)
}
fn compare(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let slots = slot(&this)?;
    let a = i.ecmascript_to_string(&arg(&args, 0))?;
    let b = i.ecmascript_to_string(&arg(&args, 1))?;
    Ok(Value::Number(
        slots.formatter.compare_utf16(a.units(), b.units()) as i8 as f64,
    ))
}
fn get_compare(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    slot(&this)?;
    let cell = this.property_cell().unwrap();
    if let Some(bound) = &cell.meta.borrow().intl_bound_compare {
        return Ok(bound.clone());
    }
    let fp = i
        .persistent_global
        .borrow()
        .intrinsic("Function")
        .and_then(|f| f.get_prop("prototype"));
    let target = native_method("", 2, compare, fp);
    let bound = super::super::function::function_bind(i, target, vec![this])?;
    bound
        .property_cell()
        .unwrap()
        .borrow_mut()
        .iter_mut()
        .find(|(k, _)| k == "name")
        .unwrap()
        .1 = Value::String("".into());
    bound
        .property_cell()
        .unwrap()
        .borrow_mut()
        .sort_by_key(|(k, _)| if k == "length" { 0 } else { 1 });
    cell.meta.borrow_mut().intl_bound_compare = Some(bound.clone());
    Ok(bound)
}
fn resolved_options(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    let o = s.formatter.resolved_options();
    Ok(Value::object(vec![
        ("locale".into(), Value::String(s.locale.into())),
        ("usage".into(), Value::String(s.usage.into())),
        ("sensitivity".into(), Value::String(s.sensitivity.into())),
        (
            "ignorePunctuation".into(),
            Value::Bool(o.alternate_handling == AlternateHandling::Shifted),
        ),
        ("collation".into(), Value::String(s.collation.into())),
        (
            "numeric".into(),
            Value::Bool(o.numeric == CollationNumericOrdering::True),
        ),
        (
            "caseFirst".into(),
            Value::String(
                match o.case_first {
                    CollationCaseFirst::Upper => "upper",
                    CollationCaseFirst::Lower => "lower",
                    _ => "false",
                }
                .into(),
            ),
        ),
    ]))
}
pub(super) fn install(e: &mut Environment, namespace: &Value) {
    let fp = e.get("Function").and_then(|f| f.get_prop("prototype"));
    let op = e.get("Object").and_then(|f| f.get_prop("prototype"));
    let c = Value::object_with_proto(vec![], fp.clone().map(Rc::new));
    super::super::make_callable(&c, construct, Some(construct));
    for (key, value) in [
        ("name", Value::String("Collator".into())),
        ("length", Value::Number(0.)),
    ] {
        nonenum(&c, key, value);
        c.property_cell().unwrap().meta.borrow_mut().set_attrs(
            key,
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        );
    }
    nonenum(
        &c,
        "supportedLocalesOf",
        native_method("supportedLocalesOf", 1, supported_locales, fp.clone()),
    );
    let p = Value::object_with_proto(vec![], op.map(Rc::new));
    nonenum(&p, "constructor", c.clone());
    tag(&p, "Intl.Collator");
    nonenum(
        &p,
        "resolvedOptions",
        native_method("resolvedOptions", 0, resolved_options, fp.clone()),
    );
    super::super::object::define_property(
        &p,
        "compare",
        &Value::descriptor_record(vec![
            (
                "get".into(),
                native_method("get compare", 0, get_compare, fp),
            ),
            ("enumerable".into(), Value::Bool(false)),
            ("configurable".into(), Value::Bool(true)),
        ]),
    )
    .unwrap();
    super::super::set_builtin_constructor_prototype(e, &c, p.clone());
    nonenum(namespace, "Collator", c.clone());
    e.install_intrinsic("Intl.Collator", c);
    e.install_intrinsic("Intl.Collator.prototype", p);
}
pub(in crate::builtins) fn compare_strings(
    i: &mut Interpreter,
    a: crate::JsString,
    b: crate::JsString,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = initialize(i, &args)?;
    Ok(Value::Number(
        s.formatter.compare_utf16(a.units(), b.units()) as i8 as f64,
    ))
}
