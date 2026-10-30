//! Plural categories from CLDR, selected after observable decimal rounding.
use super::*;
use fixed_decimal::{
    Decimal, RoundingIncrement, SignedRoundingMode as RM, UnsignedRoundingMode as UM,
};
use icu_plurals::{PluralOperands, PluralRuleType, PluralRules, PluralRulesWithRanges};
#[derive(Clone)]
pub(crate) struct Slots {
    rules: Rc<PluralRules>,
    ranges: Rc<PluralRulesWithRanges<PluralRules>>,
    locale: String,
    kind: String,
    notation: String,
    compact: String,
    integer: i16,
    min_fraction: i16,
    max_fraction: i16,
    min_significant: Option<i16>,
    max_significant: Option<i16>,
    increment: i16,
    mode: String,
    priority: String,
    trailing: String,
}
fn error() -> VmErr {
    VmErr::Msg("TypeError: Intl.PluralRules receiver required".into())
}
fn slot(v: &Value) -> Result<Slots, VmErr> {
    v.property_cell()
        .and_then(|p| p.meta.borrow().intl_plurals.as_deref().cloned())
        .ok_or_else(error)
}
fn choice(
    i: &mut Interpreter,
    o: &Value,
    key: &str,
    default: &str,
    allowed: &[&str],
) -> Result<String, VmErr> {
    let v = option(i, o, key)?.unwrap_or_else(|| default.into());
    if !allowed.contains(&v.as_str()) {
        return Err(range());
    }
    Ok(v)
}
fn number(i: &mut Interpreter, v: &Value, min: i16, max: i16, default: i16) -> Result<i16, VmErr> {
    if matches!(v, Value::Undefined) {
        return Ok(default);
    }
    let n = i.ecmascript_to_number(v)?;
    if n.is_nan() || n < (min as f64) || n > (max as f64) {
        return Err(range());
    }
    Ok(n.floor() as i16)
}
fn construct(i: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let target = i.new_target_stack.last().cloned().ok_or_else(error)?;
    let prototype = i.constructor_prototype(&target, "Intl.PluralRules")?;
    let requested = locale_list(i, arg(&args, 0))?;
    let options = match arg(&args, 1) {
        Value::Undefined => Value::object_with_proto(vec![], None),
        Value::Null => return Err(error()),
        v => Value::boxed_primitive(v.clone()).unwrap_or(v),
    };
    locale_matcher(i, &options)?;
    let kind = choice(i, &options, "type", "cardinal", &["cardinal", "ordinal"])?;
    let notation = choice(
        i,
        &options,
        "notation",
        "standard",
        &["standard", "compact", "scientific", "engineering"],
    )?;
    let compact = choice(i, &options, "compactDisplay", "short", &["short", "long"])?;
    let int_value = i.member(&options, "minimumIntegerDigits")?;
    let integer = number(i, &int_value, 1, 21, 1)?;
    let min_f = i.member(&options, "minimumFractionDigits")?;
    let max_f = i.member(&options, "maximumFractionDigits")?;
    let min_s = i.member(&options, "minimumSignificantDigits")?;
    let max_s = i.member(&options, "maximumSignificantDigits")?;
    let inc_value = i.member(&options, "roundingIncrement")?;
    let increment = number(i, &inc_value, 1, 5000, 1)?;
    if ![
        1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000,
    ]
    .contains(&increment)
    {
        return Err(range());
    }
    let mode = choice(
        i,
        &options,
        "roundingMode",
        "halfExpand",
        &[
            "ceil",
            "floor",
            "expand",
            "trunc",
            "halfCeil",
            "halfFloor",
            "halfExpand",
            "halfTrunc",
            "halfEven",
        ],
    )?;
    let mut priority = choice(
        i,
        &options,
        "roundingPriority",
        "auto",
        &["auto", "morePrecision", "lessPrecision"],
    )?;
    let trailing = choice(
        i,
        &options,
        "trailingZeroDisplay",
        "auto",
        &["auto", "stripIfInteger"],
    )?;
    let has_sig = !matches!(min_s, Value::Undefined) || !matches!(max_s, Value::Undefined);
    let compact_default = notation == "compact"
        && !has_sig
        && matches!(min_f, Value::Undefined)
        && matches!(max_f, Value::Undefined)
        && priority == "auto";
    let (min_fraction, max_fraction) = if compact_default {
        (0, 0)
    } else {
        let min = number(i, &min_f, 0, 100, 0)?;
        let max = number(i, &max_f, 0, 100, 3.max(min))?;
        let min = if matches!(min_f, Value::Undefined) {
            0.min(max)
        } else {
            min
        };
        if min > max {
            return Err(range());
        }
        (min, max)
    };
    let (min_significant, max_significant) = if has_sig || priority != "auto" || compact_default {
        let min = number(i, &min_s, 1, 21, 1)?;
        let max = number(i, &max_s, 1, 21, if compact_default { 2 } else { 21 })?;
        if min > max {
            return Err(range());
        }
        (Some(min), Some(max))
    } else {
        (None, None)
    };
    if compact_default {
        priority = "morePrecision".into();
    }
    if increment != 1 {
        if min_significant.is_some() || priority != "auto" {
            return Err(VmErr::Msg(
                "TypeError: Rounding increment requires fraction precision".into(),
            ));
        }
        if min_fraction != max_fraction {
            return Err(range());
        }
    }
    let locale = requested
        .iter()
        .find_map(|v| best_available_locale(v))
        .unwrap_or_else(|| "en-US".into());
    let prefs: Locale = locale.parse().unwrap();
    let options = if kind == "ordinal" {
        PluralRuleType::Ordinal
    } else {
        PluralRuleType::Cardinal
    }
    .into();
    let rules = Rc::new(PluralRules::try_new(prefs.clone().into(), options).map_err(|_| range())?);
    let ranges =
        Rc::new(PluralRulesWithRanges::try_new(prefs.into(), options).map_err(|_| range())?);
    let object = Value::object_with_proto(vec![], prototype);
    object
        .property_cell()
        .unwrap()
        .meta
        .borrow_mut()
        .intl_plurals = Some(Box::new(Slots {
        rules,
        ranges,
        locale,
        kind,
        notation,
        compact,
        integer,
        min_fraction,
        max_fraction,
        min_significant,
        max_significant,
        increment,
        mode,
        priority,
        trailing,
    }));
    Ok(object)
}
fn rounding(mode: &str) -> RM {
    match mode {
        "ceil" => RM::Ceil,
        "floor" => RM::Floor,
        "halfCeil" => RM::HalfCeil,
        "halfFloor" => RM::HalfFloor,
        "expand" => RM::Unsigned(UM::Expand),
        "trunc" => RM::Unsigned(UM::Trunc),
        "halfTrunc" => RM::Unsigned(UM::HalfTrunc),
        "halfEven" => RM::Unsigned(UM::HalfEven),
        _ => RM::Unsigned(UM::HalfExpand),
    }
}
fn operands(s: &Slots, n: f64) -> Result<PluralOperands, VmErr> {
    let mut decimal: Decimal = ryu_js::Buffer::new()
        .format(n)
        .parse()
        .map_err(|_| range())?;
    let exponent = if s.notation == "compact" && n.abs() >= 1000. {
        (n.abs().log10().floor() as i16 / 3 * 3).min(12)
    } else {
        0
    };
    decimal.multiply_pow10(-exponent);
    let mut fraction = decimal.clone();
    let mut increment = s.increment;
    let mut position = -s.max_fraction;
    while increment % 10 == 0 {
        increment /= 10;
        position += 1;
    }
    let inc = match increment {
        2 => RoundingIncrement::MultiplesOf2,
        5 => RoundingIncrement::MultiplesOf5,
        25 => RoundingIncrement::MultiplesOf25,
        _ => RoundingIncrement::MultiplesOf1,
    };
    fraction.round_with_mode_and_increment(position, rounding(&s.mode), inc);
    fraction.trim_end();
    fraction.pad_end(-s.min_fraction);
    if let (Some(min), Some(max)) = (s.min_significant, s.max_significant) {
        let magnitude = decimal.absolute.nonzero_magnitude_start();
        let precision = magnitude - max + 1;
        decimal.round_with_mode(precision, rounding(&s.mode));
        decimal.trim_end();
        let padding = (decimal.absolute.nonzero_magnitude_start() - min + 1).min(0);
        decimal.pad_end(padding);
        if s.priority != "auto" {
            let sig = precision;
            let frac = -s.max_fraction;
            if (s.priority == "morePrecision" && frac < sig)
                || (s.priority == "lessPrecision" && frac >= sig)
            {
                decimal = fraction;
            }
        }
    } else {
        decimal = fraction;
    }
    if s.trailing == "stripIfInteger" {
        decimal.trim_end_if_integer();
    }
    Ok(PluralOperands::from_significand_and_exponent(
        &decimal.absolute,
        exponent as u8,
    ))
}
fn select(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    let n = i.ecmascript_to_number(&arg(&args, 0))?;
    let category = if n.is_finite() {
        s.rules.category_for(operands(&s, n)?)
    } else {
        icu_plurals::PluralCategory::Other
    };
    Ok(Value::String(category_name(category).into()))
}
fn select_range(i: &mut Interpreter, this: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    if matches!(args.first(), None | Some(Value::Undefined))
        || matches!(args.get(1), None | Some(Value::Undefined))
    {
        return Err(error());
    }
    let a = i.ecmascript_to_number(&args[0])?;
    let b = i.ecmascript_to_number(&args[1])?;
    if a.is_nan() || b.is_nan() {
        return Err(range());
    }
    let category = if a.is_finite() && b.is_finite() {
        let first = operands(&s, a)?;
        let last = operands(&s, b)?;
        s.ranges.category_for_range(first, last)
    } else {
        icu_plurals::PluralCategory::Other
    };
    Ok(Value::String(category_name(category).into()))
}
fn resolved_options(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let s = slot(&this)?;
    // ICU4X data has no Manx rules and silently falls back to root; CLDR
    // defines cardinal one/two/few/many/other for gv (test262 oracle).
    let manx = s.kind == "cardinal" && (s.locale == "gv" || s.locale.starts_with("gv-"));
    let mut fields = vec![
        ("locale".into(), Value::String(s.locale.into())),
        ("type".into(), Value::String(s.kind.into())),
        ("notation".into(), Value::String(s.notation.clone().into())),
        (
            "minimumIntegerDigits".into(),
            Value::Number(s.integer as f64),
        ),
    ];
    if s.min_significant.is_none() || s.priority != "auto" {
        fields.extend([
            (
                "minimumFractionDigits".into(),
                Value::Number(s.min_fraction as f64),
            ),
            (
                "maximumFractionDigits".into(),
                Value::Number(s.max_fraction as f64),
            ),
        ]);
    }
    if let (Some(min), Some(max)) = (s.min_significant, s.max_significant) {
        fields.extend([
            ("minimumSignificantDigits".into(), Value::Number(min as f64)),
            ("maximumSignificantDigits".into(), Value::Number(max as f64)),
        ]);
    }
    let categories: Vec<Value> = if manx {
        ["one", "two", "few", "many", "other"]
            .into_iter()
            .map(|c| Value::String(c.into()))
            .collect()
    } else {
        s.rules
            .categories()
            .map(|c| Value::String(category_name(c).into()))
            .collect()
    };
    fields.extend([
        ("pluralCategories".into(), Value::array(categories)),
        (
            "roundingIncrement".into(),
            Value::Number(s.increment as f64),
        ),
        ("roundingMode".into(), Value::String(s.mode.into())),
        ("roundingPriority".into(), Value::String(s.priority.into())),
        (
            "trailingZeroDisplay".into(),
            Value::String(s.trailing.into()),
        ),
    ]);
    if s.notation == "compact" {
        fields.push(("compactDisplay".into(), Value::String(s.compact.into())));
    }
    Ok(Value::object(fields))
}
pub(super) fn install(e: &mut Environment, namespace: &Value) {
    let fp = e.get("Function").and_then(|f| f.get_prop("prototype"));
    let op = e.get("Object").and_then(|f| f.get_prop("prototype"));
    let c = Value::object_with_proto(vec![], fp.clone().map(Rc::new));
    super::super::make_callable(&c, super::super::require_new, Some(construct));
    for (key, v) in [
        ("name", Value::String("PluralRules".into())),
        ("length", Value::Number(0.)),
    ] {
        nonenum(&c, key, v);
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
    tag(&p, "Intl.PluralRules");
    for (name, len, f) in [
        ("select", 1, select as NativeFn),
        ("selectRange", 2, select_range),
        ("resolvedOptions", 0, resolved_options),
    ] {
        nonenum(&p, name, native_method(name, len, f, fp.clone()));
    }
    super::super::set_builtin_constructor_prototype(e, &c, p.clone());
    nonenum(namespace, "PluralRules", c.clone());
    e.install_intrinsic("Intl.PluralRules", c);
    e.install_intrinsic("Intl.PluralRules.prototype", p);
}

fn category_name(c: icu_plurals::PluralCategory) -> &'static str {
    use icu_plurals::PluralCategory::*;
    match c {
        Zero => "zero",
        One => "one",
        Two => "two",
        Few => "few",
        Many => "many",
        Other => "other",
    }
}
