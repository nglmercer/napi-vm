//! ECMA-402 locale identity and canonicalization. ICU carries locale data;
//! all observable conversions and object operations stay in the interpreter.
use super::{NativeFn, native_method};
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{PropAttrs, Value};
use icu_locale::{Locale, LocaleCanonicalizer};
use std::rc::Rc;
pub(crate) mod collator;
mod data;
pub(crate) mod datetime;
pub(crate) mod displaynames;
pub(crate) mod duration;
pub(crate) mod list;
pub(crate) mod number;
pub(crate) mod plurals;
pub(crate) mod relative;
pub(crate) mod segmenter;
mod supported;

#[derive(Clone, Debug)]
pub(crate) struct LocaleSlots {
    data: Locale,
    // ICU4X only stores 2–3 character languages. ECMA-402 also accepts
    // reserved 5–8 character language subtags.
    extended_language: Option<String>,
}
impl std::ops::Deref for LocaleSlots {
    type Target = Locale;
    fn deref(&self) -> &Locale {
        &self.data
    }
}
impl std::ops::DerefMut for LocaleSlots {
    fn deref_mut(&mut self) -> &mut Locale {
        &mut self.data
    }
}
impl std::fmt::Display for LocaleSlots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = self.data.to_string();
        if let Some(language) = &self.extended_language {
            write!(
                f,
                "{}{}",
                language,
                &text[self.data.id.language.as_str().len()..]
            )
        } else {
            f.write_str(&text)
        }
    }
}
fn canonicalize(loc: &mut LocaleSlots) {
    LocaleCanonicalizer::new_extended().canonicalize(&mut loc.data);
    for (key, old, replacement) in data::KEYWORD_ALIASES {
        if key.as_bytes()[1].is_ascii_digit() {
            let key = key
                .parse::<icu_locale::extensions::transform::Key>()
                .unwrap();
            if loc
                .extensions
                .transform
                .fields
                .get(&key)
                .is_some_and(|v| v.to_string() == *old)
            {
                loc.extensions
                    .transform
                    .fields
                    .set(key, replacement.parse().unwrap());
            }
        } else {
            let key = key.parse::<icu_locale::extensions::unicode::Key>().unwrap();
            if loc
                .extensions
                .unicode
                .keywords
                .get(&key)
                .is_some_and(|v| v.to_string() == *old)
            {
                loc.extensions
                    .unicode
                    .keywords
                    .set(key, replacement.parse().unwrap());
            }
        }
    }
}

fn range() -> VmErr {
    VmErr::Msg("RangeError: Invalid Unicode locale identifier".into())
}
fn type_error() -> VmErr {
    VmErr::Msg("TypeError: Intl.Locale receiver required".into())
}
fn arg(args: &[Value], n: usize) -> Value {
    args.get(n).cloned().unwrap_or(Value::Undefined)
}
fn slot(value: &Value) -> Option<LocaleSlots> {
    value
        .property_cell()
        .and_then(|p| p.meta.borrow().intl_locale.as_deref().cloned())
}
fn locale(value: &Value) -> Result<LocaleSlots, VmErr> {
    slot(value).ok_or_else(type_error)
}
fn nonenum(target: &Value, name: &str, value: Value) {
    target.set_prop(name.into(), value).unwrap();
    target.property_cell().unwrap().meta.borrow_mut().set_attrs(
        name,
        PropAttrs {
            enumerable: false,
            ..Default::default()
        },
    );
}
fn tag(target: &Value, text: &str) {
    if let Some(Value::Symbol(ref symbol)) = super::well_known("toStringTag") {
        let key = crate::interpreter::symbol_slot_key(symbol);
        target
            .set_prop(key.clone(), Value::String(text.into()))
            .unwrap();
        let cell = target.property_cell().unwrap();
        let mut meta = cell.meta.borrow_mut();
        meta.set_symbol_key(&key, symbol.clone());
        meta.set_attrs(
            &key,
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        );
    }
}
pub(super) fn install(e: &mut Environment) {
    let fp = e.get("Function").and_then(|v| v.get_prop("prototype"));
    let op = e.get("Object").and_then(|v| v.get_prop("prototype"));
    let intl = Value::object_with_proto(vec![], op.clone().map(Rc::new));
    tag(&intl, "Intl");
    nonenum(
        &intl,
        "getCanonicalLocales",
        native_method("getCanonicalLocales", 1, canonical_locales, fp.clone()),
    );
    nonenum(
        &intl,
        "supportedValuesOf",
        native_method("supportedValuesOf", 1, supported_values_of, fp.clone()),
    );
    let ctor = Value::object_with_proto(vec![], fp.clone().map(Rc::new));
    super::make_callable(&ctor, super::require_new, Some(construct));
    for (key, value) in [
        ("name", Value::String("Locale".into())),
        ("length", Value::Number(1.0)),
    ] {
        nonenum(&ctor, key, value);
        ctor.property_cell().unwrap().meta.borrow_mut().set_attrs(
            key,
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        );
    }
    let proto = Value::object_with_proto(vec![], op.map(Rc::new));
    nonenum(&proto, "constructor", ctor.clone());
    tag(&proto, "Intl.Locale");
    for (name, f) in [
        ("toString", to_string as NativeFn),
        ("maximize", maximize),
        ("minimize", minimize),
        ("getTextInfo", get_text_info),
        ("getCalendars", get_calendars),
        ("getCollations", get_collations as NativeFn),
        ("getHourCycles", get_hour_cycles),
        ("getNumberingSystems", get_numbering_systems as NativeFn),
        ("getTimeZones", get_time_zones as NativeFn),
        ("getWeekInfo", get_week_info),
    ] {
        nonenum(&proto, name, native_method(name, 0, f, fp.clone()));
    }
    for (key, f) in [
        ("baseName", get_base as NativeFn),
        ("language", get_language),
        ("script", get_script),
        ("region", get_region),
        ("variants", get_variants),
        ("calendar", get_calendar),
        ("collation", get_collation),
        ("hourCycle", get_hour_cycle),
        ("caseFirst", get_case_first),
        ("numeric", get_numeric),
        ("numberingSystem", get_numbering_system),
        ("firstDayOfWeek", get_first_day),
    ] {
        super::object::define_property(
            &proto,
            key,
            &Value::descriptor_record(vec![
                (
                    "get".into(),
                    native_method(&format!("get {key}"), 0, f, fp.clone()),
                ),
                ("configurable".into(), Value::Bool(true)),
            ]),
        )
        .unwrap();
    }
    super::set_builtin_constructor_prototype(e, &ctor, proto.clone());
    nonenum(&intl, "Locale", ctor.clone());
    e.install_intrinsic("Intl.Locale", ctor);
    e.install_intrinsic("Intl.Locale.prototype", proto);
    segmenter::install(e, &intl);
    list::install(e, &intl);
    collator::install(e, &intl);
    datetime::install(e, &intl);
    displaynames::install(e, &intl);
    plurals::install(e, &intl);
    relative::install(e, &intl);
    duration::install(e, &intl);
    number::install(e, &intl);
    e.set("Intl", intl);
}

fn parse(text: &str) -> Result<LocaleSlots, VmErr> {
    // ICU also accepts underscore separators. ECMA-402 accepts only Unicode
    // locale identifier syntax, excluding private-use-only and extlang tags.
    if text.contains('_') || !text.is_ascii() {
        return Err(range());
    }
    let first = text.split('-').next().unwrap_or("");
    if !matches!(first.len(), 2..=3 | 5..=8) || !first.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(range());
    }
    // Duplicate variants are errors even though ICU normalizes them away.
    let mut variants = std::collections::HashSet::new();
    for subtag in text.split('-').skip(1).take_while(|s| s.len() > 1) {
        if ((5..=8).contains(&subtag.len())
            || subtag.len() == 4 && subtag.as_bytes()[0].is_ascii_digit())
            && !variants.insert(subtag.to_ascii_lowercase())
        {
            return Err(range());
        }
    }
    let extended_language = (first.len() > 3).then(|| first.to_ascii_lowercase());
    let replaced = extended_language
        .as_ref()
        .map(|_| format!("und{}", &text[first.len()..]));
    let data: Locale = replaced
        .as_deref()
        .unwrap_or(text)
        .parse()
        .map_err(|_| range())?;
    let mut loc = LocaleSlots {
        data,
        extended_language,
    };
    canonicalize(&mut loc);
    Ok(loc)
}
fn option(i: &mut Interpreter, options: &Value, name: &str) -> Result<Option<String>, VmErr> {
    let value = i.member(options, name)?;
    if matches!(value, Value::Undefined) {
        Ok(None)
    } else {
        Ok(Some(i.ecmascript_to_string(&value)?.to_string()))
    }
}
fn wrap(i: &mut Interpreter, loc: LocaleSlots, construct: bool) -> Result<Value, VmErr> {
    let prototype = if construct {
        let target = i.new_target_stack.last().cloned().ok_or_else(type_error)?;
        i.constructor_prototype(&target, "Intl.Locale")?
    } else {
        i.persistent_global
            .borrow()
            .intrinsic("Intl.Locale.prototype")
            .map(Rc::new)
    };
    let result = Value::object_with_proto(vec![], prototype);
    result
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .intl_locale = Some(Box::new(loc));
    Ok(result)
}
fn construct(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let tag = arg(&args, 0);
    if !matches!(tag, Value::String(_)) && !crate::interpreter::call::is_js_object(&tag) {
        return Err(VmErr::Msg(
            "TypeError: locale tag must be a string or object".into(),
        ));
    }
    let mut loc = if let Some(loc) = slot(&tag) {
        loc
    } else {
        parse(i.ecmascript_to_string(&tag)?.as_ref())?
    };
    let options = match arg(&args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        value => super::object::to_object_receiver(&value)?,
    };
    let language = option(i, &options, "language")?;
    if let Some(text) = language {
        if !matches!(text.len(), 2..=3 | 5..=8) || !text.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(range());
        }
        loc.extended_language = (text.len() > 3).then(|| text.to_ascii_lowercase());
        loc.id.language = if text.len() > 3 { "und" } else { &text }
            .parse()
            .map_err(|_| range())?;
    }
    if let Some(text) = option(i, &options, "script")? {
        loc.id.script = Some(text.parse().map_err(|_| range())?);
    }
    if let Some(text) = option(i, &options, "region")? {
        loc.id.region = Some(text.parse().map_err(|_| range())?);
    }
    if let Some(text) = option(i, &options, "variants")? {
        let base = format!("und-{text}");
        let variants = parse(&base)?;
        if variants.id.variants.is_empty()
            || variants.id.script.is_some()
            || variants.id.region.is_some()
            || !variants.extensions.is_empty()
        {
            return Err(range());
        }
        loc.id.variants = variants.id.variants.clone();
    }
    for (name, key, allowed) in [
        ("calendar", "ca", &[][..]),
        ("collation", "co", &[][..]),
        ("firstDayOfWeek", "fw", &[][..]),
        ("hourCycle", "hc", &["h11", "h12", "h23", "h24"][..]),
        ("caseFirst", "kf", &["upper", "lower", "false"][..]),
        ("numeric", "kn", &[][..]),
        ("numberingSystem", "nu", &[][..]),
    ] {
        let text = if name == "numeric" {
            let v = i.member(&options, name)?;
            if matches!(v, Value::Undefined) {
                None
            } else {
                Some(if v.is_truthy() { "true" } else { "false" }.into())
            }
        } else {
            option(i, &options, name)?
        };
        if let Some(mut text) = text {
            if !allowed.is_empty() && !allowed.contains(&text.as_str()) {
                return Err(range());
            }
            if name == "firstDayOfWeek" {
                text = match text.as_str() {
                    "0" | "7" => "sun",
                    "1" => "mon",
                    "2" => "tue",
                    "3" => "wed",
                    "4" => "thu",
                    "5" => "fri",
                    "6" => "sat",
                    _ => &text,
                }
                .to_owned();
            }
            if text.is_empty()
                || !text.split('-').all(|s| {
                    (3..=8).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
                })
            {
                return Err(range());
            }
            let value = text.parse().map_err(|_| range())?;
            loc.extensions
                .unicode
                .keywords
                .set(key.parse().unwrap(), value);
        }
    }
    canonicalize(&mut loc);
    wrap(i, loc, true)
}
fn to_string(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::String(locale(&this)?.to_string().into()))
}
fn maximize(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let mut loc = locale(&this)?;
    if loc.extended_language.is_none() {
        maximize_id(&mut loc.id);
    }
    wrap(i, loc, false)
}
fn minimize(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let mut loc = locale(&this)?;
    if loc.extended_language.is_none() {
        minimize_id(&mut loc.id);
    }
    wrap(i, loc, false)
}
fn get_base(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::String({
        let loc = locale(&this)?;
        let mut base = loc.clone();
        base.data.extensions = Default::default();
        base.to_string().into()
    }))
}
fn get_language(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::String({
        let loc = locale(&this)?;
        loc.extended_language
            .clone()
            .unwrap_or_else(|| loc.id.language.to_string())
            .into()
    }))
}
fn string_or_undefined(value: Option<String>) -> Value {
    value
        .map(|s| Value::String(s.into()))
        .unwrap_or(Value::Undefined)
}
fn get_script(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(string_or_undefined(
        locale(&this)?.id.script.map(|s| s.to_string()),
    ))
}
fn get_region(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(string_or_undefined(
        locale(&this)?.id.region.map(|s| s.to_string()),
    ))
}
fn get_variants(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let loc = locale(&this)?;
    Ok(string_or_undefined(if loc.id.variants.is_empty() {
        None
    } else {
        Some(
            loc.id
                .variants
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("-"),
        )
    }))
}
fn keyword(this: &Value, key: &str) -> Result<Option<String>, VmErr> {
    Ok(locale(this)?
        .extensions
        .unicode
        .keywords
        .get(&key.parse::<icu_locale::extensions::unicode::Key>().unwrap())
        .map(ToString::to_string))
}
macro_rules! keyword_getter {
    ($($name:ident => $key:literal),* $(,)?) => { $(
        fn $name(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> { Ok(string_or_undefined(keyword(&this, $key)?)) }
    )* };
}
keyword_getter! { get_calendar => "ca", get_collation => "co", get_hour_cycle => "hc", get_case_first => "kf", get_numbering_system => "nu" }
fn get_numeric(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(
        keyword(&this, "kn")?.is_some_and(|s| s.is_empty() || s == "true"),
    ))
}
fn get_first_day(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(string_or_undefined(keyword(&this, "fw")?))
}
fn canonical_locales(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let input = arg(&args, 0);
    if matches!(input, Value::Undefined) {
        return Ok(Value::array(vec![]));
    }
    let list = if matches!(input, Value::String(_)) || slot(&input).is_some() {
        Value::array(vec![input])
    } else {
        super::object::to_object_receiver(&input)?
    };
    let length = super::array::array_like_length_number(i, &list)?;
    let mut seen = Vec::<String>::new();
    let mut index = 0.;
    while index < length {
        i.consume_loop()?;
        let key = Value::String(format!("{index:.0}").into());
        index += 1.;
        if !i.has_property(&list, &key)? {
            continue;
        }
        let value = i.member(&list, &format!("{:.0}", index - 1.))?;
        if !matches!(value, Value::String(_)) && !crate::interpreter::call::is_js_object(&value) {
            return Err(VmErr::Msg(
                "TypeError: locale list element must be a string or object".into(),
            ));
        }
        let text = if let Some(loc) = slot(&value) {
            loc.to_string()
        } else {
            i.ecmascript_to_string(&value)?.to_string()
        };
        let canonical = parse(&text)?.to_string();
        if !seen.contains(&canonical) {
            seen.push(canonical);
        }
    }
    Ok(Value::array(
        seen.into_iter().map(|s| Value::String(s.into())).collect(),
    ))
}

fn get_text_info(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let loc = locale(&this)?;
    let mut id = loc.id.clone();
    if id.script.is_none() && loc.extended_language.is_none() {
        maximize_id(&mut id);
    }
    let direction = match id.script.and_then(|script| {
        if script.as_str() == "Zzzz" {
            None
        } else {
            let script_id = icu_locale::LanguageIdentifier::from(Some(script));
            icu_locale::LocaleDirectionality::new_extended().get(&script_id)
        }
    }) {
        Some(icu_locale::Direction::RightToLeft) => Value::String("rtl".into()),
        Some(icu_locale::Direction::LeftToRight) => Value::String("ltr".into()),
        _ => Value::Undefined,
    };
    Ok(Value::object(vec![("direction".into(), direction)]))
}

fn keyword_in(loc: &LocaleSlots, key: &str) -> Option<String> {
    loc.extensions
        .unicode
        .keywords
        .get(&key.parse::<icu_locale::extensions::unicode::Key>().unwrap())
        .map(ToString::to_string)
}
fn region_for_info(loc: &LocaleSlots) -> String {
    if let Some(rg) = keyword_in(loc, "rg")
        && let Some(prefix) = rg.strip_suffix("zzzz")
        && let Ok(region) = prefix.parse::<icu_locale::subtags::Region>()
    {
        return region.to_string();
    }
    if let Some(region) = loc.id.region {
        return region.to_string();
    }
    if let Some(sd) = keyword_in(loc, "sd") {
        let len = if sd.as_bytes().first().is_some_and(u8::is_ascii_digit) {
            3
        } else {
            2
        };
        if sd.len() > len
            && let Ok(region) = sd[..len].parse::<icu_locale::subtags::Region>()
        {
            return region.to_string();
        }
    }
    let mut id = loc.id.clone();
    maximize_id(&mut id);
    id.region
        .map(|r| r.to_string())
        .unwrap_or_else(|| "001".into())
}
fn strings(text: &str) -> Value {
    Value::array(
        text.split_whitespace()
            .map(|s| Value::String(s.into()))
            .collect(),
    )
}
fn get_calendars(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let loc = locale(&this)?;
    if let Some(calendar) = keyword_in(&loc, "ca") {
        return Ok(Value::array(vec![Value::String(calendar.into())]));
    }
    let region = region_for_info(&loc);
    let calendars = data::CALENDARS
        .iter()
        .find(|(r, _)| *r == region)
        .or_else(|| data::CALENDARS.iter().find(|(r, _)| *r == "001"))
        .unwrap()
        .1;
    Ok(strings(calendars))
}
fn get_hour_cycles(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let loc = locale(&this)?;
    if let Some(cycle) = keyword_in(&loc, "hc") {
        return Ok(Value::array(vec![Value::String(cycle.into())]));
    }
    let region = region_for_info(&loc);
    let language_region = format!("{}_{}", loc.id.language, region);
    let cycle = data::HOURS
        .iter()
        .find(|(r, _)| *r == language_region)
        .or_else(|| data::HOURS.iter().find(|(r, _)| *r == region))
        .or_else(|| data::HOURS.iter().find(|(r, _)| *r == "001"))
        .unwrap()
        .1;
    Ok(strings(cycle))
}
fn is_private_language(language: &str) -> bool {
    let bytes = language.as_bytes();
    bytes.len() == 3 && bytes[0] == b'q' && matches!(bytes[1], b'a'..=b't')
}
fn get_collations(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let loc = locale(&this)?;
    if let Some(collation) = keyword_in(&loc, "co") {
        return Ok(Value::array(vec![Value::String(collation.into())]));
    }
    let language = loc.id.language.to_string();
    // Root and private-use locales fall back to the root collations.
    if language == "und" || is_private_language(&language) {
        return Ok(strings("emoji eor"));
    }
    let resolved =
        best_available_locale(&loc.to_string()).and_then(|base| base.parse::<Locale>().ok());
    let Some(resolved) = resolved else {
        return Ok(strings("emoji eor"));
    };
    Ok(Value::array(
        supported::SUPPORTED_COLLATIONS
            .iter()
            .filter(|name| collator::collation_supported(&language, &resolved, name))
            .map(|name| Value::String((*name).into()))
            .collect(),
    ))
}
fn get_numbering_systems(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let loc = locale(&this)?;
    if let Some(numbering) = keyword_in(&loc, "nu") {
        return Ok(Value::array(vec![Value::String(numbering.into())]));
    }
    let tag = loc.to_string();
    let numbering = if best_available_locale(&tag).is_some() {
        relative::resolve_numbering(&[tag], None)?.3
    } else {
        "latn".into()
    };
    Ok(Value::array(vec![Value::String(numbering.into())]))
}
fn get_time_zones(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let loc = locale(&this)?;
    // GetRegion without maximization: a locale without region yields undefined.
    let region = keyword_in(&loc, "rg")
        .and_then(|rg| rg.strip_suffix("zzzz").map(str::to_string))
        .or_else(|| loc.id.region.map(|r| r.to_string()));
    let Some(region) = region else {
        return Ok(Value::Undefined);
    };
    let mut zones: Vec<&str> = datetime::ZONE_TAB
        .iter()
        .filter(|(country, _)| *country == region)
        .map(|(_, zone)| *zone)
        .collect();
    zones.sort();
    zones.dedup();
    Ok(Value::array(
        zones
            .into_iter()
            .map(|zone| Value::String(zone.into()))
            .collect(),
    ))
}
fn get_week_info(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let loc = locale(&this)?;
    let region = region_for_info(&loc);
    let &(_, mut first, _minimal, start, end) = data::WEEK
        .iter()
        .find(|(r, ..)| *r == region)
        .or_else(|| data::WEEK.iter().find(|(r, ..)| *r == "001"))
        .unwrap();
    if let Some(fw) = keyword_in(&loc, "fw")
        && let Some(n) = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
            .iter()
            .position(|day| *day == fw)
    {
        first = n as u8 + 1;
    }
    let weekend = (1..=7)
        .filter(|day| {
            if start <= end {
                (start..=end).contains(day)
            } else {
                *day >= start || *day <= end
            }
        })
        .map(|n| Value::Number(n as f64))
        .collect();
    Ok(Value::object(vec![
        ("firstDay".into(), Value::Number(first as f64)),
        ("weekend".into(), Value::array(weekend)),
    ]))
}

fn maximize_id(id: &mut icu_locale::LanguageIdentifier) {
    if id.script.is_some_and(|s| s.as_str() == "Zzzz") {
        id.script = None;
    }
    if id.region.is_some_and(|r| r.as_str() == "ZZ") {
        id.region = None;
    }
    if !id.language.is_unknown() && id.script.is_some() && id.region.is_some() {
        return;
    }
    let language = id.language.to_string();
    let script = id.script.map(|s| s.to_string());
    let region = id.region.map(|r| r.to_string());
    let mut candidates = Vec::new();
    if let (Some(s), Some(r)) = (&script, &region) {
        candidates.push(format!("{language}-{s}-{r}"));
    }
    if let Some(s) = &script {
        candidates.push(format!("{language}-{s}"));
    }
    if let Some(r) = &region {
        candidates.push(format!("{language}-{r}"));
    }
    candidates.push(language);
    for candidate in candidates {
        if let Ok(index) =
            data::LIKELY_SUBTAGS.binary_search_by(|(key, _)| key.cmp(&candidate.as_str()))
            && let Ok(likely) = data::LIKELY_SUBTAGS[index]
                .1
                .parse::<icu_locale::LanguageIdentifier>()
        {
            if id.language.is_unknown() {
                id.language = likely.language;
            }
            if id.script.is_none() {
                id.script = likely.script;
            }
            if id.region.is_none() {
                id.region = likely.region;
            }
            return;
        }
    }
}
fn minimize_id(id: &mut icu_locale::LanguageIdentifier) {
    maximize_id(id);
    let maximized = id.clone();
    for (script, region) in [(None, None), (None, id.region), (id.script, None)] {
        let mut candidate = id.clone();
        candidate.script = script;
        candidate.region = region;
        let mut trial = candidate.clone();
        maximize_id(&mut trial);
        if trial == maximized {
            *id = candidate;
            return;
        }
    }
}

pub(super) fn locale_list(i: &mut Interpreter, value: Value) -> Result<Vec<String>, VmErr> {
    let result = canonical_locales(i, Value::Undefined, vec![value])?;
    let array = result.as_array().unwrap();
    Ok(array
        .borrow()
        .iter()
        .map(|v| match v {
            Value::String(s) => s.to_string(),
            _ => unreachable!(),
        })
        .collect())
}
fn best_available_locale(requested: &str) -> Option<String> {
    let mut loc = parse(requested).ok()?;
    loc.data.extensions.unicode.clear();
    let mut candidate = loc.to_string();
    loop {
        if data::AVAILABLE_LOCALES
            .binary_search(&candidate.as_str())
            .is_ok()
        {
            return Some(candidate);
        }
        let mut end = candidate.rfind('-')?;
        if end >= 2 && candidate.as_bytes()[end - 2] == b'-' {
            end -= 2;
        }
        candidate.truncate(end);
    }
}
fn locale_matcher(i: &mut Interpreter, options: &Value) -> Result<(), VmErr> {
    if let Some(matcher) = option(i, options, "localeMatcher")?
        && matcher != "lookup"
        && matcher != "best fit"
    {
        return Err(VmErr::Msg("RangeError: Invalid localeMatcher".into()));
    }
    Ok(())
}
fn supported_locales(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let requested = locale_list(i, arg(&args, 0))?;
    let options = match arg(&args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        v => super::object::to_object_receiver(&v)?,
    };
    locale_matcher(i, &options)?;
    Ok(Value::array(
        requested
            .into_iter()
            .filter(|tag| best_available_locale(tag).is_some())
            .map(|tag| Value::String(tag.into()))
            .collect(),
    ))
}
fn supported_values_of(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    // ECMA-402 Intl.supportedValuesOf: coerce the key, then return a fresh
    // array with the values the implementation observably supports.
    let key = i.ecmascript_to_string(&arg(&args, 0))?.to_string();
    let values: Vec<Value> = match key.as_str() {
        "calendar" => supported::SUPPORTED_CALENDARS
            .iter()
            .map(|name| Value::String((*name).into()))
            .collect(),
        "collation" => supported::SUPPORTED_COLLATIONS
            .iter()
            .map(|name| Value::String((*name).into()))
            .collect(),
        "currency" => supported::SUPPORTED_CURRENCIES
            .iter()
            .map(|name| Value::String((*name).into()))
            .collect(),
        "numberingSystem" => number::numbering_system_names()
            .map(|name| Value::String(name.into()))
            .collect(),
        "timeZone" => supported::SUPPORTED_TIME_ZONES
            .iter()
            .map(|name| Value::String((*name).into()))
            .collect(),
        "unit" => number::unit_names()
            .iter()
            .map(|name| Value::String((*name).into()))
            .collect(),
        _ => {
            return Err(VmErr::Msg(
                "RangeError: Invalid supportedValuesOf key".into(),
            ));
        }
    };
    Ok(Value::array(values))
}
