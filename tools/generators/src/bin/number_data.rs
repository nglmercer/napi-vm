//! Extract runtime number and unit patterns from pinned Unicode CLDR npm archives.
//!
//! Usage: `number-data <archive-dir> <output-dir>`. The directory must contain
//! `numbers-48.tgz`, `units-48.tgz` and `core-48.tgz`, the cldr-numbers-full,
//! cldr-units-full and cldr-core 48.0.0 packages, unmodified. No Test262 data
//! is consulted. Each locale record is compressed separately, so a worker only
//! decodes the locales it formats.

use napi_vm_generators::{
    Member, gzip_compress, json_string, json_string_utf8, read_tgz, sha256_hex, write_output,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const UNITS: &[&str] = &[
    "acre",
    "bit",
    "byte",
    "celsius",
    "centimeter",
    "day",
    "degree",
    "fahrenheit",
    "fluid-ounce",
    "foot",
    "gallon",
    "gigabit",
    "gigabyte",
    "gram",
    "hectare",
    "hour",
    "inch",
    "kilobit",
    "kilobyte",
    "kilogram",
    "kilometer",
    "liter",
    "megabit",
    "megabyte",
    "meter",
    "microsecond",
    "mile",
    "mile-scandinavian",
    "milliliter",
    "millimeter",
    "millisecond",
    "minute",
    "month",
    "nanosecond",
    "ounce",
    "percent",
    "petabyte",
    "pound",
    "second",
    "stone",
    "terabit",
    "terabyte",
    "week",
    "yard",
    "year",
];

fn member<'a>(members: &'a [Member], name: &str) -> Result<&'a Member, String> {
    members
        .iter()
        .find(|member| member.name == name)
        .ok_or_else(|| format!("archive has no {name}"))
}

fn json_of(member: &Member) -> Result<Value, String> {
    serde_json::from_slice(&member.data).map_err(|error| format!("{}: {error}", member.name))
}

/// The per-locale records, keyed by locale, merged from both archives.
fn records(
    numbers: &[Member],
    units: &[Member],
) -> Result<BTreeMap<String, Map<String, Value>>, String> {
    let mut records: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for archive in [numbers, units] {
        for entry in archive {
            let pieces: Vec<&str> = entry.name.split('/').collect();
            if pieces.len() != 4 || pieces[1] != "main" {
                continue;
            }
            let (locale, filename) = (pieces[2], pieces[3]);
            if !["numbers.json", "currencies.json", "units.json"].contains(&filename) {
                continue;
            }
            let document = json_of(entry)?;
            let data = &document["main"][locale];
            let result = records.entry(locale.to_string()).or_default();
            match filename {
                "numbers.json" => {
                    result.insert("numbers".into(), data["numbers"].clone());
                }
                "currencies.json" => {
                    result.insert("currencies".into(), data["numbers"]["currencies"].clone());
                }
                _ => {
                    let mut by_width = Map::new();
                    for width in ["long", "short", "narrow"] {
                        let mut selected = Map::new();
                        if let Some(items) = data["units"][width].as_object() {
                            for (key, value) in items {
                                let name =
                                    key.split_once('-').map_or(key.as_str(), |(_, rest)| rest);
                                let compound: Vec<&str> = name.split("-per-").collect();
                                let plain = compound.len() <= 2
                                    && compound.iter().all(|unit| UNITS.contains(unit));
                                if key == "per" || plain {
                                    let stored = if key == "per" { key.as_str() } else { name };
                                    selected.insert(stored.to_string(), value.clone());
                                }
                            }
                        }
                        by_width.insert(width.into(), Value::Object(selected));
                    }
                    result.insert("units".into(), Value::Object(by_width));
                }
            }
        }
    }
    Ok(records)
}

fn run(source: &Path, output: &Path) -> Result<(), String> {
    let archive = |name: &str| -> Result<Vec<u8>, String> {
        let path = source.join(name);
        std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))
    };
    let numbers_bytes = archive("numbers-48.tgz")?;
    let units_bytes = archive("units-48.tgz")?;
    let core_bytes = archive("core-48.tgz")?;
    let numbers = read_tgz(&source.join("numbers-48.tgz"))?;
    let units = read_tgz(&source.join("units-48.tgz"))?;
    let core = read_tgz(&source.join("core-48.tgz"))?;

    let records = records(&numbers, &units)?;
    let mut blob = Vec::new();
    let mut seen: BTreeMap<Vec<u8>, (usize, usize)> = BTreeMap::new();
    let mut index = Vec::new();
    for (locale, data) in &records {
        let raw = serde_json::to_string(data)
            .map_err(|error| error.to_string())?
            .into_bytes();
        let (start, end) = match seen.get(&raw) {
            Some(range) => *range,
            None => {
                let compressed = gzip_compress(&raw);
                let range = (blob.len(), blob.len() + compressed.len());
                blob.extend_from_slice(&compressed);
                seen.insert(raw, range);
                range
            }
        };
        index.push(format!("    ({}, {start}, {end}),", json_string(locale)));
    }

    let systems = json_of(member(&core, "package/supplemental/numberingSystems.json")?)?;
    let systems = systems["supplemental"]["numberingSystems"]
        .as_object()
        .ok_or("core archive has no numbering systems")?;
    let mut digits = Vec::new();
    for (key, value) in systems.iter().collect::<BTreeMap<_, _>>() {
        if value["_type"] == "numeric" {
            let digits_text = value["_digits"]
                .as_str()
                .ok_or("numeric system without digits")?;
            digits.push(format!(
                "    ({},{}),",
                json_string(key),
                json_string_utf8(digits_text)
            ));
        }
    }

    let unit_list: Vec<String> = UNITS.iter().map(|unit| json_string(unit)).collect();
    let index_text = format!(
        "// Generated from Unicode CLDR 48.0.0 by tools/generators (number-data).\n\
         pub(super) const INDEX: &[(&str, usize, usize)] = &[\n{}\n];\n\
         pub(super) const UNITS: &[&str] = &[{}];\n\
         pub(super) const NUMBERING_SYSTEMS: &[(&str, &str)] = &[\n{}\n];\n",
        index.join("\n"),
        unit_list.join(","),
        digits.join("\n")
    );
    let license = member(&numbers, "package/LICENSE")?;
    let sources = format!(
        "{{\n  \"cldr_version\": \"48.0.0\",\n  \"archive_sha256\": {{\n    \"numbers\": \"{}\",\n    \"units\": \"{}\",\n    \"core\": \"{}\"\n  }},\n  \"locales\": {},\n  \"unique_records\": {},\n  \"blob_sha256\": \"{}\"\n}}\n",
        sha256_hex(&numbers_bytes),
        sha256_hex(&units_bytes),
        sha256_hex(&core_bytes),
        records.len(),
        seen.len(),
        sha256_hex(&blob)
    );

    write_output(output, "number-patterns.bin", "")?;
    std::fs::write(output.join("number-patterns.bin"), &blob).map_err(|error| error.to_string())?;
    write_output(output, "number-pattern-index.rs", &index_text)?;
    std::fs::write(output.join("LICENSE-CLDR.txt"), &license.data)
        .map_err(|error| error.to_string())?;
    write_output(output, "number-pattern-sources.json", &sources)?;
    println!(
        "{} locales; {} records; {} bytes",
        records.len(),
        seen.len(),
        blob.len()
    );
    Ok(())
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [source, output] = arguments.as_slice() else {
        eprintln!("usage: number-data <archive-dir> <output-dir>");
        return ExitCode::from(2);
    };
    match run(&PathBuf::from(source), &PathBuf::from(output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("number-data: {message}");
            ExitCode::FAILURE
        }
    }
}
