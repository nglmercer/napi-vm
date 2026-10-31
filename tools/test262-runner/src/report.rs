//! Report documents: writing, reading (plain or gzip), comparing, and summarizing.

use crate::options::Options;
use serde_json::{Map, Value as Json, json};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

pub const STATUSES: [&str; 6] = ["pass", "fail", "skip", "timeout", "crash", "harness_error"];

const DENOMINATOR: &str = "all selected variants, including skips and errors";
const LIMITATION: &str =
    "Host capabilities depend on the selected worker; GC requests run at quiescent host boundaries";

/// Run-level facts recorded beside the outcomes.
pub struct Header {
    pub revision: String,
    pub selection: Vec<String>,
    pub groups: Vec<String>,
    pub engine_sha256: String,
    pub skip_features: Vec<String>,
    pub worker_jobs: usize,
    pub timeout_seconds: f64,
    pub resumed_files: usize,
}

pub fn counts(results: &[Json]) -> Map<String, Json> {
    let mut counts = Map::new();
    for status in STATUSES {
        let count = results.iter().filter(|row| row["status"] == status).count();
        counts.insert(status.into(), json!(count));
    }
    counts
}

pub fn describe(counts: &Map<String, Json>) -> String {
    STATUSES
        .iter()
        .map(|status| format!("{status} {}", counts[*status]))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The report document. Key order matches the reports already archived.
pub fn document(header: &Header, results: Vec<Json>) -> Json {
    let counts = counts(&results);
    let total = results.len();
    let passed = counts["pass"].as_u64().unwrap_or(0) as f64;
    let percentage = if total == 0 {
        Json::Null
    } else {
        json!(100.0 * passed / total as f64)
    };
    json!({
        "domain": "ECMAScript",
        "suite": "Test262",
        "revision": header.revision,
        "selection": header.selection,
        "groups": header.groups,
        "engine_sha256": header.engine_sha256,
        "skip_features": header.skip_features,
        "denominator": DENOMINATOR,
        "worker_jobs": header.worker_jobs,
        "timeout_seconds": header.timeout_seconds,
        "resumed_files": header.resumed_files,
        "total": total,
        "counts": counts,
        "pass_percentage": percentage,
        "limitations": [LIMITATION],
        "results": results,
    })
}

pub fn write(path: &Path, document: &Json) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(document).map_err(|error| error.to_string())?;
    text.push('\n');
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Read a report, decompressing `.gz` files.
pub fn read(path: &str) -> Result<Json, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    let json = if path.ends_with(".gz") {
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .read_to_end(&mut decoded)
            .map_err(|error| format!("{path}: {error}"))?;
        decoded
    } else {
        bytes
    };
    serde_json::from_slice(&json).map_err(|error| format!("{path}: {error}"))
}

fn indexed(report: &Json) -> Result<BTreeMap<(String, String), Json>, String> {
    let mut rows = BTreeMap::new();
    for row in report["results"]
        .as_array()
        .ok_or("report has no results")?
    {
        let key = (
            row["test"].as_str().unwrap_or_default().to_string(),
            row["variant"].as_str().unwrap_or_default().to_string(),
        );
        if rows.insert(key.clone(), row.clone()).is_some() {
            return Err(format!("duplicate outcome: {} [{}]", key.0, key.1));
        }
    }
    if report["total"].as_u64() != Some(rows.len() as u64) {
        return Err("outcome count does not match report total".into());
    }
    Ok(rows)
}

fn detail(row: &Json) -> String {
    let status = row["status"].as_str().unwrap_or("?");
    let message = row["engine"]["message"]
        .as_str()
        .or(row["message"].as_str())
        .unwrap_or("");
    if message.is_empty() {
        return status.to_string();
    }
    let message: String = message.chars().take(140).collect();
    format!("{status}: {message}")
}

/// `compare BASELINE CURRENT [--subset] [--output FILE] [--allow-config-change]`.
/// With `--subset`, only variants present in CURRENT are compared; this is how
/// a `retest` report is checked against the full report it came from.
pub fn compare(options: &Options) -> Result<ExitCode, String> {
    let paths = options.require_positional(2, "BASELINE and CURRENT")?;
    let baseline = read(&paths[0])?;
    let current = read(&paths[1])?;
    let subset = options.flag("subset");
    if !options.flag("allow-config-change") {
        for field in ["revision", "worker_jobs", "timeout_seconds"] {
            if baseline[field] != current[field] {
                return Err(format!(
                    "incompatible {field}: {} vs {} (pass --allow-config-change)",
                    baseline[field], current[field]
                ));
            }
        }
    }
    let before = indexed(&baseline)?;
    let after = indexed(&current)?;
    let mut gains = Vec::new();
    let mut losses = Vec::new();
    let mut changed = Vec::new();
    let mut added = Vec::new();
    for (key, new) in &after {
        let Some(old) = before.get(key) else {
            added.push(json!({"test": key.0, "variant": key.1, "after": new["status"]}));
            continue;
        };
        if old["status"] == new["status"] {
            continue;
        }
        let entry = json!({"test": key.0, "variant": key.1,
            "before": old["status"], "after": new["status"]});
        if new["status"] == "pass" {
            gains.push(entry);
        } else if old["status"] == "pass" {
            println!("LOSS {} [{}]: {}", key.0, key.1, detail(new));
            losses.push(entry);
        } else {
            changed.push(entry);
        }
    }
    let mut removed = Vec::new();
    if !subset {
        for key in before.keys().filter(|key| !after.contains_key(*key)) {
            removed.push(json!({"test": key.0, "variant": key.1}));
        }
    }
    for entry in &gains {
        println!(
            "GAIN {} [{}] (was {})",
            entry["test"].as_str().unwrap_or(""),
            entry["variant"].as_str().unwrap_or(""),
            entry["before"]
        );
    }
    let not_compared = before.len() - before.keys().filter(|key| after.contains_key(*key)).count();
    println!(
        "baseline {}/{} -> current {}/{}: {} gains, {} losses, {} other changes, {} added, {} removed, {} not in current",
        baseline["counts"]["pass"],
        baseline["total"],
        current["counts"]["pass"],
        current["total"],
        gains.len(),
        losses.len(),
        changed.len(),
        added.len(),
        removed.len(),
        not_compared,
    );
    if let Some(output) = options.one("output") {
        let comparison = json!({"gains": gains, "losses": losses, "changed": changed,
            "added": added, "removed": removed});
        write(Path::new(output), &comparison)?;
    }
    // Only lost passes fail the comparison; additions and removals are reported.
    Ok(if losses.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Test area used for grouping: `language/<a>/<b>` or `<a>/<b>`.
fn area(test: &str) -> String {
    let parts: Vec<&str> = test.split('/').collect();
    let depth = if parts[0] == "language" { 3 } else { 2 };
    parts[..parts.len().min(depth)].join("/")
}

/// `summary REPORT [--by area] [--json FILE] [--limit N] [--slowest N]`.
/// Every outcome stays in its group and in the denominator; nothing is filtered.
pub fn summary(options: &Options) -> Result<ExitCode, String> {
    let path = &options.require_positional(1, "one REPORT")?[0];
    let report = read(path)?;
    let results = report["results"]
        .as_array()
        .ok_or("report has no results")?;
    let counts = counts(results);
    println!(
        "{}/{} passed ({}), revision {}, engine {}",
        counts["pass"],
        results.len(),
        describe(&counts),
        report["revision"].as_str().unwrap_or("?"),
        report["engine_sha256"].as_str().unwrap_or("?")
    );

    let mut groups: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut phases: BTreeMap<String, usize> = BTreeMap::new();
    for row in results {
        let test = row["test"].as_str().unwrap_or_default();
        let status = row["status"].as_str().unwrap_or("?").to_string();
        *groups
            .entry(area(test))
            .or_default()
            .entry(status.clone())
            .or_default() += 1;
        if status != "pass" {
            let phase = row["engine"]["phase"]
                .as_str()
                .unwrap_or(&status)
                .to_string();
            *phases.entry(phase).or_default() += 1;
        }
    }
    let non_passing = |counts: &BTreeMap<String, usize>| -> usize {
        counts
            .iter()
            .filter(|(status, _)| *status != "pass")
            .map(|(_, count)| count)
            .sum()
    };
    let mut ordered: Vec<(&String, &BTreeMap<String, usize>)> = groups.iter().collect();
    ordered.sort_by(|a, b| non_passing(b.1).cmp(&non_passing(a.1)).then(a.0.cmp(b.0)));
    let limit: usize = options.number("limit", 20)?;
    if ordered
        .first()
        .is_some_and(|(_, counts)| non_passing(counts) > 0)
    {
        println!("non-passing by area (most first):");
        for (name, counts) in ordered
            .iter()
            .filter(|(_, c)| non_passing(c) > 0)
            .take(limit)
        {
            let total: usize = counts.values().sum();
            println!("  {:6} / {:6}  {name}", non_passing(counts), total);
        }
    }

    let listed: Vec<&Json> = results
        .iter()
        .filter(|row| row["status"] != "pass")
        .collect();
    let shown: usize = options.number("show", 50)?;
    for row in listed.iter().take(shown) {
        println!(
            "  {} [{}] {}",
            row["test"].as_str().unwrap_or(""),
            row["variant"].as_str().unwrap_or(""),
            detail(row)
        );
    }
    if listed.len() > shown {
        println!("  ... {} more (use --show N)", listed.len() - shown);
    }

    let slowest: usize = options.number("slowest", 0)?;
    if slowest > 0 {
        let mut timed: Vec<&Json> = results
            .iter()
            .filter(|row| row["seconds"].is_number())
            .collect();
        timed.sort_by(|a, b| {
            b["seconds"]
                .as_f64()
                .partial_cmp(&a["seconds"].as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        println!("slowest variants:");
        for row in timed.into_iter().take(slowest) {
            println!(
                "  {:>7.2}s {} {} [{}]",
                row["seconds"].as_f64().unwrap_or(0.0),
                row["status"].as_str().unwrap_or(""),
                row["test"].as_str().unwrap_or(""),
                row["variant"].as_str().unwrap_or("")
            );
        }
    }

    if let Some(output) = options.one("json") {
        let by_area: Vec<Json> = ordered
            .iter()
            .map(|(name, counts)| {
                json!({"path": name, "total": counts.values().sum::<usize>(),
                    "non_passing": non_passing(counts), "counts": counts})
            })
            .collect();
        let document = json!({
            "revision": report["revision"], "engine_sha256": report["engine_sha256"],
            "total": results.len(), "counts": Json::Object(counts.clone()),
            "pass_percentage": report["pass_percentage"],
            "non_passing_phases": phases, "groups": by_area,
        });
        write(Path::new(output), &document)?;
    }
    Ok(ExitCode::SUCCESS)
}
