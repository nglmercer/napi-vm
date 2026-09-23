//! `Date` static methods: `now`, `parse`, and `UTC`.
//!
//! Time math uses the civil-calendar algorithm from Howard Hinnant
//! (`days_from_civil`), so no external date crate is required. All values are
//! milliseconds since the Unix epoch (1970-01-01T00:00:00Z); parsed strings are
//! interpreted as UTC (the sandbox has no local-timezone concept).

#[cfg(not(target_arch = "wasm32"))]
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter};
use crate::value::{BuiltinConstructor, PropAttrs, Value};

const DATE_PROTOTYPE_METHODS: &[&str] = &[
    "getTime",
    "valueOf",
    "getFullYear",
    "getUTCFullYear",
    "getMonth",
    "getUTCMonth",
    "getDate",
    "getUTCDate",
    "getDay",
    "getUTCDay",
    "getHours",
    "getUTCHours",
    "getMinutes",
    "getUTCMinutes",
    "getSeconds",
    "getUTCSeconds",
    "getMilliseconds",
    "getUTCMilliseconds",
    "getTimezoneOffset",
    "setTime",
    "toISOString",
    "toJSON",
    "toString",
    "toUTCString",
];

pub(super) fn install(e: &mut Environment) {
    if let Some(d) = e.get("Date") {
        if let Value::Object { props } = &d {
            props.meta.borrow_mut().builtin_constructor = Some(BuiltinConstructor::Date);
        }
        d.set_prop("now".to_string(), super::nf("now", date_now))
            .expect("built-in Date property");
        d.set_prop("parse".to_string(), super::nf("parse", date_parse))
            .expect("built-in Date property");
        d.set_prop("UTC".to_string(), super::nf("UTC", date_utc))
            .expect("built-in Date property");
        super::make_callable(&d, date_call, Some(date_construct));

        let object_prototype = e
            .get("Object")
            .and_then(|object| object.get_prop("prototype"));
        let prototype =
            Value::object_with_proto(Vec::new(), object_prototype.map(std::rc::Rc::new));
        prototype
            .set_prop("constructor".into(), d.clone())
            .expect("Date.prototype constructor");
        for name in DATE_PROTOTYPE_METHODS {
            prototype
                .set_prop(
                    (*name).into(),
                    date_member(name).expect("listed Date.prototype method"),
                )
                .expect("Date.prototype method");
        }
        if let Value::Object { props } = &prototype {
            for name in std::iter::once("constructor").chain(DATE_PROTOTYPE_METHODS.iter().copied())
            {
                props.meta.borrow_mut().set_attrs(
                    name,
                    PropAttrs {
                        enumerable: false,
                        ..PropAttrs::default()
                    },
                );
            }
        }
        super::set_builtin_constructor_prototype(e, &d, prototype);
    }
}

/// `Date(…)` called without `new` is the current time as a string, which is
/// what the specification says regardless of its arguments.
fn date_call(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::String(to_iso(now_ms())))
}

/// `new Date(…)`: no arguments is now, one number is epoch milliseconds, one
/// string is parsed, and several are UTC civil-time components.
fn date_construct(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let ms = match a.len() {
        0 => now_ms(),
        1 => match &a[0] {
            Value::String(s) => parse_iso(s).unwrap_or(f64::NAN),
            Value::Date(existing) => existing.get(),
            other => other.to_number(),
        },
        _ => {
            let utc = date_utc(interp, Value::Undefined, a)?;
            utc.to_number()
        }
    };
    Ok(Value::Date(std::rc::Rc::new(std::cell::Cell::new(ms))))
}

/// Members readable on a `Date` instance.
///
/// The sandbox has no local timezone, so the `getX` accessors and their
/// `getUTCX` counterparts are the same function: everything is UTC.
pub fn date_member(key: &str) -> Option<Value> {
    let callable: super::NativeFn = match key {
        "getTime" | "valueOf" => date_get_time,
        "getFullYear" | "getUTCFullYear" => date_get_full_year,
        "getMonth" | "getUTCMonth" => date_get_month,
        "getDate" | "getUTCDate" => date_get_date,
        "getDay" | "getUTCDay" => date_get_day,
        "getHours" | "getUTCHours" => date_get_hours,
        "getMinutes" | "getUTCMinutes" => date_get_minutes,
        "getSeconds" | "getUTCSeconds" => date_get_seconds,
        "getMilliseconds" | "getUTCMilliseconds" => date_get_millis,
        // No local timezone means no offset from UTC.
        "getTimezoneOffset" => date_zero,
        "setTime" => date_set_time,
        "toISOString" | "toJSON" => date_to_iso,
        "toString" | "toUTCString" => date_to_iso,
        _ => return None,
    };
    Some(super::nf(key, callable))
}

fn epoch(this: &Value) -> f64 {
    match this {
        Value::Date(ms) => ms.get(),
        other => other.to_number(),
    }
}

/// UTC civil components of an instant, in the order the accessors index them.
///
/// A tuple struct rather than a plain tuple so the `component!` accessors can
/// keep indexing it positionally without the type becoming unreadable.
struct Civil(i64, i64, i64, i64, i64, i64, i64, i64);

/// Split epoch milliseconds into UTC civil components.
fn civil(ms: f64) -> Option<Civil> {
    if !ms.is_finite() {
        return None;
    }
    let total = ms as i64;
    let days = total.div_euclid(86_400_000);
    let rest = total.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    // 1970-01-01 was a Thursday, so day 0 is weekday 4.
    let weekday = (days + 4).rem_euclid(7);
    Some(Civil(
        year,
        month,
        day,
        rest / 3_600_000,
        (rest / 60_000) % 60,
        (rest / 1_000) % 60,
        rest % 1_000,
        weekday,
    ))
}

/// Inverse of `days_from_civil` (Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + i64::from(m <= 2), m, d)
}

fn to_iso(ms: f64) -> String {
    match civil(ms) {
        Some(Civil(year, month, day, hour, minute, second, millis, _)) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            year, month, day, hour, minute, second, millis
        ),
        None => "Invalid Date".to_string(),
    }
}

/// Build one component accessor from a projection over the civil parts.
macro_rules! component {
    ($name:ident, $index:tt, $adjust:expr) => {
        fn $name(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
            Ok(Value::Number(match civil(epoch(&this)) {
                Some(parts) => {
                    let adjust: fn(i64) -> i64 = $adjust;
                    adjust(parts.$index) as f64
                }
                None => f64::NAN,
            }))
        }
    };
}

component!(date_get_full_year, 0, |v| v);
// JavaScript's month index is zero-based; the civil calendar's is not.
component!(date_get_month, 1, |v| v - 1);
component!(date_get_date, 2, |v| v);
component!(date_get_hours, 3, |v| v);
component!(date_get_minutes, 4, |v| v);
component!(date_get_seconds, 5, |v| v);
component!(date_get_millis, 6, |v| v);
component!(date_get_day, 7, |v| v);

fn date_get_time(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(epoch(&this)))
}

fn date_zero(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(0.0))
}

fn date_set_time(_: &mut Interpreter, this: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let ms = a.first().map(|v| v.to_number()).unwrap_or(f64::NAN);
    if let Value::Date(slot) = &this {
        slot.set(ms);
    }
    Ok(Value::Number(ms))
}

fn date_to_iso(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::String(to_iso(epoch(&this))))
}

/// The ISO rendering of a date, for the formatter and the N-API boundary.
pub fn iso_string(ms: f64) -> String {
    to_iso(ms)
}

fn date_now(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(now_ms()))
}

/// Milliseconds since the Unix epoch. On native targets this reads the system
/// clock; on `wasm32` (where `SystemTime` panics) it asks the JS host.
#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    js_sys::Date::now()
}

fn date_utc(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let num = |i: usize, dflt: f64| a.get(i).map(|v| v.to_number()).unwrap_or(dflt);
    let mut year = num(0, 1970.0) as i64;
    // Two-digit years map into the 1900s, matching JavaScript.
    if (0..=99).contains(&year) {
        year += 1900;
    }
    let month = num(1, 0.0) as i64 + 1; // month index is zero-based
    let day = num(2, 1.0) as i64;
    let hour = num(3, 0.0) as i64;
    let minute = num(4, 0.0) as i64;
    let second = num(5, 0.0) as i64;
    let millis = num(6, 0.0) as i64;
    Ok(Value::Number(utc_ms(
        year, month, day, hour, minute, second, millis,
    )))
}

fn date_parse(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let s = match a.first() {
        Some(Value::String(s)) => s.clone(),
        Some(v) => interp.vs(v)?,
        None => return Ok(Value::Number(f64::NAN)),
    };
    Ok(Value::Number(parse_iso(&s).unwrap_or(f64::NAN)))
}

/// Milliseconds since the epoch for a UTC civil date/time.
fn utc_ms(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64, ms: i64) -> f64 {
    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3_600 + minute * 60 + second;
    (secs * 1_000 + ms) as f64
}

/// Days since 1970-01-01 for a civil date (Hinnant's algorithm). Negative for
/// dates before the epoch.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400; // [0, 399]
    let mshift = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mshift + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Parse an ISO-8601 string (`YYYY-MM-DD[THH:mm[:ss[.sss]]][Z|±HH:mm]`) into
/// epoch milliseconds. Returns `None` when the string is not recognizable.
fn parse_iso(s: &str) -> Option<f64> {
    let s = s.trim();
    let (date_part, time_part) = if let Some(idx) = s.find('T') {
        (&s[..idx], Some(&s[idx + 1..]))
    } else if let Some(idx) = s.find(' ') {
        (&s[..idx], Some(&s[idx + 1..]))
    } else {
        (s, None)
    };

    let dseps: Vec<&str> = date_part.split(['-', '/']).collect();
    let year: i64 = dseps.first()?.parse().ok()?;
    let month: i64 = dseps.get(1).and_then(|x| x.parse().ok()).unwrap_or(1);
    let day: i64 = dseps.get(2).and_then(|x| x.parse().ok()).unwrap_or(1);

    let mut hour = 0i64;
    let mut minute = 0i64;
    let mut second = 0i64;
    let mut millis = 0i64;
    let mut tz_offset_secs = 0i64;

    if let Some(mut t) = time_part {
        // Peel off a trailing timezone designator first.
        if let Some(zidx) = t.find('Z') {
            t = &t[..zidx];
        } else if let Some(sign_idx) = t.rfind(['+', '-']) {
            let sign = &t[sign_idx..sign_idx + 1];
            let tz_parts: Vec<&str> = t[sign_idx + 1..].split(':').collect();
            let tzh: i64 = tz_parts.first().and_then(|x| x.parse().ok()).unwrap_or(0);
            let tzm: i64 = tz_parts.get(1).and_then(|x| x.parse().ok()).unwrap_or(0);
            let mag = tzh * 3_600 + tzm * 60;
            tz_offset_secs = if sign == "-" { -mag } else { mag };
            t = &t[..sign_idx];
        }

        let (hms, frac) = match t.find('.') {
            Some(dot) => (&t[..dot], Some(&t[dot + 1..])),
            None => (t, None),
        };
        let parts: Vec<&str> = hms.split(':').collect();
        hour = parts.first().and_then(|x| x.parse().ok()).unwrap_or(0);
        minute = parts.get(1).and_then(|x| x.parse().ok()).unwrap_or(0);
        second = parts.get(2).and_then(|x| x.parse().ok()).unwrap_or(0);
        if let Some(fr) = frac {
            let digits: String = fr.chars().take(3).collect();
            millis = format!("{:0<3}", digits).parse().unwrap_or(0);
        }
    }

    let base = utc_ms(year, month, day, hour, minute, second, millis);
    Some(base - f64::from(tz_offset_secs as i32) * 1_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_zero() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn known_dates() {
        // 2000-01-01 is 10957 days after the epoch.
        assert_eq!(days_from_civil(2000, 1, 1), 10_957);
        // 1969-12-31 is one day before.
        assert_eq!(days_from_civil(1969, 12, 31), -1);
    }

    #[test]
    fn parse_date_only() {
        assert_eq!(parse_iso("1970-01-01"), Some(0.0));
        assert_eq!(parse_iso("2000-01-01"), Some(946_684_800_000.0));
    }

    #[test]
    fn parse_with_time_and_z() {
        assert_eq!(parse_iso("1970-01-01T00:00:01Z"), Some(1_000.0));
        assert_eq!(parse_iso("1970-01-01T01:00:00Z"), Some(3_600_000.0));
    }

    #[test]
    fn parse_with_offset() {
        // 01:00 at +01:00 is midnight UTC.
        assert_eq!(parse_iso("1970-01-01T01:00:00+01:00"), Some(0.0));
    }

    #[test]
    fn parse_millis() {
        assert_eq!(parse_iso("1970-01-01T00:00:00.500Z"), Some(500.0));
    }
}
