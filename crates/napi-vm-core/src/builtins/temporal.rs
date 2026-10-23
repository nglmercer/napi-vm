//! Temporal objects. JavaScript observable operations stay in the VM; calendar
//! and nanosecond arithmetic are delegated to temporal_rs.
use crate::error::VmErr;
use std::rc::Rc;
use std::str::FromStr;
use temporal_rs::options::{
    DifferenceSettings, Disambiguation, DisplayCalendar, DisplayOffset, DisplayTimeZone,
    OffsetDisambiguation, Overflow, RelativeTo, RoundingIncrement, RoundingMode, RoundingOptions,
    ToStringRoundingOptions, Unit,
};
use temporal_rs::partial::{PartialDate, PartialTime, PartialZonedDateTime};
use temporal_rs::provider::COMPILED_TZ_PROVIDER as TZ;
use temporal_rs::{
    Calendar, Duration, Instant, MonthCode, PlainDate, PlainDateTime, PlainMonthDay, PlainTime,
    PlainYearMonth, TimeZone, UtcOffset, ZonedDateTime,
};
fn vm_err<T>(message: &str) -> Result<T, VmErr> {
    Err(VmErr::Msg(message.into()))
}
use super::{NativeFn, native_method};
use crate::interpreter::{Environment, Interpreter};
use crate::value::{PropAttrs, Value};

#[derive(Debug, Clone)]
pub(crate) enum Slots {
    Time(PlainTime),
    Date(PlainDate),
    DateTime(PlainDateTime),
    Zoned(ZonedDateTime),
    YearMonth(PlainYearMonth),
    MonthDay(PlainMonthDay),
    Instant(Instant),
    Duration(Duration),
}

fn err(error: temporal_rs::TemporalError) -> VmErr {
    VmErr::Msg(error.to_string())
}
fn arg(args: &[Value], index: usize) -> Value {
    args.get(index).cloned().unwrap_or(Value::Undefined)
}
fn slot(value: &Value) -> Option<Slots> {
    value
        .property_cell()
        .and_then(|p| p.meta.borrow().temporal.clone())
}
fn time(value: &Value) -> Result<PlainTime, VmErr> {
    match slot(value) {
        Some(Slots::Time(t)) => Ok(t),
        _ => vm_err("TypeError: Temporal.PlainTime receiver required"),
    }
}
fn duration(value: &Value) -> Result<Duration, VmErr> {
    match slot(value) {
        Some(Slots::Duration(d)) => Ok(d),
        _ => vm_err("TypeError: Temporal.Duration receiver required"),
    }
}
macro_rules! locale_method {
    ($($name:ident => $brand:ident),+ $(,)?) => {$(
        fn $name(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
            $brand(&t)?;
            super::intl::datetime::temporal_to_locale(i, t, a)
        }
    )+};
}
locale_method!(
    time_locale => time,
    date_locale => date,
    datetime_locale => datetime,
    yearmonth_locale => yearmonth,
    monthday_locale => monthday,
    instant_locale => instant,
    zoned_locale => zoned,
);
fn duration_locale(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    duration(&t)?;
    super::intl::duration::temporal_to_locale(i, t, a)
}
fn method(target: &Value, name: &str, length: usize, f: NativeFn, fp: &Option<Value>) {
    target
        .set_prop(name.into(), native_method(name, length, f, fp.clone()))
        .unwrap();
    target.property_cell().unwrap().meta.borrow_mut().set_attrs(
        name,
        PropAttrs {
            enumerable: false,
            ..Default::default()
        },
    );
}
fn tag(target: &Value, name: &str) {
    if let Some(Value::Symbol(ref symbol)) = super::well_known("toStringTag") {
        let key = crate::interpreter::symbol_slot_key(symbol);
        target
            .set_prop(key.clone(), Value::String(name.into()))
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
    let op = e.get("Object").and_then(|v| v.get_prop("prototype"));
    let fp = e.get("Function").and_then(|v| v.get_prop("prototype"));
    let namespace = Value::object_with_proto(vec![], op.clone().map(Rc::new));
    tag(&namespace, "Temporal");
    for (name, length, construct, from, compare) in [
        (
            "PlainTime",
            0,
            time_construct as NativeFn,
            time_from as NativeFn,
            time_compare as NativeFn,
        ),
        (
            "Instant",
            1,
            instant_construct as NativeFn,
            instant_from as NativeFn,
            instant_compare as NativeFn,
        ),
        (
            "PlainDate",
            3,
            date_construct as NativeFn,
            date_from as NativeFn,
            date_compare as NativeFn,
        ),
        (
            "PlainDateTime",
            3,
            datetime_construct as NativeFn,
            datetime_from as NativeFn,
            datetime_compare as NativeFn,
        ),
        (
            "PlainYearMonth",
            2,
            yearmonth_construct as NativeFn,
            yearmonth_from as NativeFn,
            yearmonth_compare as NativeFn,
        ),
        (
            "PlainMonthDay",
            2,
            monthday_construct as NativeFn,
            monthday_from as NativeFn,
            monthday_from as NativeFn,
        ),
        (
            "ZonedDateTime",
            2,
            zoned_construct as NativeFn,
            zoned_from as NativeFn,
            zoned_compare as NativeFn,
        ),
        (
            "Duration",
            0,
            duration_construct as NativeFn,
            duration_from as NativeFn,
            duration_compare as NativeFn,
        ),
    ] {
        let ctor = Value::object_with_proto(vec![], fp.clone().map(Rc::new));
        super::make_callable(&ctor, super::require_new, Some(construct));
        for (key, value) in [
            ("name", Value::String(name.into())),
            ("length", Value::Number(length as f64)),
        ] {
            ctor.set_prop(key.into(), value).unwrap();
            ctor.property_cell().unwrap().meta.borrow_mut().set_attrs(
                key,
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        let proto = Value::object_with_proto(vec![], op.clone().map(Rc::new));
        proto.set_prop("constructor".into(), ctor.clone()).unwrap();
        proto.property_cell().unwrap().meta.borrow_mut().set_attrs(
            "constructor",
            PropAttrs {
                enumerable: false,
                ..Default::default()
            },
        );
        tag(&proto, &format!("Temporal.{name}"));
        method(&ctor, "from", 1, from, &fp);
        if name != "PlainMonthDay" {
            method(&ctor, "compare", 2, compare, &fp);
        }
        if name == "Instant" {
            method(
                &ctor,
                "fromEpochMilliseconds",
                1,
                instant_from_milliseconds,
                &fp,
            );
            method(
                &ctor,
                "fromEpochNanoseconds",
                1,
                instant_from_nanoseconds,
                &fp,
            );
        }
        let methods: &[(&str, usize, NativeFn)] = if name == "PlainTime" {
            &[
                ("add", 1, time_add),
                ("subtract", 1, time_subtract),
                ("with", 1, time_with),
                ("until", 1, time_until),
                ("since", 1, time_since),
                ("round", 1, time_round),
                ("equals", 1, time_equals),
                ("toString", 0, time_string),
                ("toJSON", 0, time_json),
                ("toLocaleString", 0, time_locale),
                ("valueOf", 0, value_of),
            ]
        } else if name == "PlainDate" {
            &[
                ("add", 1, date_add),
                ("subtract", 1, date_subtract),
                ("with", 1, date_with),
                ("withCalendar", 1, date_with_calendar),
                ("toPlainDateTime", 0, date_to_datetime),
                ("toZonedDateTime", 1, date_to_zoned),
                ("toPlainYearMonth", 0, date_to_yearmonth),
                ("toPlainMonthDay", 0, date_to_monthday),
                ("until", 1, date_until),
                ("since", 1, date_since),
                ("equals", 1, date_equals),
                ("toString", 0, date_string),
                ("toJSON", 0, date_json),
                ("toLocaleString", 0, date_locale),
                ("valueOf", 0, value_of),
            ]
        } else if name == "PlainYearMonth" {
            &[
                ("add", 1, yearmonth_add),
                ("subtract", 1, yearmonth_subtract),
                ("with", 1, yearmonth_with),
                ("toPlainDate", 1, yearmonth_to_date),
                ("until", 1, yearmonth_until),
                ("since", 1, yearmonth_since),
                ("equals", 1, yearmonth_equals),
                ("toString", 0, yearmonth_string),
                ("toJSON", 0, yearmonth_json),
                ("toLocaleString", 0, yearmonth_locale),
                ("valueOf", 0, value_of),
            ]
        } else if name == "PlainMonthDay" {
            &[
                ("with", 1, monthday_with),
                ("equals", 1, monthday_equals),
                ("toString", 0, monthday_string),
                ("toJSON", 0, monthday_json),
                ("toLocaleString", 0, monthday_locale),
                ("toPlainDate", 1, monthday_to_date),
                ("valueOf", 0, value_of),
            ]
        } else if name == "PlainDateTime" {
            &[
                ("add", 1, datetime_add),
                ("subtract", 1, datetime_subtract),
                ("with", 1, datetime_with),
                ("withCalendar", 1, datetime_with_calendar),
                ("until", 1, datetime_until),
                ("since", 1, datetime_since),
                ("equals", 1, datetime_equals),
                ("toString", 0, datetime_string),
                ("toJSON", 0, datetime_json),
                ("toLocaleString", 0, datetime_locale),
                ("valueOf", 0, value_of),
                ("round", 1, datetime_round),
                ("withPlainTime", 0, datetime_with_time),
                ("toPlainDate", 0, datetime_to_date),
                ("toZonedDateTime", 1, datetime_to_zoned),
                ("toPlainTime", 0, datetime_to_time),
            ]
        } else if name == "ZonedDateTime" {
            &[
                ("add", 1, zoned_add),
                ("subtract", 1, zoned_subtract),
                ("with", 1, zoned_with),
                ("withCalendar", 1, zoned_with_calendar),
                ("until", 1, zoned_until),
                ("since", 1, zoned_since),
                ("equals", 1, zoned_equals),
                ("toString", 0, zoned_string),
                ("toJSON", 0, zoned_json),
                ("toLocaleString", 0, zoned_locale),
                ("valueOf", 0, value_of),
                ("round", 1, zoned_round),
                ("withPlainTime", 0, zoned_with_time),
                ("toPlainDate", 0, zoned_to_date),
                ("toPlainDateTime", 0, zoned_to_datetime),
                ("toInstant", 0, zoned_to_instant),
                ("withTimeZone", 1, zoned_with_timezone),
                ("startOfDay", 0, zoned_start_of_day),
                ("getTimeZoneTransition", 1, zoned_transition),
                ("toPlainTime", 0, zoned_to_time),
            ]
        } else if name == "Instant" {
            &[
                ("add", 1, instant_add),
                ("subtract", 1, instant_subtract),
                ("until", 1, instant_until),
                ("since", 1, instant_since),
                ("round", 1, instant_round),
                ("equals", 1, instant_equals),
                ("toString", 0, instant_string),
                ("toJSON", 0, instant_json),
                ("toLocaleString", 0, instant_locale),
                ("valueOf", 0, value_of),
                ("toZonedDateTimeISO", 1, instant_to_zoned),
            ]
        } else {
            &[
                ("add", 1, duration_add),
                ("subtract", 1, duration_subtract),
                ("with", 1, duration_with),
                ("round", 1, duration_round),
                ("total", 1, duration_total),
                ("negated", 0, duration_negated),
                ("abs", 0, duration_abs),
                ("toString", 0, duration_string),
                ("toJSON", 0, duration_json),
                ("toLocaleString", 0, duration_locale),
                ("valueOf", 0, value_of),
            ]
        };
        for (key, length, f) in methods {
            method(&proto, key, *length, *f, &fp);
        }
        let getters: &[(&str, NativeFn)] = if name == "PlainTime" {
            &[
                ("hour", get_hour),
                ("minute", get_minute),
                ("second", get_second),
                ("millisecond", get_millisecond),
                ("microsecond", get_microsecond),
                ("nanosecond", get_nanosecond),
            ]
        } else if name == "PlainDate" {
            &[
                ("year", date_year),
                ("month", date_month),
                ("day", date_day),
                ("monthCode", date_month_code),
                ("calendarId", date_calendar_id),
                ("dayOfWeek", date_day_of_week),
                ("dayOfYear", date_day_of_year),
                ("weekOfYear", date_week_of_year),
                ("yearOfWeek", date_year_of_week),
                ("daysInWeek", date_days_in_week),
                ("daysInMonth", date_days_in_month),
                ("daysInYear", date_days_in_year),
                ("monthsInYear", date_months_in_year),
                ("inLeapYear", date_in_leap_year),
                ("era", date_era),
                ("eraYear", date_era_year),
            ]
        } else if name == "PlainYearMonth" {
            &[
                ("year", yearmonth_year),
                ("month", yearmonth_month),
                ("monthCode", yearmonth_month_code),
                ("calendarId", yearmonth_calendar_id),
                ("daysInMonth", yearmonth_days_in_month),
                ("daysInYear", yearmonth_days_in_year),
                ("monthsInYear", yearmonth_months_in_year),
                ("inLeapYear", yearmonth_in_leap_year),
                ("era", yearmonth_era),
                ("eraYear", yearmonth_era_year),
            ]
        } else if name == "PlainMonthDay" {
            &[
                ("day", monthday_day),
                ("monthCode", monthday_month_code),
                ("calendarId", monthday_calendar_id),
            ]
        } else if name == "PlainDateTime" {
            &[
                ("hour", datetime_hour),
                ("minute", datetime_minute),
                ("second", datetime_second),
                ("millisecond", datetime_millisecond),
                ("microsecond", datetime_microsecond),
                ("nanosecond", datetime_nanosecond),
                ("year", datetime_year),
                ("month", datetime_month),
                ("day", datetime_day),
                ("monthCode", datetime_month_code),
                ("calendarId", datetime_calendar_id),
                ("dayOfWeek", datetime_day_of_week),
                ("dayOfYear", datetime_day_of_year),
                ("weekOfYear", datetime_week_of_year),
                ("yearOfWeek", datetime_year_of_week),
                ("daysInWeek", datetime_days_in_week),
                ("daysInMonth", datetime_days_in_month),
                ("daysInYear", datetime_days_in_year),
                ("monthsInYear", datetime_months_in_year),
                ("inLeapYear", datetime_in_leap_year),
                ("era", datetime_era),
                ("eraYear", datetime_era_year),
            ]
        } else if name == "ZonedDateTime" {
            &[
                ("hour", zoned_hour),
                ("minute", zoned_minute),
                ("second", zoned_second),
                ("millisecond", zoned_millisecond),
                ("microsecond", zoned_microsecond),
                ("nanosecond", zoned_nanosecond),
                ("epochMilliseconds", zoned_milliseconds),
                ("epochNanoseconds", zoned_nanoseconds),
                ("timeZoneId", zoned_timezone_id),
                ("offset", zoned_offset),
                ("offsetNanoseconds", zoned_offset_nanoseconds),
                ("hoursInDay", zoned_hours_in_day),
                ("year", zoned_year),
                ("month", zoned_month),
                ("day", zoned_day),
                ("monthCode", zoned_month_code),
                ("calendarId", zoned_calendar_id),
                ("dayOfWeek", zoned_day_of_week),
                ("dayOfYear", zoned_day_of_year),
                ("weekOfYear", zoned_week_of_year),
                ("yearOfWeek", zoned_year_of_week),
                ("daysInWeek", zoned_days_in_week),
                ("daysInMonth", zoned_days_in_month),
                ("daysInYear", zoned_days_in_year),
                ("monthsInYear", zoned_months_in_year),
                ("inLeapYear", zoned_in_leap_year),
                ("era", zoned_era),
                ("eraYear", zoned_era_year),
            ]
        } else if name == "Instant" {
            &[
                ("epochMilliseconds", instant_milliseconds),
                ("epochNanoseconds", instant_nanoseconds),
            ]
        } else {
            &[
                ("years", get_years),
                ("months", get_months),
                ("weeks", get_weeks),
                ("days", get_days),
                ("hours", get_hours),
                ("minutes", get_minutes),
                ("seconds", get_seconds),
                ("milliseconds", get_milliseconds),
                ("microseconds", get_microseconds),
                ("nanoseconds", get_nanoseconds),
                ("sign", get_sign),
                ("blank", get_blank),
            ]
        };
        for (key, f) in getters {
            super::object::define_property(
                &proto,
                key,
                &Value::descriptor_record(vec![
                    (
                        "get".into(),
                        native_method(&format!("get {key}"), 0, *f, fp.clone()),
                    ),
                    ("configurable".into(), Value::Bool(true)),
                ]),
            )
            .expect("Temporal accessor");
        }
        ctor.set_prop("prototype".into(), proto.clone()).unwrap();
        ctor.property_cell().unwrap().meta.borrow_mut().set_attrs(
            "prototype",
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: false,
            },
        );
        namespace.set_prop(name.into(), ctor.clone()).unwrap();
        namespace
            .property_cell()
            .unwrap()
            .meta
            .borrow_mut()
            .set_attrs(
                name,
                PropAttrs {
                    enumerable: false,
                    ..Default::default()
                },
            );
        e.install_intrinsic(&format!("Temporal.{name}"), ctor);
        e.install_intrinsic(&format!("Temporal.{name}.prototype"), proto);
    }
    let now = Value::object_with_proto(vec![], op.clone().map(Rc::new));
    tag(&now, "Temporal.Now");
    for (name, f) in [
        ("instant", now_instant as NativeFn),
        ("timeZoneId", now_timezone_id),
        ("zonedDateTimeISO", now_zoned_iso),
        ("plainDateTimeISO", now_datetime_iso),
        ("plainDateISO", now_date_iso),
        ("plainTimeISO", now_time_iso),
    ] {
        method(&now, name, 0, f, &fp);
    }
    namespace.set_prop("Now".into(), now).unwrap();
    namespace
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .set_attrs(
            "Now",
            PropAttrs {
                enumerable: false,
                ..Default::default()
            },
        );
    e.set("Temporal", namespace);
    e.set_property_attributes(
        "Temporal",
        PropAttrs {
            enumerable: false,
            ..Default::default()
        },
    );
}
fn wrap(interp: &mut Interpreter, data: Slots, construct: bool) -> Result<Value, VmErr> {
    let name = match data {
        Slots::Zoned(_) => "Temporal.ZonedDateTime",
        Slots::YearMonth(_) => "Temporal.PlainYearMonth",
        Slots::MonthDay(_) => "Temporal.PlainMonthDay",
        Slots::DateTime(_) => "Temporal.PlainDateTime",
        Slots::Date(_) => "Temporal.PlainDate",
        Slots::Instant(_) => "Temporal.Instant",
        Slots::Time(_) => "Temporal.PlainTime",
        Slots::Duration(_) => "Temporal.Duration",
    };
    let default = interp
        .persistent_global
        .borrow()
        .intrinsic(&format!("{name}.prototype"));
    let proto = if construct {
        if let Some(target) = interp.new_target_stack.last().cloned() {
            interp
                .constructor_prototype(&target, name)?
                .map(|p| (*p).clone())
                .or(default)
        } else {
            default
        }
    } else {
        default
    };
    let value = Value::object_with_proto(vec![], proto.map(Rc::new));
    value.property_cell().unwrap().meta.borrow_mut().temporal = Some(data);
    Ok(value)
}
fn options(value: Value) -> Result<Value, VmErr> {
    if matches!(value, Value::Undefined) {
        return Ok(Value::object_with_proto(vec![], None));
    }
    if !crate::interpreter::call::is_js_object(&value) {
        return vm_err("TypeError: options must be an object");
    }
    Ok(value)
}
fn string_option(
    interp: &mut Interpreter,
    options: &Value,
    key: &str,
) -> Result<Option<String>, VmErr> {
    let v = interp.get_prop_value_str(options, key)?;
    if matches!(v, Value::Undefined) {
        Ok(None)
    } else {
        Ok(Some(interp.ecmascript_to_string(&v)?.to_string()))
    }
}
fn overflow(interp: &mut Interpreter, opts: Value) -> Result<Overflow, VmErr> {
    let opts = options(opts)?;
    match string_option(interp, &opts, "overflow")?.as_deref() {
        None | Some("constrain") => Ok(Overflow::Constrain),
        Some("reject") => Ok(Overflow::Reject),
        _ => vm_err("RangeError: invalid overflow"),
    }
}
fn integer(interp: &mut Interpreter, v: &Value, integral: bool) -> Result<f64, VmErr> {
    let n = interp.ecmascript_to_number(v)?;
    if !n.is_finite() || (integral && n.trunc() != n) {
        return vm_err("RangeError: invalid Temporal field");
    }
    Ok(n.trunc())
}
const TIME_FIELDS: [&str; 6] = [
    "hour",
    "minute",
    "second",
    "millisecond",
    "microsecond",
    "nanosecond",
];
const DURATION_FIELDS: [&str; 10] = [
    "years",
    "months",
    "weeks",
    "days",
    "hours",
    "minutes",
    "seconds",
    "milliseconds",
    "microseconds",
    "nanoseconds",
];
fn fields(
    interp: &mut Interpreter,
    item: &Value,
    names: &[&str],
    integral: bool,
) -> Result<Vec<Option<f64>>, VmErr> {
    let mut order: Vec<_> = names.iter().enumerate().collect();
    order.sort_by_key(|(_, name)| **name);
    let mut values = vec![None; names.len()];
    let mut any = false;
    for (index, name) in order {
        let v = interp.get_prop_value_str(item, name)?;
        if !matches!(v, Value::Undefined) {
            any = true;
            values[index] = Some(integer(interp, &v, integral)?);
        }
    }
    if !any {
        return vm_err("TypeError: no Temporal fields supplied");
    }
    Ok(values)
}
fn make_time(fields: &[f64], overflow: Overflow) -> Result<PlainTime, VmErr> {
    let limits = [23., 59., 59., 999., 999., 999.];
    let mut n = [0u16; 6];
    for i in 0..6 {
        if overflow == Overflow::Reject && !(0.0..=limits[i]).contains(&fields[i]) {
            return vm_err("RangeError: invalid time field");
        }
        n[i] = fields[i].clamp(0., limits[i]) as u16;
    }
    PlainTime::try_new(n[0] as u8, n[1] as u8, n[2] as u8, n[3], n[4], n[5]).map_err(err)
}
fn make_duration(n: &[f64]) -> Result<Duration, VmErr> {
    // Date fields are bounded by the Temporal representation, before casting.
    if n[..4].iter().any(|v| v.abs() >= i64::MAX as f64)
        || n.iter().any(|v| v.abs() >= i128::MAX as f64)
    {
        return vm_err("RangeError: duration field out of range");
    }
    Duration::new(
        n[0] as i64,
        n[1] as i64,
        n[2] as i64,
        n[3] as i64,
        n[4] as i64,
        n[5] as i64,
        n[6] as i64,
        n[7] as i64,
        n[8] as i128,
        n[9] as i128,
    )
    .map_err(err)
}
fn to_time(interp: &mut Interpreter, item: &Value, opts: Value) -> Result<PlainTime, VmErr> {
    if let Some(Slots::Zoned(d)) = slot(item) {
        overflow(interp, opts)?;
        return Ok(d.to_plain_time());
    }
    if let Some(Slots::DateTime(t)) = slot(item) {
        overflow(interp, opts)?;
        return Ok(t.to_plain_time());
    }
    if let Some(Slots::Time(t)) = slot(item) {
        overflow(interp, opts)?;
        return Ok(t);
    }
    if crate::interpreter::call::is_js_object(item) {
        let n = fields(interp, item, &TIME_FIELDS, false)?;
        let o = overflow(interp, opts)?;
        return make_time(&n.iter().map(|v| v.unwrap_or(0.)).collect::<Vec<_>>(), o);
    }
    let Value::String(s) = item else {
        return vm_err("TypeError: time must be a string or object");
    };
    let time = PlainTime::from_utf8(s.to_string().as_bytes()).map_err(err)?;
    overflow(interp, opts)?;
    Ok(time)
}
fn to_duration(interp: &mut Interpreter, item: &Value) -> Result<Duration, VmErr> {
    if let Some(Slots::Duration(d)) = slot(item) {
        return Ok(d);
    }
    if crate::interpreter::call::is_js_object(item) {
        let n = fields(interp, item, &DURATION_FIELDS, true)?;
        return make_duration(&n.iter().map(|v| v.unwrap_or(0.)).collect::<Vec<_>>());
    }
    let Value::String(s) = item else {
        return vm_err("TypeError: duration must be a string or object");
    };
    Duration::from_utf8(s.to_string().as_bytes()).map_err(err)
}
fn time_construct(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let mut n = vec![];
    for x in 0..6 {
        let v = arg(&a, x);
        n.push(if matches!(v, Value::Undefined) {
            0.
        } else {
            integer(i, &v, false)?
        });
    }
    let t = make_time(&n, Overflow::Reject)?;
    wrap(i, Slots::Time(t), true)
}
fn duration_construct(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let mut n = vec![];
    for x in 0..10 {
        let v = arg(&a, x);
        n.push(if matches!(v, Value::Undefined) {
            0.
        } else {
            integer(i, &v, true)?
        });
    }
    let d = make_duration(&n)?;
    wrap(i, Slots::Duration(d), true)
}
fn time_from(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = to_time(i, &arg(&a, 0), arg(&a, 1))?;
    wrap(i, Slots::Time(t), false)
}
fn duration_from(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = to_duration(i, &arg(&a, 0))?;
    wrap(i, Slots::Duration(d), false)
}
fn time_compare(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let first = to_time(i, &arg(&a, 0), Value::Undefined)?;
    let b = to_time(i, &arg(&a, 1), Value::Undefined)?;
    Ok(Value::Number(first.cmp(&b) as i8 as f64))
}
fn duration_compare(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let first = to_duration(i, &arg(&a, 0))?;
    let second = to_duration(i, &arg(&a, 1))?;
    let opts = options(arg(&a, 2))?;
    let relative = relative_to(i, &opts)?;
    if relative.is_none()
        && first != second
        && (calendar_duration(&first) || calendar_duration(&second))
    {
        return vm_err("RangeError: relativeTo is required for calendar durations");
    }
    Ok(Value::Number(
        first
            .compare_with_provider(&second, relative, &*TZ)
            .map_err(err)? as i8 as f64,
    ))
}
fn value_of(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    vm_err("TypeError: Temporal values cannot be converted to primitives")
}
macro_rules! time_getters {($($f:ident=>$field:ident),*)=>{$(fn $f(_: &mut Interpreter,t:Value,_:Vec<Value>)->Result<Value,VmErr>{Ok(Value::Number(time(&t)?.$field()as f64))})*}}
time_getters!(get_hour=>hour,get_minute=>minute,get_second=>second,get_millisecond=>millisecond,get_microsecond=>microsecond,get_nanosecond=>nanosecond);
macro_rules! duration_getters {($($f:ident=>$field:ident),*)=>{$(fn $f(_: &mut Interpreter,t:Value,_:Vec<Value>)->Result<Value,VmErr>{Ok(Value::Number(duration(&t)?.$field()as f64))})*}}
duration_getters!(get_years=>years,get_months=>months,get_weeks=>weeks,get_days=>days,get_hours=>hours,get_minutes=>minutes,get_seconds=>seconds,get_milliseconds=>milliseconds,get_microseconds=>microseconds,get_nanoseconds=>nanoseconds);
fn get_sign(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(duration(&t)?.sign() as i8 as f64))
}
fn get_blank(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(duration(&t)?.is_zero()))
}
fn time_add(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = time(&t)?;
    let d = to_duration(i, &arg(&a, 0))?;
    wrap(i, Slots::Time(t.add(&d).map_err(err)?), false)
}
fn time_subtract(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = time(&t)?;
    let d = to_duration(i, &arg(&a, 0))?;
    wrap(i, Slots::Time(t.subtract(&d).map_err(err)?), false)
}
fn time_equals(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = time(&t)?;
    Ok(Value::Bool(t == to_time(i, &arg(&a, 0), Value::Undefined)?))
}
fn time_with(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = time(&t)?;
    let item = arg(&a, 0);
    if !crate::interpreter::call::is_js_object(&item) || slot(&item).is_some() {
        return vm_err("TypeError: partial time object required");
    };
    for key in ["calendar", "timeZone"] {
        if !matches!(i.get_prop_value_str(&item, key)?, Value::Undefined) {
            return vm_err("TypeError: calendar and timeZone are forbidden");
        }
    }
    let fields = fields(i, &item, &TIME_FIELDS, false)?;
    let old = [
        t.hour() as f64,
        t.minute() as f64,
        t.second() as f64,
        t.millisecond() as f64,
        t.microsecond() as f64,
        t.nanosecond() as f64,
    ];
    let n: Vec<_> = fields
        .iter()
        .enumerate()
        .map(|(x, v)| v.unwrap_or(old[x]))
        .collect();
    let o = overflow(i, arg(&a, 1))?;
    wrap(i, Slots::Time(make_time(&n, o)?), false)
}
fn duration_negated(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let d = duration(&t)?.negated();
    wrap(i, Slots::Duration(d), false)
}
fn duration_abs(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let d = duration(&t)?.abs();
    wrap(i, Slots::Duration(d), false)
}
fn duration_add(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = duration(&t)?;
    let other = to_duration(i, &arg(&a, 0))?;
    wrap(i, Slots::Duration(d.add(&other).map_err(err)?), false)
}
fn duration_subtract(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = duration(&t)?;
    let other = to_duration(i, &arg(&a, 0))?;
    wrap(i, Slots::Duration(d.subtract(&other).map_err(err)?), false)
}
fn duration_with(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = duration(&t)?;
    let item = arg(&a, 0);
    if !crate::interpreter::call::is_js_object(&item) {
        return vm_err("TypeError: partial duration object required");
    };
    let fields = fields(i, &item, &DURATION_FIELDS, true)?;
    let old = [
        d.years() as f64,
        d.months() as f64,
        d.weeks() as f64,
        d.days() as f64,
        d.hours() as f64,
        d.minutes() as f64,
        d.seconds() as f64,
        d.milliseconds() as f64,
        d.microseconds() as f64,
        d.nanoseconds() as f64,
    ];
    let n: Vec<_> = fields
        .iter()
        .enumerate()
        .map(|(x, v)| v.unwrap_or(old[x]))
        .collect();
    wrap(i, Slots::Duration(make_duration(&n)?), false)
}
fn unit(i: &mut Interpreter, opts: &Value, key: &str) -> Result<Option<Unit>, VmErr> {
    string_option(i, opts, key)?
        .map(|s| {
            Unit::from_str(&s).map_err(|_| VmErr::Msg("RangeError: invalid Temporal unit".into()))
        })
        .transpose()
}
fn mode(i: &mut Interpreter, opts: &Value) -> Result<Option<RoundingMode>, VmErr> {
    string_option(i, opts, "roundingMode")?
        .map(|s| RoundingMode::from_str(&s).map_err(err))
        .transpose()
}
fn increment(i: &mut Interpreter, opts: &Value) -> Result<Option<RoundingIncrement>, VmErr> {
    let v = i.get_prop_value_str(opts, "roundingIncrement")?;
    if matches!(v, Value::Undefined) {
        Ok(None)
    } else {
        let n = i.ecmascript_to_number(&v)?;
        Ok(Some(RoundingIncrement::try_from(n).map_err(err)?))
    }
}
fn string_options_prefix(i: &mut Interpreter, v: Value) -> Result<ToStringRoundingOptions, VmErr> {
    let v = options(v)?;
    let mut o = ToStringRoundingOptions::default();
    let digits = i.get_prop_value_str(&v, "fractionalSecondDigits")?;
    if !matches!(digits, Value::Undefined) {
        o.precision = if let Value::Number(n) = digits {
            if n.is_nan() || !(0.0..10.0).contains(&n) {
                return vm_err("RangeError: invalid fractionalSecondDigits");
            }
            temporal_rs::parsers::Precision::Digit(n.floor() as u8)
        } else {
            if i.ecmascript_to_string(&digits)? != "auto" {
                return vm_err("RangeError: invalid fractionalSecondDigits");
            }
            temporal_rs::parsers::Precision::Auto
        };
    }
    Ok(o)
}
fn string_options(i: &mut Interpreter, v: Value) -> Result<ToStringRoundingOptions, VmErr> {
    let v = options(v)?;
    let mut o = string_options_prefix(i, v.clone())?;
    o.rounding_mode = mode(i, &v)?;
    o.smallest_unit = unit(i, &v, "smallestUnit")?;
    Ok(o)
}
fn time_string(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = time(&t)?;
    let opts = string_options(i, arg(&a, 0))?;
    Value::checked_string(t.to_ixdtf_string(opts).map_err(err)?)
}
fn time_json(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(time(&t)?.to_ixdtf_string(Default::default()).map_err(err)?)
}
fn duration_string(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = duration(&t)?;
    let opts = string_options(i, arg(&a, 0))?;
    Value::checked_string(t.as_temporal_string(opts).map_err(err)?)
}
fn duration_json(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(
        duration(&t)?
            .as_temporal_string(Default::default())
            .map_err(err)?,
    )
}
fn time_round(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = time(&t)?;
    let v = arg(&a, 0);
    let opts = if let Value::String(ref s) = v {
        Value::object_with_proto(
            vec![("smallestUnit".into(), Value::String(s.clone()))],
            None,
        )
    } else {
        if matches!(v, Value::Undefined) {
            return vm_err("TypeError: rounding options required");
        };
        options(v)?
    };
    let mut o = RoundingOptions::default();
    o.increment = increment(i, &opts)?;
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    if o.smallest_unit.is_none() {
        return vm_err("RangeError: smallestUnit required");
    };
    wrap(i, Slots::Time(t.round(o).map_err(err)?), false)
}
fn difference(i: &mut Interpreter, t: Value, a: Vec<Value>, since: bool) -> Result<Value, VmErr> {
    let t = time(&t)?;
    let other = to_time(i, &arg(&a, 0), Value::Undefined)?;
    let opts = options(arg(&a, 1))?;
    let mut o = DifferenceSettings::default();
    o.largest_unit = unit(i, &opts, "largestUnit")?;
    o.increment = increment(i, &opts)?;
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    let d = if since {
        t.since(&other, o)
    } else {
        t.until(&other, o)
    }
    .map_err(err)?;
    wrap(i, Slots::Duration(d), false)
}
fn time_until(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    difference(i, t, a, false)
}
fn time_since(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    difference(i, t, a, true)
}

fn relative_to(i: &mut Interpreter, opts: &Value) -> Result<Option<RelativeTo>, VmErr> {
    let value = i.get_prop_value_str(opts, "relativeTo")?;
    if matches!(value, Value::Undefined) {
        return Ok(None);
    }
    if let Some(Slots::Date(d)) = slot(&value) {
        return Ok(Some(RelativeTo::PlainDate(d)));
    }
    if let Some(Slots::Zoned(d)) = slot(&value) {
        return Ok(Some(RelativeTo::ZonedDateTime(d)));
    }
    if let Some(Slots::DateTime(d)) = slot(&value) {
        return Ok(Some(RelativeTo::PlainDate(d.to_plain_date())));
    }
    if let Value::String(text) = &value {
        return RelativeTo::try_from_str_with_provider(text.as_ref(), &*TZ)
            .map(Some)
            .map_err(err);
    }
    if !crate::interpreter::call::is_js_object(&value) {
        return vm_err("TypeError: relativeTo must be an object or string");
    }
    let cal = i.get_prop_value_str(&value, "calendar")?;
    let calendar = if matches!(cal, Value::Undefined) {
        Calendar::ISO
    } else {
        let Value::String(text) = &cal else {
            return vm_err("TypeError: calendar must be a string");
        };
        Calendar::try_from_utf8(text.to_string().as_bytes()).map_err(err)?
    };
    let mut date = PartialDate::new().with_calendar(calendar.clone());
    let mut time = PartialTime::new();
    let mut offset = None;
    let mut timezone = None;
    for key in [
        "day",
        "hour",
        "microsecond",
        "millisecond",
        "minute",
        "month",
        "monthCode",
        "nanosecond",
        "offset",
        "second",
        "timeZone",
        "year",
    ] {
        let field = i.get_prop_value_str(&value, key)?;
        if matches!(field, Value::Undefined) {
            continue;
        }
        match key {
            "monthCode" => {
                let code = i.ecmascript_to_string(&field)?;
                date.calendar_fields.month_code =
                    Some(MonthCode::try_from_utf8(code.to_string().as_bytes()).map_err(err)?);
            }
            "offset" => {
                let primitive = i.coerce_object_to_primitive(&field, "string")?;
                let Value::String(text) = &primitive else {
                    return vm_err("TypeError: offset must be a string");
                };
                offset = Some(UtcOffset::from_utf8(text.to_string().as_bytes()).map_err(err)?);
            }
            "timeZone" => {
                let Value::String(text) = &field else {
                    return vm_err("TypeError: timeZone must be a string");
                };
                timezone = Some(TimeZone::try_from_str(text.as_ref()).map_err(err)?);
            }
            _ => {
                let n = integer(i, &field, false)?;
                if key == "year" {
                    if n < i32::MIN as f64 || n > i32::MAX as f64 {
                        return vm_err("RangeError: year out of range");
                    };
                    date.calendar_fields.year = Some(n as i32);
                    continue;
                }
                if matches!(key, "month" | "day") {
                    if n <= 0. {
                        return vm_err("RangeError: month and day must be positive");
                    };
                    if key == "month" {
                        date.calendar_fields.month = Some(n.min(255.) as u8)
                    } else {
                        date.calendar_fields.day = Some(n.min(255.) as u8)
                    };
                    continue;
                }
                let n = n.clamp(0., 65535.) as u16;
                match key {
                    "hour" => time.hour = Some(n.min(255) as u8),
                    "minute" => time.minute = Some(n.min(255) as u8),
                    "second" => time.second = Some(n.min(255) as u8),
                    "millisecond" => time.millisecond = Some(n),
                    "microsecond" => time.microsecond = Some(n),
                    "nanosecond" => time.nanosecond = Some(n),
                    _ => unreachable!(),
                }
            }
        }
    }
    if let Some(timezone) = timezone {
        let mut partial = PartialZonedDateTime::new()
            .with_calendar_fields(date.calendar_fields)
            .with_time(time)
            .with_timezone(Some(timezone));
        partial.calendar = calendar;
        if let Some(offset) = offset {
            partial = partial.with_offset(offset)
        }
        let zdt = ZonedDateTime::from_partial_with_provider(
            partial,
            Some(Overflow::Constrain),
            None,
            None,
            &*TZ,
        )
        .map_err(err)?;
        Ok(Some(RelativeTo::ZonedDateTime(zdt)))
    } else {
        Ok(Some(RelativeTo::PlainDate(
            PlainDate::from_partial(date, Some(Overflow::Constrain)).map_err(err)?,
        )))
    }
}
fn shorthand(value: Value, key: &str) -> Result<Value, VmErr> {
    if let Value::String(text) = &value {
        return Ok(Value::object_with_proto(
            vec![(key.into(), Value::String(text.clone()))],
            None,
        ));
    }
    if matches!(value, Value::Undefined) {
        return vm_err("TypeError: options required");
    };
    options(value)
}
fn duration_round(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = duration(&t)?;
    let opts = shorthand(arg(&a, 0), "smallestUnit")?;
    let mut o = RoundingOptions::default();
    o.largest_unit = unit(i, &opts, "largestUnit")?;
    let relative = relative_to(i, &opts)?;
    o.increment = increment(i, &opts)?;
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    if o.largest_unit.is_none() && o.smallest_unit.is_none() {
        return vm_err("RangeError: a rounding unit is required");
    }
    let result = d.round_with_provider(o, relative, &*TZ).map_err(err)?;
    wrap(i, Slots::Duration(result), false)
}
fn duration_total(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = duration(&t)?;
    let opts = shorthand(arg(&a, 0), "unit")?;
    let relative = relative_to(i, &opts)?;
    let Some(unit) = unit(i, &opts, "unit")? else {
        return vm_err("RangeError: unit is required");
    };
    if unit == Unit::Auto {
        return vm_err("RangeError: invalid total unit");
    }
    if relative.is_none()
        && (calendar_duration(&d) || matches!(unit, Unit::Year | Unit::Month | Unit::Week))
    {
        return vm_err("RangeError: relativeTo is required for calendar units");
    }
    Ok(Value::Number(
        d.total_with_provider(unit, relative, &*TZ)
            .map_err(err)?
            .as_inner(),
    ))
}

fn calendar_duration(d: &Duration) -> bool {
    d.years() != 0 || d.months() != 0 || d.weeks() != 0
}

fn instant(value: &Value) -> Result<Instant, VmErr> {
    match slot(value) {
        Some(Slots::Instant(t)) => Ok(t),
        _ => vm_err("TypeError: Temporal.Instant receiver required"),
    }
}
fn to_instant(i: &mut Interpreter, value: &Value) -> Result<Instant, VmErr> {
    if let Some(Slots::Zoned(d)) = slot(value) {
        return Ok(d.to_instant());
    }
    if let Some(Slots::Instant(t)) = slot(value) {
        return Ok(t);
    }
    let value = i.coerce_object_to_primitive(value, "string")?;
    let Value::String(ref s) = value else {
        return vm_err("TypeError: instant must be a string");
    };
    Instant::from_utf8(s.to_string().as_bytes()).map_err(err)
}
fn instant_construct(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let n = i.ecmascript_to_bigint(&arg(&a, 0))?;
    let Value::BigInt(ref n) = n else {
        unreachable!()
    };
    let n = n
        .to_string()
        .parse::<i128>()
        .map_err(|_| VmErr::Msg("RangeError: instant out of range".into()))?;
    wrap(i, Slots::Instant(Instant::try_new(n).map_err(err)?), true)
}
fn instant_from_nanoseconds(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let _ = t;
    let n = i.ecmascript_to_bigint(&arg(&a, 0))?;
    let Value::BigInt(ref n) = n else {
        unreachable!()
    };
    let n = n
        .to_string()
        .parse::<i128>()
        .map_err(|_| VmErr::Msg("RangeError: instant out of range".into()))?;
    wrap(i, Slots::Instant(Instant::try_new(n).map_err(err)?), false)
}
fn instant_from_milliseconds(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let n = i.ecmascript_to_number(&arg(&a, 0))?;
    if !n.is_finite() || n.trunc() != n || n.abs() > 8.64e15 {
        return vm_err("RangeError: invalid epoch milliseconds");
    }
    wrap(
        i,
        Slots::Instant(Instant::from_epoch_milliseconds(n as i64).map_err(err)?),
        false,
    )
}
fn instant_from(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = to_instant(i, &arg(&a, 0))?;
    wrap(i, Slots::Instant(t), false)
}
fn instant_compare(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = to_instant(i, &arg(&a, 0))?;
    let other = to_instant(i, &arg(&a, 1))?;
    Ok(Value::Number(t.cmp(&other) as i8 as f64))
}
fn instant_milliseconds(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(instant(&t)?.epoch_milliseconds() as f64))
}
fn instant_nanoseconds(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let n = instant(&t)?.epoch_nanoseconds().as_i128();
    Ok(Value::BigInt(Rc::new(
        crate::bigint::BigInt::parse(&n.to_string())
            .map_err(|_| VmErr::Msg("RangeError: invalid BigInt".into()))?,
    )))
}
fn instant_equals(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = instant(&t)?;
    Ok(Value::Bool(t == to_instant(i, &arg(&a, 0))?))
}
fn instant_add(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = instant(&t)?;
    let d = to_duration(i, &arg(&a, 0))?;
    wrap(i, Slots::Instant(t.add(&d).map_err(err)?), false)
}
fn instant_subtract(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = instant(&t)?;
    let d = to_duration(i, &arg(&a, 0))?;
    wrap(i, Slots::Instant(t.subtract(&d).map_err(err)?), false)
}
fn instant_difference(
    i: &mut Interpreter,
    t: Value,
    a: Vec<Value>,
    since: bool,
) -> Result<Value, VmErr> {
    let t = instant(&t)?;
    let other = to_instant(i, &arg(&a, 0))?;
    let opts = options(arg(&a, 1))?;
    let mut o = DifferenceSettings::default();
    o.largest_unit = unit(i, &opts, "largestUnit")?;
    o.increment = increment(i, &opts)?;
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    let d = if since {
        t.since(&other, o)
    } else {
        t.until(&other, o)
    }
    .map_err(err)?;
    wrap(i, Slots::Duration(d), false)
}
fn instant_until(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    instant_difference(i, t, a, false)
}
fn instant_since(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    instant_difference(i, t, a, true)
}
fn instant_round(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = instant(&t)?;
    let v = arg(&a, 0);
    let opts = if let Value::String(ref s) = v {
        Value::object_with_proto(
            vec![("smallestUnit".into(), Value::String(s.clone()))],
            None,
        )
    } else {
        if matches!(v, Value::Undefined) {
            return vm_err("TypeError: rounding options required");
        }
        options(v)?
    };
    let mut o = RoundingOptions::default();
    o.increment = increment(i, &opts)?;
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    if o.smallest_unit.is_none() {
        return vm_err("RangeError: smallestUnit required");
    }
    wrap(i, Slots::Instant(t.round(o).map_err(err)?), false)
}
fn instant_json(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(
        instant(&t)?
            .to_ixdtf_string_with_provider(None, Default::default(), &*TZ)
            .map_err(err)?,
    )
}
fn instant_string(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = instant(&t)?;
    let opts = options(arg(&a, 0))?;
    let o = string_options(i, opts.clone())?;
    let zone = i.get_prop_value_str(&opts, "timeZone")?;
    let zone = if matches!(zone, Value::Undefined) {
        None
    } else {
        let Value::String(ref s) = zone else {
            return vm_err("TypeError: timeZone must be a string");
        };
        Some(TimeZone::try_from_str(s.as_ref()).map_err(err)?)
    };
    Value::checked_string(
        t.to_ixdtf_string_with_provider(zone, o, &*TZ)
            .map_err(err)?,
    )
}

fn date(v: &Value) -> Result<PlainDate, VmErr> {
    match slot(v) {
        Some(Slots::Date(d)) => Ok(d),
        _ => vm_err("TypeError: Temporal.PlainDate receiver required"),
    }
}
fn calendar_value(v: &Value) -> Result<Calendar, VmErr> {
    if let Some(Slots::Zoned(d)) = slot(v) {
        return Ok(d.calendar().clone());
    }
    match slot(v) {
        Some(Slots::YearMonth(d)) => return Ok(d.calendar().clone()),
        Some(Slots::MonthDay(d)) => return Ok(d.calendar().clone()),
        _ => {}
    }
    if let Some(Slots::DateTime(d)) = slot(v) {
        return Ok(d.calendar().clone());
    }
    if let Some(Slots::Date(d)) = slot(v) {
        return Ok(d.calendar().clone());
    }
    let Value::String(s) = v else {
        return vm_err("TypeError: calendar must be a string");
    };
    let text = s.to_string();
    if text.eq_ignore_ascii_case("islamic") {
        return vm_err("RangeError: unsupported calendar");
    }
    if let Ok(c) = Calendar::try_from_utf8(text.as_bytes()) {
        return Ok(c);
    }
    if let Ok(d) = PlainDate::from_utf8(text.as_bytes()) {
        return Ok(d.calendar().clone());
    }
    if let Ok(d) = temporal_rs::PlainYearMonth::from_utf8(text.as_bytes()) {
        return Ok(d.calendar().clone());
    }
    if let Ok(d) = temporal_rs::PlainMonthDay::from_utf8(text.as_bytes()) {
        return Ok(d.calendar().clone());
    }
    PlainTime::from_utf8(text.as_bytes()).map_err(err)?;
    // PlainTime validates annotations but does not retain the calendar.
    for annotation in text.split('[').skip(1) {
        let annotation = annotation.trim_start_matches('!');
        if let Some(id) = annotation
            .strip_prefix("u-ca=")
            .and_then(|a| a.strip_suffix(']'))
        {
            return Calendar::try_from_utf8(id.as_bytes()).map_err(err);
        }
    }
    Ok(Calendar::ISO)
}
fn calendar(i: &mut Interpreter, v: &Value) -> Result<Calendar, VmErr> {
    let c = i.get_prop_value_str(v, "calendar")?;
    if matches!(c, Value::Undefined) {
        Ok(Calendar::ISO)
    } else {
        calendar_value(&c)
    }
}
fn date_fields(
    i: &mut Interpreter,
    v: &Value,
    cal: &Calendar,
) -> Result<temporal_rs::fields::CalendarFields, VmErr> {
    let mut fields = temporal_rs::fields::CalendarFields::default();
    let names: &[&str] = if matches!(cal.identifier(), "iso8601" | "chinese" | "dangi") {
        &["day", "month", "monthCode", "year"]
    } else {
        &["day", "era", "eraYear", "month", "monthCode", "year"]
    };
    for key in names {
        let x = i.get_prop_value_str(v, key)?;
        if matches!(x, Value::Undefined) {
            continue;
        }
        read_date_field(i, &mut fields, key, &x)?;
    }
    Ok(fields)
}
fn to_date(i: &mut Interpreter, v: &Value, opts: Value) -> Result<PlainDate, VmErr> {
    if let Some(Slots::Zoned(d)) = slot(v) {
        overflow(i, opts)?;
        return Ok(d.to_plain_date());
    }
    if let Some(Slots::DateTime(d)) = slot(v) {
        overflow(i, opts)?;
        return Ok(d.to_plain_date());
    }
    if let Some(Slots::Date(d)) = slot(v) {
        overflow(i, opts)?;
        return Ok(d);
    }
    if crate::interpreter::call::is_js_object(v) {
        let cal = calendar(i, v)?;
        let f = date_fields(i, v, &cal)?;
        let o = overflow(i, opts)?;
        let mut p = PartialDate::new().with_calendar(cal);
        p.calendar_fields = f;
        return PlainDate::from_partial(p, Some(o)).map_err(err);
    }
    let Value::String(s) = v else {
        return vm_err("TypeError: date must be a string or object");
    };
    let d = PlainDate::from_utf8(s.to_string().as_bytes()).map_err(err)?;
    overflow(i, opts)?;
    Ok(d)
}
fn date_construct(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let y = integer(i, &arg(&a, 0), false)?;
    let m = integer(i, &arg(&a, 1), false)?;
    let d = integer(i, &arg(&a, 2), false)?;
    let c = arg(&a, 3);
    let c = if matches!(c, Value::Undefined) {
        Calendar::ISO
    } else {
        let Value::String(_) = &c else {
            return vm_err("TypeError: calendar must be a string");
        };
        let Value::String(ref text) = c else {
            unreachable!()
        };
        if text.to_string().eq_ignore_ascii_case("islamic") {
            return vm_err("RangeError: unsupported calendar");
        }
        Calendar::try_from_utf8(text.to_string().as_bytes()).map_err(err)?
    };
    if y < i32::MIN as f64
        || y > i32::MAX as f64
        || !(1.0..=12.).contains(&m)
        || !(1.0..=31.).contains(&d)
    {
        return vm_err("RangeError: invalid date");
    }
    wrap(
        i,
        Slots::Date(PlainDate::try_new(y as i32, m as u8, d as u8, c).map_err(err)?),
        true,
    )
}
fn date_from(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = to_date(i, &arg(&a, 0), arg(&a, 1))?;
    wrap(i, Slots::Date(d), false)
}
fn date_compare(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = to_date(i, &arg(&a, 0), Value::Undefined)?;
    let b = to_date(i, &arg(&a, 1), Value::Undefined)?;
    Ok(Value::Number(d.compare_iso(&b) as i8 as f64))
}
macro_rules! date_getters {($($f:ident=>$field:ident),*)=>{$(fn $f(_: &mut Interpreter,t:Value,_:Vec<Value>)->Result<Value,VmErr>{Ok(Value::Number(date(&t)?.$field()as f64))})*}}
date_getters!(date_year=>year,date_month=>month,date_day=>day,date_day_of_week=>day_of_week,date_day_of_year=>day_of_year,date_days_in_week=>days_in_week,date_days_in_month=>days_in_month,date_days_in_year=>days_in_year,date_months_in_year=>months_in_year);
fn date_month_code(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(date(&t)?.month_code().as_str().to_string())
}
fn date_calendar_id(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(date(&t)?.calendar().identifier().to_string())
}
fn date_week_of_year(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(date(&t)?
        .week_of_year()
        .map(|x| Value::Number(x as f64))
        .unwrap_or(Value::Undefined))
}
fn date_year_of_week(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(date(&t)?
        .year_of_week()
        .map(|x| Value::Number(x as f64))
        .unwrap_or(Value::Undefined))
}
fn date_in_leap_year(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(date(&t)?.in_leap_year()))
}
fn date_era(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(date(&t)?
        .era()
        .map(|x| Value::String(x.to_string().into()))
        .unwrap_or(Value::Undefined))
}
fn date_era_year(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(date(&t)?
        .era_year()
        .map(|x| Value::Number(x as f64))
        .unwrap_or(Value::Undefined))
}
fn date_json(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(date(&t)?.to_ixdtf_string(DisplayCalendar::Auto))
}
fn date_string(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = date(&t)?;
    let opts = options(arg(&a, 0))?;
    let c = string_option(i, &opts, "calendarName")?
        .map(|s| DisplayCalendar::from_str(&s).map_err(err))
        .transpose()?
        .unwrap_or(DisplayCalendar::Auto);
    Value::checked_string(d.to_ixdtf_string(c))
}
fn date_with_calendar(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = date(&t)?;
    let c = calendar_value(&arg(&a, 0))?;
    wrap(i, Slots::Date(d.with_calendar(c)), false)
}
fn date_with(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = date(&t)?;
    let v = arg(&a, 0);
    if !crate::interpreter::call::is_js_object(&v) || slot(&v).is_some() {
        return vm_err("TypeError: partial date object required");
    }
    for k in ["calendar", "timeZone"] {
        if !matches!(i.get_prop_value_str(&v, k)?, Value::Undefined) {
            return vm_err("TypeError: calendar and timeZone are forbidden");
        }
    }
    let f = date_fields(i, &v, d.calendar())?;
    let o = overflow(i, arg(&a, 1))?;
    wrap(i, Slots::Date(d.with(f, Some(o)).map_err(err)?), false)
}
fn date_add(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = date(&t)?;
    let v = to_duration(i, &arg(&a, 0))?;
    let o = overflow(i, arg(&a, 1))?;
    wrap(i, Slots::Date(d.add(&v, Some(o)).map_err(err)?), false)
}
fn date_subtract(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = date(&t)?;
    let v = to_duration(i, &arg(&a, 0))?;
    let o = overflow(i, arg(&a, 1))?;
    wrap(i, Slots::Date(d.subtract(&v, Some(o)).map_err(err)?), false)
}
fn date_equals(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = date(&t)?;
    Ok(Value::Bool(d == to_date(i, &arg(&a, 0), Value::Undefined)?))
}
fn date_difference(
    i: &mut Interpreter,
    t: Value,
    a: Vec<Value>,
    since: bool,
) -> Result<Value, VmErr> {
    let d = date(&t)?;
    let other = to_date(i, &arg(&a, 0), Value::Undefined)?;
    let opts = options(arg(&a, 1))?;
    let mut o = DifferenceSettings::default();
    o.largest_unit = unit(i, &opts, "largestUnit")?;
    o.increment = increment(i, &opts)?;
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    let v = if since {
        d.since(&other, o)
    } else {
        d.until(&other, o)
    }
    .map_err(err)?;
    wrap(i, Slots::Duration(v), false)
}
fn date_until(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    date_difference(i, t, a, false)
}
fn date_since(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    date_difference(i, t, a, true)
}

fn read_date_field(
    i: &mut Interpreter,
    fields: &mut temporal_rs::fields::CalendarFields,
    key: &str,
    x: &Value,
) -> Result<(), VmErr> {
    match key {
        "monthCode" => {
            let x = i.coerce_object_to_primitive(x, "string")?;
            let Value::String(ref s) = x else {
                return vm_err("TypeError: monthCode must be a string");
            };
            fields.month_code =
                Some(MonthCode::try_from_utf8(s.to_string().as_bytes()).map_err(err)?);
        }
        "era" => {
            let s = i.ecmascript_to_string(x)?;
            fields.era = Some(
                s.to_string()
                    .parse()
                    .map_err(|_| VmErr::Msg("RangeError: invalid era".into()))?,
            );
        }
        _ => {
            let n = integer(i, x, false)?;
            match key {
                "month" | "day" => {
                    if n <= 0. {
                        return vm_err("RangeError: month and day must be positive");
                    }
                    if key == "month" {
                        fields.month = Some(n.min(255.) as u8)
                    } else {
                        fields.day = Some(n.min(255.) as u8)
                    }
                }
                "year" | "eraYear" => {
                    if n < i32::MIN as f64 || n > i32::MAX as f64 {
                        return vm_err("RangeError: year out of range");
                    }
                    if key == "year" {
                        fields.year = Some(n as i32)
                    } else {
                        fields.era_year = Some(n as i32)
                    }
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(())
}
macro_rules! datetime_getters {($($f:ident=>$field:ident),*)=>{$(fn $f(_: &mut Interpreter,t:Value,_:Vec<Value>)->Result<Value,VmErr>{Ok(Value::Number(datetime(&t)?.$field()as f64))})*}}
datetime_getters!(datetime_year=>year,datetime_month=>month,datetime_day=>day,datetime_day_of_week=>day_of_week,datetime_day_of_year=>day_of_year,datetime_days_in_week=>days_in_week,datetime_days_in_month=>days_in_month,datetime_days_in_year=>days_in_year,datetime_months_in_year=>months_in_year);
fn datetime_month_code(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(datetime(&t)?.month_code().as_str().to_string())
}
fn datetime_calendar_id(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(datetime(&t)?.calendar().identifier().to_string())
}
fn datetime_week_of_year(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(datetime(&t)?
        .week_of_year()
        .map(|x| Value::Number(x as f64))
        .unwrap_or(Value::Undefined))
}
fn datetime_year_of_week(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(datetime(&t)?
        .year_of_week()
        .map(|x| Value::Number(x as f64))
        .unwrap_or(Value::Undefined))
}
fn datetime_in_leap_year(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(datetime(&t)?.in_leap_year()))
}
fn datetime_era(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(datetime(&t)?
        .era()
        .map(|x| Value::String(x.to_string().into()))
        .unwrap_or(Value::Undefined))
}
fn datetime_era_year(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(datetime(&t)?
        .era_year()
        .map(|x| Value::Number(x as f64))
        .unwrap_or(Value::Undefined))
}

fn datetime(v: &Value) -> Result<PlainDateTime, VmErr> {
    match slot(v) {
        Some(Slots::DateTime(d)) => Ok(d),
        _ => vm_err("TypeError: Temporal.PlainDateTime receiver required"),
    }
}
fn datetime_fields(
    i: &mut Interpreter,
    v: &Value,
    cal: &Calendar,
) -> Result<(temporal_rs::fields::CalendarFields, Vec<Option<f64>>), VmErr> {
    let mut f = temporal_rs::fields::CalendarFields::default();
    let mut time = vec![None; 6];
    for key in [
        "day",
        "era",
        "eraYear",
        "hour",
        "microsecond",
        "millisecond",
        "minute",
        "month",
        "monthCode",
        "nanosecond",
        "second",
        "year",
    ] {
        if matches!(key, "era" | "eraYear")
            && matches!(cal.identifier(), "iso8601" | "chinese" | "dangi")
        {
            continue;
        }
        let x = i.get_prop_value_str(v, key)?;
        if matches!(x, Value::Undefined) {
            continue;
        }
        if let Some(n) = TIME_FIELDS.iter().position(|s| *s == key) {
            time[n] = Some(integer(i, &x, false)?);
        } else {
            read_date_field(i, &mut f, key, &x)?;
        }
    }
    Ok((f, time))
}
fn partial_time(n: &[Option<f64>], o: Overflow) -> Result<PartialTime, VmErr> {
    let vals: Vec<_> = n.iter().map(|n| n.unwrap_or(0.)).collect();
    let t = make_time(&vals, o)?;
    Ok(PartialTime {
        hour: n[0].map(|_| t.hour()),
        minute: n[1].map(|_| t.minute()),
        second: n[2].map(|_| t.second()),
        millisecond: n[3].map(|_| t.millisecond()),
        microsecond: n[4].map(|_| t.microsecond()),
        nanosecond: n[5].map(|_| t.nanosecond()),
    })
}
fn to_datetime(i: &mut Interpreter, v: &Value, opts: Value) -> Result<PlainDateTime, VmErr> {
    if let Some(Slots::Zoned(d)) = slot(v) {
        overflow(i, opts)?;
        return Ok(d.to_plain_date_time());
    }
    if let Some(Slots::DateTime(d)) = slot(v) {
        overflow(i, opts)?;
        return Ok(d);
    }
    if let Some(Slots::Date(d)) = slot(v) {
        overflow(i, opts)?;
        return d.to_plain_date_time(None).map_err(err);
    }
    if crate::interpreter::call::is_js_object(v) {
        let cal = calendar(i, v)?;
        let (f, t) = datetime_fields(i, v, &cal)?;
        let o = overflow(i, opts)?;
        return PlainDateTime::from_partial(
            temporal_rs::partial::PartialDateTime {
                calendar: cal,
                fields: temporal_rs::fields::DateTimeFields {
                    calendar_fields: f,
                    time: partial_time(&t, o)?,
                },
            },
            Some(o),
        )
        .map_err(err);
    }
    let Value::String(s) = v else {
        return vm_err("TypeError: date-time must be a string or object");
    };
    let d = PlainDateTime::from_utf8(s.to_string().as_bytes()).map_err(err)?;
    overflow(i, opts)?;
    Ok(d)
}
fn datetime_construct(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let mut n = vec![];
    for x in 0..9 {
        let v = arg(&a, x);
        n.push(if x >= 3 && matches!(v, Value::Undefined) {
            0.
        } else {
            integer(i, &v, false)?
        });
    }
    let c = arg(&a, 9);
    let c = if matches!(c, Value::Undefined) {
        Calendar::ISO
    } else {
        let Value::String(ref s) = c else {
            return vm_err("TypeError: calendar must be a string");
        };
        if s.to_string().eq_ignore_ascii_case("islamic") {
            return vm_err("RangeError: unsupported calendar");
        }
        Calendar::try_from_utf8(s.to_string().as_bytes()).map_err(err)?
    };
    if n[0] < i32::MIN as f64
        || n[0] > i32::MAX as f64
        || !(1.0..=12.).contains(&n[1])
        || !(1.0..=31.).contains(&n[2])
    {
        return vm_err("RangeError: invalid date");
    }
    let d = PlainDate::try_new(n[0] as i32, n[1] as u8, n[2] as u8, c).map_err(err)?;
    let t = make_time(&n[3..], Overflow::Reject)?;
    wrap(
        i,
        Slots::DateTime(PlainDateTime::from_date_and_time(d, t).map_err(err)?),
        true,
    )
}
fn datetime_from(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = to_datetime(i, &arg(&a, 0), arg(&a, 1))?;
    wrap(i, Slots::DateTime(d), false)
}
fn datetime_compare(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = to_datetime(i, &arg(&a, 0), Value::Undefined)?;
    let b = to_datetime(i, &arg(&a, 1), Value::Undefined)?;
    Ok(Value::Number(d.compare_iso(&b) as i8 as f64))
}
fn datetime_json(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(
        datetime(&t)?
            .to_ixdtf_string(Default::default(), DisplayCalendar::Auto)
            .map_err(err)?,
    )
}
fn datetime_string(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = datetime(&t)?;
    let opts = options(arg(&a, 0))?;
    let c = string_option(i, &opts, "calendarName")?
        .map(|s| DisplayCalendar::from_str(&s).map_err(err))
        .transpose()?
        .unwrap_or(DisplayCalendar::Auto);
    let o = string_options(i, opts)?;
    Value::checked_string(d.to_ixdtf_string(o, c).map_err(err)?)
}
fn datetime_with_calendar(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = datetime(&t)?;
    let c = calendar_value(&arg(&a, 0))?;
    wrap(i, Slots::DateTime(d.with_calendar(c)), false)
}
fn datetime_with(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = datetime(&t)?;
    let v = arg(&a, 0);
    if !crate::interpreter::call::is_js_object(&v) || slot(&v).is_some() {
        return vm_err("TypeError: partial date-time required");
    }
    for k in ["calendar", "timeZone"] {
        if !matches!(i.get_prop_value_str(&v, k)?, Value::Undefined) {
            return vm_err("TypeError: calendar and timeZone forbidden");
        }
    }
    let (f, t) = datetime_fields(i, &v, d.calendar())?;
    let o = overflow(i, arg(&a, 1))?;
    let d = d
        .with(
            temporal_rs::fields::DateTimeFields {
                calendar_fields: f,
                time: partial_time(&t, o)?,
            },
            Some(o),
        )
        .map_err(err)?;
    wrap(i, Slots::DateTime(d), false)
}
fn datetime_with_time(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = datetime(&t)?;
    let v = arg(&a, 0);
    let t = if matches!(v, Value::Undefined) {
        None
    } else {
        Some(to_time(i, &v, Value::Undefined)?)
    };
    wrap(i, Slots::DateTime(d.with_time(t).map_err(err)?), false)
}
fn datetime_to_date(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let d = datetime(&t)?.to_plain_date();
    wrap(i, Slots::Date(d), false)
}
fn datetime_to_time(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let d = datetime(&t)?.to_plain_time();
    wrap(i, Slots::Time(d), false)
}
fn date_to_datetime(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = date(&t)?;
    let v = arg(&a, 0);
    let t = if matches!(v, Value::Undefined) {
        None
    } else {
        Some(to_time(i, &v, Value::Undefined)?)
    };
    wrap(
        i,
        Slots::DateTime(d.to_plain_date_time(t).map_err(err)?),
        false,
    )
}
fn datetime_add(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = datetime(&t)?;
    let v = to_duration(i, &arg(&a, 0))?;
    let o = overflow(i, arg(&a, 1))?;
    wrap(i, Slots::DateTime(d.add(&v, Some(o)).map_err(err)?), false)
}
fn datetime_subtract(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = datetime(&t)?;
    let v = to_duration(i, &arg(&a, 0))?;
    let o = overflow(i, arg(&a, 1))?;
    wrap(
        i,
        Slots::DateTime(d.subtract(&v, Some(o)).map_err(err)?),
        false,
    )
}
fn datetime_equals(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = datetime(&t)?;
    Ok(Value::Bool(
        d == to_datetime(i, &arg(&a, 0), Value::Undefined)?,
    ))
}
fn datetime_difference(
    i: &mut Interpreter,
    t: Value,
    a: Vec<Value>,
    since: bool,
) -> Result<Value, VmErr> {
    let d = datetime(&t)?;
    let other = to_datetime(i, &arg(&a, 0), Value::Undefined)?;
    let opts = options(arg(&a, 1))?;
    let mut o = DifferenceSettings::default();
    o.largest_unit = unit(i, &opts, "largestUnit")?;
    o.increment = increment(i, &opts)?;
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    let v = if since {
        d.since(&other, o)
    } else {
        d.until(&other, o)
    }
    .map_err(err)?;
    wrap(i, Slots::Duration(v), false)
}
fn datetime_until(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    datetime_difference(i, t, a, false)
}
fn datetime_since(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    datetime_difference(i, t, a, true)
}
fn datetime_round(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = datetime(&t)?;
    let v = arg(&a, 0);
    let opts = if let Value::String(ref s) = v {
        Value::object_with_proto(
            vec![("smallestUnit".into(), Value::String(s.clone()))],
            None,
        )
    } else {
        if matches!(v, Value::Undefined) {
            return vm_err("TypeError: rounding options required");
        }
        options(v)?
    };
    let mut o = RoundingOptions::default();
    o.increment = increment(i, &opts)?;
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    if o.smallest_unit.is_none() {
        return vm_err("RangeError: smallestUnit required");
    }
    wrap(i, Slots::DateTime(t.round(o).map_err(err)?), false)
}

datetime_getters!(datetime_hour=>hour,datetime_minute=>minute,datetime_second=>second,datetime_millisecond=>millisecond,datetime_microsecond=>microsecond,datetime_nanosecond=>nanosecond);
fn yearmonth_fields(
    i: &mut Interpreter,
    v: &Value,
    cal: &Calendar,
) -> Result<temporal_rs::fields::CalendarFields, VmErr> {
    let mut fields = temporal_rs::fields::CalendarFields::default();
    let names: &[&str] = if matches!(cal.identifier(), "iso8601" | "chinese" | "dangi") {
        &["day", "month", "monthCode", "year"]
    } else {
        &["day", "era", "eraYear", "month", "monthCode", "year"]
    };
    for key in names {
        if *key == "day" {
            continue;
        }
        let x = i.get_prop_value_str(v, key)?;
        if matches!(x, Value::Undefined) {
            continue;
        }
        read_date_field(i, &mut fields, key, &x)?;
    }
    Ok(fields)
}
fn yearmonth_json(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(yearmonth(&t)?.to_ixdtf_string(DisplayCalendar::Auto))
}
fn yearmonth_string(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = yearmonth(&t)?;
    let opts = options(arg(&a, 0))?;
    let c = string_option(i, &opts, "calendarName")?
        .map(|s| DisplayCalendar::from_str(&s).map_err(err))
        .transpose()?
        .unwrap_or(DisplayCalendar::Auto);
    Value::checked_string(d.to_ixdtf_string(c))
}
fn yearmonth_add(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = yearmonth(&t)?;
    let v = to_duration(i, &arg(&a, 0))?;
    let o = overflow(i, arg(&a, 1))?;
    wrap(i, Slots::YearMonth(d.add(&v, o).map_err(err)?), false)
}
fn yearmonth_subtract(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = yearmonth(&t)?;
    let v = to_duration(i, &arg(&a, 0))?;
    let o = overflow(i, arg(&a, 1))?;
    wrap(i, Slots::YearMonth(d.subtract(&v, o).map_err(err)?), false)
}
fn yearmonth_equals(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = yearmonth(&t)?;
    Ok(Value::Bool(
        d == to_yearmonth(i, &arg(&a, 0), Value::Undefined)?,
    ))
}
fn yearmonth_difference(
    i: &mut Interpreter,
    t: Value,
    a: Vec<Value>,
    since: bool,
) -> Result<Value, VmErr> {
    let d = yearmonth(&t)?;
    let other = to_yearmonth(i, &arg(&a, 0), Value::Undefined)?;
    let opts = options(arg(&a, 1))?;
    let mut o = DifferenceSettings::default();
    o.largest_unit = unit(i, &opts, "largestUnit")?;
    o.increment = increment(i, &opts)?;
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    let v = if since {
        d.since(&other, o)
    } else {
        d.until(&other, o)
    }
    .map_err(err)?;
    wrap(i, Slots::Duration(v), false)
}
fn yearmonth_until(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    yearmonth_difference(i, t, a, false)
}
fn yearmonth_since(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    yearmonth_difference(i, t, a, true)
}

fn yearmonth(v: &Value) -> Result<PlainYearMonth, VmErr> {
    match slot(v) {
        Some(Slots::YearMonth(d)) => Ok(d),
        _ => vm_err("TypeError: Temporal.PlainYearMonth receiver required"),
    }
}
fn monthday(v: &Value) -> Result<PlainMonthDay, VmErr> {
    match slot(v) {
        Some(Slots::MonthDay(d)) => Ok(d),
        _ => vm_err("TypeError: Temporal.PlainMonthDay receiver required"),
    }
}
macro_rules! yearmonth_getters {($($f:ident=>$field:ident),*)=>{$(fn $f(_: &mut Interpreter,t:Value,_:Vec<Value>)->Result<Value,VmErr>{Ok(Value::Number(yearmonth(&t)?.$field()as f64))})*}}
yearmonth_getters!(yearmonth_year=>year,yearmonth_month=>month,yearmonth_days_in_month=>days_in_month,yearmonth_days_in_year=>days_in_year,yearmonth_months_in_year=>months_in_year);
fn yearmonth_month_code(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(yearmonth(&t)?.month_code().as_str().to_string())
}
fn yearmonth_calendar_id(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(yearmonth(&t)?.calendar().identifier().to_string())
}
fn yearmonth_in_leap_year(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(yearmonth(&t)?.in_leap_year()))
}
fn yearmonth_era(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(yearmonth(&t)?
        .era()
        .map(|x| Value::String(x.to_string().into()))
        .unwrap_or(Value::Undefined))
}
fn yearmonth_era_year(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(yearmonth(&t)?
        .era_year()
        .map(|x| Value::Number(x as f64))
        .unwrap_or(Value::Undefined))
}

fn constructor_calendar(v: &Value) -> Result<Calendar, VmErr> {
    if matches!(v, Value::Undefined) {
        return Ok(Calendar::ISO);
    }
    let Value::String(s) = v else {
        return vm_err("TypeError: calendar must be a string");
    };
    if s.to_string().eq_ignore_ascii_case("islamic") {
        return vm_err("RangeError: unsupported calendar");
    }
    Calendar::try_from_utf8(s.to_string().as_bytes()).map_err(err)
}
fn yearmonth_construct(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let y = integer(i, &arg(&a, 0), false)?;
    let m = integer(i, &arg(&a, 1), false)?;
    let c = constructor_calendar(&arg(&a, 2))?;
    let v = arg(&a, 3);
    let d = if matches!(v, Value::Undefined) {
        1.
    } else {
        integer(i, &v, false)?
    };
    if y < i32::MIN as f64
        || y > i32::MAX as f64
        || !(1.0..=12.).contains(&m)
        || !(1.0..=31.).contains(&d)
    {
        return vm_err("RangeError: invalid year month");
    }
    wrap(
        i,
        Slots::YearMonth(
            PlainYearMonth::try_new(y as i32, m as u8, Some(d as u8), c).map_err(err)?,
        ),
        true,
    )
}
fn monthday_construct(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let m = integer(i, &arg(&a, 0), false)?;
    let d = integer(i, &arg(&a, 1), false)?;
    let c = constructor_calendar(&arg(&a, 2))?;
    let v = arg(&a, 3);
    let y = if matches!(v, Value::Undefined) {
        1972.
    } else {
        integer(i, &v, false)?
    };
    if y < i32::MIN as f64
        || y > i32::MAX as f64
        || !(1.0..=12.).contains(&m)
        || !(1.0..=31.).contains(&d)
    {
        return vm_err("RangeError: invalid month day");
    }
    wrap(
        i,
        Slots::MonthDay(
            PlainMonthDay::new_with_overflow(m as u8, d as u8, c, Overflow::Reject, Some(y as i32))
                .map_err(err)?,
        ),
        true,
    )
}
fn to_yearmonth(i: &mut Interpreter, v: &Value, opts: Value) -> Result<PlainYearMonth, VmErr> {
    if let Some(Slots::YearMonth(d)) = slot(v) {
        overflow(i, opts)?;
        return Ok(d);
    }
    if crate::interpreter::call::is_js_object(v) {
        let c = calendar(i, v)?;
        let f = yearmonth_fields(i, v, &c)?;
        let o = overflow(i, opts)?;
        return PlainYearMonth::from_partial(
            temporal_rs::partial::PartialYearMonth {
                calendar: c,
                calendar_fields: f.into(),
            },
            Some(o),
        )
        .map_err(err);
    }
    let Value::String(s) = v else {
        return vm_err("TypeError: year-month must be a string or object");
    };
    let d = PlainYearMonth::from_utf8(s.to_string().as_bytes()).map_err(err)?;
    overflow(i, opts)?;
    Ok(d)
}
fn to_monthday(i: &mut Interpreter, v: &Value, opts: Value) -> Result<PlainMonthDay, VmErr> {
    if let Some(Slots::MonthDay(d)) = slot(v) {
        overflow(i, opts)?;
        return Ok(d);
    }
    if crate::interpreter::call::is_js_object(v) {
        let c = calendar(i, v)?;
        let f = date_fields(i, v, &c)?;
        let o = overflow(i, opts)?;
        let mut p = PartialDate::new().with_calendar(c);
        p.calendar_fields = f;
        return PlainMonthDay::from_partial(p, Some(o)).map_err(err);
    }
    let Value::String(s) = v else {
        return vm_err("TypeError: month-day must be a string or object");
    };
    let d = PlainMonthDay::from_utf8(s.to_string().as_bytes()).map_err(err)?;
    overflow(i, opts)?;
    Ok(d)
}
fn yearmonth_from(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = to_yearmonth(i, &arg(&a, 0), arg(&a, 1))?;
    wrap(i, Slots::YearMonth(d), false)
}
fn monthday_from(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = to_monthday(i, &arg(&a, 0), arg(&a, 1))?;
    wrap(i, Slots::MonthDay(d), false)
}
fn yearmonth_compare(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = to_yearmonth(i, &arg(&a, 0), Value::Undefined)?;
    let b = to_yearmonth(i, &arg(&a, 1), Value::Undefined)?;
    Ok(Value::Number(d.compare_iso(&b) as i8 as f64))
}
fn yearmonth_with(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = yearmonth(&t)?;
    let v = arg(&a, 0);
    partial_temporal(i, &v)?;
    let f = yearmonth_fields(i, &v, d.calendar())?;
    let o = overflow(i, arg(&a, 1))?;
    wrap(
        i,
        Slots::YearMonth(d.with(f.into(), Some(o)).map_err(err)?),
        false,
    )
}
fn partial_temporal(i: &mut Interpreter, v: &Value) -> Result<(), VmErr> {
    if !crate::interpreter::call::is_js_object(v) || slot(v).is_some() {
        return vm_err("TypeError: partial Temporal object required");
    }
    for k in ["calendar", "timeZone"] {
        if !matches!(i.get_prop_value_str(v, k)?, Value::Undefined) {
            return vm_err("TypeError: calendar and timeZone forbidden");
        }
    }
    Ok(())
}
fn monthday_with(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = monthday(&t)?;
    let v = arg(&a, 0);
    partial_temporal(i, &v)?;
    let f = date_fields(i, &v, d.calendar())?;
    let o = overflow(i, arg(&a, 1))?;
    wrap(i, Slots::MonthDay(d.with(f, Some(o)).map_err(err)?), false)
}
fn yearmonth_to_date(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = yearmonth(&t)?;
    let v = arg(&a, 0);
    if !crate::interpreter::call::is_js_object(&v) {
        return vm_err("TypeError: date fields required");
    }
    let mut f = temporal_rs::fields::CalendarFields::default();
    let x = i.get_prop_value_str(&v, "day")?;
    if matches!(x, Value::Undefined) {
        return vm_err("TypeError: day required");
    }
    read_date_field(i, &mut f, "day", &x)?;
    wrap(
        i,
        Slots::Date(d.to_plain_date(Some(f)).map_err(err)?),
        false,
    )
}
fn monthday_to_date(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = monthday(&t)?;
    let v = arg(&a, 0);
    if !crate::interpreter::call::is_js_object(&v) {
        return vm_err("TypeError: date fields required");
    }
    let mut f = temporal_rs::fields::CalendarFields::default();
    let keys: &[&str] = if matches!(d.calendar_id(), "iso8601" | "chinese" | "dangi") {
        &["year"]
    } else {
        &["era", "eraYear", "year"]
    };
    for k in keys {
        let x = i.get_prop_value_str(&v, k)?;
        if !matches!(x, Value::Undefined) {
            read_date_field(i, &mut f, k, &x)?;
        }
    }
    wrap(
        i,
        Slots::Date(d.to_plain_date(Some(f)).map_err(err)?),
        false,
    )
}
fn monthday_day(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(monthday(&t)?.day() as f64))
}
fn monthday_month_code(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(monthday(&t)?.month_code().as_str().to_string())
}
fn monthday_calendar_id(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(monthday(&t)?.calendar_id().to_string())
}
fn monthday_equals(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = monthday(&t)?;
    Ok(Value::Bool(
        d == to_monthday(i, &arg(&a, 0), Value::Undefined)?,
    ))
}
fn monthday_json(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(monthday(&t)?.to_ixdtf_string(DisplayCalendar::Auto))
}
fn monthday_string(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = monthday(&t)?;
    let opts = options(arg(&a, 0))?;
    let c = string_option(i, &opts, "calendarName")?
        .map(|s| DisplayCalendar::from_str(&s).map_err(err))
        .transpose()?
        .unwrap_or(DisplayCalendar::Auto);
    Value::checked_string(d.to_ixdtf_string(c))
}
fn date_to_yearmonth(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let d = date(&t)?.to_plain_year_month().map_err(err)?;
    wrap(i, Slots::YearMonth(d), false)
}
fn date_to_monthday(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let d = date(&t)?.to_plain_month_day().map_err(err)?;
    wrap(i, Slots::MonthDay(d), false)
}
macro_rules! zoned_getters {($($f:ident=>$field:ident),*)=>{$(fn $f(_: &mut Interpreter,t:Value,_:Vec<Value>)->Result<Value,VmErr>{Ok(Value::Number(zoned(&t)?.$field()as f64))})*}}
zoned_getters!(zoned_year=>year,zoned_month=>month,zoned_day=>day,zoned_day_of_week=>day_of_week,zoned_day_of_year=>day_of_year,zoned_days_in_week=>days_in_week,zoned_days_in_month=>days_in_month,zoned_days_in_year=>days_in_year,zoned_months_in_year=>months_in_year);
fn zoned_month_code(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(zoned(&t)?.month_code().as_str().to_string())
}
fn zoned_calendar_id(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(zoned(&t)?.calendar().identifier().to_string())
}
fn zoned_week_of_year(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(zoned(&t)?
        .week_of_year()
        .map(|x| Value::Number(x as f64))
        .unwrap_or(Value::Undefined))
}
fn zoned_year_of_week(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(zoned(&t)?
        .year_of_week()
        .map(|x| Value::Number(x as f64))
        .unwrap_or(Value::Undefined))
}
fn zoned_in_leap_year(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Bool(zoned(&t)?.in_leap_year()))
}
fn zoned_era(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(zoned(&t)?
        .era()
        .map(|x| Value::String(x.to_string().into()))
        .unwrap_or(Value::Undefined))
}
fn zoned_era_year(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(zoned(&t)?
        .era_year()
        .map(|x| Value::Number(x as f64))
        .unwrap_or(Value::Undefined))
}

zoned_getters!(zoned_hour=>hour,zoned_minute=>minute,zoned_second=>second,zoned_millisecond=>millisecond,zoned_microsecond=>microsecond,zoned_nanosecond=>nanosecond,zoned_offset_nanoseconds=>offset_nanoseconds);

fn zoned(v: &Value) -> Result<ZonedDateTime, VmErr> {
    match slot(v) {
        Some(Slots::Zoned(d)) => Ok(d),
        _ => vm_err("TypeError: Temporal.ZonedDateTime receiver required"),
    }
}
fn timezone(v: &Value) -> Result<TimeZone, VmErr> {
    if let Some(Slots::Zoned(d)) = slot(v) {
        return Ok(*d.time_zone());
    }
    let Value::String(s) = v else {
        return vm_err("TypeError: time zone must be a string");
    };
    TimeZone::try_from_str(s.as_ref()).map_err(err)
}
fn disambiguation(i: &mut Interpreter, v: &Value) -> Result<Option<Disambiguation>, VmErr> {
    string_option(i, v, "disambiguation")?
        .map(|s| {
            Disambiguation::from_str(&s)
                .map_err(|_| VmErr::Msg("RangeError: invalid disambiguation".into()))
        })
        .transpose()
}
fn offset_option(i: &mut Interpreter, v: &Value) -> Result<Option<OffsetDisambiguation>, VmErr> {
    string_option(i, v, "offset")?
        .map(|s| {
            OffsetDisambiguation::from_str(&s)
                .map_err(|_| VmErr::Msg("RangeError: invalid disambiguation".into()))
        })
        .transpose()
}
fn zoned_options(
    i: &mut Interpreter,
    v: Value,
) -> Result<
    (
        Option<Disambiguation>,
        Option<OffsetDisambiguation>,
        Overflow,
    ),
    VmErr,
> {
    let v = options(v)?;
    let d = disambiguation(i, &v)?;
    let o = offset_option(i, &v)?;
    let f = overflow(i, v)?;
    Ok((d, o, f))
}
fn epoch_bigint(i: &mut Interpreter, v: &Value) -> Result<i128, VmErr> {
    let n = i.ecmascript_to_bigint(v)?;
    let Value::BigInt(ref n) = n else {
        unreachable!()
    };
    n.to_string()
        .parse()
        .map_err(|_| VmErr::Msg("RangeError: epoch nanoseconds out of range".into()))
}
fn zoned_construct(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let n = epoch_bigint(i, &arg(&a, 0))?;
    let z = arg(&a, 1);
    if !matches!(z, Value::String(_)) {
        return vm_err("TypeError: time zone must be a string");
    }
    let Value::String(ref text) = z else {
        unreachable!()
    };
    let z = TimeZone::try_from_identifier_str_with_provider(text.as_ref(), &*TZ).map_err(err)?;
    let c = constructor_calendar(&arg(&a, 2))?;
    wrap(
        i,
        Slots::Zoned(ZonedDateTime::try_new_with_provider(n, z, c, &*TZ).map_err(err)?),
        true,
    )
}
type ZonedFields = (
    temporal_rs::fields::CalendarFields,
    Vec<Option<f64>>,
    Option<UtcOffset>,
    Option<TimeZone>,
);
fn zoned_fields(
    i: &mut Interpreter,
    v: &Value,
    c: &Calendar,
    with: bool,
) -> Result<ZonedFields, VmErr> {
    let mut f = temporal_rs::fields::CalendarFields::default();
    let mut t = vec![None; 6];
    let mut o = None;
    let mut z = None;
    for key in [
        "day",
        "era",
        "eraYear",
        "hour",
        "microsecond",
        "millisecond",
        "minute",
        "month",
        "monthCode",
        "nanosecond",
        "offset",
        "second",
        "timeZone",
        "year",
    ] {
        if (with && key == "timeZone")
            || (matches!(key, "era" | "eraYear")
                && matches!(c.identifier(), "iso8601" | "chinese" | "dangi"))
        {
            continue;
        }
        let x = i.get_prop_value_str(v, key)?;
        if matches!(x, Value::Undefined) {
            continue;
        }
        match key {
            "offset" => {
                let x = i.coerce_object_to_primitive(&x, "string")?;
                let Value::String(ref s) = x else {
                    return vm_err("TypeError: offset must be a string");
                };
                o = Some(UtcOffset::from_utf8(s.to_string().as_bytes()).map_err(err)?);
            }
            "timeZone" => z = Some(timezone(&x)?),
            _ => {
                if let Some(n) = TIME_FIELDS.iter().position(|s| *s == key) {
                    t[n] = Some(integer(i, &x, false)?);
                } else {
                    read_date_field(i, &mut f, key, &x)?;
                }
            }
        }
    }
    Ok((f, t, o, z))
}
fn to_zoned(i: &mut Interpreter, v: &Value, opts: Value) -> Result<ZonedDateTime, VmErr> {
    if let Some(Slots::Zoned(d)) = slot(v) {
        zoned_options(i, opts)?;
        return Ok(d);
    }
    if crate::interpreter::call::is_js_object(v) {
        let c = calendar(i, v)?;
        let (f, t, o, z) = zoned_fields(i, v, &c, false)?;
        if z.is_none() {
            return vm_err("TypeError: timeZone required");
        }
        let (d, off, over) = zoned_options(i, opts)?;
        let p = PartialZonedDateTime {
            calendar: c,
            timezone: z,
            fields: temporal_rs::fields::ZonedDateTimeFields {
                calendar_fields: f,
                time: partial_time(&t, over)?,
                offset: o,
            },
        };
        return ZonedDateTime::from_partial_with_provider(p, Some(over), d, off, &*TZ).map_err(err);
    }
    let Value::String(s) = v else {
        return vm_err("TypeError: zoned date-time must be a string or object");
    };
    // Parse and validate before reading options; resolution follows their values.
    let parsed = temporal_rs::parsed_intermediates::ParsedZonedDateTime::from_utf8_with_provider(
        s.to_string().as_bytes(),
        &*TZ,
    )
    .map_err(err)?;
    let (d, o, _) = zoned_options(i, opts)?;
    ZonedDateTime::from_parsed_with_provider(
        parsed,
        d.unwrap_or(Disambiguation::Compatible),
        o.unwrap_or(OffsetDisambiguation::Reject),
        &*TZ,
    )
    .map_err(err)
}
fn zoned_from(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = to_zoned(i, &arg(&a, 0), arg(&a, 1))?;
    wrap(i, Slots::Zoned(d), false)
}
fn zoned_compare(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = to_zoned(i, &arg(&a, 0), Value::Undefined)?;
    let b = to_zoned(i, &arg(&a, 1), Value::Undefined)?;
    Ok(Value::Number(d.compare_instant(&b) as i8 as f64))
}
fn zoned_milliseconds(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(zoned(&t)?.epoch_milliseconds() as f64))
}
fn zoned_nanoseconds(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let n = zoned(&t)?.epoch_nanoseconds().as_i128();
    Ok(Value::BigInt(Rc::new(
        crate::bigint::BigInt::parse(&n.to_string())
            .map_err(|_| VmErr::Msg("RangeError: invalid BigInt".into()))?,
    )))
}
fn zoned_timezone_id(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(
        zoned(&t)?
            .time_zone()
            .identifier_with_provider(&*TZ)
            .map_err(err)?,
    )
}
fn zoned_offset(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(zoned(&t)?.offset())
}
fn zoned_hours_in_day(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Ok(Value::Number(
        zoned(&t)?.hours_in_day_with_provider(&*TZ).map_err(err)?,
    ))
}
fn zoned_json(_: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string(zoned(&t)?.to_string_with_provider(&*TZ).map_err(err)?)
}
fn zoned_string(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?;
    let opts = options(arg(&a, 0))?;
    let c = string_option(i, &opts, "calendarName")?
        .map(|s| DisplayCalendar::from_str(&s).map_err(err))
        .transpose()?
        .unwrap_or(DisplayCalendar::Auto);
    // Read the remaining formatting options in alphabetical order.
    let mut o = string_options_prefix(i, opts.clone())?;
    let off = string_option(i, &opts, "offset")?
        .map(|s| DisplayOffset::from_str(&s).map_err(err))
        .transpose()?
        .unwrap_or(DisplayOffset::Auto);
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    let tz = string_option(i, &opts, "timeZoneName")?
        .map(|s| DisplayTimeZone::from_str(&s).map_err(err))
        .transpose()?
        .unwrap_or(DisplayTimeZone::Auto);
    Value::checked_string(
        d.to_ixdtf_string_with_provider(off, tz, c, o, &*TZ)
            .map_err(err)?,
    )
}
fn zoned_add(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?;
    let v = to_duration(i, &arg(&a, 0))?;
    let o = overflow(i, arg(&a, 1))?;
    wrap(
        i,
        Slots::Zoned(d.add_with_provider(&v, Some(o), &*TZ).map_err(err)?),
        false,
    )
}
fn zoned_subtract(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?;
    let v = to_duration(i, &arg(&a, 0))?;
    let o = overflow(i, arg(&a, 1))?;
    wrap(
        i,
        Slots::Zoned(d.subtract_with_provider(&v, Some(o), &*TZ).map_err(err)?),
        false,
    )
}
fn zoned_equals(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?;
    let v = to_zoned(i, &arg(&a, 0), Value::Undefined)?;
    Ok(Value::Bool(d.equals_with_provider(&v, &*TZ).map_err(err)?))
}
fn zoned_with(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?;
    let v = arg(&a, 0);
    partial_temporal(i, &v)?;
    let (f, t, off, _) = zoned_fields(i, &v, d.calendar(), true)?;
    let (dis, o, over) = zoned_options(i, arg(&a, 1))?;
    wrap(
        i,
        Slots::Zoned(
            d.with_with_provider(
                temporal_rs::fields::ZonedDateTimeFields {
                    calendar_fields: f,
                    time: partial_time(&t, over)?,
                    offset: off,
                },
                dis,
                o.or(Some(OffsetDisambiguation::Prefer)),
                Some(over),
                &*TZ,
            )
            .map_err(err)?,
        ),
        false,
    )
}
fn zoned_with_timezone(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?;
    let z = timezone(&arg(&a, 0))?;
    wrap(
        i,
        Slots::Zoned(d.with_time_zone_with_provider(z, &*TZ).map_err(err)?),
        false,
    )
}
fn zoned_with_calendar(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?;
    let c = calendar_value(&arg(&a, 0))?;
    wrap(i, Slots::Zoned(d.with_calendar(c)), false)
}
fn zoned_with_time(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?;
    let v = arg(&a, 0);
    let t = if matches!(v, Value::Undefined) {
        None
    } else {
        Some(to_time(i, &v, Value::Undefined)?)
    };
    wrap(
        i,
        Slots::Zoned(d.with_plain_time_and_provider(t, &*TZ).map_err(err)?),
        false,
    )
}
fn zoned_start_of_day(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?.start_of_day_with_provider(&*TZ).map_err(err)?;
    wrap(i, Slots::Zoned(d), false)
}
fn zoned_transition(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?;
    let v = arg(&a, 0);
    if matches!(v, Value::Undefined) {
        return vm_err("TypeError: transition options required");
    }
    let s = if let Value::String(ref s) = v {
        s.to_string()
    } else {
        let v = options(v)?;
        string_option(i, &v, "direction")?
            .ok_or_else(|| VmErr::Msg("RangeError: direction required".into()))?
    };
    let dir = match s.as_str() {
        "next" => temporal_rs::provider::TransitionDirection::Next,
        "previous" => temporal_rs::provider::TransitionDirection::Previous,
        _ => return vm_err("RangeError: invalid direction"),
    };
    match d
        .get_time_zone_transition_with_provider(dir, &*TZ)
        .map_err(err)?
    {
        Some(d) => wrap(i, Slots::Zoned(d), false),
        None => Ok(Value::Null),
    }
}
fn zoned_to_date(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?.to_plain_date();
    wrap(i, Slots::Date(d), false)
}
fn zoned_to_time(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?.to_plain_time();
    wrap(i, Slots::Time(d), false)
}
fn zoned_to_datetime(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?.to_plain_date_time();
    wrap(i, Slots::DateTime(d), false)
}
fn zoned_to_instant(i: &mut Interpreter, t: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let d = zoned(&t)?.to_instant();
    wrap(i, Slots::Instant(d), false)
}
fn instant_to_zoned(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = instant(&t)?;
    let z = timezone(&arg(&a, 0))?;
    wrap(
        i,
        Slots::Zoned(
            d.to_zoned_date_time_iso_with_provider(z, &*TZ)
                .map_err(err)?,
        ),
        false,
    )
}
fn datetime_to_zoned(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = datetime(&t)?;
    let z = timezone(&arg(&a, 0))?;
    let opts = options(arg(&a, 1))?;
    let dis = disambiguation(i, &opts)?.unwrap_or(Disambiguation::Compatible);
    wrap(
        i,
        Slots::Zoned(
            d.to_zoned_date_time_with_provider(z, dis, &*TZ)
                .map_err(err)?,
        ),
        false,
    )
}
fn date_to_zoned(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let d = date(&t)?;
    let v = arg(&a, 0);
    let (z, t) = if crate::interpreter::call::is_js_object(&v)
        && !matches!(slot(&v), Some(Slots::Zoned(_)))
    {
        let z = i.get_prop_value_str(&v, "timeZone")?;
        if matches!(z, Value::Undefined) {
            return vm_err("TypeError: timeZone required");
        }
        let z = timezone(&z)?;
        let t = i.get_prop_value_str(&v, "plainTime")?;
        (z, t)
    } else {
        (timezone(&v)?, Value::Undefined)
    };
    let time = if matches!(t, Value::Undefined) {
        None
    } else {
        Some(to_time(i, &t, Value::Undefined)?)
    };
    wrap(
        i,
        Slots::Zoned(
            d.to_zoned_date_time_with_provider(z, time, &*TZ)
                .map_err(err)?,
        ),
        false,
    )
}
fn zoned_difference(
    i: &mut Interpreter,
    t: Value,
    a: Vec<Value>,
    since: bool,
) -> Result<Value, VmErr> {
    let d = zoned(&t)?;
    let other = to_zoned(i, &arg(&a, 0), Value::Undefined)?;
    let opts = options(arg(&a, 1))?;
    let mut o = DifferenceSettings::default();
    o.largest_unit = unit(i, &opts, "largestUnit")?;
    o.increment = increment(i, &opts)?;
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    let v = if since {
        d.since_with_provider(&other, o, &*TZ)
    } else {
        d.until_with_provider(&other, o, &*TZ)
    }
    .map_err(err)?;
    wrap(i, Slots::Duration(v), false)
}
fn zoned_until(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    zoned_difference(i, t, a, false)
}
fn zoned_since(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    zoned_difference(i, t, a, true)
}
fn zoned_round(i: &mut Interpreter, t: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let t = zoned(&t)?;
    let v = arg(&a, 0);
    let opts = if let Value::String(ref s) = v {
        Value::object_with_proto(
            vec![("smallestUnit".into(), Value::String(s.clone()))],
            None,
        )
    } else {
        if matches!(v, Value::Undefined) {
            return vm_err("TypeError: rounding options required");
        }
        options(v)?
    };
    let mut o = RoundingOptions::default();
    o.increment = increment(i, &opts)?;
    o.rounding_mode = mode(i, &opts)?;
    o.smallest_unit = unit(i, &opts, "smallestUnit")?;
    if o.smallest_unit.is_none() {
        return vm_err("RangeError: smallestUnit required");
    }
    wrap(
        i,
        Slots::Zoned(t.round_with_provider(o, &*TZ).map_err(err)?),
        false,
    )
}

fn current_instant() -> Result<Instant, VmErr> {
    Instant::from_epoch_milliseconds(super::date::now_ms() as i64).map_err(err)
}
fn now_instant(i: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    wrap(i, Slots::Instant(current_instant()?), false)
}
fn now_timezone_id(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    Value::checked_string("UTC")
}
fn current_zoned(a: &[Value]) -> Result<ZonedDateTime, VmErr> {
    let v = arg(a, 0);
    let z = if matches!(v, Value::Undefined) {
        TimeZone::try_from_str("UTC").map_err(err)?
    } else {
        timezone(&v)?
    };
    current_instant()?
        .to_zoned_date_time_iso_with_provider(z, &*TZ)
        .map_err(err)
}
fn now_zoned_iso(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    wrap(i, Slots::Zoned(current_zoned(&a)?), false)
}
fn now_datetime_iso(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    wrap(
        i,
        Slots::DateTime(current_zoned(&a)?.to_plain_date_time()),
        false,
    )
}
fn now_date_iso(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    wrap(i, Slots::Date(current_zoned(&a)?.to_plain_date()), false)
}
fn now_time_iso(i: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    wrap(i, Slots::Time(current_zoned(&a)?.to_plain_time()), false)
}
