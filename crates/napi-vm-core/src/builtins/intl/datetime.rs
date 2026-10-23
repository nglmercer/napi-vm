//! ECMA-402 date and time formatting: guest coercions and option handling
//! live here; ICU owns locale patterns, calendars, and zone names, while
//! chrono-tz supplies IANA offsets.
use super::*;
include!("cldr/timezone-names.rs");
use icu_calendar::{AnyCalendar, AnyCalendarKind};
use icu_datetime::fieldsets::builder::{DateFields, FieldSetBuilder, ZoneStyle};
use icu_datetime::fieldsets::enums::CompositeFieldSet;
use icu_datetime::options::{Length, SubsecondDigits, TimePrecision, YearStyle};
use icu_datetime::{DateTimeFormatter, DateTimeFormatterPreferences};
use std::rc::Rc;

pub(crate) const SUPPORTED_CALENDARS: &[&str] = &[
    "buddhist",
    "chinese",
    "coptic",
    "dangi",
    "ethioaa",
    "ethiopic",
    "gregory",
    "hebrew",
    "indian",
    "islamic-civil",
    "islamic-tbla",
    "islamic-umalqura",
    "iso8601",
    "japanese",
    "persian",
    "roc",
];

#[derive(Clone)]
pub(crate) struct Slots {
    locale: String,
    calendar: String,
    numbering: String,
    hour_cycle: Option<String>,
    hour12: Option<bool>,
    time_zone: String,
    weekday: Option<String>,
    era: Option<String>,
    year: Option<String>,
    month: Option<String>,
    day: Option<String>,
    day_period: Option<String>,
    hour: Option<String>,
    minute: Option<String>,
    second: Option<String>,
    fractional: Option<u8>,
    time_zone_name: Option<String>,
    date_style: Option<String>,
    time_style: Option<String>,
    digit_map: Option<Vec<char>>,
    formatter: Rc<DateTimeFormatter<CompositeFieldSet>>,
    calendar_kind: AnyCalendarKind,
    strip_time_digits: bool,
    truncate_year: bool,
    /// Date fields padded into the ICU fieldset (ICU cannot pair month-only
    /// or year-only dates with time) that must be dropped from the output.
    omit_day: bool,
    omit_month: bool,
    omit_year: bool,
    /// True when the format fell back to the bare-constructor YMD default
    /// (no explicit fields, styles, or Date-method forcing): the visible
    /// fields then adapt to the Temporal input class.
    defaults: bool,
}
fn error() -> VmErr {
    VmErr::Msg("TypeError: Intl.DateTimeFormat receiver required".into())
}
fn slot(v: &Value) -> Result<Slots, VmErr> {
    v.property_cell()
        .and_then(|p| p.meta.borrow().intl_datetime.clone())
        .ok_or_else(error)
}
fn method(target: &Value, name: &str, length: usize, f: NativeFn, fp: &Option<Value>) {
    nonenum(target, name, native_method(name, length, f, fp.clone()));
}
pub(super) fn install(e: &mut Environment, namespace: &Value) {
    let fp = e.get("Function").and_then(|f| f.get_prop("prototype"));
    let op = e.get("Object").and_then(|o| o.get_prop("prototype"));
    let ctor = Value::object_with_proto(vec![], fp.clone().map(Rc::new));
    super::super::make_callable(&ctor, construct, Some(construct));
    for (k, v) in [
        ("name", Value::String("DateTimeFormat".into())),
        ("length", Value::Number(0.)),
    ] {
        nonenum(&ctor, k, v);
        ctor.property_cell().unwrap().meta.borrow_mut().set_attrs(
            k,
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: true,
            },
        );
    }
    method(
        &ctor,
        "supportedLocalesOf",
        1,
        super::supported_locales,
        &fp,
    );
    let proto = Value::object_with_proto(vec![], op.map(Rc::new));
    nonenum(&proto, "constructor", ctor.clone());
    tag(&proto, "Intl.DateTimeFormat");
    for (name, len, f) in [
        ("resolvedOptions", 0, resolved_options as NativeFn),
        ("formatToParts", 1, format_to_parts),
        ("formatRange", 2, format_range),
        ("formatRangeToParts", 2, format_range_to_parts),
    ] {
        method(&proto, name, len, f, &fp);
    }
    super::super::object::define_property(
        &proto,
        "format",
        &Value::descriptor_record(vec![
            ("get".into(), native_method("get format", 0, get_format, fp)),
            ("enumerable".into(), Value::Bool(false)),
            ("configurable".into(), Value::Bool(true)),
        ]),
    )
    .unwrap();
    super::super::set_builtin_constructor_prototype(e, &ctor, proto.clone());
    nonenum(namespace, "DateTimeFormat", ctor.clone());
    e.install_intrinsic("Intl.DateTimeFormat", ctor);
    e.install_intrinsic("Intl.DateTimeFormat.prototype", proto);
}
fn choice(
    i: &mut Interpreter,
    o: &Value,
    key: &str,
    allowed: &[&str],
) -> Result<Option<String>, VmErr> {
    let result = option(i, o, key)?;
    if result
        .as_ref()
        .is_some_and(|s| !allowed.contains(&s.as_str()))
    {
        return Err(range());
    }
    Ok(result)
}
fn type_check(text: &str) -> bool {
    !text.is_empty()
        && text
            .split('-')
            .all(|s| (3..=8).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric()))
}
/// Canonical calendar for a syntactically valid option or locale value:
/// ASCII-lowercased with CLDR aliases resolved.
fn canonical_calendar(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    match lower.as_str() {
        "islamicc" => "islamic-civil".into(),
        "ethiopic-amete-alem" => "ethioaa".into(),
        _ => lower,
    }
}
fn default_hour_cycle(requested: &[String], base: &str) -> String {
    // A valid hour-cycle Unicode extension on the first available locale
    // acts as the default.
    if let Some(hc) = ext_hour_cycle(requested) {
        return hc;
    }
    let (language, region) = match base.parse::<Locale>() {
        Ok(loc) => (
            loc.id.language.to_string(),
            loc.id
                .region
                .map(|region| region.to_string())
                .unwrap_or_else(|| "001".into()),
        ),
        Err(_) => ("en".into(), "001".into()),
    };
    let language_region = format!("{language}_{region}");
    for (name, cycles) in data::HOURS {
        if *name == language_region {
            return cycles.split_whitespace().next().unwrap_or("h23").into();
        }
    }
    for (name, cycles) in data::HOURS {
        if *name == region {
            return cycles.split_whitespace().next().unwrap_or("h23").into();
        }
    }
    "h23".into()
}
fn default_calendar(requested: &[String]) -> String {
    ext_calendar(requested).unwrap_or_else(|| "gregory".into())
}
fn number_option(value: Option<String>) -> Result<Option<String>, VmErr> {
    if let Some(nu) = &value
        && !type_check(nu)
    {
        return Err(range());
    }
    Ok(value)
}
/// Raw option values in CreateDateTimeFormat order, before locale fallback
/// and extension reflection.
struct RawOptions {
    calendar: Option<String>,
    numbering: Option<String>,
    hour12: Option<bool>,
    hour_cycle: Option<String>,
    time_zone: Option<String>,
    weekday: Option<String>,
    era: Option<String>,
    year: Option<String>,
    month: Option<String>,
    day: Option<String>,
    day_period: Option<String>,
    hour: Option<String>,
    minute: Option<String>,
    second: Option<String>,
    fractional: Option<u8>,
    time_zone_name: Option<String>,
    date_style: Option<String>,
    time_style: Option<String>,
}
fn read_raw_options(i: &mut Interpreter, options: &Value) -> Result<RawOptions, VmErr> {
    locale_matcher(i, options)?;
    // CreateDateTimeFormat order: calendar, numberingSystem, hour12,
    // hourCycle, timeZone, components, formatMatcher, dateStyle, timeStyle.
    let calendar_option = option(i, options, "calendar")?;
    if let Some(ca) = &calendar_option
        && !type_check(ca)
    {
        return Err(range());
    }
    let numbering = number_option(option(i, options, "numberingSystem")?)?;
    let hour12 = match i.member(options, "hour12")? {
        Value::Undefined => None,
        v => Some(v.is_truthy()),
    };
    // An unsupported but well-formed hourCycle value is ignored (falling
    // back to the locale extension or default) rather than throwing, matching
    // the calendar fallback semantics pinned by
    // resolved-calendar-unicode-extensions-and-options.
    let hour_cycle_option = option(i, options, "hourCycle")?;
    let time_zone = option(i, options, "timeZone")?;
    let weekday = choice(i, options, "weekday", &["long", "short", "narrow"])?;
    let era = choice(i, options, "era", &["long", "short", "narrow"])?;
    let year = choice(i, options, "year", &["numeric", "2-digit"])?;
    let month = choice(
        i,
        options,
        "month",
        &["numeric", "2-digit", "long", "short", "narrow"],
    )?;
    let day = choice(i, options, "day", &["numeric", "2-digit"])?;
    let day_period = choice(i, options, "dayPeriod", &["short", "long", "narrow"])?;
    let hour = choice(i, options, "hour", &["numeric", "2-digit"])?;
    let minute = choice(i, options, "minute", &["numeric", "2-digit"])?;
    let second = choice(i, options, "second", &["numeric", "2-digit"])?;
    let fractional = match i.member(options, "fractionalSecondDigits")? {
        Value::Undefined => None,
        v => {
            let n = i.ecmascript_to_number(&v)?;
            if !n.is_finite() || n < 1.0 || n > 3.0 {
                return Err(range());
            }
            Some(n.floor() as u8)
        }
    };
    let time_zone_name = choice(
        i,
        options,
        "timeZoneName",
        &[
            "short",
            "long",
            "shortOffset",
            "longOffset",
            "shortGeneric",
            "longGeneric",
        ],
    )?;
    let format_matcher = option(i, options, "formatMatcher")?;
    if let Some(matcher) = &format_matcher
        && matcher != "basic"
        && matcher != "best fit"
    {
        return Err(range());
    }
    let date_style = choice(
        i,
        options,
        "dateStyle",
        &["full", "long", "medium", "short"],
    )?;
    let time_style = choice(
        i,
        options,
        "timeStyle",
        &["full", "long", "medium", "short"],
    )?;
    if (date_style.is_some() || time_style.is_some())
        && (weekday.is_some()
            || era.is_some()
            || year.is_some()
            || month.is_some()
            || day.is_some()
            || day_period.is_some()
            || hour.is_some()
            || minute.is_some()
            || second.is_some()
            || fractional.is_some()
            || time_zone_name.is_some())
    {
        return Err(VmErr::Msg(
            "TypeError: dateStyle/timeStyle cannot be combined with field options".into(),
        ));
    }
    Ok(RawOptions {
        calendar: calendar_option,
        numbering,
        hour12,
        hour_cycle: hour_cycle_option,
        time_zone,
        weekday,
        era,
        year,
        month,
        day,
        day_period,
        hour,
        minute,
        second,
        fractional,
        time_zone_name,
        date_style,
        time_style,
    })
}
/// Resolve raw options against the requested locales: numbering, calendar
/// and hour-cycle fallback with extension reflection, time zone, and the
/// `Date.prototype.toLocale*` field forcing.
fn resolve_parts(
    requested: &[String],
    mut raw: RawOptions,
    force_date: bool,
    force_time: bool,
) -> Result<Slots, VmErr> {
    // Date.prototype.toLocale* defaults: with no styles present, force the
    // missing date/time fields to numeric.
    if raw.date_style.is_none() && raw.time_style.is_none() {
        if force_date
            && raw.weekday.is_none()
            && raw.year.is_none()
            && raw.month.is_none()
            && raw.day.is_none()
        {
            raw.year = Some("numeric".into());
            raw.month = Some("numeric".into());
            raw.day = Some("numeric".into());
        }
        if force_time
            && raw.hour.is_none()
            && raw.minute.is_none()
            && raw.second.is_none()
            && raw.fractional.is_none()
            && raw.day_period.is_none()
        {
            raw.hour = Some("numeric".into());
            raw.minute = Some("numeric".into());
            raw.second = Some("numeric".into());
        }
    }
    let (nu_locale, base, _, numbering, digit_map, formatter_nu) =
        super::relative::resolve_display_numbering(requested, raw.numbering)?;
    // Calendar resolution: a supported option wins; otherwise the locale
    // extension wins; otherwise the default. The extension is reflected in
    // the resolved locale when it supplied the value or equals the option.
    let ext_ca = ext_calendar(requested);
    let opt_ca = raw.calendar.map(|ca| canonical_calendar(&ca));
    let opt_ca_supported = opt_ca
        .as_ref()
        .is_some_and(|ca| SUPPORTED_CALENDARS.contains(&ca.as_str()));
    let calendar = if opt_ca_supported {
        opt_ca.clone().unwrap()
    } else {
        ext_ca
            .clone()
            .unwrap_or_else(|| default_calendar(requested))
    };
    let keep_ca = ext_ca
        .as_ref()
        .is_some_and(|ext| !opt_ca_supported || opt_ca.as_ref() == Some(ext));
    // Hour-cycle resolution: an explicit hour12 option discards the hourCycle
    // option and constrains the locale default to the matching class (h11
    // for Japanese, h12 elsewhere); otherwise a supported option wins, then
    // the locale extension, then the default.
    let ext_hc = ext_hour_cycle(requested);
    let opt_hc_supported = raw
        .hour_cycle
        .as_ref()
        .is_some_and(|hc| matches!(hc.as_str(), "h11" | "h12" | "h23" | "h24"));
    let base_hc = if opt_hc_supported {
        raw.hour_cycle.clone().unwrap()
    } else {
        default_hour_cycle(requested, &base)
    };
    let hour_cycle = match raw.hour12 {
        Some(true) => {
            if language_of(&base) == "ja" {
                "h11".into()
            } else if base_hc == "h11" || base_hc == "h12" {
                base_hc
            } else {
                "h12".into()
            }
        }
        Some(false) => {
            if base_hc == "h23" || base_hc == "h24" {
                base_hc
            } else {
                "h23".into()
            }
        }
        None => base_hc,
    };
    // An hour12 option always drops the hc extension; otherwise the extension
    // is reflected when it supplied the value or equals the option.
    let keep_hc = ext_hc.as_ref().is_some_and(|ext| {
        raw.hour12.is_none() && (!opt_hc_supported || raw.hour_cycle.as_ref() == Some(ext))
    });
    // Resolved locale: base plus the reflected ca/hc/nu extensions in
    // canonical key order.
    let nu_value = nu_locale
        .strip_prefix(base.as_str())
        .and_then(|suffix| suffix.strip_prefix("-u-nu-"));
    let mut locale = base.clone();
    let mut separator = "-u-";
    let extensions: [(&str, Option<String>); 3] = [
        ("ca", keep_ca.then(|| calendar.clone())),
        ("hc", keep_hc.then(|| hour_cycle.clone())),
        ("nu", nu_value.map(|nu| nu.to_string())),
    ];
    for (key, value) in extensions {
        if let Some(value) = value {
            locale.push_str(separator);
            locale.push_str(key);
            locale.push('-');
            locale.push_str(&value);
            separator = "-";
        }
    }
    let time_zone = match raw.time_zone {
        None => "UTC".into(),
        Some(name) => canonicalize_time_zone(&name)?,
    };
    finish(Parts {
        locale,
        base,
        calendar,
        numbering,
        digit_map,
        formatter_nu,
        hour12: raw.hour12,
        hour_cycle,
        time_zone,
        weekday: raw.weekday,
        era: raw.era,
        year: raw.year,
        month: raw.month,
        day: raw.day,
        day_period: raw.day_period,
        hour: raw.hour,
        minute: raw.minute,
        second: raw.second,
        fractional: raw.fractional,
        time_zone_name: raw.time_zone_name,
        date_style: raw.date_style,
        time_style: raw.time_style,
    })
}
fn construct(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    // Legacy Intl constructors run the same initialization whether they are
    // called with `new` or as plain functions; a plain call on an existing
    // instance additionally publishes the fresh value under
    // Intl.FallbackSymbol (mirrors NumberFormat).
    let target = i.new_target_stack.last().cloned().unwrap_or_else(|| {
        i.persistent_global
            .borrow()
            .intrinsic("Intl.DateTimeFormat")
            .unwrap()
    });
    let prototype = i.constructor_prototype(&target, "Intl.DateTimeFormat")?;
    let requested = locale_list(i, arg(&args, 0))?;
    let options = match arg(&args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        v => super::super::object::to_object_receiver(&v)?,
    };
    let raw = read_raw_options(i, &options)?;
    let slots = resolve_parts(&requested, raw, false, false)?;
    // A fresh object is always built; when called (no new.target) on an
    // instance, the fresh value is published under Intl.FallbackSymbol on
    // the receiver and the receiver itself is returned.
    let result = Value::object_with_proto(vec![], prototype);
    result
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .intl_datetime = Some(slots);
    let new_target = i.new_target_stack.last().cloned();
    let ctor = i
        .persistent_global
        .borrow()
        .intrinsic("Intl.DateTimeFormat")
        .unwrap();
    if new_target.is_none() && i.ordinary_instance_of(&this, &ctor)?.is_truthy() {
        let symbol = i
            .persistent_global
            .borrow()
            .intrinsic("Intl.FallbackSymbol")
            .unwrap();
        let descriptor = Value::descriptor_record(vec![
            ("value".into(), result),
            ("writable".into(), Value::Bool(false)),
            ("enumerable".into(), Value::Bool(false)),
            ("configurable".into(), Value::Bool(false)),
        ]);
        if !i.define_own_property(&this, &symbol, &descriptor)? {
            return Err(error());
        }
        return Ok(this);
    }
    Ok(result)
}
struct Parts {
    locale: String,
    base: String,
    calendar: String,
    numbering: String,
    digit_map: Option<Vec<char>>,
    formatter_nu: String,
    hour12: Option<bool>,
    /// Fully resolved hour cycle (option, hour12 constraint, extension, or
    /// default); gated to `None` in slots when no time is shown.
    hour_cycle: String,
    time_zone: String,
    weekday: Option<String>,
    era: Option<String>,
    year: Option<String>,
    month: Option<String>,
    day: Option<String>,
    day_period: Option<String>,
    hour: Option<String>,
    minute: Option<String>,
    second: Option<String>,
    fractional: Option<u8>,
    time_zone_name: Option<String>,
    date_style: Option<String>,
    time_style: Option<String>,
}
/// An offset time zone such as "+03", "-0730", or "+05:45": fixed minutes
/// east of UTC with its normalized display form.
fn parse_offset_zone(name: &str) -> Option<(i32, String)> {
    let (sign, rest) = match name.as_bytes().first()? {
        b'+' => (1i32, &name[1..]),
        b'-' => (-1i32, &name[1..]),
        _ => return None,
    };
    if !rest.bytes().all(|b| b.is_ascii_digit() || b == b':') {
        return None;
    }
    let (hours, minutes) = if let Some((h, m)) = rest.split_once(':') {
        if h.len() != 2 || m.len() != 2 {
            return None;
        }
        (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?)
    } else if rest.len() == 2 {
        (rest.parse::<i32>().ok()?, 0)
    } else if rest.len() == 4 {
        (
            rest[..2].parse::<i32>().ok()?,
            rest[2..].parse::<i32>().ok()?,
        )
    } else {
        return None;
    };
    if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
        return None;
    }
    let total = sign * (hours * 60 + minutes);
    let normalized = format!(
        "{}{:02}:{:02}",
        if sign < 0 { '-' } else { '+' },
        hours,
        minutes
    );
    Some((total, normalized))
}
fn canonical_name(name: &str) -> Option<&'static str> {
    ZONE_NAMES
        .iter()
        .find(|zone| zone.eq_ignore_ascii_case(name))
        .copied()
        .or_else(|| {
            LINK_TARGETS
                .iter()
                .find(|(link, _)| link.eq_ignore_ascii_case(name))
                .map(|(link, _)| *link)
        })
}
fn link_target(name: &str) -> Option<&'static str> {
    LINK_TARGETS
        .iter()
        .find(|(link, _)| *link == name)
        .map(|(_, target)| *target)
}
fn country_of(name: &str) -> Option<&'static str> {
    for table in [ZONE1970_TAB, ZONE_TAB] {
        if let Some((code, _)) = table.iter().find(|(_, tz)| *tz == name) {
            return Some(*code);
        }
    }
    if let Some((_, code)) = LINK_COUNTRIES.iter().find(|(link, _)| *link == name) {
        return Some(*code);
    }
    link_target(name).and_then(country_of)
}
/// ECMA-402 primary time zone identifier: links resolve per the IANA
/// database with zone.tab/backzone fallbacks, and Etc/UTC, Etc/GMT, and
/// GMT fold to UTC.
fn primary_identifier(name: &str) -> Option<String> {
    let canonical = canonical_name(name)?;
    let mut current = canonical;
    for _ in 0..8 {
        match link_target(current) {
            Some(target) => current = target,
            None => break,
        }
    }
    let zone = current;
    let in_zone_tab = ZONE_TAB.iter().any(|(_, tz)| *tz == canonical);
    let mut primary = if link_target(canonical).is_some() && !in_zone_tab {
        if zone.starts_with("Etc/") {
            zone.to_string()
        } else {
            let countries = (country_of(canonical), country_of(zone));
            match countries {
                (Some(id), Some(zone_cc)) if id == zone_cc => zone.to_string(),
                (Some(id), _) => {
                    let rows: Vec<&str> = ZONE_TAB
                        .iter()
                        .filter(|(code, _)| *code == id)
                        .map(|(_, tz)| *tz)
                        .collect();
                    if rows.len() == 1 {
                        rows[0].to_string()
                    } else {
                        BACKZONE_LINKS
                            .iter()
                            .find(|(link, _)| *link == canonical)
                            .map(|(_, target)| target.to_string())?
                    }
                }
                _ => return None,
            }
        }
    } else {
        canonical.to_string()
    };
    if primary == "Etc/UTC" || primary == "Etc/GMT" || primary == "GMT" {
        primary = "UTC".into();
    }
    Some(primary)
}
fn time_range() -> VmErr {
    VmErr::Msg("RangeError: Invalid time value".into())
}
fn canonicalize_time_zone(name: &str) -> Result<String, VmErr> {
    // Offset zones use canonical "+HH:MM" spelling; a zero offset is "+00:00".
    if let Some((total, normalized)) = parse_offset_zone(name) {
        if total == 0 {
            return Ok("+00:00".into());
        }
        return Ok(normalized);
    }
    // Named zones keep their identifier with canonical casing: links and
    // zones alike are preserved in resolvedOptions (ECMA-402
    // GetAvailableNamedTimeZoneIdentifier); link resolution only affects
    // offset computation via computation_zone.
    canonical_name(name).map(str::to_string).ok_or_else(range)
}
/// Canonical supported `ca` Unicode extension of the first available
/// requested locale, if any.
fn ext_calendar(requested: &[String]) -> Option<String> {
    for tag in requested {
        if best_available_locale(tag).is_none() {
            continue;
        }
        if let Ok(loc) = tag.parse::<Locale>()
            && let Some(ca) = loc
                .extensions
                .unicode
                .keywords
                .get(&"ca".parse().unwrap())
                .map(ToString::to_string)
        {
            let canonical = canonical_calendar(&ca);
            if SUPPORTED_CALENDARS.contains(&canonical.as_str()) {
                return Some(canonical);
            }
        }
        break;
    }
    None
}
/// Valid `hc` Unicode extension of the first available requested locale, if
/// any.
fn ext_hour_cycle(requested: &[String]) -> Option<String> {
    for tag in requested {
        if best_available_locale(tag).is_none() {
            continue;
        }
        if let Ok(loc) = tag.parse::<Locale>()
            && let Some(hc) = loc
                .extensions
                .unicode
                .keywords
                .get(&"hc".parse().unwrap())
                .map(ToString::to_string)
        {
            match hc.as_str() {
                "h11" | "h12" | "h23" | "h24" => return Some(hc),
                _ => {}
            }
        }
        break;
    }
    None
}
/// Language subtag of a resolved base locale, defaulting to "en".
fn language_of(base: &str) -> String {
    base.parse::<Locale>()
        .map(|loc| loc.id.language.to_string())
        .unwrap_or_else(|_| "en".into())
}
/// Canonical identifier for computation: offset zones stay fixed offsets,
/// named zones resolve to their primary identifier.
fn computation_zone(name: &str) -> String {
    if parse_offset_zone(name).is_some() {
        return name.into();
    }
    primary_identifier(name).unwrap_or_else(|| name.into())
}
/// Seconds east of UTC for `zone` at `epoch_ms`, plus whether the zone
/// observes daylight saving at that instant.
fn zone_offset(zone: &str, epoch_ms: i64) -> Result<i32, VmErr> {
    if let Some((total, _)) = parse_offset_zone(zone) {
        return Ok(total * 60);
    }
    let canonical = computation_zone(zone);
    if canonical.eq_ignore_ascii_case("UTC") {
        return Ok(0);
    }
    let tz: chrono_tz::Tz = canonical
        .parse()
        .map_err(|_| VmErr::Msg("RangeError: Invalid time zone".into()))?;
    let seconds = epoch_ms.div_euclid(1000);
    let naive = chrono::DateTime::from_timestamp(seconds, 0)
        .map(|dt| dt.naive_utc())
        .ok_or_else(range)?;
    use chrono::{Offset, TimeZone as _};
    Ok(tz.offset_from_utc_datetime(&naive).fix().local_minus_utc())
}
fn style_length(style: &str) -> Length {
    match style {
        "full" | "long" => Length::Long,
        "medium" => Length::Medium,
        _ => Length::Short,
    }
}
fn component_length(p: &Parts) -> Length {
    let mut widths = Vec::new();
    for w in [&p.weekday, &p.era, &p.month, &p.day_period] {
        if let Some(w) = w
            && matches!(w.as_str(), "long" | "short" | "narrow")
        {
            widths.push(w.as_str());
        }
    }
    if widths.is_empty() {
        Length::Short
    } else if widths.iter().all(|w| *w == "long") {
        Length::Long
    } else {
        Length::Medium
    }
}
fn zone_style(name: &str) -> ZoneStyle {
    match name {
        "long" => ZoneStyle::SpecificLong,
        "longOffset" => ZoneStyle::LocalizedOffsetLong,
        "longGeneric" => ZoneStyle::GenericLong,
        "shortOffset" => ZoneStyle::LocalizedOffsetShort,
        "shortGeneric" => ZoneStyle::GenericShort,
        _ => ZoneStyle::SpecificShort,
    }
}
fn any_calendar_kind(calendar: &str) -> AnyCalendarKind {
    match calendar {
        "buddhist" => AnyCalendarKind::Buddhist,
        "chinese" => AnyCalendarKind::Chinese,
        "coptic" => AnyCalendarKind::Coptic,
        "dangi" => AnyCalendarKind::Dangi,
        "ethioaa" => AnyCalendarKind::EthiopianAmeteAlem,
        "ethiopic" => AnyCalendarKind::Ethiopian,
        "hebrew" => AnyCalendarKind::Hebrew,
        "indian" => AnyCalendarKind::Indian,
        "islamic-civil" => AnyCalendarKind::HijriTabularTypeIIFriday,
        "islamic-tbla" => AnyCalendarKind::HijriTabularTypeIIThursday,
        "islamic-umalqura" => AnyCalendarKind::HijriUmmAlQura,
        "iso8601" => AnyCalendarKind::Iso,
        "japanese" => AnyCalendarKind::Japanese,
        "persian" => AnyCalendarKind::Persian,
        "roc" => AnyCalendarKind::Roc,
        _ => AnyCalendarKind::Gregorian,
    }
}
fn subsecond_digits(n: u8) -> SubsecondDigits {
    match n {
        1 => SubsecondDigits::S1,
        2 => SubsecondDigits::S2,
        _ => SubsecondDigits::S3,
    }
}
fn time_clip(value: f64) -> f64 {
    if !value.is_finite() || value.abs() > 8.64e15 {
        f64::NAN
    } else {
        value.trunc() + 0.
    }
}
/// Proleptic Gregorian civil date for `days` since the unix epoch.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    ((y + (m <= 2) as i64) as i32, m, d)
}
/// Temporal input class for format adaptation and range comparison.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TemporalClass {
    Date,
    Time,
    DateTime,
    MonthDay,
    YearMonth,
    // Note: ZonedDateTime is rejected during resolution, so it needs no tag.
    Instant,
}
/// A resolved formattable moment: ISO calendar fields plus the target
/// calendar, or `None` when the input must throw.
struct Moment {
    kind: AnyCalendarKind,
    stamp: i64,
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    ms: u16,
    /// Temporal input class and the Temporal object's own calendar, if any.
    tclass: Option<TemporalClass>,
    tcalendar: Option<String>,
}
/// Field compatibility between a Temporal input class and the resolved
/// format. The format must overlap the type in at least one explicit field
/// or applicable style; anything outside the overlap is silently dropped.
/// A lone dateStyle/timeStyle needs a matching side, while with both
/// styles present the inapplicable one is ignored.
fn ensure_temporal_compatible(s: &Slots, class: TemporalClass) -> Result<(), VmErr> {
    // Date fields each Temporal class renders.
    let date_overlap = |name: &str| match class {
        TemporalClass::Date | TemporalClass::DateTime | TemporalClass::Instant => true,
        TemporalClass::YearMonth => matches!(name, "era" | "year" | "month"),
        TemporalClass::MonthDay => matches!(name, "month" | "day"),
        TemporalClass::Time => false,
    };
    let time_overlap = matches!(
        class,
        TemporalClass::DateTime | TemporalClass::Instant | TemporalClass::Time
    );
    let mut overlap = false;
    for (name, field) in [
        ("weekday", &s.weekday),
        ("era", &s.era),
        ("year", &s.year),
        ("month", &s.month),
        ("day", &s.day),
    ] {
        if field.is_some() && date_overlap(name) {
            overlap = true;
        }
    }
    if time_overlap
        && (s.day_period.is_some()
            || s.hour.is_some()
            || s.minute.is_some()
            || s.second.is_some()
            || s.fractional.is_some())
    {
        overlap = true;
    }
    // DateStyle overlaps date-capable types (or a present timeStyle, which
    // absorbs it); timeStyle overlaps time-capable types (or a present
    // dateStyle). Era overlaps date-capable types like any date field;
    // era-only formats take the default fill and adapt instead of throwing.
    let date_capable = !matches!(class, TemporalClass::Time);
    if s.date_style.is_some() && (date_capable || s.time_style.is_some()) {
        overlap = true;
    }
    if s.time_style.is_some() && (time_overlap || s.date_style.is_some()) {
        overlap = true;
    }
    if !overlap {
        if !date_capable || (!time_overlap && s.date_style.is_none()) {
            return Err(VmErr::Msg(
                "TypeError: date fields cannot format a dateless Temporal object".into(),
            ));
        }
        return Err(VmErr::Msg(
            "TypeError: time fields cannot format a timeless Temporal object".into(),
        ));
    }
    Ok(())
}
fn temporal_kind(calendar_id: &str) -> Option<AnyCalendarKind> {
    match calendar_id {
        "buddhist" => Some(AnyCalendarKind::Buddhist),
        "chinese" => Some(AnyCalendarKind::Chinese),
        "coptic" => Some(AnyCalendarKind::Coptic),
        "dangi" => Some(AnyCalendarKind::Dangi),
        "ethiopic" => Some(AnyCalendarKind::Ethiopian),
        "ethioaa" => Some(AnyCalendarKind::EthiopianAmeteAlem),
        "gregory" => Some(AnyCalendarKind::Gregorian),
        "hebrew" => Some(AnyCalendarKind::Hebrew),
        "indian" => Some(AnyCalendarKind::Indian),
        "islamic" => Some(AnyCalendarKind::HijriTabularTypeIIFriday),
        "islamic-civil" => Some(AnyCalendarKind::HijriTabularTypeIIFriday),
        "islamic-tbla" => Some(AnyCalendarKind::HijriTabularTypeIIThursday),
        "islamic-umalqura" => Some(AnyCalendarKind::HijriUmmAlQura),
        "iso8601" => None,
        "japanese" => Some(AnyCalendarKind::Japanese),
        "persian" => Some(AnyCalendarKind::Persian),
        "roc" => Some(AnyCalendarKind::Roc),
        _ => None,
    }
}
/// Resolve a `format` argument to ISO fields plus the calendar to render
/// with. Plain Temporal objects keep their own calendar; everything else
/// renders with the formatter's calendar.
fn resolve_moment(i: &mut Interpreter, s: &Slots, value: Value) -> Result<Moment, VmErr> {
    use super::super::temporal::Slots as Temporal;
    // format/formatToParts default a missing argument to now.
    if matches!(value, Value::Undefined) {
        return epoch_moment(s, super::super::date::now_ms() as i64);
    }
    let temporal = value
        .property_cell()
        .and_then(|p| p.meta.borrow().temporal.clone());
    if let Some(t) = temporal {
        let kind_of = |id: &str| temporal_kind(id).unwrap_or(s.calendar_kind);
        match t {
            Temporal::Zoned(_) | Temporal::Duration(_) => {
                return Err(VmErr::Msg(
                    "TypeError: value cannot be formatted as a date".into(),
                ));
            }
            Temporal::Instant(instant) => {
                let mut moment = epoch_moment(s, instant.epoch_milliseconds())?;
                moment.tclass = Some(TemporalClass::Instant);
                return Ok(moment);
            }
            Temporal::Date(date) => {
                if !s.defaults {
                    ensure_temporal_compatible(s, TemporalClass::Date)?;
                }
                let iso = date
                    .to_plain_date_time(None)
                    .map_err(|e| VmErr::Msg(e.to_string()))?;
                return Ok(Moment {
                    kind: kind_of(date.calendar().identifier()),
                    stamp: stamp_of(iso.iso_year(), iso.iso_month(), iso.iso_day()),
                    year: iso.iso_year(),
                    month: iso.iso_month(),
                    day: iso.iso_day(),
                    hour: 0,
                    minute: 0,
                    second: 0,
                    ms: 0,
                    tclass: Some(TemporalClass::Date),
                    tcalendar: Some(date.calendar().identifier().into()),
                });
            }
            Temporal::Time(time) => {
                if !s.defaults {
                    ensure_temporal_compatible(s, TemporalClass::Time)?;
                }
                return Ok(Moment {
                    kind: s.calendar_kind,
                    stamp: stamp_of(1970, 1, 1),
                    year: 1970,
                    month: 1,
                    day: 1,
                    hour: time.hour(),
                    minute: time.minute(),
                    second: time.second(),
                    ms: time.millisecond(),
                    tclass: Some(TemporalClass::Time),
                    tcalendar: None,
                });
            }
            Temporal::DateTime(datetime) => {
                return Ok(Moment {
                    kind: kind_of(datetime.calendar().identifier()),
                    stamp: stamp_of(
                        datetime.iso_year(),
                        datetime.iso_month(),
                        datetime.iso_day(),
                    ),
                    year: datetime.iso_year(),
                    month: datetime.iso_month(),
                    day: datetime.iso_day(),
                    hour: datetime.hour(),
                    minute: datetime.minute(),
                    second: datetime.second(),
                    ms: datetime.millisecond(),
                    tclass: Some(TemporalClass::DateTime),
                    tcalendar: Some(datetime.calendar().identifier().into()),
                });
            }
            Temporal::YearMonth(ym) => {
                if !s.defaults {
                    ensure_temporal_compatible(s, TemporalClass::YearMonth)?;
                }
                return Ok(Moment {
                    kind: kind_of(ym.calendar().identifier()),
                    stamp: stamp_of(ym.year(), ym.month(), 1),
                    year: ym.year(),
                    month: ym.month(),
                    day: 1,
                    hour: 0,
                    minute: 0,
                    second: 0,
                    ms: 0,
                    tclass: Some(TemporalClass::YearMonth),
                    tcalendar: Some(ym.calendar().identifier().into()),
                });
            }
            Temporal::MonthDay(md) => {
                // Month codes look like "M08" (leap months append "L").
                let code = md.month_code().as_str().to_string();
                let month: u8 = code
                    .bytes()
                    .skip_while(|b| !b.is_ascii_digit())
                    .take(2)
                    .collect::<Vec<_>>()
                    .as_slice()
                    .try_into()
                    .ok()
                    .and_then(|pair: &[u8; 2]| core::str::from_utf8(pair).ok()?.parse().ok())
                    .unwrap_or(1);
                if !s.defaults {
                    ensure_temporal_compatible(s, TemporalClass::MonthDay)?;
                }
                return Ok(Moment {
                    kind: kind_of(md.calendar().identifier()),
                    stamp: stamp_of(1972, month, md.day()),
                    year: 1972,
                    month,
                    day: md.day(),
                    hour: 0,
                    minute: 0,
                    second: 0,
                    ms: 0,
                    tclass: Some(TemporalClass::MonthDay),
                    tcalendar: Some(md.calendar().identifier().into()),
                });
            }
        }
    }
    if let Value::Date(slot) = &value {
        let ms = slot.get();
        if ms.is_nan() {
            return Err(time_range());
        }
        return epoch_moment(s, ms as i64);
    }
    let primitive = match &value {
        Value::Object { .. } => i.coerce_object_to_primitive(&value, "number")?,
        other => other.clone(),
    };
    let ms = time_clip(i.ecmascript_to_number(&primitive)?);
    if ms.is_nan() {
        return Err(time_range());
    }
    epoch_moment(s, ms as i64)
}
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = year as i64 - (month <= 2) as i64;
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
fn stamp_of(year: i32, month: u8, day: u8) -> i64 {
    days_from_civil(year, month as u32, day as u32) * 86_400 + 43_200
}
use icu_calendar::{Date as CalDate, Iso};
use icu_time::zone::{UtcOffset, ZoneNameTimestamp};
use icu_time::{Time, TimeZone, ZonedDateTime};
use std::fmt;
use writeable::{Part, PartsWrite, Writeable};
#[derive(Default)]
struct Collector {
    text: String,
    spans: Vec<(Part, usize, usize)>,
}
impl fmt::Write for Collector {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.text.push_str(s);
        Ok(())
    }
}
impl PartsWrite for Collector {
    type SubPartsWrite = Self;
    fn with_part(
        &mut self,
        part: Part,
        mut f: impl FnMut(&mut Self::SubPartsWrite) -> fmt::Result,
    ) -> fmt::Result {
        let start = self.text.len();
        f(self)?;
        self.spans.push((part, start, self.text.len()));
        Ok(())
    }
}
fn build_input(
    s: &Slots,
    m: &Moment,
) -> Result<ZonedDateTime<AnyCalendar, icu_time::TimeZoneInfo<icu_time::zone::models::AtTime>>, VmErr>
{
    // ICU constructors cap years at +/-9999. Beyond that, format through a
    // probe year on the same 400-year proleptic cycle (same leap status,
    // weekday, and era side); collect_parts substitutes the true year.
    let iso_year = if m.year > 9999 || m.year < -9999 {
        if m.year >= 1 {
            2000 + (m.year - 2000).rem_euclid(400)
        } else {
            let probe = (m.year % 400 + 400) % 400 - 399;
            if probe == 0 { -400 } else { probe }
        }
    } else {
        m.year
    };
    let iso: CalDate<Iso> =
        CalDate::try_new_iso(iso_year, m.month, m.day).map_err(|_| time_range())?;
    let date = iso.to_calendar(AnyCalendar::new(m.kind));
    let time = Time::try_new(m.hour, m.minute, m.second, m.ms as u32 * 1_000_000)
        .map_err(|_| time_range())?;
    let zone_id = computation_zone(&s.time_zone);
    let offset = zone_offset(&s.time_zone, m.stamp * 1000).unwrap_or(0);
    let zone = TimeZone::from_iana_id(&zone_id)
        .with_offset(Some(
            UtcOffset::try_from_seconds(offset).map_err(|_| time_range())?,
        ))
        .with_zone_name_timestamp(ZoneNameTimestamp::from_epoch_seconds(m.stamp));
    Ok(ZonedDateTime { date, time, zone })
}
fn ecma_type(value: &str) -> &str {
    match value {
        "era" => "era",
        "year" => "year",
        "relatedYear" => "relatedYear",
        "month" => "month",
        "day" => "day",
        "weekday" => "weekday",
        "dayPeriod" => "dayPeriod",
        "hour" => "hour",
        "minute" => "minute",
        "second" => "second",
        "timeZoneName" => "timeZoneName",
        "yearName" => "yearName",
        _ => "literal",
    }
}
/// Display year for proleptic Gregorian output: years on the BC side render
/// one-based (year 0 is "1 BC").
fn display_year(year: i32) -> String {
    if year <= 0 {
        (1 - year).to_string()
    } else {
        year.to_string()
    }
}
/// Decimal separator for a time fraction under `numbering`: arab-style
/// systems use U+066B, everything else keeps the formatter's separator.
fn fraction_separator(numbering: &str) -> Option<char> {
    match numbering {
        "arab" | "arabext" => Some('\u{066b}'),
        _ => None,
    }
}
/// Rule-based English day-period name for `hour` (CLDR dayPeriod data as
/// observed through the reference implementation), or `None` when the
/// locale is not English.
fn day_period_name(locale: &str, width: &str, hour: u8) -> Option<String> {
    let language = locale
        .parse::<Locale>()
        .map(|loc| loc.id.language.to_string())
        .unwrap_or_default();
    if language != "en" {
        return None;
    }
    let name = match hour {
        12 if width == "narrow" => "n",
        12 => "noon",
        0..=11 => "in the morning",
        13..=17 => "in the afternoon",
        18..=20 => "in the evening",
        _ => "at night",
    };
    Some(name.into())
}
/// Zero digit for padding under `numbering`, from the digit map when the
/// formatter remaps, from CLDR digit data for natively formatted systems,
/// or ASCII zero as a last resort.
fn zero_for(numbering: &str, digit_map: Option<&Vec<char>>) -> char {
    if let Some(digits) = digit_map {
        return digits[0];
    }
    super::number::digit_map(numbering)
        .and_then(|digits| digits.into_iter().next())
        .unwrap_or('0')
}
fn pad2(text: &str, two_digit: bool, zero: char) -> String {
    let chars: Vec<char> = text.chars().collect();
    if two_digit {
        if chars.len() == 1 {
            return format!("{zero}{text}");
        }
    } else if chars.len() == 2 && chars[0] == zero {
        return chars[1].to_string();
    }
    text.into()
}
/// Format one moment into ECMA-402 `(type, value)` parts.
fn collect_parts(s: &Slots, m: &Moment) -> Result<Vec<(String, String)>, VmErr> {
    let input = build_input(s, m)?;
    let mut sink = Collector::default();
    s.formatter
        .format(&input)
        .write_to_parts(&mut sink)
        .map_err(|_| range())?;
    let mut spans: Vec<(usize, usize, &str)> = sink
        .spans
        .iter()
        .filter(|(part, _, _)| part.category == "datetime")
        .map(|(part, start, end)| (*start, *end, part.value))
        .collect();
    spans.sort();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut cursor = 0;
    for (start, end, value) in spans {
        if start < cursor {
            continue;
        }
        if start > cursor {
            out.push(("literal".into(), sink.text[cursor..start].into()));
        }
        out.push((ecma_type(value).into(), sink.text[start..end].into()));
        cursor = end;
    }
    if cursor < sink.text.len() {
        out.push(("literal".into(), sink.text[cursor..].into()));
    }
    // Split seconds carrying a fraction into second, separator, fraction.
    // The fraction may already carry localized digits (ICU formats natively
    // for supported numbering systems), so split on the separator itself.
    let mut split = Vec::new();
    for (kind, text) in out {
        if kind == "second"
            && let Some(dot) = text.rfind(['.', ',', '\u{066b}'])
        {
            let (head, tail) = text.split_at(dot);
            let mut chars = tail.chars();
            let sep = chars.next().unwrap_or('.').to_string();
            let frac: String = chars.collect();
            if !head.is_empty() && !frac.is_empty() && frac.chars().all(|c| c.is_alphanumeric()) {
                split.push(("second".into(), head.into()));
                split.push(("literal".into(), sep));
                split.push(("fractionalSecond".into(), frac));
                continue;
            }
        }
        split.push((kind, text));
    }
    let mut out = split;
    // Proleptic Gregorian years: a numeric year shows the full year (the
    // ICU short pattern truncates to two digits), and out-of-range probe
    // years are replaced with the true display year. A numeric month the
    // ICU pattern renders as text (short patterns with era) is likewise
    // replaced with the month number.
    let gregorian = matches!(s.calendar.as_str(), "gregory" | "iso8601");
    let huge = m.year > 9999 || m.year < -9999;
    if gregorian && (huge || s.year.as_deref() == Some("numeric")) {
        let full = display_year(m.year);
        for (kind, text) in &mut out {
            if kind == "year" {
                *text = full.clone();
            }
        }
    }
    if gregorian && matches!(s.month.as_deref(), Some("numeric") | Some("2-digit")) {
        let zero = zero_for(&s.numbering, s.digit_map.as_ref());
        for (kind, text) in &mut out {
            if kind == "month" && !text.bytes().all(|b| b.is_ascii_digit()) {
                *text = pad2(
                    &m.month.to_string(),
                    s.month.as_deref() == Some("2-digit"),
                    zero,
                );
            }
        }
    }
    if s.strip_time_digits {
        // Day-period-only formatting renders the rule-based day period name.
        if let Some(width) = s.day_period.as_deref()
            && let Some(name) = day_period_name(&s.locale, width, m.hour)
        {
            return Ok(vec![("dayPeriod".into(), name)]);
        }
        out.retain(|(kind, _)| {
            !matches!(
                kind.as_str(),
                "hour" | "minute" | "second" | "fractionalSecond"
            )
        });
        let joined = out
            .iter()
            .map(|(_, text)| text.as_str())
            .collect::<String>();
        let trimmed = joined.split_whitespace().collect::<Vec<_>>().join(" ");
        return Ok(if trimmed.is_empty() {
            Vec::new()
        } else {
            vec![("literal".into(), trimmed)]
        });
    }
    // Drop fieldset padding (month-only/year-only dates paired with time).
    {
        let mut kinds = Vec::new();
        if s.omit_day {
            kinds.push("day");
        }
        if s.omit_month {
            kinds.push("month");
        }
        if s.omit_year {
            kinds.push("year");
        }
        if !kinds.is_empty() {
            out = drop_kinds(out, &kinds);
        }
    }
    // Without an hour option, minute/second stand alone: drop the hour span.
    // A requested dayPeriod is kept for rule substitution below.
    if s.hour.is_none()
        && s.time_style.is_none()
        && (s.minute.is_some() || s.second.is_some() || s.fractional.is_some())
    {
        if s.day_period.is_some() {
            out = drop_kinds(out, &["hour"]);
        } else {
            out = drop_kinds(out, &["hour", "dayPeriod"]);
        }
    }
    // Day period handling: a dayPeriod option selects rule-based day period
    // names, and dayPeriod-adjacent separators normalize to a plain space
    // (pinned expectations predate CLDR's narrow no-break space).
    let rule_name = s
        .day_period
        .as_deref()
        .and_then(|width| day_period_name(&s.locale, width, m.hour));
    for index in 0..out.len() {
        if out[index].0 != "dayPeriod" {
            continue;
        }
        if let Some(name) = rule_name.as_deref() {
            out[index].1 = name.into();
        }
        if index > 0 && out[index - 1].0 == "literal" && out[index - 1].1 == "\u{202f}" {
            out[index - 1].1 = " ".into();
        }
        if index + 1 < out.len() && out[index + 1].0 == "literal" && out[index + 1].1 == "\u{202f}"
        {
            out[index + 1].1 = " ".into();
        }
    }
    // Temporal classes render only their overlap: anything outside it is
    // silently dropped (explicit fields, inapplicable style blocks, and
    // the time zone name zoneless types cannot carry). Bare defaults adapt
    // the same way, narrowing to the input class.
    {
        let mut kinds: Vec<&str> = Vec::new();
        match m.tclass {
            // Plain dates and non-Temporal defaults show the date block.
            None if s.defaults => {
                kinds.extend(["dayPeriod", "hour", "minute", "second", "fractionalSecond"])
            }
            None => {}
            Some(TemporalClass::Date) => {
                kinds.extend(["dayPeriod", "hour", "minute", "second", "fractionalSecond"])
            }
            Some(TemporalClass::Time) => kinds.extend([
                "weekday",
                "era",
                "year",
                "month",
                "day",
                "relatedYear",
                "yearName",
            ]),
            Some(TemporalClass::MonthDay) => kinds.extend([
                "weekday",
                "era",
                "year",
                "relatedYear",
                "yearName",
                "dayPeriod",
                "hour",
                "minute",
                "second",
                "fractionalSecond",
            ]),
            Some(TemporalClass::YearMonth) => kinds.extend([
                "weekday",
                "day",
                "dayPeriod",
                "hour",
                "minute",
                "second",
                "fractionalSecond",
            ]),
            Some(TemporalClass::DateTime) | Some(TemporalClass::Instant) => {}
        }
        if matches!(
            m.tclass,
            Some(
                TemporalClass::Date
                    | TemporalClass::Time
                    | TemporalClass::DateTime
                    | TemporalClass::MonthDay
                    | TemporalClass::YearMonth
            )
        ) {
            kinds.push("timeZoneName");
        }
        if !kinds.is_empty() {
            out = drop_kinds(out, &kinds);
        }
    }
    // Localize the fraction separator for arab-style numbering systems.
    if let Some(sep) = fraction_separator(&s.numbering) {
        for index in 0..out.len() {
            if out[index].0 == "fractionalSecond"
                && index > 0
                && out[index - 1].0 == "literal"
                && out[index - 1].1 == "."
            {
                out[index - 1].1 = sep.to_string();
            }
        }
    }
    // Two-digit years show the last two digits, zero-padded.
    if s.truncate_year {
        for (kind, text) in &mut out {
            if kind == "year" && text.bytes().all(|b| b.is_ascii_digit()) {
                let short = if text.len() > 2 {
                    text[text.len() - 2..].to_string()
                } else {
                    text.clone()
                };
                *text = if short.len() < 2 {
                    format!("0{short}")
                } else {
                    short
                };
            }
        }
    }
    // Width surgery for month/day/hour plus digit remapping.
    let widths = [
        ("month", s.month.as_deref()),
        ("day", s.day.as_deref()),
        ("hour", s.hour.as_deref()),
    ];
    let zero = zero_for(&s.numbering, s.digit_map.as_ref());
    // Remap substituted ASCII digits even when ICU formats natively: the
    // CLDR digit data covers those numbering systems too (mapping only
    // touches ASCII digits, so native output is unaffected).
    let native_digits;
    let digits = match s.digit_map.as_ref() {
        Some(map) => Some(map),
        None => {
            native_digits = super::number::digit_map(&s.numbering);
            native_digits.as_ref()
        }
    };
    for (kind, text) in &mut out {
        // h24 formats through h23: midnight renders as 24.
        if kind == "hour" && s.hour_cycle.as_deref() == Some("h24") && m.hour == 0 {
            *text = "24".into();
        }
        if let Some((_, width)) = widths.iter().find(|(k, _)| k == kind)
            && let Some(width) = width
            && (kind != "hour" || *width == "2-digit")
        {
            *text = pad2(text, *width == "2-digit", zero);
        }
        if matches!(
            kind.as_str(),
            "year" | "month" | "day" | "hour" | "minute" | "second" | "fractionalSecond"
        ) && text.bytes().all(|b| b.is_ascii_digit())
            && let Some(digits) = digits
        {
            *text = super::relative::map_digit_text(text, digits);
        }
    }
    Ok(out)
}
fn join_parts(parts: &[(String, String)]) -> String {
    parts.iter().map(|(_, text)| text.as_str()).collect()
}
fn unwrap(i: &mut Interpreter, this: Value) -> Result<Value, VmErr> {
    if slot(&this).is_ok() {
        return Ok(this);
    }
    let ctor = i
        .persistent_global
        .borrow()
        .intrinsic("Intl.DateTimeFormat")
        .unwrap();
    if i.ordinary_instance_of(&this, &ctor)?.is_truthy() {
        let symbol = i
            .persistent_global
            .borrow()
            .intrinsic("Intl.FallbackSymbol")
            .unwrap();
        return i.get_prop_value(&this, &symbol);
    }
    Err(error())
}
fn resolved_options(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let this = unwrap(i, this)?;
    let s = slot(&this)?;
    let mut fields = vec![
        ("locale".into(), Value::String(s.locale.into())),
        ("calendar".into(), Value::String(s.calendar.into())),
        ("numberingSystem".into(), Value::String(s.numbering.into())),
        ("timeZone".into(), Value::String(s.time_zone.into())),
    ];
    if let Some(hour_cycle) = s.hour_cycle {
        fields.push(("hourCycle".into(), Value::String(hour_cycle.into())));
    }
    if let Some(hour12) = s.hour12 {
        fields.push(("hour12".into(), Value::Bool(hour12)));
    }
    for (key, value) in [
        ("weekday", &s.weekday),
        ("era", &s.era),
        ("year", &s.year),
        ("month", &s.month),
        ("day", &s.day),
        ("dayPeriod", &s.day_period),
        ("hour", &s.hour),
        ("minute", &s.minute),
        ("second", &s.second),
        ("timeZoneName", &s.time_zone_name),
    ] {
        if let Some(value) = value {
            fields.push((key.into(), Value::String(value.clone().into())));
        }
    }
    if let Some(fractional) = s.fractional {
        fields.push((
            "fractionalSecondDigits".into(),
            Value::Number(fractional as f64),
        ));
    }
    if let Some(date_style) = s.date_style {
        fields.push(("dateStyle".into(), Value::String(date_style.into())));
    }
    if let Some(time_style) = s.time_style {
        fields.push(("timeStyle".into(), Value::String(time_style.into())));
    }
    Ok(Value::object(fields))
}
fn format(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let this = unwrap(i, this)?;
    let s = slot(&this)?;
    let moment = resolve_moment(i, &s, arg(&args, 0))?;
    Ok(Value::String(
        join_parts(&collect_parts(&s, &moment)?).into(),
    ))
}
/// Field forcing for the `Date.prototype.toLocale*` methods, which share
/// the DateTimeFormat machinery with required/defaults presets.
pub(crate) enum DateLocaleKind {
    DateTime,
    DateOnly,
    TimeOnly,
}
pub(crate) fn temporal_to_locale(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    use super::super::temporal::Slots as Temporal;
    let temporal = this
        .property_cell()
        .and_then(|p| p.meta.borrow().temporal.clone())
        .ok_or_else(error)?;
    let requested = locale_list(i, arg(&args, 0))?;
    let options = match arg(&args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        value => super::super::object::to_object_receiver(&value)?,
    };
    let mut raw = read_raw_options(i, &options)?;
    if matches!(&temporal, Temporal::Time(_)) && raw.date_style.is_some()
        || matches!(
            &temporal,
            Temporal::Date(_) | Temporal::YearMonth(_) | Temporal::MonthDay(_)
        ) && raw.time_style.is_some()
    {
        return Err(VmErr::Msg(
            "TypeError: style cannot format this Temporal type".into(),
        ));
    }
    if let Temporal::Zoned(zoned) = &temporal {
        if raw.time_zone.is_some() {
            return Err(VmErr::Msg(
                "TypeError: timeZone option is not allowed for ZonedDateTime".into(),
            ));
        }
        raw.time_zone = Some(
            zoned
                .time_zone()
                .identifier_with_provider(&*temporal_rs::provider::COMPILED_TZ_PROVIDER)
                .map_err(|e| VmErr::Msg(e.to_string()))?,
        );
        let no_fields = raw.weekday.is_none()
            && raw.year.is_none()
            && raw.month.is_none()
            && raw.day.is_none()
            && raw.day_period.is_none()
            && raw.hour.is_none()
            && raw.minute.is_none()
            && raw.second.is_none()
            && raw.fractional.is_none();
        if no_fields
            && raw.date_style.is_none()
            && raw.time_style.is_none()
            && raw.time_zone_name.is_none()
        {
            raw.time_zone_name = Some("short".into());
        }
    }
    let s = resolve_parts(&requested, raw, false, false)?;
    let calendar = match &temporal {
        Temporal::Date(value) => Some(value.calendar().identifier()),
        Temporal::DateTime(value) => Some(value.calendar().identifier()),
        Temporal::YearMonth(value) => Some(value.calendar().identifier()),
        Temporal::MonthDay(value) => Some(value.calendar().identifier()),
        Temporal::Zoned(value) => Some(value.calendar().identifier()),
        _ => None,
    };
    if let Some(calendar) = calendar {
        let iso_allowed = !matches!(&temporal, Temporal::YearMonth(_) | Temporal::MonthDay(_));
        if canonical_calendar(calendar) != s.calendar && !(iso_allowed && calendar == "iso8601") {
            return Err(VmErr::Msg(
                "RangeError: Temporal calendar does not match formatter calendar".into(),
            ));
        }
    }
    let mut moment = if let Temporal::Zoned(value) = &temporal {
        let mut moment = epoch_moment(&s, value.epoch_milliseconds())?;
        moment.tclass = Some(TemporalClass::Instant);
        moment
    } else {
        resolve_moment(i, &s, this)?
    };
    // ISO plain values may be rendered in the locale's chosen calendar.
    moment.kind = s.calendar_kind;
    Ok(Value::String(
        join_parts(&collect_parts(&s, &moment)?).into(),
    ))
}

pub(crate) fn date_to_locale(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
    kind: DateLocaleKind,
) -> Result<Value, VmErr> {
    let name = match kind {
        DateLocaleKind::DateTime => "toLocaleString",
        DateLocaleKind::DateOnly => "toLocaleDateString",
        DateLocaleKind::TimeOnly => "toLocaleTimeString",
    };
    let Value::Date(slot) = &this else {
        return Err(VmErr::Msg(format!(
            "TypeError: Date.prototype.{name} called on incompatible receiver"
        )));
    };
    let ms = slot.get();
    if ms.is_nan() {
        return Ok(Value::String("Invalid Date".into()));
    }
    let requested = locale_list(i, arg(&args, 0))?;
    let options = match arg(&args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        v => super::super::object::to_object_receiver(&v)?,
    };
    let raw = read_raw_options(i, &options)?;
    // Field forcing mirrors CreateDateTimeFormat required/defaults: "any"
    // (toLocaleString) fills date+time only when both are absent, "date"
    // fills a missing date, "time" fills a missing time. Styles disable all
    // forcing (handled in resolve_parts).
    let has_date =
        raw.weekday.is_some() || raw.year.is_some() || raw.month.is_some() || raw.day.is_some();
    let has_time = raw.day_period.is_some()
        || raw.hour.is_some()
        || raw.minute.is_some()
        || raw.second.is_some()
        || raw.fractional.is_some();
    let (force_date, force_time) = match kind {
        DateLocaleKind::DateTime => (!has_date && !has_time, !has_date && !has_time),
        DateLocaleKind::DateOnly => (!has_date, false),
        DateLocaleKind::TimeOnly => (false, !has_time),
    };
    let slots = resolve_parts(&requested, raw, force_date, force_time)?;
    let moment = epoch_moment(&slots, ms as i64)?;
    Ok(Value::String(
        join_parts(&collect_parts(&slots, &moment)?).into(),
    ))
}
fn get_format(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let this = unwrap(i, this)?;
    slot(&this)?;
    let cell = this.property_cell().unwrap();
    if let Some(value) = &cell.meta.borrow().intl_bound_format {
        return Ok(value.clone());
    }
    let fp = i
        .persistent_global
        .borrow()
        .intrinsic("Function")
        .and_then(|f| f.get_prop("prototype"));
    let target = native_method("", 1, format, fp);
    let bound = super::super::function::function_bind(i, target, vec![this])?;
    {
        let p = bound.property_cell().unwrap();
        let mut p = p.borrow_mut();
        p.iter_mut().find(|(k, _)| k == "name").unwrap().1 = Value::String("".into());
        p.sort_by_key(|(k, _)| if k == "length" { 0 } else { 1 });
    }
    cell.meta.borrow_mut().intl_bound_format = Some(bound.clone());
    Ok(bound)
}
fn parts_array(parts: &[(String, String)], source: Option<&str>) -> Result<Value, VmErr> {
    let mut items = Vec::new();
    for (kind, text) in parts {
        let mut fields = vec![
            ("type".into(), Value::String(kind.clone().into())),
            ("value".into(), Value::String(text.clone().into())),
        ];
        if let Some(source) = source {
            fields.push(("source".into(), Value::String(source.into())));
        }
        items.push(Value::object_with_proto(fields, None));
    }
    Value::checked_array(items)
}
fn format_to_parts(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let this = unwrap(i, this)?;
    let s = slot(&this)?;
    let moment = resolve_moment(i, &s, arg(&args, 0))?;
    parts_array(&collect_parts(&s, &moment)?, None)
}
/// Calendar-field ranks for range collapsing: larger ranks collapse first.
fn field_ranks(m: &Moment) -> [i64; 6] {
    [
        m.year as i64,
        m.month as i64,
        m.day as i64,
        m.hour as i64,
        m.minute as i64,
        m.second as i64,
    ]
}
fn collapse_range(
    start: &[(String, String)],
    end: &[(String, String)],
    start_fields: [i64; 6],
    end_fields: [i64; 6],
) -> Vec<(String, String, &'static str)> {
    // Identical sides collapse to a single shared date.
    if start == end {
        return start
            .iter()
            .map(|(kind, text)| (kind.clone(), text.clone(), "shared"))
            .collect();
    }
    // Fully numeric patterns show both sides in full, as do sides with
    // different part structures (e.g. an emergent BC era on one side).
    let same_shape =
        start.len() == end.len() && start.iter().zip(end.iter()).all(|(a, b)| a.0 == b.0);
    let textual = start.iter().chain(end.iter()).any(|(kind, text)| {
        matches!(
            kind.as_str(),
            "month" | "weekday" | "era" | "dayPeriod" | "timeZoneName"
        ) && !text.bytes().all(|b| b.is_ascii_digit())
    });
    if !textual || !same_shape {
        let mut out = Vec::new();
        for (kind, text) in start {
            out.push((kind.clone(), text.clone(), "startRange"));
        }
        out.push(("literal".into(), "\u{2013}".into(), "shared"));
        for (kind, text) in end {
            out.push((kind.clone(), text.clone(), "endRange"));
        }
        return out;
    }
    // Largest differing field rank drives the per-side window: fields from
    // that rank down to the smallest shown field render on both sides, while
    // larger (equal) fields stay shared.
    let diff_rank = [0, 1, 2, 3, 4, 5]
        .into_iter()
        .find(|rank| start_fields[*rank] != end_fields[*rank]);
    let rank_of = |kind: &str| match kind {
        "year" | "era" | "relatedYear" | "yearName" => Some(0),
        "month" => Some(1),
        "weekday" | "day" => Some(2),
        "hour" | "dayPeriod" => Some(3),
        "minute" => Some(4),
        "second" | "fractionalSecond" => Some(5),
        _ => None,
    };
    let max_rank = start.iter().filter_map(|(kind, _)| rank_of(kind)).max();
    let (Some(diff_rank), Some(max_rank)) = (diff_rank, max_rank) else {
        return start
            .iter()
            .map(|(kind, text)| (kind.clone(), text.clone(), "shared"))
            .collect();
    };
    let in_window =
        |kind: &str| rank_of(kind).is_some_and(|rank| rank >= diff_rank && rank <= max_rank);
    let first = start.iter().position(|(kind, _)| in_window(kind));
    let last = start.iter().rposition(|(kind, _)| in_window(kind));
    let Some((first, last)) = first.zip(last) else {
        return start
            .iter()
            .map(|(kind, text)| (kind.clone(), text.clone(), "shared"))
            .collect();
    };
    let mut out = Vec::new();
    for (kind, text) in &start[..first] {
        out.push((kind.clone(), text.clone(), "shared"));
    }
    for (kind, text) in &start[first..=last] {
        out.push((kind.clone(), text.clone(), "startRange"));
    }
    out.push(("literal".into(), "\u{2013}".into(), "shared"));
    for (kind, text) in &end[first..=last] {
        out.push((kind.clone(), text.clone(), "endRange"));
    }
    for (kind, text) in &end[last + 1..] {
        out.push((kind.clone(), text.clone(), "shared"));
    }
    out
}
fn range_undefined(args: &[Value]) -> bool {
    matches!(arg(args, 0), Value::Undefined) || matches!(arg(args, 1), Value::Undefined)
}
/// Whether both range sides convert without a Temporal/non-Temporal split.
/// A Temporal brand on exactly one side is a TypeError, but only after both
/// sides convert (the plain side's valueOf runs first and may itself throw).
fn range_sides_compatible(i: &mut Interpreter, args: &[Value]) -> Result<bool, VmErr> {
    let brand = |value: &Value| {
        value
            .property_cell()
            .and_then(|cell| cell.meta.borrow().temporal.clone())
            .is_some()
    };
    let first = arg(args, 0);
    let second = arg(args, 1);
    if brand(&first) != brand(&second) {
        let plain = if brand(&first) { second } else { first };
        if matches!(plain, Value::Object { .. }) {
            let primitive = i.coerce_object_to_primitive(&plain, "number")?;
            i.ecmascript_to_number(&primitive)?;
        }
        return Ok(false);
    }
    Ok(true)
}
fn range_type_error(what: &str) -> VmErr {
    VmErr::Msg(format!("TypeError: {what} requires two dates"))
}
/// Pair compatibility after conversion: distinct Temporal types are a
/// TypeError, differing Temporal calendars a RangeError.
fn check_range_pair(start: &Moment, end: &Moment, what: &str) -> Result<(), VmErr> {
    match (&start.tclass, &end.tclass) {
        (Some(first), Some(second)) => {
            if first != second {
                return Err(VmErr::Msg(format!(
                    "TypeError: {what} requires matching Temporal types"
                )));
            }
            if start.tcalendar != end.tcalendar {
                return Err(VmErr::Msg(format!(
                    "RangeError: {what} requires matching Temporal calendars"
                )));
            }
            Ok(())
        }
        _ => {
            if start.kind != end.kind {
                return Err(VmErr::Msg(format!(
                    "TypeError: {what} requires matching calendars"
                )));
            }
            Ok(())
        }
    }
}
fn format_range(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let this = unwrap(i, this)?;
    let s = slot(&this)?;
    if range_undefined(&args) || !range_sides_compatible(i, &args)? {
        return Err(range_type_error("formatRange"));
    }
    let start = resolve_moment(i, &s, arg(&args, 0))?;
    let end = resolve_moment(i, &s, arg(&args, 1))?;
    check_range_pair(&start, &end, "formatRange")?;
    let start_parts = collect_parts(&s, &start)?;
    let end_parts = collect_parts(&s, &end)?;
    Ok(Value::String(
        join_parts(
            &collapse_range(
                &start_parts,
                &end_parts,
                field_ranks(&start),
                field_ranks(&end),
            )
            .into_iter()
            .map(|(kind, text, _)| (kind, text))
            .collect::<Vec<_>>(),
        )
        .into(),
    ))
}
fn format_range_to_parts(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = unwrap(i, this)?;
    let s = slot(&this)?;
    if range_undefined(&args) || !range_sides_compatible(i, &args)? {
        return Err(range_type_error("formatRangeToParts"));
    }
    let start = resolve_moment(i, &s, arg(&args, 0))?;
    let end = resolve_moment(i, &s, arg(&args, 1))?;
    check_range_pair(&start, &end, "formatRangeToParts")?;
    let start_parts = collect_parts(&s, &start)?;
    let end_parts = collect_parts(&s, &end)?;
    let collapsed = collapse_range(
        &start_parts,
        &end_parts,
        field_ranks(&start),
        field_ranks(&end),
    );
    let mut items = Vec::new();
    for (kind, text, source) in collapsed {
        items.push(Value::object_with_proto(
            vec![
                ("type".into(), Value::String(kind.into())),
                ("value".into(), Value::String(text.into())),
                ("source".into(), Value::String(source.into())),
            ],
            None,
        ));
    }
    Value::checked_array(items)
}
fn epoch_moment(s: &Slots, epoch_ms: i64) -> Result<Moment, VmErr> {
    let offset = zone_offset(&s.time_zone, epoch_ms).unwrap_or(0);
    let local = epoch_ms + offset as i64 * 1000;
    let (year, month, day) = civil_from_days(local.div_euclid(86_400_000));
    let rem = local.rem_euclid(86_400_000) as u32;
    Ok(Moment {
        kind: s.calendar_kind,
        stamp: epoch_ms.div_euclid(1000),
        year,
        month: month as u8,
        day: day as u8,
        hour: (rem / 3_600_000) as u8,
        minute: (rem / 60_000 % 60) as u8,
        second: (rem / 1000 % 60) as u8,
        ms: (rem % 1000) as u16,
        tclass: None,
        tcalendar: None,
    })
}
fn date_combo(p: &Parts) -> Option<DateFields> {
    let weekday = p.weekday.is_some();
    let year = p.year.is_some();
    let month = p.month.is_some();
    let day = p.day.is_some();
    match (weekday, year, month, day) {
        (false, false, false, false) => None,
        (true, false, false, false) => Some(DateFields::E),
        (false, false, false, true) => Some(DateFields::D),
        (false, false, true, true) => Some(DateFields::MD),
        (false, true, true, true) => Some(DateFields::YMD),
        (false, false, true, false) => Some(DateFields::M),
        (false, true, true, false) => Some(DateFields::YM),
        (false, true, false, false) => Some(DateFields::Y),
        (true, false, false, true) => Some(DateFields::DE),
        (true, false, true, true) => Some(DateFields::MDE),
        (true, true, true, true) => Some(DateFields::YMDE),
        (false, true, false, true) => Some(DateFields::YMD),
        // Sparse weekday combinations have no ICU field set; fall back to
        // the full date and let callers narrow the visible fields.
        (true, _, _, _) => Some(DateFields::YMDE),
    }
}
fn finish(mut p: Parts) -> Result<Slots, VmErr> {
    let has_time = p.hour.is_some()
        || p.minute.is_some()
        || p.second.is_some()
        || p.fractional.is_some()
        || p.day_period.is_some()
        || p.time_style.is_some();
    // Era alone does not count as a date field: like an empty field set it
    // takes the numeric default (and adapts to Temporal input classes).
    let has_date = p.weekday.is_some() || p.year.is_some() || p.month.is_some() || p.day.is_some();
    let defaults = p.date_style.is_none() && p.time_style.is_none() && !has_date && !has_time;
    if defaults {
        p.year = Some("numeric".into());
        p.month = Some("numeric".into());
        p.day = Some("numeric".into());
    }
    // An era without any other date field still formats a full date.
    if p.era.is_some()
        && p.year.is_none()
        && p.month.is_none()
        && p.day.is_none()
        && p.weekday.is_none()
    {
        p.year = Some("numeric".into());
        p.month = Some("numeric".into());
        p.day = Some("numeric".into());
    }
    // timeStyle full/long carry an implicit time zone name.
    if p.time_zone_name.is_none() {
        p.time_zone_name = match p.time_style.as_deref() {
            Some("full") => Some("long".into()),
            Some("long") => Some("short".into()),
            _ => None,
        };
    }
    let mut builder = FieldSetBuilder::new();
    if let Some(style) = &p.date_style {
        builder.date_fields = Some(if style == "full" {
            DateFields::YMDE
        } else {
            DateFields::YMD
        });
        builder.length = Some(style_length(style));
    } else if let Some(combo) = date_combo(&p) {
        builder.date_fields = Some(combo);
        builder.length = Some(component_length(&p));
    }
    builder.year_style = if p.era.is_some() && p.year.is_some() {
        Some(YearStyle::WithEra)
    } else {
        None
    };
    builder.time_precision = if p.time_style.is_some() {
        Some(match p.time_style.as_deref() {
            Some("short") => TimePrecision::Minute,
            _ => match p.fractional {
                Some(n) => TimePrecision::Subsecond(subsecond_digits(n)),
                None => TimePrecision::Second,
            },
        })
    } else if p.second.is_some() || p.fractional.is_some() {
        Some(match p.fractional {
            Some(n) => TimePrecision::Subsecond(subsecond_digits(n)),
            None => TimePrecision::Second,
        })
    } else if p.minute.is_some() {
        Some(TimePrecision::Minute)
    } else if p.hour.is_some() || p.day_period.is_some() {
        Some(TimePrecision::Hour)
    } else if defaults {
        // The bare-constructor default formats date and time together so
        // per-call adaptation can narrow to the Temporal input class.
        Some(TimePrecision::Second)
    } else {
        None
    };
    builder.zone_style = p.time_zone_name.as_deref().map(zone_style);
    // ICU cannot pair month-only or year-only dates with time: pad the
    // fieldset up to a buildable combo and drop the padding afterwards.
    let (omit_day, omit_month, omit_year) = if builder.time_precision.is_some() {
        match builder.date_fields {
            Some(DateFields::YM) | Some(DateFields::Y) => {
                builder.date_fields = Some(DateFields::YMD);
                (p.day.is_none(), p.month.is_none(), p.year.is_none())
            }
            Some(DateFields::M) => {
                builder.date_fields = Some(DateFields::MD);
                (p.day.is_none(), p.month.is_none(), p.year.is_none())
            }
            _ => (false, false, false),
        }
    } else {
        (false, false, false)
    };
    let fieldset = builder.build_composite().map_err(|_| range())?;
    // ICU preferences only model h11/h12/h23: format h24 through h23 and
    // rewrite the midnight hour afterwards.
    let prefs_cycle = if p.hour_cycle == "h24" {
        "h23"
    } else {
        p.hour_cycle.as_str()
    };
    let prefs_locale: Locale = format!(
        "{}-u-ca-{}-nu-{}-hc-{}",
        p.base, p.calendar, p.formatter_nu, prefs_cycle
    )
    .parse()
    .map_err(|_| range())?;
    let prefs: DateTimeFormatterPreferences = prefs_locale.into();
    let formatter =
        DateTimeFormatter::<CompositeFieldSet>::try_new(prefs, fieldset).map_err(|_| range())?;
    let strip_time_digits =
        p.day_period.is_some() && p.hour.is_none() && p.minute.is_none() && p.second.is_none();
    let truncate_year =
        p.date_style.as_deref() == Some("short") || p.year.as_deref() == Some("2-digit");
    // hourCycle/hour12 are only present when time is shown (a time field or
    // timeStyle); hour12 derives from the resolved cycle when not explicit.
    let (hour_cycle, hour12) = if has_time {
        let twelve = p
            .hour12
            .unwrap_or(p.hour_cycle == "h11" || p.hour_cycle == "h12");
        (Some(p.hour_cycle.clone()), Some(twelve))
    } else {
        (None, None)
    };
    let calendar_kind = any_calendar_kind(&p.calendar);
    Ok(Slots {
        locale: p.locale,
        calendar: p.calendar,
        numbering: p.numbering,
        hour_cycle,
        hour12,
        time_zone: p.time_zone,
        weekday: p.weekday,
        era: p.era,
        year: p.year,
        month: p.month,
        day: p.day,
        day_period: p.day_period,
        hour: p.hour,
        minute: p.minute,
        second: p.second,
        fractional: p.fractional,
        time_zone_name: p.time_zone_name,
        date_style: p.date_style,
        time_style: p.time_style,
        digit_map: p.digit_map,
        formatter: Rc::new(formatter),
        calendar_kind,
        strip_time_digits,
        truncate_year,
        omit_day,
        omit_month,
        omit_year,
        defaults,
    })
}
/// Drop spans of the given kinds plus one neighboring literal each, then
/// trim edge literals stranded against a dropped block (date-time glue).
fn drop_kinds(parts: Vec<(String, String)>, kinds: &[&str]) -> Vec<(String, String)> {
    let mut drop = vec![false; parts.len()];
    for (index, (kind, _)) in parts.iter().enumerate() {
        if kinds.contains(&kind.as_str()) {
            drop[index] = true;
        }
    }
    for index in 0..parts.len() {
        if !drop[index] {
            continue;
        }
        let following =
            index + 1 < parts.len() && parts[index + 1].0 == "literal" && !drop[index + 1];
        if following {
            drop[index + 1] = true;
        } else if index > 0 && parts[index - 1].0 == "literal" && !drop[index - 1] {
            drop[index - 1] = true;
        }
    }
    let mut kept: Vec<(usize, (String, String))> = parts
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !drop[*index])
        .collect();
    while kept.first().is_some_and(|(index, (kind, _))| {
        kind == "literal" && *index > 0 && drop.get(index - 1) == Some(&true)
    }) {
        kept.remove(0);
    }
    while kept
        .last()
        .is_some_and(|(index, (kind, _))| kind == "literal" && drop.get(index + 1) == Some(&true))
    {
        kept.pop();
    }
    kept.into_iter().map(|(_, part)| part).collect()
}
