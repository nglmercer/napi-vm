//! Duration records use exact integer arithmetic; ICU owns localized patterns.
use super::*;
use fixed_decimal::{Decimal, Sign, SignedRoundingMode, UnsignedRoundingMode};
use icu_decimal::{DecimalFormatter, options::GroupingStrategy};
use icu_experimental::dimension::units::{
    categorized_formatter::CategorizedFormatter,
    options::{UnitsFormatterOptions, Width},
};
use icu_experimental::duration::{DurationFormatterPreferences, provider::DigitalDurationDataV1};
use icu_experimental::measure::category::Duration as DurationUnit;
use icu_list::{
    ListFormatter,
    options::{ListFormatterOptions, ListLength},
};
use icu_provider::prelude::*;
use std::fmt;
use writeable::{Part, PartsWrite, Writeable};

const NAMES: [&str; 10] = [
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
const SINGULAR: [&str; 10] = [
    "year",
    "month",
    "week",
    "day",
    "hour",
    "minute",
    "second",
    "millisecond",
    "microsecond",
    "nanosecond",
];
const MAX_NS: u128 = (1u128 << 53) * 1_000_000_000;
#[derive(Clone)]
pub(crate) struct Slots {
    locale: String,
    numbering: String,
    style: String,
    styles: [String; 10],
    displays: [String; 10],
    fraction: Option<u8>,
    units: Rc<Vec<CategorizedFormatter<DurationUnit>>>,
    decimal: Rc<DecimalFormatter>,
    list: Rc<ListFormatter>,
    separator: String,
    digit_map: Option<Vec<char>>,
}
fn error() -> VmErr {
    VmErr::Msg("TypeError: Intl.DurationFormat receiver required".into())
}
fn slot(v: &Value) -> Result<Slots, VmErr> {
    v.property_cell()
        .and_then(|p| p.meta.borrow().intl_duration.as_deref().cloned())
        .ok_or_else(error)
}
fn choice(
    i: &mut Interpreter,
    o: &Value,
    name: &str,
    allowed: &[&str],
) -> Result<Option<String>, VmErr> {
    let result = option(i, o, name)?;
    if result
        .as_ref()
        .is_some_and(|s| !allowed.contains(&s.as_str()))
    {
        return Err(range());
    }
    Ok(result)
}
fn numeric(style: &str) -> bool {
    matches!(style, "numeric" | "2-digit")
}
pub(super) fn install(e: &mut Environment, namespace: &Value) {
    let fp = e.get("Function").and_then(|f| f.get_prop("prototype"));
    let op = e.get("Object").and_then(|o| o.get_prop("prototype"));
    let ctor = Value::object_with_proto(vec![], fp.clone().map(Rc::new));
    super::super::make_callable(&ctor, super::super::require_new, Some(construct));
    for (key, value) in [
        ("name", Value::String("DurationFormat".into())),
        ("length", Value::Number(0.)),
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
    nonenum(
        &ctor,
        "supportedLocalesOf",
        native_method("supportedLocalesOf", 1, supported_locales, fp.clone()),
    );
    let proto = Value::object_with_proto(vec![], op.map(Rc::new));
    nonenum(&proto, "constructor", ctor.clone());
    tag(&proto, "Intl.DurationFormat");
    for (name, len, f) in [
        ("resolvedOptions", 0, resolved_options as NativeFn),
        ("format", 1, format),
        ("formatToParts", 1, format_to_parts),
    ] {
        nonenum(&proto, name, native_method(name, len, f, fp.clone()));
    }
    super::super::set_builtin_constructor_prototype(e, &ctor, proto.clone());
    nonenum(namespace, "DurationFormat", ctor.clone());
    e.install_intrinsic("Intl.DurationFormat", ctor);
    e.install_intrinsic("Intl.DurationFormat.prototype", proto);
}
fn construct(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let target = i.new_target_stack.last().cloned().ok_or_else(error)?;
    let prototype = i.constructor_prototype(&target, "Intl.DurationFormat")?;
    let requested = locale_list(i, arg(&args, 0))?;
    let options = match arg(&args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        v if crate::interpreter::call::is_js_object(&v) => v,
        _ => return Err(error()),
    };
    locale_matcher(i, &options)?;
    let nu = option(i, &options, "numberingSystem")?;
    let (locale, base, _, numbering, digit_map, formatter_nu) =
        super::relative::resolve_display_numbering(&requested, nu)?;
    let style = choice(
        i,
        &options,
        "style",
        &["long", "short", "narrow", "digital"],
    )?
    .unwrap_or_else(|| "short".into());
    let format_locale: Locale = format!("{base}-u-nu-{formatter_nu}")
        .parse()
        .map_err(|_| range())?;
    let prefs: DurationFormatterPreferences = format_locale.clone().into();
    let data_locale = DigitalDurationDataV1::make_locale(prefs.locale_preferences);
    let data: DataResponse<DigitalDurationDataV1> = icu_experimental::provider::Baked
        .load(DataRequest {
            id: DataIdentifierBorrowed::for_locale(&data_locale),
            ..Default::default()
        })
        .map_err(|e| VmErr::Msg(format!("RangeError: {e}")))?;
    let mut styles: [String; 10] = Default::default();
    let mut displays: [String; 10] = Default::default();
    for index in 0..10 {
        let allowed: &[&str] = if index < 4 {
            &["long", "short", "narrow"]
        } else if index < 7 {
            &["long", "short", "narrow", "numeric", "2-digit"]
        } else {
            &["long", "short", "narrow", "numeric"]
        };
        let given = choice(i, &options, NAMES[index], allowed)?;
        let prev_numeric = index > 4 && numeric(&styles[index - 1]);
        let (mut field, mut display_default) = if let Some(given) = given {
            (given, "always")
        } else if style == "digital" {
            (
                (if index < 4 { "short" } else { "numeric" }).into(),
                if (4..7).contains(&index) {
                    "always"
                } else {
                    "auto"
                },
            )
        } else if prev_numeric {
            (
                "numeric".into(),
                if index == 5 || index == 6 {
                    "always"
                } else {
                    "auto"
                },
            )
        } else {
            (style.clone(), "auto")
        };
        if index >= 7 && field == "numeric" {
            display_default = "auto";
        }
        let display = choice(
            i,
            &options,
            &format!("{}Display", NAMES[index]),
            &["auto", "always"],
        )?
        .unwrap_or_else(|| display_default.into());
        if (index >= 7 && field == "numeric" && display == "always")
            || (prev_numeric && !numeric(&field))
        {
            return Err(range());
        }
        if prev_numeric && (index == 5 || index == 6) {
            field = "2-digit".into();
        }
        if index == 4 && numeric(&field) && data.payload.get().hms_padding.h == 2 {
            field = "2-digit".into();
        }
        styles[index] = field;
        displays[index] = display;
    }
    let fractional = i.member(&options, "fractionalDigits")?;
    let fraction = if matches!(fractional, Value::Undefined) {
        None
    } else {
        let number = i.ecmascript_to_number(&fractional)?;
        if !number.is_finite() || !(0.0..=9.0).contains(&number) {
            return Err(range());
        }
        Some(number.floor() as u8)
    };
    let constructors = [
        DurationUnit::year(),
        DurationUnit::month(),
        DurationUnit::week(),
        DurationUnit::day(),
        DurationUnit::hour(),
        DurationUnit::minute(),
        DurationUnit::second(),
        DurationUnit::millisecond(),
        DurationUnit::microsecond(),
        DurationUnit::nanosecond(),
    ];
    let units = constructors
        .into_iter()
        .enumerate()
        .map(|(j, unit)| {
            let width = match styles[j].as_str() {
                "long" => Width::Long,
                "narrow" => Width::Narrow,
                _ => Width::Short,
            };
            let mut unit_options = UnitsFormatterOptions::default();
            unit_options.width = width;
            CategorizedFormatter::try_new_outlier(format_locale.clone().into(), unit, unit_options)
                .map_err(|e| VmErr::Msg(format!("RangeError: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let decimal =
        DecimalFormatter::try_new(format_locale.clone().into(), GroupingStrategy::Never.into())
            .map_err(|e| VmErr::Msg(format!("RangeError: {e}")))?;
    let length = match style.as_str() {
        "long" => ListLength::Wide,
        "narrow" => ListLength::Narrow,
        _ => ListLength::Short,
    };
    let list = ListFormatter::try_new_unit(
        format_locale.into(),
        ListFormatterOptions::default().with_length(length),
    )
    .map_err(|e| VmErr::Msg(format!("RangeError: {e}")))?;
    let result = Value::object_with_proto(vec![], prototype);
    result
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .intl_duration = Some(Box::new(Slots {
        locale,
        numbering,
        style,
        styles,
        displays,
        fraction,
        units: Rc::new(units),
        decimal: Rc::new(decimal),
        list: Rc::new(list),
        separator: data.payload.get().separator.to_string(),
        digit_map,
    }));
    Ok(result)
}
fn resolved_options(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    let mut fields = vec![
        ("locale".into(), Value::String(s.locale.into())),
        ("numberingSystem".into(), Value::String(s.numbering.into())),
        ("style".into(), Value::String(s.style.into())),
    ];
    for (j, name) in NAMES.iter().enumerate() {
        fields.push(((*name).into(), Value::String(s.styles[j].clone().into())));
        fields.push((
            format!("{name}Display"),
            Value::String(s.displays[j].clone().into()),
        ));
    }
    if let Some(digits) = s.fraction {
        fields.push(("fractionalDigits".into(), Value::Number(digits as f64)));
    }
    Ok(Value::object(fields))
}

fn record(i: &mut Interpreter, value: Value) -> Result<([u128; 10], bool), VmErr> {
    // The pinned Temporal integration accepts duration strings and reads
    // branded Duration slots directly, without invoking prototype getters.
    let temporal =
        match &value {
            Value::String(text) => Some(
                temporal_rs::Duration::from_utf8(text.to_string().as_bytes())
                    .map_err(|e| VmErr::Msg(e.to_string()))?,
            ),
            _ => value.property_cell().and_then(|p| {
                match p.meta.borrow().temporal.as_deref().cloned() {
                    Some(crate::builtins::temporal::Slots::Duration(d)) => Some(d),
                    _ => None,
                }
            }),
        };
    if let Some(d) = temporal {
        let fields = [
            d.years() as i128,
            d.months() as i128,
            d.weeks() as i128,
            d.days() as i128,
            d.hours() as i128,
            d.minutes() as i128,
            d.seconds() as i128,
            d.milliseconds() as i128,
            d.microseconds(),
            d.nanoseconds(),
        ];
        return Ok((
            fields.map(i128::unsigned_abs),
            fields.iter().any(|v| *v < 0),
        ));
    }
    if !crate::interpreter::call::is_js_object(&value) {
        return Err(error());
    }
    let mut raw = [0.0; 10];
    let mut any = false;
    // ToDurationRecord reads fields in alphabetical order, converting each
    // value immediately but checking bounds and mixed signs only afterwards.
    for j in [3, 4, 8, 7, 5, 1, 9, 6, 2, 0] {
        let v = i.member(&value, NAMES[j])?;
        if !matches!(v, Value::Undefined) {
            any = true;
            let n = i.ecmascript_to_number(&v)?;
            if !n.is_finite() || n.fract() != 0.0 {
                return Err(range());
            }
            raw[j] = n;
        }
    }
    if !any {
        return Err(error());
    }
    let negative = raw.iter().any(|n| *n < 0.0);
    if negative && raw.iter().any(|n| *n > 0.0) {
        return Err(range());
    }
    if raw[..3].iter().any(|n| n.abs() >= 4294967296.0) {
        return Err(range());
    }
    let mut values = [0u128; 10];
    for j in 0..10 {
        if raw[j].abs() > MAX_NS as f64 {
            return Err(range());
        }
        values[j] = raw[j].abs() as u128;
    }
    let mut ns = 0u128;
    for (j, scale) in [
        (3, 86_400_000_000_000u128),
        (4, 3_600_000_000_000),
        (5, 60_000_000_000),
        (6, 1_000_000_000),
        (7, 1_000_000),
        (8, 1000),
        (9, 1),
    ] {
        if values[j] > MAX_NS / scale {
            return Err(range());
        }
        ns += values[j] * scale;
    }
    if ns >= MAX_NS {
        return Err(range());
    }
    Ok((values, negative))
}
#[derive(Clone)]
struct Piece {
    kind: &'static str,
    text: String,
    unit: Option<&'static str>,
}
fn push(out: &mut Vec<Piece>, kind: &'static str, text: &str, unit: Option<&'static str>) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut()
        && last.kind == kind
        && last.unit == unit
    {
        last.text.push_str(text);
        return;
    }
    out.push(Piece {
        kind,
        text: text.into(),
        unit,
    });
}
struct UnitSink {
    out: Vec<Piece>,
    kind: Option<&'static str>,
    unit: &'static str,
    label: bool,
}
impl fmt::Write for UnitSink {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if let Some(kind) = self.kind {
            push(&mut self.out, kind, text, Some(self.unit));
        } else if self.label {
            // Unit-pattern literals consist of the localized label and its
            // surrounding spacing; keep spacing as literal parts.
            let mut start = 0;
            let mut old = None;
            for (offset, c) in text.char_indices() {
                let literal =
                    c.is_whitespace() || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}');
                if let Some(previous) = old
                    && previous != literal
                {
                    push(
                        &mut self.out,
                        if previous { "literal" } else { "unit" },
                        &text[start..offset],
                        Some(self.unit),
                    );
                    start = offset;
                }
                old = Some(literal);
            }
            if let Some(literal) = old {
                push(
                    &mut self.out,
                    if literal { "literal" } else { "unit" },
                    &text[start..],
                    Some(self.unit),
                );
            }
        } else {
            push(&mut self.out, "literal", text, Some(self.unit));
        }
        Ok(())
    }
}
impl PartsWrite for UnitSink {
    type SubPartsWrite = Self;
    fn with_part(
        &mut self,
        part: Part,
        mut f: impl FnMut(&mut Self) -> fmt::Result,
    ) -> fmt::Result {
        let old = self.kind;
        if part.category == "decimal" {
            self.kind = Some(part.value);
        }
        let result = f(self);
        self.kind = old;
        result
    }
}
fn number_value(values: &[u128; 10], j: usize, s: &Slots) -> Result<(Decimal, bool), VmErr> {
    let fractional = (6..9).contains(&j) && numeric(&s.styles[j + 1]);
    let mut value: Decimal = if fractional {
        let exponent = (9 - j) * 3;
        let mut total = 0u128;
        for (k, v) in values.iter().enumerate().skip(j) {
            total += v * 10u128.pow(((9 - k) * 3) as u32);
        }
        let mut decimal: Decimal = total.to_string().parse().map_err(|_| range())?;
        decimal.multiply_pow10(-(exponent as i16));
        decimal
    } else {
        values[j].to_string().parse().map_err(|_| range())?
    };
    let nonzero = !value.is_zero();
    if fractional {
        if let Some(digits) = s.fraction {
            value.round_with_mode(
                -(digits as i16),
                SignedRoundingMode::Unsigned(UnsignedRoundingMode::Trunc),
            );
            value.pad_end(-(digits as i16));
        } else {
            value.trim_end();
        }
    }
    Ok((value, nonzero))
}
struct ListSink<'a> {
    elements: &'a [Vec<Piece>],
    out: Vec<Piece>,
    element: usize,
    inside: bool,
}
impl fmt::Write for ListSink<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if !self.inside {
            push(&mut self.out, "literal", text, None);
        }
        Ok(())
    }
}
impl PartsWrite for ListSink<'_> {
    type SubPartsWrite = Self;
    fn with_part(
        &mut self,
        part: Part,
        mut f: impl FnMut(&mut Self) -> fmt::Result,
    ) -> fmt::Result {
        if part == icu_list::parts::ELEMENT {
            self.out.extend(self.elements[self.element].clone());
            self.element += 1;
            let old = self.inside;
            self.inside = true;
            let result = f(self);
            self.inside = old;
            result
        } else {
            f(self)
        }
    }
}
fn render(i: &mut Interpreter, this: Value, args: Vec<Value>, parts: bool) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    let (values, negative) = record(i, arg(&args, 0))?;
    let mut elements = Vec::<Vec<Piece>>::new();
    let mut signed = false;
    let mut j = 0;
    while j < 10 {
        if j >= 7 && numeric(&s.styles[j]) {
            j += 1;
            continue;
        }
        if (4..=6).contains(&j) && numeric(&s.styles[j]) {
            let mut block = Vec::new();
            let mut shown = [false; 3];
            for k in j..=6 {
                let (_, nonzero) = number_value(&values, k, &s)?;
                shown[k - 4] = nonzero || s.displays[k] == "always";
            }
            if j == 4 && shown[0] && shown[2] {
                shown[1] = true;
            }
            for k in j..=6 {
                if !shown[k - 4] {
                    continue;
                }
                let (mut value, _) = number_value(&values, k, &s)?;
                if s.styles[k] == "2-digit" {
                    value.pad_start(2);
                }
                if negative && !signed {
                    value.set_sign(Sign::Negative);
                    signed = true;
                }
                if !block.is_empty() {
                    push(&mut block, "literal", &s.separator, None);
                }
                let mut sink = UnitSink {
                    out: Vec::new(),
                    kind: None,
                    unit: SINGULAR[k],
                    label: false,
                };
                s.decimal
                    .format(&value)
                    .write_to_parts(&mut sink)
                    .map_err(|_| error())?;
                block.extend(sink.out);
            }
            if !block.is_empty() {
                elements.push(block);
            }
            j = 7;
            continue;
        }
        let (mut value, nonzero) = number_value(&values, j, &s)?;
        if nonzero || s.displays[j] == "always" {
            if negative && !signed {
                value.set_sign(Sign::Negative);
                signed = true;
            }
            let mut sink = UnitSink {
                out: Vec::new(),
                kind: None,
                unit: SINGULAR[j],
                label: true,
            };
            s.units[j]
                .format_fixed_decimal(&value)
                .write_to_parts(&mut sink)
                .map_err(|_| error())?;
            elements.push(sink.out);
        }
        j += 1;
    }
    let strings = elements
        .iter()
        .map(|p| p.iter().map(|p| p.text.as_str()).collect::<String>())
        .collect::<Vec<_>>();
    let mut sink = ListSink {
        elements: &elements,
        out: Vec::new(),
        element: 0,
        inside: false,
    };
    s.list
        .format(strings.iter())
        .write_to_parts(&mut sink)
        .map_err(|_| error())?;
    if let Some(digits) = s.digit_map.as_ref() {
        for piece in &mut sink.out {
            if matches!(piece.kind, "integer" | "fraction" | "exponentInteger") {
                piece.text = super::relative::map_digit_text(&piece.text, digits);
            }
        }
    }
    if !parts {
        return Ok(Value::String(
            sink.out
                .iter()
                .map(|p| p.text.as_str())
                .collect::<String>()
                .into(),
        ));
    }
    Value::checked_array(
        sink.out
            .into_iter()
            .map(|p| {
                let mut fields = vec![
                    ("type".into(), Value::String(p.kind.into())),
                    ("value".into(), Value::String(p.text.into())),
                ];
                if let Some(unit) = p.unit {
                    fields.push(("unit".into(), Value::String(unit.into())));
                }
                Value::object(fields)
            })
            .collect(),
    )
}
fn format(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    render(i, this, args, false)
}
pub(crate) fn temporal_to_locale(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let constructor = i
        .persistent_global
        .borrow()
        .intrinsic("Intl.DurationFormat")
        .unwrap();
    let formatter = i.reflect_constructor(&constructor, args, constructor.clone())?;
    format(i, formatter, vec![this])
}
fn format_to_parts(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    render(i, this, args, true)
}
