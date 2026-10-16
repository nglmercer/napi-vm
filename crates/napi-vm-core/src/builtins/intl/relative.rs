//! ECMA-402 relative times: observable guest operations with ICU/CLDR patterns.
use super::*;
use fixed_decimal::{Decimal, Sign, SignedRoundingMode, UnsignedRoundingMode};
use icu_decimal::DecimalFormatter;
use icu_experimental::relativetime::options::Numeric;
use icu_experimental::relativetime::{
    RelativeTimeFormatter as Formatter, RelativeTimeFormatterOptions,
    RelativeTimeFormatterPreferences,
};
use icu_provider::prelude::*;
use std::{cell::RefCell, fmt};
use writeable::{Part, PartsWrite, Writeable};

const UNITS: [&str; 8] = [
    "second", "minute", "hour", "day", "week", "month", "quarter", "year",
];
#[derive(Clone)]
pub(crate) struct Slots {
    locale: String,
    numbering: String,
    style: String,
    numeric: String,
    formatters: Rc<Vec<Formatter>>,
    always: Option<Rc<Vec<Formatter>>>,
    decimal: Rc<DecimalFormatter>,
    digit_map: Option<Vec<char>>,
}
fn error() -> VmErr {
    VmErr::Msg("TypeError: Intl.RelativeTimeFormat receiver required".into())
}
fn slot(this: &Value) -> Result<Slots, VmErr> {
    this.property_cell()
        .and_then(|p| p.meta.borrow().intl_relative.clone())
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
        ("name", Value::String("RelativeTimeFormat".into())),
        ("length", Value::Number(0.)),
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
    tag(&prototype, "Intl.RelativeTimeFormat");
    method(&prototype, "resolvedOptions", 0, resolved_options, &fp);
    method(&prototype, "format", 2, format, &fp);
    method(&prototype, "formatToParts", 2, format_to_parts, &fp);
    super::super::set_builtin_constructor_prototype(e, &constructor, prototype.clone());
    nonenum(namespace, "RelativeTimeFormat", constructor.clone());
    e.install_intrinsic("Intl.RelativeTimeFormat", constructor);
    e.install_intrinsic("Intl.RelativeTimeFormat.prototype", prototype);
}

// ICU's last successful digit-data request identifies the actual numbering
// system, including locale defaults and fallback from unsupported options.
struct InspectDigits(RefCell<String>);
impl<M: DataMarker> DataProvider<M> for InspectDigits
where
    icu_decimal::provider::Baked: DataProvider<M>,
{
    fn load(&self, req: DataRequest) -> Result<DataResponse<M>, DataError> {
        let result = icu_decimal::provider::Baked.load(req);
        if result.is_ok()
            && std::any::TypeId::of::<M>()
                == std::any::TypeId::of::<icu_decimal::provider::DecimalDigitsV1>()
        {
            *self.0.borrow_mut() = req.id.marker_attributes.as_str().into();
        }
        result
    }
}
fn decimal_for(base: &str, numbering: Option<&str>) -> Result<(DecimalFormatter, String), VmErr> {
    let tag = numbering.map_or_else(|| base.to_string(), |nu| format!("{base}-u-nu-{nu}"));
    let locale: Locale = tag.parse().map_err(|_| range())?;
    let provider = InspectDigits(RefCell::new(String::new()));
    let formatter =
        DecimalFormatter::try_new_unstable(&provider, locale.into(), Default::default())
            .map_err(|e| VmErr::Msg(format!("RangeError: {e}")))?;
    Ok((formatter, provider.0.into_inner()))
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
        return Err(range());
    }
    Ok(value)
}
pub(super) fn resolve_numbering(
    requested: &[String],
    numbering_option: Option<String>,
) -> Result<(String, String, DecimalFormatter, String), VmErr> {
    if let Some(nu) = &numbering_option {
        if !nu
            .split('-')
            .all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|c| c.is_ascii_alphanumeric()))
        {
            return Err(range());
        }
    }
    let (base, extension) = requested
        .iter()
        .find_map(|tag| {
            let base = best_available_locale(tag)?;
            let loc = parse(tag).ok()?;
            let extension = loc
                .extensions
                .unicode
                .keywords
                .get(&"nu".parse().unwrap())
                .map(ToString::to_string)
                .filter(|nu| !nu.is_empty());
            Some((base, extension))
        })
        .unwrap_or_else(|| ("en-US".into(), None));
    let (mut decimal, mut numbering) = decimal_for(&base, extension.as_deref())?;
    let mut extension_used = extension.as_ref().is_some_and(|nu| *nu == numbering);
    if let Some(nu) = numbering_option {
        let (candidate, resolved) = decimal_for(&base, Some(&nu))?;
        if resolved == nu {
            if resolved != numbering {
                extension_used = false;
            }
            decimal = candidate;
            numbering = resolved;
        }
    }
    let locale = if extension_used {
        format!("{base}-u-nu-{numbering}")
    } else {
        base.clone()
    };
    Ok((locale, base, decimal, numbering))
}
pub(super) fn resolve_display_numbering(
    requested: &[String],
    numbering_option: Option<String>,
) -> Result<
    (
        String,
        String,
        DecimalFormatter,
        String,
        Option<Vec<char>>,
        String,
    ),
    VmErr,
> {
    // ICU only honors the numbering systems it implements. Systems with CLDR
    // digit data are still observable: format with the ICU-resolved numbering
    // and remap the decimal spans afterwards, mirroring NumberFormat.
    let (mut locale, base, decimal, mut numbering) =
        resolve_numbering(requested, numbering_option.clone())?;
    let icu_numbering = numbering.clone();
    let extension = requested
        .iter()
        .find(|tag| best_available_locale(tag).is_some())
        .and_then(|tag| tag.parse::<Locale>().ok())
        .and_then(|loc| {
            loc.extensions
                .unicode
                .keywords
                .get(&"nu".parse().unwrap())
                .map(ToString::to_string)
        })
        .filter(|nu| super::number::digits_available(nu));
    let chosen = numbering_option
        .filter(|nu| super::number::digits_available(nu))
        .or_else(|| extension.clone())
        .unwrap_or_else(|| numbering.clone());
    if chosen != numbering {
        locale = if extension.as_deref() == Some(&chosen) {
            format!("{base}-u-nu-{chosen}")
        } else {
            base.clone()
        };
        numbering = chosen;
    }
    let formatter_nu = if numbering == icu_numbering {
        numbering.clone()
    } else {
        "latn".into()
    };
    let digit_map = if formatter_nu == numbering {
        None
    } else {
        super::number::digit_map(&numbering)
    };
    Ok((locale, base, decimal, numbering, digit_map, formatter_nu))
}
pub(super) fn map_digit_text(text: &str, digits: &[char]) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_digit() {
                digits[(c as u8 - b'0') as usize]
            } else {
                c
            }
        })
        .collect()
}
pub(super) fn remap_digits(parts: &mut [(&'static str, String)], digit_map: Option<&Vec<char>>) {
    if let Some(digits) = digit_map {
        for (kind, text) in parts {
            if matches!(*kind, "integer" | "fraction" | "exponentInteger") {
                *text = map_digit_text(text, digits);
            }
        }
    }
}
fn construct(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let target = i.new_target_stack.last().cloned().ok_or_else(error)?;
    let prototype = i.constructor_prototype(&target, "Intl.RelativeTimeFormat")?;
    let requested = locale_list(i, arg(&args, 0))?;
    let options = match arg(&args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        v => super::super::object::to_object_receiver(&v)?,
    };
    locale_matcher(i, &options)?;
    let numbering_option = option(i, &options, "numberingSystem")?;
    let (locale, base, decimal, numbering, digit_map, formatter_nu) =
        resolve_display_numbering(&requested, numbering_option)?;
    let style = choice(i, &options, "style", "long", &["long", "short", "narrow"])?;
    let numeric = choice(i, &options, "numeric", "always", &["always", "auto"])?;
    let mut format_options = RelativeTimeFormatterOptions::default();
    format_options.numeric = if numeric == "auto" {
        Numeric::Auto
    } else {
        Numeric::Always
    };
    let formatter_locale: Locale = format!("{base}-u-nu-{formatter_nu}")
        .parse()
        .map_err(|_| range())?;
    let prefs: RelativeTimeFormatterPreferences = formatter_locale.into();
    let constructors = match style.as_str() {
        "long" => [
            Formatter::try_new_long_second,
            Formatter::try_new_long_minute,
            Formatter::try_new_long_hour,
            Formatter::try_new_long_day,
            Formatter::try_new_long_week,
            Formatter::try_new_long_month,
            Formatter::try_new_long_quarter,
            Formatter::try_new_long_year,
        ],
        "short" => [
            Formatter::try_new_short_second,
            Formatter::try_new_short_minute,
            Formatter::try_new_short_hour,
            Formatter::try_new_short_day,
            Formatter::try_new_short_week,
            Formatter::try_new_short_month,
            Formatter::try_new_short_quarter,
            Formatter::try_new_short_year,
        ],
        _ => [
            Formatter::try_new_narrow_second,
            Formatter::try_new_narrow_minute,
            Formatter::try_new_narrow_hour,
            Formatter::try_new_narrow_day,
            Formatter::try_new_narrow_week,
            Formatter::try_new_narrow_month,
            Formatter::try_new_narrow_quarter,
            Formatter::try_new_narrow_year,
        ],
    };
    let formatters = constructors
        .into_iter()
        .map(|f| f(prefs, format_options).map_err(|e| VmErr::Msg(format!("RangeError: {e}"))))
        .collect::<Result<Vec<_>, _>>()?;
    let always = if numeric == "auto" {
        format_options.numeric = Numeric::Always;
        Some(Rc::new(
            constructors
                .into_iter()
                .map(|f| {
                    f(prefs, format_options).map_err(|e| VmErr::Msg(format!("RangeError: {e}")))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ))
    } else {
        None
    };
    let result = Value::object_with_proto(vec![], prototype);
    result
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .intl_relative = Some(Slots {
        locale,
        numbering,
        style,
        numeric,
        formatters: Rc::new(formatters),
        always,
        decimal: Rc::new(decimal),
        digit_map,
    });
    Ok(result)
}
fn resolved_options(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    Ok(Value::object(vec![
        ("locale".into(), Value::String(s.locale.into())),
        ("style".into(), Value::String(s.style.into())),
        ("numeric".into(), Value::String(s.numeric.into())),
        ("numberingSystem".into(), Value::String(s.numbering.into())),
    ]))
}

#[derive(Default)]
struct Sink {
    kind: &'static str,
    parts: Vec<(&'static str, String)>,
    special: bool,
}
impl fmt::Write for Sink {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if text.is_empty() {
            return Ok(());
        }
        let kind = if self.kind.is_empty() {
            "literal"
        } else {
            self.kind
        };
        if let Some((last_kind, last)) = self.parts.last_mut() {
            if *last_kind == kind {
                last.push_str(text);
                return Ok(());
            }
        }
        self.parts.push((kind, text.into()));
        Ok(())
    }
}
impl PartsWrite for Sink {
    type SubPartsWrite = Self;
    fn with_part(
        &mut self,
        part: Part,
        mut f: impl FnMut(&mut Self) -> fmt::Result,
    ) -> fmt::Result {
        let old = self.kind;
        if part.category == "decimal" {
            self.kind = part.value;
        }
        if part.category == "relativetime" && part.value == "literal" {
            self.special = true;
        }
        let result = f(self);
        self.kind = old;
        result
    }
}
fn render(i: &mut Interpreter, this: Value, args: Vec<Value>, parts: bool) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    let number = i.ecmascript_to_number(&arg(&args, 0))?;
    let unit = i.ecmascript_to_string(&arg(&args, 1))?.to_string();
    let unit = unit.strip_suffix('s').unwrap_or(&unit);
    let index = UNITS.iter().position(|u| *u == unit).ok_or_else(range)?;
    if !number.is_finite() {
        return Err(range());
    }
    let mut value: Decimal = ryu_js::Buffer::new()
        .format(number.abs())
        .parse()
        .map_err(|_| range())?;
    value.round_with_mode(
        -3,
        SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
    );
    value.trim_end();
    let signed = value.clone().with_sign(if number.is_sign_negative() {
        Sign::Negative
    } else {
        Sign::None
    });
    // Auto relative words apply to the original numeric value, before decimal
    // rounding (0.0001 seconds is numeric zero seconds, not "now").
    let formatter = if number.fract() != 0.0 {
        s.always
            .as_ref()
            .map_or(&s.formatters[index], |f| &f[index])
    } else {
        &s.formatters[index]
    };
    let formatted = formatter.format(signed);
    if !parts && s.digit_map.is_none() {
        return Ok(Value::String(
            formatted.write_to_string().into_owned().into(),
        ));
    }
    let mut relative = Sink::default();
    formatted
        .write_to_parts(&mut relative)
        .map_err(|_| error())?;
    let full = relative
        .parts
        .iter()
        .map(|(_, s)| s.as_str())
        .collect::<String>();
    let output = if relative.special {
        vec![("literal", full)]
    } else {
        // ICU relative pattern interpolation currently erases nested decimal
        // spans. Recover the single number placeholder using the identical
        // decimal formatter and preserve its integer/group/fraction spans.
        let mut numeric = Sink::default();
        s.decimal
            .format(&value)
            .write_to_parts(&mut numeric)
            .map_err(|_| error())?;
        let text = numeric
            .parts
            .iter()
            .map(|(_, s)| s.as_str())
            .collect::<String>();
        let start = full.find(&text).ok_or_else(error)?;
        remap_digits(&mut numeric.parts, s.digit_map.as_ref());
        let mut output = Vec::new();
        if start > 0 {
            output.push(("literal", full[..start].into()));
        }
        output.extend(numeric.parts);
        if start + text.len() < full.len() {
            output.push(("literal", full[start + text.len()..].into()));
        }
        output
    };
    if !parts {
        return Ok(Value::String(
            output
                .into_iter()
                .map(|(_, text)| text)
                .collect::<String>()
                .into(),
        ));
    }
    Value::checked_array(
        output
            .into_iter()
            .map(|(kind, text)| {
                let mut fields = vec![
                    ("type".into(), Value::String(kind.into())),
                    ("value".into(), Value::String(text.into())),
                ];
                if kind != "literal" {
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
fn format_to_parts(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    render(i, this, args, true)
}
