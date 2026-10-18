//! Strict ECMA-262 Unicode property lookup using the shared ICU data version.
use icu_properties::props::{ChangesWhenCasefolded, GeneralCategory, GeneralCategoryGroup, Script};
use icu_properties::script::ScriptWithExtensions;
use icu_properties::{CodePointMapData, CodePointSetData, PropertyParser};
use regex_syntax::hir::{ClassUnicode, ClassUnicodeRange};
use std::ops::RangeInclusive;

fn ranges(property: &str) -> Option<Vec<RangeInclusive<u32>>> {
    if let Some((name, value)) = property.split_once('=') {
        return match name {
            "General_Category" | "gc" => {
                let group = PropertyParser::<GeneralCategoryGroup>::new().get_strict(value)?;
                Some(
                    CodePointMapData::<GeneralCategory>::new()
                        .iter_ranges_for_group(group)
                        .collect(),
                )
            }
            "Script" | "sc" | "Script_Extensions" | "scx" => {
                let script = PropertyParser::<Script>::new().get_strict(value)?;
                if matches!(name, "Script" | "sc") {
                    Some(
                        CodePointMapData::<Script>::new()
                            .iter_ranges_for_value(script)
                            .collect(),
                    )
                } else {
                    Some(
                        ScriptWithExtensions::new()
                            .get_script_extensions_ranges(script)
                            .collect(),
                    )
                }
            }
            _ => None,
        };
    }
    match property {
        "Any" => return Some(std::iter::once(0..=0x10ffff).collect()),
        "ASCII" => return Some(std::iter::once(0..=0x7f).collect()),
        "Assigned" => {
            return Some(
                CodePointMapData::<GeneralCategory>::new()
                    .iter_ranges_for_value_complemented(GeneralCategory::Unassigned)
                    .collect(),
            );
        }
        _ => {}
    }
    if let Some(group) = PropertyParser::<GeneralCategoryGroup>::new().get_strict(property) {
        return Some(
            CodePointMapData::<GeneralCategory>::new()
                .iter_ranges_for_group(group)
                .collect(),
        );
    }
    Some(
        CodePointSetData::new_for_ecma262(property.as_bytes())?
            .iter_ranges()
            .collect(),
    )
}

pub(super) fn property(property: &str) -> Option<(ClassUnicode, bool)> {
    let ranges = ranges(property)?;
    let surrogates = ranges.iter().any(|r| r.contains(&0xd800));
    // ClassUnicode stores Rust chars; preserve the surrogate membership
    // separately and split any interval crossing that gap.
    let scalar_ranges = ranges.into_iter().flat_map(|r| {
        let (lo, hi) = (*r.start(), *r.end());
        [(lo, hi.min(0xd7ff)), (lo.max(0xe000), hi)]
            .into_iter()
            .filter(|&(lo, hi)| lo <= hi)
            .map(|(lo, hi)| {
                ClassUnicodeRange::new(char::from_u32(lo).unwrap(), char::from_u32(hi).unwrap())
            })
    });
    Some((ClassUnicode::new(scalar_ranges), surrogates))
}

/// Equivalence classes for Unicode simple (not full or Turkic) case folding.
fn fold_classes() -> &'static Vec<Vec<char>> {
    static CLASSES: std::sync::OnceLock<Vec<Vec<char>>> = std::sync::OnceLock::new();
    CLASSES.get_or_init(|| {
        let mapper = icu_casemap::CaseMapper::new();
        let mut groups = std::collections::BTreeMap::<char, Vec<char>>::new();
        for range in CodePointSetData::new::<ChangesWhenCasefolded>().iter_ranges() {
            for cp in range {
                let Some(c) = char::from_u32(cp) else {
                    continue;
                };
                let folded = mapper.simple_fold(c);
                if folded != c {
                    groups.entry(folded).or_insert_with(|| vec![folded]).push(c);
                }
            }
        }
        groups.into_values().collect()
    })
}

pub(super) fn case_closure(class: &ClassUnicode) -> ClassUnicode {
    let mut ranges = class.ranges().to_vec();
    for group in fold_classes() {
        if group.iter().any(|c| {
            let index = class.ranges().partition_point(|r| r.end() < *c);
            class.ranges().get(index).is_some_and(|r| r.start() <= *c)
        }) {
            ranges.extend(group.iter().map(|c| ClassUnicodeRange::new(*c, *c)));
        }
    }
    ClassUnicode::new(ranges)
}

pub(super) fn canonical(c: u32, unicode: bool) -> u32 {
    let Some(ch) = char::from_u32(c) else {
        return c;
    };
    if unicode {
        return icu_casemap::CaseMapper::new().simple_fold(ch) as u32;
    }
    // Legacy Canonicalize uses a single-character full uppercase mapping
    // and never maps a non-ASCII input to ASCII.
    let mut upper = ch.to_uppercase();
    let first = upper.next().unwrap() as u32;
    if upper.next().is_some() || (c >= 128 && first < 128) {
        c
    } else {
        first
    }
}

pub(super) fn range_matches(c: u32, low: u32, high: u32, unicode: bool) -> bool {
    type Groups = std::collections::HashMap<u32, Vec<u32>>;
    static UNICODE: std::sync::OnceLock<Groups> = std::sync::OnceLock::new();
    static LEGACY: std::sync::OnceLock<Groups> = std::sync::OnceLock::new();
    let groups = if unicode {
        UNICODE.get_or_init(|| {
            fold_classes()
                .iter()
                .map(|g| (g[0] as u32, g.iter().map(|c| *c as u32).collect()))
                .collect()
        })
    } else {
        LEGACY.get_or_init(|| {
            let mut groups = Groups::new();
            for c in 0..=0xffff {
                let key = canonical(c, false);
                if key != c {
                    groups.entry(key).or_insert_with(|| vec![key]).push(c);
                }
            }
            groups
        })
    };
    let key = canonical(c, unicode);
    (low..=high).contains(&key)
        || groups
            .get(&key)
            .is_some_and(|g| g.iter().any(|c| (low..=high).contains(c)))
}
