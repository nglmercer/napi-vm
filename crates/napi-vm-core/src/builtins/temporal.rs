//! Temporal objects. JavaScript observable operations stay in the VM; calendar
//! and nanosecond arithmetic are delegated to temporal_rs.
use crate::error::VmErr;
use std::rc::Rc;
use std::str::FromStr;
use temporal_rs::options::{
    DifferenceSettings, Overflow, RelativeTo, RoundingIncrement, RoundingMode, RoundingOptions,
    ToStringRoundingOptions, Unit,
};
use temporal_rs::partial::{PartialDate, PartialTime, PartialZonedDateTime};
use temporal_rs::provider::COMPILED_TZ_PROVIDER as TZ;
use temporal_rs::{
    Calendar, Duration, MonthCode, PlainDate, PlainTime, TimeZone, UtcOffset, ZonedDateTime,
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
        method(&ctor, "compare", 2, compare, &fp);
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
                ("toLocaleString", 0, time_json),
                ("valueOf", 0, value_of),
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
                ("toLocaleString", 0, duration_json),
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
fn string_options(i: &mut Interpreter, v: Value) -> Result<ToStringRoundingOptions, VmErr> {
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
