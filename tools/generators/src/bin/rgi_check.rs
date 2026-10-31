//! Validate the candidate RGI emoji sets against the pinned test262 match and
//! non-match lists. Exits nonzero with a summary when a candidate mismatches.
//!
//! Usage: `rgi-check <emoji-test.txt> <emoji-data.txt> <test262-strings-dir>`.
//! The strings directory is `test/built-ins/RegExp/property-escapes/generated/strings`
//! of the pinned corpus; `emoji-data.txt` is Unicode 17.0's `emoji/emoji-data.txt`.

use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

type Units = Vec<u16>;
type Sequences = BTreeSet<Vec<u32>>;

fn units(code_points: &[u32]) -> Units {
    let mut out = Vec::new();
    for &cp in code_points {
        if cp > 0xFFFF {
            let cp = cp - 0x10000;
            out.push((0xD800 + (cp >> 10)) as u16);
            out.push((0xDC00 + (cp & 0x3FF)) as u16);
        } else {
            out.push(cp as u16);
        }
    }
    out
}

/// Decode one JavaScript string literal body into code points (lone surrogates allowed).
fn js_decode(text: &str) -> Vec<u32> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' {
            out.push(chars[i] as u32);
            i += 1;
            continue;
        }
        let hex = |from: usize, to: usize| -> u32 {
            let digits: String = chars[from..to].iter().collect();
            u32::from_str_radix(&digits, 16).expect("hex escape in corpus")
        };
        match chars[i + 1] {
            'u' if chars[i + 2] == '{' => {
                let close = (i..chars.len())
                    .find(|&j| chars[j] == '}')
                    .expect("closing brace");
                out.push(hex(i + 3, close));
                i = close + 1;
            }
            'u' => {
                out.push(hex(i + 2, i + 6));
                i += 6;
            }
            'n' => {
                out.push('\n' as u32);
                i += 2;
            }
            'x' => {
                out.push(hex(i + 2, i + 4));
                i += 4;
            }
            other => {
                out.push(other as u32);
                i += 2;
            }
        }
    }
    out
}

/// Quoted string bodies of a JavaScript array literal.
fn strings_in(blob: &str) -> BTreeSet<Units> {
    let quoted = Regex::new(r#""((?:[^"\\]|\\.)*)""#).expect("string pattern");
    quoted
        .captures_iter(blob)
        .map(|capture| units(&js_decode(&capture[1])))
        .collect()
}

/// The match and non-match sets of one corpus file.
fn load_case(strings: &Path, name: &str) -> Result<(BTreeSet<Units>, BTreeSet<Units>), String> {
    let path = strings.join(format!("{name}.js"));
    let source =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let matches = Regex::new(r"(?s)matchStrings: \[(.*?)\],").expect("match pattern");
    let non_matches = Regex::new(r"(?s)nonMatchStrings: \[(.*?)\]").expect("non-match pattern");
    let matched = &matches
        .captures(&source)
        .ok_or(format!("{name}: no matchStrings"))?[1];
    let unmatched = &non_matches
        .captures(&source)
        .ok_or(format!("{name}: no nonMatchStrings"))?[1];
    Ok((strings_in(matched), strings_in(unmatched)))
}

fn parse_ranges(path: &Path, property: &str) -> Result<BTreeSet<u32>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let range = Regex::new(r"^([0-9A-F]+)(?:\.\.([0-9A-F]+))?\s*;\s*(\w+)").expect("range pattern");
    let mut points = BTreeSet::new();
    for line in text.lines() {
        let Some(found) = range.captures(line) else {
            continue;
        };
        if &found[3] != property {
            continue;
        }
        let low = u32::from_str_radix(&found[1], 16).unwrap_or(0);
        let high = found
            .get(2)
            .map_or(low, |m| u32::from_str_radix(m.as_str(), 16).unwrap_or(0));
        points.extend(low..=high);
    }
    Ok(points)
}

/// Fully-qualified and component sequences (`singles`, `sequences`) up to `max_version`.
fn fully_qualified(path: &Path, max_version: &[u32]) -> Result<(Sequences, Sequences), String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let entry =
        Regex::new(r"^([0-9A-F ]+)\s*;\s*(fully-qualified|component)\s+#\s+\S+\s+E([0-9.]+)")
            .expect("entry pattern");
    let (mut singles, mut sequences) = (BTreeSet::new(), BTreeSet::new());
    for line in text.lines() {
        let Some(found) = entry.captures(line) else {
            continue;
        };
        let version: Vec<u32> = found[3]
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect();
        if version.as_slice() > max_version {
            continue;
        }
        let cps: Vec<u32> = found[1]
            .split_whitespace()
            .map(|cp| u32::from_str_radix(cp, 16).unwrap_or(0))
            .collect();
        if cps.len() == 1 {
            singles.insert(cps);
        } else {
            sequences.insert(cps);
        }
    }
    Ok((singles, sequences))
}

fn hex_units(sequence: &[u16]) -> String {
    sequence
        .iter()
        .map(|u| format!("{u:04X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn check(strings: &Path, name: &str, candidate: &BTreeSet<Units>) -> Result<bool, String> {
    let (matched, non) = load_case(strings, name)?;
    let missing: Vec<&Units> = matched.difference(candidate).collect();
    let extra: Vec<&Units> = candidate.intersection(&non).collect();
    let unlisted = candidate
        .iter()
        .filter(|s| !matched.contains(*s) && !non.contains(*s))
        .count();
    println!(
        "{name}: match={} candidate={} missing={} bad_extra={} unlisted={unlisted}",
        matched.len(),
        candidate.len(),
        missing.len(),
        extra.len()
    );
    for sequence in missing.iter().take(5) {
        println!("  MISSING: {}", hex_units(sequence));
    }
    for sequence in extra.iter().take(5) {
        println!("  BAD EXTRA: {}", hex_units(sequence));
    }
    Ok(missing.is_empty() && extra.is_empty())
}

fn run(emoji_test: &Path, emoji_data: &Path, strings: &Path) -> Result<bool, String> {
    const MAX: &[u32] = &[17, 0];
    let mut ok = true;
    let keycap: BTreeSet<Units> = "0123456789#*"
        .chars()
        .map(|base| units(&[base as u32, 0xFE0F, 0x20E3]))
        .collect();
    ok &= check(strings, "Emoji_Keycap_Sequence", &keycap)?;

    let (_, sequences) = fully_qualified(emoji_test, MAX)?;
    let zwj: BTreeSet<Units> = sequences
        .iter()
        .filter(|s| s.contains(&0x200D))
        .map(|s| units(s))
        .collect();
    let flag: BTreeSet<Units> = sequences
        .iter()
        .filter(|s| s.len() == 2 && s.iter().all(|cp| (0x1F1E6..=0x1F1FF).contains(cp)))
        .map(|s| units(s))
        .collect();
    let tag: BTreeSet<Units> = sequences
        .iter()
        .filter(|s| s.last() == Some(&0xE007F))
        .map(|s| units(s))
        .collect();
    let modifier: BTreeSet<Units> = sequences
        .iter()
        .filter(|s| s.len() == 2 && (0x1F3FB..=0x1F3FF).contains(&s[1]) && !s.contains(&0x200D))
        .map(|s| units(s))
        .collect();
    ok &= check(strings, "RGI_Emoji_ZWJ_Sequence", &zwj)?;
    ok &= check(strings, "RGI_Emoji_Flag_Sequence", &flag)?;
    ok &= check(strings, "RGI_Emoji_Tag_Sequence", &tag)?;
    ok &= check(strings, "RGI_Emoji_Modifier_Sequence", &modifier)?;

    let emoji = parse_ranges(emoji_data, "Emoji")?;
    let component = parse_ranges(emoji_data, "Emoji_Component")?;
    let modifier_property = parse_ranges(emoji_data, "Emoji_Modifier")?;
    let presentation = parse_ranges(emoji_data, "Emoji_Presentation")?;
    let mut basic = BTreeSet::new();
    for cp in emoji
        .difference(&component)
        .filter(|cp| !modifier_property.contains(cp))
    {
        if presentation.contains(cp) {
            basic.insert(units(&[*cp]));
        } else {
            let mut sequence = units(&[*cp]);
            sequence.push(0xFE0F);
            basic.insert(sequence);
        }
    }
    ok &= check(strings, "Basic_Emoji", &basic)?;

    let (singles, _) = fully_qualified(emoji_test, MAX)?;
    let mut rgi: BTreeSet<Units> = BTreeSet::new();
    rgi.extend(flag);
    rgi.extend(tag);
    rgi.extend(modifier);
    rgi.extend(keycap);
    rgi.extend(zwj);
    rgi.extend(singles.iter().map(|s| units(s)));
    ok &= check(strings, "RGI_Emoji", &rgi)?;
    Ok(ok)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [emoji_test, emoji_data, strings] = arguments.as_slice() else {
        eprintln!("usage: rgi-check <emoji-test.txt> <emoji-data.txt> <test262-strings-dir>");
        return ExitCode::from(2);
    };
    match run(
        &PathBuf::from(emoji_test),
        &PathBuf::from(emoji_data),
        &PathBuf::from(strings),
    ) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("rgi-check: {message}");
            ExitCode::FAILURE
        }
    }
}
