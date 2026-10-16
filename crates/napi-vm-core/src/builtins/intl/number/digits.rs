use super::*;
use fixed_decimal::{RoundingIncrement, SignedRoundingMode as RM, UnsignedRoundingMode as UM};
#[derive(Clone)]
pub(super) struct Digits {
    pub integer: i16,
    pub min_f: Option<i16>,
    pub max_f: Option<i16>,
    pub min_s: Option<i16>,
    pub max_s: Option<i16>,
    pub increment: i16,
    pub mode: String,
    pub priority: String,
    pub trailing: String,
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
pub(super) fn read(
    i: &mut Interpreter,
    o: &Value,
    min_default: i16,
    max_default: i16,
    notation: &str,
) -> Result<Digits, VmErr> {
    let v = i.member(o, "minimumIntegerDigits")?;
    let integer = number(i, &v, 1, 21, 1)?;
    let min_f = i.member(o, "minimumFractionDigits")?;
    let max_f = i.member(o, "maximumFractionDigits")?;
    let min_s = i.member(o, "minimumSignificantDigits")?;
    let max_s = i.member(o, "maximumSignificantDigits")?;
    let inc = i.member(o, "roundingIncrement")?;
    let increment = number(i, &inc, 1, 5000, 1)?;
    if ![
        1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000,
    ]
    .contains(&increment)
    {
        return Err(range());
    }
    let mode = choice(
        i,
        o,
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
        o,
        "roundingPriority",
        "auto",
        &["auto", "morePrecision", "lessPrecision"],
    )?;
    let trailing = choice(
        i,
        o,
        "trailingZeroDisplay",
        "auto",
        &["auto", "stripIfInteger"],
    )?;
    let has_s = !matches!(min_s, Value::Undefined) || !matches!(max_s, Value::Undefined);
    let has_f = !matches!(min_f, Value::Undefined) || !matches!(max_f, Value::Undefined);
    let compact = notation == "compact" && !has_s && !has_f && priority == "auto";
    let need_s = has_s || priority != "auto" || compact;
    let need_f = priority != "auto" || !has_s || compact;
    let (min_s, max_s) = if need_s {
        let min = number(i, &min_s, 1, 21, 1)?;
        let max = number(i, &max_s, 1, 21, if compact { 2 } else { 21 })?;
        if min > max {
            return Err(range());
        }
        (Some(min), Some(max))
    } else {
        (None, None)
    };
    let (min_f, max_f) = if need_f {
        if compact {
            (Some(0), Some(0))
        } else {
            let min = number(
                i,
                &min_f,
                0,
                100,
                if matches!(max_f, Value::Undefined) {
                    min_default
                } else {
                    0
                },
            )?;
            let max = number(i, &max_f, 0, 100, max_default.max(min))?;
            let min = if matches!(min_f, Value::Undefined) {
                min_default.min(max)
            } else {
                min
            };
            if min > max {
                return Err(range());
            }
            (Some(min), Some(max))
        }
    } else {
        (None, None)
    };
    if compact {
        priority = "morePrecision".into();
    }
    if increment != 1 {
        if min_s.is_some() || priority != "auto" {
            return Err(VmErr::Msg(
                "TypeError: Rounding increment requires fraction digits".into(),
            ));
        }
        if min_f != max_f {
            return Err(range());
        }
    }
    Ok(Digits {
        integer,
        min_f,
        max_f,
        min_s,
        max_s,
        increment,
        mode,
        priority,
        trailing,
    })
}
fn mode(name: &str) -> RM {
    match name {
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
impl Digits {
    pub fn round(&self, input: &Decimal) -> Decimal {
        let fraction = self.max_f.map(|max| {
            let mut value = input.clone();
            let mut inc = self.increment;
            let mut position = -max;
            while inc % 10 == 0 {
                inc /= 10;
                position += 1;
            }
            let increment = match inc {
                2 => RoundingIncrement::MultiplesOf2,
                5 => RoundingIncrement::MultiplesOf5,
                25 => RoundingIncrement::MultiplesOf25,
                _ => RoundingIncrement::MultiplesOf1,
            };
            value.round_with_mode_and_increment(position, mode(&self.mode), increment);
            value.trim_end();
            value.pad_end(-self.min_f.unwrap());
            value
        });
        let mut value = if let Some(max) = self.max_s {
            let precision = input.absolute.nonzero_magnitude_start() - max + 1;
            let mut sig = input.clone();
            sig.round_with_mode(precision, mode(&self.mode));
            sig.trim_end();
            let padding = (sig.absolute.nonzero_magnitude_start() - self.min_s.unwrap() + 1).min(0);
            sig.pad_end(padding);
            if let Some(fraction) = fraction {
                let f = -self.max_f.unwrap();
                if (self.priority == "morePrecision" && f < precision)
                    || (self.priority == "lessPrecision" && f >= precision)
                {
                    fraction
                } else {
                    sig
                }
            } else {
                sig
            }
        } else {
            fraction.unwrap()
        };
        if self.trailing == "stripIfInteger" {
            value.trim_end_if_integer();
        }
        value.pad_start(self.integer);
        value
    }
}
