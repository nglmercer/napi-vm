//! ECMA-402 number formatting: guest coercions/rounding, ICU digits, CLDR patterns.
use super::*;
use fixed_decimal::{Decimal, Sign};
use icu_decimal::{DecimalFormatter, options::GroupingStrategy};
use icu_plurals::{PluralOperands, PluralRules};
use icu_provider::prelude::*;
use serde_json::Value as Json;
use std::{fmt, sync::Arc};
use writeable::{Part, PartsWrite, Writeable};
mod data;
mod digits;

#[derive(Clone)]
pub(crate) struct Slots {
    locale: String,
    numbering: String,
    style: String,
    currency: Option<String>,
    currency_display: String,
    currency_sign: String,
    unit: Option<String>,
    unit_display: String,
    notation: String,
    compact: String,
    grouping: Option<String>,
    sign: String,
    digits: digits::Digits,
    decimal: Rc<DecimalFormatter>,
    plain: Rc<DecimalFormatter>,
    rules: Rc<PluralRules>,
    data: Arc<Json>,
    digit_map: Option<Vec<char>>,
}
fn error() -> VmErr {
    VmErr::Msg("TypeError: Intl.NumberFormat receiver required".into())
}
fn slot(v: &Value) -> Result<Slots, VmErr> {
    v.property_cell()
        .and_then(|p| p.meta.borrow().intl_number.clone())
        .ok_or_else(error)
}
pub(super) fn numbering_system_names() -> impl Iterator<Item = &'static str> {
    data::NUMBERING_SYSTEMS.iter().map(|entry| entry.0)
}
pub(super) fn digit_map(name: &str) -> Option<Vec<char>> {
    data::digits(name).map(|digits| digits.chars().collect())
}
pub(super) fn digits_available(name: &str) -> bool {
    data::digits(name).is_some()
}
pub(super) fn unit_names() -> &'static [&'static str] {
    data::UNITS
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
fn currency_digits(code: &str) -> i16 {
    use icu_experimental::dimension::{
        currency::CurrencyType, provider::currency::fractions::CurrencyFractionsV1,
    };
    let code: CurrencyType = code.parse().unwrap();
    let response: DataResponse<CurrencyFractionsV1> = icu_experimental::provider::Baked
        .load(Default::default())
        .unwrap();
    response.payload.get().resolve(code).digits as i16
}
fn initialize(i: &mut Interpreter, args: &[Value]) -> Result<Slots, VmErr> {
    let requested = locale_list(i, arg(args, 0))?;
    let options = match arg(args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        v => super::super::object::to_object_receiver(&v)?,
    };
    locale_matcher(i, &options)?;
    let nu = option(i, &options, "numberingSystem")?;
    let (mut locale, base, _, mut numbering) =
        super::relative::resolve_numbering(&requested, nu.clone())?;
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
        .filter(|nu| data::digits(nu).is_some());
    let chosen = nu
        .filter(|nu| data::digits(nu).is_some())
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
        numbering.as_str()
    } else {
        "latn"
    };
    let digit_map = if formatter_nu == numbering {
        None
    } else {
        Some(data::digits(&numbering).unwrap().chars().collect())
    };
    let style = choice(
        i,
        &options,
        "style",
        "decimal",
        &["decimal", "percent", "currency", "unit"],
    )?;
    let currency = option(i, &options, "currency")?;
    if currency
        .as_ref()
        .is_some_and(|s| s.len() != 3 || !s.bytes().all(|c| c.is_ascii_alphabetic()))
    {
        return Err(range());
    }
    if style == "currency" && currency.is_none() {
        return Err(VmErr::Msg("TypeError: Currency code required".into()));
    }
    let currency = currency.map(|s| s.to_ascii_uppercase());
    let currency_display = choice(
        i,
        &options,
        "currencyDisplay",
        "symbol",
        &["code", "symbol", "narrowSymbol", "name"],
    )?;
    let currency_sign = choice(
        i,
        &options,
        "currencySign",
        "standard",
        &["standard", "accounting"],
    )?;
    let unit = option(i, &options, "unit")?;
    if unit.as_ref().is_some_and(|s| !data::unit_valid(s)) {
        return Err(range());
    }
    if style == "unit" && unit.is_none() {
        return Err(VmErr::Msg("TypeError: Unit required".into()));
    }
    let unit_display = choice(
        i,
        &options,
        "unitDisplay",
        "short",
        &["short", "narrow", "long"],
    )?;
    let notation = choice(
        i,
        &options,
        "notation",
        "standard",
        &["standard", "scientific", "engineering", "compact"],
    )?;
    let (min, max) = if style == "currency" && notation == "standard" {
        let d = currency_digits(currency.as_ref().unwrap());
        (d, d)
    } else if style == "percent" {
        (0, 0)
    } else {
        (0, 3)
    };
    let digits = digits::read(i, &options, min, max, &notation)?;
    let compact = choice(i, &options, "compactDisplay", "short", &["short", "long"])?;
    let default_group = if notation == "compact" {
        "min2"
    } else {
        "auto"
    };
    let grouping = i.member(&options, "useGrouping")?;
    let grouping = match grouping {
        Value::Undefined => Some(default_group.into()),
        Value::Bool(false) => None,
        Value::Bool(true) => Some("always".into()),
        v if !v.is_truthy() => None,
        v => {
            let s = i.ecmascript_to_string(&v)?.to_string();
            match s.as_str() {
                "true" | "false" => Some(default_group.into()),
                "auto" | "min2" | "always" => Some(s),
                _ => return Err(range()),
            }
        }
    };
    let sign = choice(
        i,
        &options,
        "signDisplay",
        "auto",
        &["auto", "always", "never", "exceptZero", "negative"],
    )?;
    let prefs: Locale = format!("{base}-u-nu-{formatter_nu}")
        .parse()
        .map_err(|_| range())?;
    let grouping_strategy = match grouping.as_deref() {
        None => GroupingStrategy::Never,
        Some("min2") => GroupingStrategy::Min2,
        Some("always") => GroupingStrategy::Always,
        _ => GroupingStrategy::Auto,
    };
    let decimal = DecimalFormatter::try_new(prefs.clone().into(), grouping_strategy.into())
        .map_err(|_| range())?;
    let plain = DecimalFormatter::try_new(prefs.into(), GroupingStrategy::Never.into())
        .map_err(|_| range())?;
    let rules = PluralRules::try_new_cardinal(base.parse::<Locale>().unwrap().into())
        .map_err(|_| range())?;
    Ok(Slots {
        locale,
        numbering,
        style,
        currency,
        currency_display,
        currency_sign,
        unit,
        unit_display,
        notation,
        compact,
        grouping,
        sign,
        digits,
        decimal: Rc::new(decimal),
        plain: Rc::new(plain),
        rules: Rc::new(rules),
        data: data::locale(
            requested
                .iter()
                .find(|tag| best_available_locale(tag).is_some())
                .map(String::as_str)
                .unwrap_or(&base),
        )?,
        digit_map,
    })
}
fn construct(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let ctor = i
        .persistent_global
        .borrow()
        .intrinsic("Intl.NumberFormat")
        .unwrap();
    let new_target = i.new_target_stack.last().cloned();
    let prototype =
        i.constructor_prototype(new_target.as_ref().unwrap_or(&ctor), "Intl.NumberFormat")?;
    let s = initialize(i, &args)?;
    let object = Value::object_with_proto(vec![], prototype);
    object
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .intl_number = Some(s);
    if new_target.is_none() && i.ordinary_instance_of(&this, &ctor)?.is_truthy() {
        let symbol = i
            .persistent_global
            .borrow()
            .intrinsic("Intl.FallbackSymbol")
            .unwrap();
        let descriptor = Value::descriptor_record(vec![
            ("value".into(), object),
            ("writable".into(), Value::Bool(false)),
            ("enumerable".into(), Value::Bool(false)),
            ("configurable".into(), Value::Bool(false)),
        ]);
        if !i.define_own_property(&this, &symbol, &descriptor)? {
            return Err(error());
        }
        return Ok(this);
    }
    Ok(object)
}
fn unwrap(i: &mut Interpreter, this: Value) -> Result<Value, VmErr> {
    if slot(&this).is_ok() {
        return Ok(this);
    }
    let ctor = i
        .persistent_global
        .borrow()
        .intrinsic("Intl.NumberFormat")
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
fn resolved_options(i: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let this = unwrap(i, this)?;
    let s = slot(&this)?;
    let mut fields = vec![
        ("locale".into(), Value::String(s.locale.into())),
        ("numberingSystem".into(), Value::String(s.numbering.into())),
        ("style".into(), Value::String(s.style.clone().into())),
    ];
    if s.style == "currency" {
        for (k, v) in [
            ("currency", s.currency.unwrap()),
            ("currencyDisplay", s.currency_display),
            ("currencySign", s.currency_sign),
        ] {
            fields.push((k.into(), Value::String(v.into())));
        }
    }
    if s.style == "unit" {
        fields.push(("unit".into(), Value::String(s.unit.unwrap().into())));
        fields.push(("unitDisplay".into(), Value::String(s.unit_display.into())));
    }
    fields.push((
        "minimumIntegerDigits".into(),
        Value::Number(s.digits.integer as f64),
    ));
    for (k, v) in [
        ("minimumFractionDigits", s.digits.min_f),
        ("maximumFractionDigits", s.digits.max_f),
        ("minimumSignificantDigits", s.digits.min_s),
        ("maximumSignificantDigits", s.digits.max_s),
    ] {
        if let Some(v) = v {
            fields.push((k.into(), Value::Number(v as f64)));
        }
    }
    fields.push((
        "useGrouping".into(),
        s.grouping
            .map_or(Value::Bool(false), |s| Value::String(s.into())),
    ));
    fields.push(("notation".into(), Value::String(s.notation.clone().into())));
    if s.notation == "compact" {
        fields.push(("compactDisplay".into(), Value::String(s.compact.into())));
    }
    fields.push(("signDisplay".into(), Value::String(s.sign.into())));
    fields.push((
        "roundingIncrement".into(),
        Value::Number(s.digits.increment as f64),
    ));
    for (k, v) in [
        ("roundingMode", s.digits.mode),
        ("roundingPriority", s.digits.priority),
        ("trailingZeroDisplay", s.digits.trailing),
    ] {
        fields.push((k.into(), Value::String(v.into())));
    }
    Ok(Value::object(fields))
}
pub(super) fn install(e: &mut Environment, namespace: &Value) {
    let fp = e.get("Function").and_then(|f| f.get_prop("prototype"));
    let op = e.get("Object").and_then(|o| o.get_prop("prototype"));
    let ctor = Value::object_with_proto(vec![], fp.clone().map(Rc::new));
    super::super::make_callable(&ctor, construct, Some(construct));
    for (k, v) in [
        ("name", Value::String("NumberFormat".into())),
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
    nonenum(
        &ctor,
        "supportedLocalesOf",
        native_method("supportedLocalesOf", 1, supported_locales, fp.clone()),
    );
    let proto = Value::object_with_proto(vec![], op.map(Rc::new));
    nonenum(&proto, "constructor", ctor.clone());
    tag(&proto, "Intl.NumberFormat");
    for (name, len, f) in [
        ("resolvedOptions", 0, resolved_options as NativeFn),
        ("formatToParts", 1, format_to_parts),
        ("formatRange", 2, format_range),
        ("formatRangeToParts", 2, format_range_to_parts),
    ] {
        nonenum(&proto, name, native_method(name, len, f, fp.clone()));
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
    nonenum(namespace, "NumberFormat", ctor.clone());
    e.install_intrinsic("Intl.NumberFormat", ctor);
    e.install_intrinsic("Intl.NumberFormat.prototype", proto);
    e.install_intrinsic(
        "Intl.FallbackSymbol",
        super::super::symbol::new_symbol(Some("IntlLegacyConstructedSymbol".into())),
    );
}

#[derive(Clone)]
enum Math {
    Finite(Decimal),
    Nan,
    Infinity(bool),
}
fn parse_decimal(text: &str) -> Option<Decimal> {
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if ["0x", "0X", "0o", "0O", "0b", "0B"]
        .iter()
        .any(|p| text.starts_with(p))
    {
        return crate::bigint::BigInt::parse(text)
            .ok()?
            .to_string()
            .parse()
            .ok();
    }
    let (mantissa, exponent) = if let Some(index) = text.find(['e', 'E']) {
        (&text[..index], text[index + 1..].parse::<i16>().ok()?)
    } else {
        (text, 0)
    };
    let mut mantissa = mantissa.strip_prefix('+').unwrap_or(mantissa).to_string();
    if mantissa.starts_with('.') {
        mantissa.insert(0, '0');
    } else if mantissa.starts_with("-.") {
        mantissa.insert(1, '0');
    }
    if mantissa.ends_with('.') {
        mantissa.push('0');
    }
    let mut value: Decimal = mantissa.parse().ok()?;
    value.multiply_pow10(exponent);
    Some(value)
}
fn mathematical(i: &mut Interpreter, value: &Value) -> Result<Math, VmErr> {
    let value = i.coerce_object_to_primitive(value, "number")?;
    if let Value::BigInt(ref n) = value {
        return Ok(Math::Finite(n.to_string().parse().map_err(|_| range())?));
    }
    let number = i.ecmascript_to_number(&value)?;
    if number.is_nan() {
        return Ok(Math::Nan);
    }
    if number.is_infinite() {
        return Ok(Math::Infinity(number.is_sign_negative()));
    }
    let mut decimal: Decimal = if let Value::String(ref text) = value {
        let text = text.to_string();
        parse_decimal(&text)
    } else {
        None
    }
    .or_else(|| parse_decimal(ryu_js::Buffer::new().format(number)))
    .ok_or_else(range)?;
    if number.is_sign_negative() {
        decimal.set_sign(Sign::Negative);
    }
    Ok(Math::Finite(decimal))
}
#[derive(Clone, Debug, PartialEq)]
struct Piece {
    kind: &'static str,
    text: String,
}
fn push(out: &mut Vec<Piece>, kind: &'static str, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.kind == kind {
            last.text.push_str(text);
            return;
        }
    }
    out.push(Piece {
        kind,
        text: text.into(),
    });
}
#[derive(Default)]
struct Sink {
    out: Vec<Piece>,
    kind: Option<&'static str>,
}
impl fmt::Write for Sink {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        push(&mut self.out, self.kind.unwrap_or("literal"), text);
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
            self.kind = Some(part.value);
        }
        let r = f(self);
        self.kind = old;
        r
    }
}
fn decimal_parts(f: &DecimalFormatter, v: &Decimal) -> Vec<Piece> {
    let mut sink = Sink::default();
    f.format(v).write_to_parts(&mut sink).unwrap();
    sink.out
}
fn text(parts: &[Piece]) -> String {
    parts.iter().map(|p| p.text.as_str()).collect()
}
fn label(out: &mut Vec<Piece>, kind: &'static str, value: &str) {
    let mid = value.trim_matches(char::is_whitespace);
    if mid.is_empty() {
        push(out, "literal", value);
        return;
    }
    let start = value.find(mid).unwrap();
    push(out, "literal", &value[..start]);
    push(out, kind, mid);
    push(out, "literal", &value[start + mid.len()..]);
}
fn unquote(pattern: &str) -> String {
    let mut out = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                out.push('\'');
            }
        } else {
            out.push(c);
        }
    }
    out
}
fn category(s: &Slots, v: &Decimal) -> &'static str {
    match s
        .rules
        .category_for(PluralOperands::from_significand_and_exponent(
            &v.absolute,
            0,
        )) {
        icu_plurals::PluralCategory::Zero => "zero",
        icu_plurals::PluralCategory::One => "one",
        icu_plurals::PluralCategory::Two => "two",
        icu_plurals::PluralCategory::Few => "few",
        icu_plurals::PluralCategory::Many => "many",
        _ => "other",
    }
}
fn plural<'a>(object: &'a Json, prefix: &str, cat: &str) -> Option<&'a str> {
    object
        .get(format!("{prefix}-count-{cat}"))
        .or_else(|| object.get(format!("{prefix}-count-other")))
        .and_then(Json::as_str)
}
fn number_data<'a>(s: &'a Slots, prefix: &str) -> &'a Json {
    s.data["numbers"]
        .get(format!("{prefix}-numberSystem-{}", s.numbering))
        .unwrap_or(&s.data["numbers"][format!("{prefix}-numberSystem-latn")])
}
fn symbol<'a>(s: &'a Slots, key: &str) -> &'a str {
    number_data(s, "symbols")[key].as_str().unwrap_or("")
}
fn sign_parts(s: &Slots, negative: bool) -> Vec<Piece> {
    let raw = symbol(s, if negative { "minusSign" } else { "plusSign" });
    let mut parts = Vec::new();
    for c in raw.chars() {
        let t = c.to_string();
        push(
            &mut parts,
            if c == '+' {
                "plusSign"
            } else if c == '-' || c == '−' {
                "minusSign"
            } else {
                "literal"
            },
            &t,
        );
    }
    parts
}

// Decimal pattern placeholder substitution, with CLDR quoting and typed affixes.
fn pattern(s: &Slots, pattern: &str, number: &[Piece], currency: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut quoted = false;
    let mut numeric = false;
    let mut inserted = false;
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                push(&mut out, "literal", "'");
            } else {
                quoted = !quoted;
            }
            continue;
        }
        if !quoted
            && matches!(c, '#' | '0' | '@' | '.' | ',' | 'E' | '+')
            && (numeric || matches!(c, '#' | '0' | '@'))
        {
            if !inserted {
                out.extend_from_slice(number);
                inserted = true;
            }
            numeric = true;
            continue;
        }
        numeric = false;
        match c {
            '¤' if !quoted => {
                while chars.peek() == Some(&'¤') {
                    chars.next();
                }
                push(&mut out, "currency", currency);
            }
            '%' if !quoted => push(&mut out, "percentSign", symbol(s, "percentSign")),
            _ => push(&mut out, "literal", &c.to_string()),
        }
    }
    out
}
fn unit_pattern(s: &Slots, unit: &str, cat: &str) -> String {
    let units = &s.data["units"][&s.unit_display];
    if let Some(p) = plural(&units[unit], "unitPattern", cat) {
        return p.into();
    }
    if let Some((first, second)) = unit.split_once("-per-") {
        let numerator = plural(&units[first], "unitPattern", cat).unwrap_or("{0}");
        if let Some(per) = units[second]["perUnitPattern"].as_str() {
            return per.replace("{0}", numerator);
        }
        let denom = plural(&units[second], "unitPattern", "one")
            .unwrap_or(second)
            .replace("{0}", "")
            .trim()
            .to_string();
        return units["per"]["compoundUnitPattern"]
            .as_str()
            .unwrap_or("{0}/{1}")
            .replace("{0}", numerator)
            .replace("{1}", &denom);
    }
    format!("{{0}} {unit}")
}
fn substitute(pattern: &str, number: &[Piece], kind: &'static str) -> Vec<Piece> {
    let mut out = Vec::new();
    if let Some((before, after)) = pattern.split_once("{0}") {
        label(&mut out, kind, before);
        out.extend_from_slice(number);
        label(&mut out, kind, after);
    } else {
        label(&mut out, kind, pattern);
    }
    out
}
fn compact_pattern(s: &Slots, magnitude: i16, cat: &str) -> (i16, String) {
    let formats = &number_data(s, "decimalFormats")[&s.compact]["decimalFormat"];
    for mag in (0..=magnitude.min(30)).rev() {
        let power = format!("1{}", "0".repeat(mag as usize));
        if let Some(p) = plural(formats, &power, cat) {
            let zeros = p.chars().filter(|c| *c == '0').count();
            if p != "0" {
                return (mag - (zeros.max(1) as i16 - 1), p.into());
            }
        }
    }
    (0, "0".into())
}
fn render(s: &Slots, input: &Math) -> Result<Vec<Piece>, VmErr> {
    let mut value = match input {
        Math::Finite(v) => Some(v.clone()),
        _ => None,
    };
    if s.style == "percent" {
        if let Some(v) = &mut value {
            v.multiply_pow10(2);
        }
    }
    let mut exponent = 0;
    let mut compact = "0".to_string();
    if let Some(v) = &value {
        if !v.is_zero() {
            let mag = v.absolute.nonzero_magnitude_start();
            match s.notation.as_str() {
                "scientific" => exponent = mag,
                "engineering" => exponent = mag.div_euclid(3) * 3,
                "compact" => {
                    (exponent, compact) = compact_pattern(s, mag, "other");
                }
                _ => {}
            }
        }
    }
    let mut rounded = value.as_ref().map(|v| {
        let mut v = v.clone();
        v.multiply_pow10(-exponent);
        s.digits.round(&v)
    });
    if let (Some(original), Some(result)) = (&value, &rounded) {
        if !result.is_zero() {
            let grown = result.absolute.nonzero_magnitude_start();
            let adjust = match s.notation.as_str() {
                "scientific" if grown >= 1 => 1,
                "engineering" if grown >= 3 => 3,
                "compact" => {
                    let (next, p) = compact_pattern(
                        s,
                        original.absolute.nonzero_magnitude_start() + 1,
                        "other",
                    );
                    if next > exponent
                        && grown + exponent > original.absolute.nonzero_magnitude_start()
                    {
                        compact = p;
                        next - exponent
                    } else {
                        0
                    }
                }
                _ => 0,
            };
            if adjust != 0 {
                exponent += adjust;
                let mut v = original.clone();
                v.multiply_pow10(-exponent);
                rounded = Some(s.digits.round(&v));
            }
        }
    }
    let negative = match input {
        Math::Infinity(n) => *n,
        Math::Nan => false,
        _ => rounded.as_ref().is_some_and(|v| v.sign() == Sign::Negative),
    };
    let zero = rounded.as_ref().is_some_and(|v| v.is_zero());
    let sign = match s.sign.as_str() {
        "never" => None,
        "always" => Some(negative),
        "exceptZero" if !zero && !matches!(input, Math::Nan) => Some(negative),
        "negative" if negative && !zero => Some(true),
        "auto" if negative => Some(true),
        _ => None,
    };
    let cat = rounded.as_ref().map_or("other", |v| category(s, v));
    let mut number = if let Some(v) = &rounded {
        let v = v.clone().with_sign(Sign::None);
        decimal_parts(
            if matches!(s.notation.as_str(), "scientific" | "engineering") {
                &s.plain
            } else {
                &s.decimal
            },
            &v,
        )
    } else {
        vec![Piece {
            kind: if matches!(input, Math::Nan) {
                "nan"
            } else {
                "infinity"
            },
            text: symbol(
                s,
                if matches!(input, Math::Nan) {
                    "nan"
                } else {
                    "infinity"
                },
            )
            .into(),
        }]
    };
    if rounded.is_some() && matches!(s.notation.as_str(), "scientific" | "engineering") {
        push(&mut number, "exponentSeparator", symbol(s, "exponential"));
        if exponent < 0 {
            let sign = sign_parts(s, true);
            for mut p in sign {
                if p.kind == "minusSign" {
                    p.kind = "exponentMinusSign";
                }
                number.push(p);
            }
        }
        let e: Decimal = exponent.unsigned_abs().to_string().parse().unwrap();
        for mut p in decimal_parts(&s.plain, &e) {
            if p.kind == "integer" {
                p.kind = "exponentInteger";
            }
            number.push(p);
        }
    } else if s.notation == "compact" && exponent != 0 {
        let (_, p) = compact_pattern(
            s,
            value.as_ref().unwrap().absolute.nonzero_magnitude_start(),
            cat,
        );
        if p != "0" {
            compact = p;
        }
        let mut out = Vec::new();
        if let (Some(first), Some(last)) = (compact.find('0'), compact.rfind('0')) {
            label(&mut out, "compact", &unquote(&compact[..first]));
            out.extend(number);
            label(&mut out, "compact", &unquote(&compact[last + 1..]));
        } else {
            label(&mut out, "compact", &unquote(&compact));
        }
        number = out;
    }
    if let Some(digits) = &s.digit_map {
        for p in &mut number {
            if matches!(p.kind, "integer" | "fraction" | "exponentInteger") {
                p.text = p
                    .text
                    .chars()
                    .map(|c| {
                        if c.is_ascii_digit() {
                            digits[(c as u8 - b'0') as usize]
                        } else {
                            c
                        }
                    })
                    .collect();
            }
        }
    }
    let mut out = match s.style.as_str() {
        "unit" => {
            if let Some(n) = sign {
                let mut signed = sign_parts(s, n);
                signed.append(&mut number);
                number = signed;
            }
            return Ok(substitute(
                &unit_pattern(s, s.unit.as_ref().unwrap(), cat),
                &number,
                "unit",
            ));
        }
        "currency" => {
            let code = s.currency.as_ref().unwrap();
            let c = &s.data["currencies"][code];
            let currency = match s.currency_display.as_str() {
                "code" => code.as_str(),
                "name" => plural(c, "displayName", cat)
                    .or_else(|| c["displayName"].as_str())
                    .unwrap_or(code),
                "narrowSymbol" => c["symbol-alt-narrow"]
                    .as_str()
                    .or_else(|| c["symbol"].as_str())
                    .unwrap_or(code),
                _ => c["symbol"].as_str().unwrap_or(code),
            };
            let patterns = number_data(s, "currencyFormats");
            if s.currency_display == "name" {
                let p = plural(patterns, "unitPattern", cat)
                    .unwrap_or("{0} {1}")
                    .replace("{1}", currency);
                substitute(&p, &number, "currency")
            } else {
                let key = if s.currency_sign == "accounting" {
                    "accounting"
                } else {
                    "standard"
                };
                let p = patterns[key].as_str().unwrap_or("¤#,##0.00");
                let alpha = (p.starts_with('¤')
                    && currency.chars().last().is_some_and(char::is_alphabetic))
                    || (!p.starts_with('¤')
                        && currency.chars().next().is_some_and(char::is_alphabetic));
                let p = if alpha {
                    patterns[format!("{key}-alphaNextToNumber")]
                        .as_str()
                        .unwrap_or(p)
                } else {
                    p
                };
                let p = if sign == Some(true) {
                    p.split_once(';').map_or(p, |p| p.1)
                } else {
                    p.split(';').next().unwrap()
                };
                let explicit_negative = sign == Some(true) && p.contains('(');
                let mut out = pattern(s, p, &number, currency);
                if explicit_negative {
                    return Ok(out);
                }
                if let Some(n) = sign {
                    let mut signed = sign_parts(s, n);
                    signed.append(&mut out);
                    out = signed;
                }
                return Ok(out);
            }
        }
        "percent" => {
            let p = number_data(s, "percentFormats")["standard"]
                .as_str()
                .unwrap_or("#,##0%");
            pattern(s, p.split(';').next().unwrap(), &number, "")
        }
        _ => number,
    };
    if let Some(n) = sign {
        let mut signed = sign_parts(s, n);
        signed.append(&mut out);
        out = signed;
    }
    Ok(out)
}
fn output(parts: Vec<Piece>, as_parts: bool) -> Result<Value, VmErr> {
    if !as_parts {
        return Ok(Value::String(text(&parts).into()));
    }
    Value::checked_array(
        parts
            .into_iter()
            .map(|p| {
                Value::object(vec![
                    ("type".into(), Value::String(p.kind.into())),
                    ("value".into(), Value::String(p.text.into())),
                ])
            })
            .collect(),
    )
}
fn format(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    let value = mathematical(i, &arg(&args, 0))?;
    output(render(&s, &value)?, false)
}
fn format_to_parts(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    let value = mathematical(i, &arg(&args, 0))?;
    output(render(&s, &value)?, true)
}
fn range_parts(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
    as_parts: bool,
) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    if matches!(arg(&args, 0), Value::Undefined) || matches!(arg(&args, 1), Value::Undefined) {
        return Err(error());
    }
    let a = mathematical(i, &arg(&args, 0))?;
    let b = mathematical(i, &arg(&args, 1))?;
    if matches!(a, Math::Nan) || matches!(b, Math::Nan) {
        return Err(range());
    }
    let mut a = render(&s, &a)?;
    let mut b = render(&s, &b)?;
    let mut result = Vec::<(Piece, &str)>::new();
    if a == b {
        result.push((
            Piece {
                kind: "approximatelySign",
                text: symbol(&s, "approximatelySign").into(),
            },
            "shared",
        ));
        result.extend(a.into_iter().map(|p| (p, "shared")));
    } else {
        let mut suffix = Vec::new();
        while a.last() == b.last()
            && a.last()
                .is_some_and(|p| matches!(p.kind, "currency" | "unit" | "percentSign" | "literal"))
        {
            suffix.push(a.pop().unwrap());
            b.pop();
        }
        suffix.reverse();
        let prefix_currency = a
            .iter()
            .take_while(|p| !matches!(p.kind, "integer" | "nan" | "infinity"))
            .any(|p| p.kind == "currency");
        if !suffix.is_empty() || (prefix_currency && s.digits.min_f.unwrap_or(0) > 0) {
            while a.first() == b.first()
                && a.first()
                    .is_some_and(|p| !matches!(p.kind, "integer" | "fraction" | "nan" | "infinity"))
            {
                result.push((a.remove(0), "shared"));
                b.remove(0);
            }
        }
        let mut separator = number_data(&s, "miscPatterns")["range"]
            .as_str()
            .unwrap_or("{0}–{1}")
            .replace("{0}", "")
            .replace("{1}", "");
        if s.style == "currency"
            && prefix_currency
            && s.digits.min_f.unwrap_or(0) == 0
            && suffix.is_empty()
            && separator.trim() == separator
        {
            separator = format!(" {separator} ");
        }
        result.extend(a.into_iter().map(|p| (p, "startRange")));
        result.push((
            Piece {
                kind: "literal",
                text: separator,
            },
            "shared",
        ));
        result.extend(b.into_iter().map(|p| (p, "endRange")));
        result.extend(suffix.into_iter().map(|p| (p, "shared")));
    }
    if !as_parts {
        return Ok(Value::String(
            result
                .iter()
                .map(|(p, _)| p.text.as_str())
                .collect::<String>()
                .into(),
        ));
    }
    Value::checked_array(
        result
            .into_iter()
            .map(|(p, source)| {
                Value::object(vec![
                    ("type".into(), Value::String(p.kind.into())),
                    ("value".into(), Value::String(p.text.into())),
                    ("source".into(), Value::String(source.into())),
                ])
            })
            .collect(),
    )
}
fn format_range(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    range_parts(i, this, args, false)
}
fn format_range_to_parts(
    i: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    range_parts(i, this, args, true)
}
pub(in crate::builtins) fn to_locale(
    i: &mut Interpreter,
    value: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let s = initialize(i, &args)?;
    let value = mathematical(i, &value)?;
    output(render(&s, &value)?, false)
}
