//! Generate pinned RGI emoji tables for the regex engine from `emoji-test.txt`.
//!
//! Usage: `emoji-tables <emoji-test.txt> <output.rs>`. Only entries with an
//! Emoji version of at most 17.0 are kept, matching the "Unicode v17.0.0" corpus
//! metadata of the generated test262 property-escape tests. Tables are checked
//! against the pinned counts below; `rgi-check` validates them against test262.

use napi_vm_generators::write_output;
use regex::Regex;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;

const HEADER: &str = "//! Generated RGI emoji tables. DO NOT EDIT.
//!
//! Produced by `cargo run -p napi-vm-generators --bin emoji-tables` from
//! emoji-test.txt, keeping entries up to Emoji 17.0. The `rgi-check` tool
//! reads these sets against the pinned test262 lists; tools/generators/README.md
//! records its current mismatches.

#![allow(clippy::all)]
";

const MAX_VERSION: &[u32] = &[17, 0];

fn expected(name: &str) -> usize {
    match name {
        "RGI_EMOJI_ZWJ_SEQUENCE" => 1614,
        "RGI_EMOJI_FLAG_SEQUENCE" => 259,
        "RGI_EMOJI_TAG_SEQUENCE" => 3,
        "RGI_EMOJI_MODIFIER_SEQUENCE" => 665,
        "RGI_EMOJI_SINGLES" => 1400 - 207,
        "BASIC_EMOJI_PAIRS" => 207,
        _ => unreachable!("unknown table {name}"),
    }
}

/// UTF-16 code units of a code point sequence.
pub fn units(code_points: &[u32]) -> Vec<u16> {
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

type Sequences = BTreeSet<Vec<u32>>;

/// Fully-qualified and component sequences (`singles`, `sequences`) up to `max_version`.
pub fn load(path: &PathBuf) -> Result<(Sequences, Sequences), String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let pattern =
        Regex::new(r"^([0-9A-F ]+)\s*;\s*(fully-qualified|component)\s+#\s+\S+\s+E([0-9.]+)")
            .map_err(|error| error.to_string())?;
    let (mut singles, mut sequences) = (BTreeSet::new(), BTreeSet::new());
    for line in text.lines() {
        let Some(found) = pattern.captures(line) else {
            continue;
        };
        let version: Vec<u32> = found[3]
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect();
        if version.as_slice() > MAX_VERSION {
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

fn emit_table(name: &str, sequences: &Sequences) -> Result<String, String> {
    let mut ordered: Vec<Vec<u16>> = sequences.iter().map(|seq| units(seq)).collect();
    ordered.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    if ordered.len() != expected(name) {
        return Err(format!(
            "{name}: {} entries, expected {}",
            ordered.len(),
            expected(name)
        ));
    }
    let mut lines = vec![format!("pub const {name}: &[&[u16]] = &[")];
    for seq in &ordered {
        let encoded: Vec<String> = seq.iter().map(|u| format!("0x{u:04X}")).collect();
        lines.push(format!("    &[{}],", encoded.join(", ")));
    }
    lines.push("];\n".into());
    Ok(lines.join("\n"))
}

fn emit_scalars(name: &str, points: &BTreeSet<u32>) -> Result<String, String> {
    if points.len() != expected(name) {
        return Err(format!(
            "{name}: {} entries, expected {}",
            points.len(),
            expected(name)
        ));
    }
    let mut lines = vec![format!("pub const {name}: &[u32] = &[")];
    let mut line = String::new();
    for cp in points {
        let chunk = format!("0x{cp:04X}, ");
        if line.len() + chunk.len() > 96 {
            lines.push(format!("    {line}"));
            line = chunk;
        } else {
            line.push_str(&chunk);
        }
    }
    lines.push(format!("    {line}"));
    lines.push("];\n".into());
    Ok(lines.join("\n"))
}

fn run(input: &PathBuf, output: &PathBuf) -> Result<(), String> {
    let (singles, seqs) = load(input)?;
    let mut pieces = vec![HEADER.to_string()];
    let zwj: Sequences = seqs
        .iter()
        .filter(|s| s.contains(&0x200D))
        .cloned()
        .collect();
    pieces.push(emit_table("RGI_EMOJI_ZWJ_SEQUENCE", &zwj)?);
    let flag: Sequences = seqs
        .iter()
        .filter(|s| s.len() == 2 && s.iter().all(|cp| (0x1F1E6..=0x1F1FF).contains(cp)))
        .cloned()
        .collect();
    pieces.push(emit_table("RGI_EMOJI_FLAG_SEQUENCE", &flag)?);
    let tag: Sequences = seqs
        .iter()
        .filter(|s| s.last() == Some(&0xE007F))
        .cloned()
        .collect();
    pieces.push(emit_table("RGI_EMOJI_TAG_SEQUENCE", &tag)?);
    let modifier: Sequences = seqs
        .iter()
        .filter(|s| s.len() == 2 && (0x1F3FB..=0x1F3FF).contains(&s[1]) && !s.contains(&0x200D))
        .cloned()
        .collect();
    pieces.push(emit_table("RGI_EMOJI_MODIFIER_SEQUENCE", &modifier)?);
    let pairs: Sequences = seqs
        .iter()
        .filter(|s| s.len() == 2 && s[1] == 0xFE0F)
        .cloned()
        .collect();
    pieces.push(emit_table("BASIC_EMOJI_PAIRS", &pairs)?);
    let singles: BTreeSet<u32> = singles.iter().map(|cps| cps[0]).collect();
    pieces.push(emit_scalars("RGI_EMOJI_SINGLES", &singles)?);
    let text = pieces.join("\n") + "\n";
    let directory = output.parent().map(PathBuf::from).unwrap_or_default();
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("bad output path")?;
    write_output(&directory, name, &text)?;
    // The committed tables are rustfmt output; format the file the same way.
    let status = std::process::Command::new("rustfmt")
        .args(["--edition", "2024"])
        .arg(output)
        .status()
        .map_err(|error| format!("rustfmt: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("rustfmt failed".into())
    }
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = arguments.as_slice() else {
        eprintln!("usage: emoji-tables <emoji-test.txt> <output.rs>");
        return ExitCode::from(2);
    };
    match run(&PathBuf::from(input), &PathBuf::from(output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("emoji-tables: {message}");
            ExitCode::FAILURE
        }
    }
}
