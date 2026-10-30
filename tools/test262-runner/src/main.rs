//! Test262 validation for the isolated `napi-vm-test262` worker.
//!
//! Run from the repository root:
//!
//! ```text
//! cargo run -p napi-vm-test262-runner --release -- <command> [options]
//! ```
//!
//! Commands: `run`, `time`, `compare`, `summary`, `request`, `bench`.
//! See `tools/test262/README.md` for the full reference.

mod metadata;
mod worker;

use metadata::Variant;
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use worker::Corpus;

const STATUSES: [&str; 6] = ["pass", "fail", "skip", "timeout", "crash", "harness_error"];
const USAGE: &str = "\
usage: test262 <command> [options]

commands:
  run [--path P]... [--group G]... [--skip-feature F]... [--jobs N] [--timeout S]
      [--output FILE] [--checkpoint FILE [--resume]]
        Run selected variants (default: the whole corpus) and write a report.
  time PATH... [--timeout S] [--variant script|strict|module]
        Time each variant of the given tests or directories, uncapped by default.
  compare BASELINE CURRENT [--output FILE] [--allow-config-change]
        Gains, losses, and status changes between two reports (.json or .json.gz).
        Exits 1 when any variant lost its pass.
  summary REPORT [--slowest N]
        Counts, every non-passing variant, and the slowest variants.
  request TEST [--variant script|strict|module] [--output FILE]
        Write the exact worker request for one variant (for profiling).
  bench FILE... [--harness] [--repeat N] [--timeout S]
        Time JavaScript snippet files through the worker.

common options:
  --corpus DIR      Test262 checkout (default .napi-vm/test262)
  --revision SHA    required corpus commit (default tools/test262/corpus.lock.json)
  --engine FILE     worker binary (default target/release/napi-vm-test262)
";

struct Options {
    positional: Vec<String>,
    values: HashMap<String, Vec<String>>,
    flags: Vec<String>,
}

impl Options {
    fn parse(
        arguments: &[String],
        value_options: &[&str],
        flag_options: &[&str],
    ) -> Result<Self, String> {
        let mut options = Options {
            positional: Vec::new(),
            values: HashMap::new(),
            flags: Vec::new(),
        };
        let mut iter = arguments.iter();
        while let Some(argument) = iter.next() {
            if let Some(name) = argument.strip_prefix("--") {
                let (name, inline) = match name.split_once('=') {
                    Some((name, value)) => (name, Some(value.to_string())),
                    None => (name, None),
                };
                if flag_options.contains(&name) {
                    options.flags.push(name.to_string());
                } else if value_options.contains(&name) {
                    let value = match inline {
                        Some(value) => value,
                        None => iter
                            .next()
                            .cloned()
                            .ok_or_else(|| format!("--{name} requires a value"))?,
                    };
                    options
                        .values
                        .entry(name.to_string())
                        .or_default()
                        .push(value);
                } else {
                    return Err(format!("unknown option --{name}"));
                }
            } else {
                options.positional.push(argument.clone());
            }
        }
        Ok(options)
    }

    fn one(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)
            .and_then(|values| values.last())
            .map(String::as_str)
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.values.get(name).cloned().unwrap_or_default()
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }

    fn number<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        match self.one(name) {
            Some(value) => value
                .parse()
                .map_err(|_| format!("--{name} must be a number, got {value}")),
            None => Ok(default),
        }
    }

    fn seconds(&self, name: &str, default: f64) -> Result<Duration, String> {
        let seconds: f64 = self.number(name, default)?;
        if !(seconds > 0.0 && seconds.is_finite()) {
            return Err(format!("--{name} must be positive"));
        }
        Ok(Duration::from_secs_f64(seconds))
    }
}

const COMMON: [&str; 3] = ["corpus", "revision", "engine"];

fn corpus(options: &Options, check_revision: bool) -> Result<(Corpus, String), String> {
    let root = PathBuf::from(options.one("corpus").unwrap_or(".napi-vm/test262"))
        .canonicalize()
        .map_err(|error| format!("corpus: {error}"))?;
    let revision = git(&root, &["rev-parse", "HEAD"])?;
    if check_revision {
        let expected = match options.one("revision") {
            Some(revision) => revision.to_string(),
            None => {
                let lock = std::fs::read_to_string("tools/test262/corpus.lock.json")
                    .map_err(|error| format!("corpus.lock.json (pass --revision): {error}"))?;
                let lock: Json = serde_json::from_str(&lock).map_err(|error| error.to_string())?;
                lock["revision"]
                    .as_str()
                    .ok_or("corpus.lock.json has no revision")?
                    .to_string()
            }
        };
        if revision != expected {
            return Err(format!(
                "corpus revision mismatch: {revision}, expected {expected}"
            ));
        }
        if !git(&root, &["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
            return Err("corpus has modified or untracked files".into());
        }
    }
    Ok((Corpus { root }, revision))
}

fn git(root: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {}: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Copy the worker so a concurrent `cargo build` cannot change it mid-run.
struct EngineSnapshot {
    path: PathBuf,
    sha256: String,
}

impl EngineSnapshot {
    fn take(options: &Options) -> Result<Self, String> {
        let source = PathBuf::from(
            options
                .one("engine")
                .unwrap_or("target/release/napi-vm-test262"),
        );
        let bytes = std::fs::read(&source)
            .map_err(|error| format!("engine {}: {error}", source.display()))?;
        let sha256 = hex(&Sha256::digest(&bytes));
        let directory =
            std::env::temp_dir().join(format!("napi-vm-test262-{}", std::process::id()));
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let path = directory.join("worker");
        std::fs::copy(&source, &path).map_err(|error| format!("snapshot engine: {error}"))?;
        Ok(Self { path, sha256 })
    }
}

impl Drop for EngineSnapshot {
    fn drop(&mut self) {
        if let Some(directory) = self.path.parent() {
            let _ = std::fs::remove_dir_all(directory);
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn collect_js(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    let entries =
        std::fs::read_dir(path).map_err(|error| format!("{}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let child = entry.path();
        if child.is_dir() {
            collect_js(&child, out)?;
        } else if child.extension().is_some_and(|extension| extension == "js") {
            out.push(child);
        }
    }
    Ok(())
}

/// Union of explicit paths and named groups; no selection means everything.
fn select(corpus: &Corpus, paths: &[String], groups: &[String]) -> Result<Vec<PathBuf>, String> {
    let test_dir = corpus.test_dir();
    let mut selections: Vec<(String, Option<regex::Regex>)> =
        paths.iter().map(|path| (path.clone(), None)).collect();
    if !groups.is_empty() {
        let catalog = std::fs::read_to_string("tools/test262/groups.json")
            .map_err(|error| format!("groups.json: {error}"))?;
        let catalog: Json = serde_json::from_str(&catalog).map_err(|error| error.to_string())?;
        for group in groups {
            let entry = catalog
                .get(group)
                .ok_or_else(|| format!("unknown focused group: {group}"))?;
            let pattern = match entry.get("pattern").and_then(Json::as_str) {
                Some(pattern) => {
                    Some(regex::Regex::new(pattern).map_err(|error| error.to_string())?)
                }
                None => None,
            };
            for path in entry["paths"].as_array().into_iter().flatten() {
                selections.push((
                    path.as_str().unwrap_or_default().to_string(),
                    pattern.clone(),
                ));
            }
        }
    }
    if selections.is_empty() {
        selections.push((".".into(), None));
    }
    let mut tests = std::collections::BTreeSet::new();
    for (selection, pattern) in selections {
        let selected = test_dir
            .join(&selection)
            .canonicalize()
            .map_err(|_| format!("selection does not exist: {selection}"))?;
        if !selected.starts_with(&test_dir) {
            return Err(format!("selection escapes test directory: {selection}"));
        }
        let mut files = Vec::new();
        collect_js(&selected, &mut files)?;
        for file in files {
            let relative = relative(&test_dir, &file);
            if pattern
                .as_ref()
                .is_none_or(|pattern| pattern.is_match(&relative))
            {
                tests.insert(file);
            }
        }
    }
    // PathBuf ordering is component-wise, matching the Python runner.
    Ok(tests
        .into_iter()
        .filter(|test| !test.to_string_lossy().ends_with("_FIXTURE.js"))
        .collect())
}

fn relative(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Every variant row for one test file.
fn run_file(
    corpus: &Corpus,
    engine: &Path,
    test: &Path,
    timeout: Duration,
    skip: &[String],
) -> Vec<Json> {
    let name = relative(&corpus.test_dir(), test);
    let prepared = Corpus::read(test).and_then(|source| {
        let metadata = metadata::parse(&source)?;
        Ok((source, metadata))
    });
    let (source, metadata) = match prepared {
        Ok(prepared) => prepared,
        Err(message) => {
            return vec![
                json!({"test": name, "variant": "metadata", "status": "harness_error", "message": message}),
            ];
        }
    };
    let mut skipped: Vec<String> = metadata
        .features
        .iter()
        .filter(|feature| skip.contains(feature))
        .cloned()
        .collect();
    skipped.sort();
    skipped.dedup();
    metadata::variants(&metadata)
        .into_iter()
        .map(|variant| {
            if !skipped.is_empty() {
                return json!({"test": name, "variant": variant.name(), "status": "skip", "features": skipped});
            }
            match corpus.request(&name, &source, &metadata, variant) {
                Ok(request) => {
                    let (execution, elapsed) = worker::execute(engine, &request, timeout);
                    worker::row(&name, variant, execution, &metadata, elapsed)
                }
                Err(message) => json!({"test": name, "variant": "metadata", "status": "harness_error", "message": message}),
            }
        })
        .collect()
}

fn counts(results: &[Json]) -> serde_json::Map<String, Json> {
    let mut counts = serde_json::Map::new();
    for status in STATUSES {
        let count = results.iter().filter(|row| row["status"] == status).count();
        counts.insert(status.into(), json!(count));
    }
    counts
}

fn command_run(arguments: &[String]) -> Result<ExitCode, String> {
    let options = Options::parse(
        arguments,
        &[
            &COMMON[..],
            &[
                "path",
                "group",
                "skip-feature",
                "jobs",
                "timeout",
                "output",
                "checkpoint",
            ],
        ]
        .concat(),
        &["resume"],
    )?;
    if !options.positional.is_empty() {
        return Err(format!("run takes --path, not {}", options.positional[0]));
    }
    let jobs: usize = options.number("jobs", 2)?;
    if jobs == 0 {
        return Err("--jobs must be positive".into());
    }
    let timeout = options.seconds("timeout", 5.0)?;
    let resume = options.flag("resume");
    let checkpoint_path = options.one("checkpoint").map(PathBuf::from);
    if resume && checkpoint_path.is_none() {
        return Err("--resume requires --checkpoint".into());
    }
    let output = PathBuf::from(options.one("output").unwrap_or("test262-results.json"));
    let (corpus, revision) = corpus(&options, true)?;
    let paths = options.all("path");
    let groups = options.all("group");
    let skip = options.all("skip-feature");
    let tests = select(&corpus, &paths, &groups)?;
    let names: Vec<String> = tests
        .iter()
        .map(|test| relative(&corpus.test_dir(), test))
        .collect();
    let engine = EngineSnapshot::take(&options)?;
    let selections: Vec<String> = if paths.is_empty() && groups.is_empty() {
        vec![".".into()]
    } else {
        paths.clone()
    };
    let configuration = json!({
        "revision": revision, "engine_sha256": engine.sha256, "worker_jobs": jobs,
        "timeout_seconds": timeout.as_secs_f64(), "skip_features": skip, "selection": selections,
        "groups": groups, "selection_sha256": hex(&Sha256::digest(json!(names).to_string())),
    });

    let mut results: Vec<Json> = Vec::new();
    let mut resumed = 0;
    if resume {
        let path = checkpoint_path.as_ref().expect("checked above");
        let (count, rows) = restore_checkpoint(path, &configuration, &names)?;
        resumed = count;
        results = rows;
        eprintln!("Test262: restored {resumed}/{} files", names.len());
    }
    let mut journal = match &checkpoint_path {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(resume)
                .write(true)
                .truncate(!resume)
                .open(path)
                .map_err(|error| format!("checkpoint: {error}"))?;
            if !resume {
                writeln!(file, "{configuration}").map_err(|error| error.to_string())?;
                file.sync_all().map_err(|error| error.to_string())?;
            }
            Some(file)
        }
        None => None,
    };

    let pending: Arc<Vec<PathBuf>> = Arc::new(tests[resumed..].to_vec());
    let next = Arc::new(AtomicUsize::new(0));
    let corpus = Arc::new(corpus);
    let skip = Arc::new(skip);
    let (sender, receiver) = mpsc::channel::<(usize, Vec<Json>)>();
    let mut handles = Vec::new();
    for _ in 0..jobs {
        let (pending, next, corpus, skip, sender) = (
            pending.clone(),
            next.clone(),
            corpus.clone(),
            skip.clone(),
            sender.clone(),
        );
        let engine = engine.path.clone();
        handles.push(std::thread::spawn(move || {
            loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(test) = pending.get(index) else {
                    break;
                };
                let rows = run_file(&corpus, &engine, test, timeout, &skip);
                if sender.send((index, rows)).is_err() {
                    break;
                }
            }
        }));
    }
    drop(sender);
    // Journal entries are written as an ordered prefix so resume can trust them.
    let started = Instant::now();
    let mut buffered: BTreeMap<usize, Vec<Json>> = BTreeMap::new();
    let mut written = 0;
    for (index, rows) in receiver {
        buffered.insert(index, rows);
        while let Some(rows) = buffered.remove(&written) {
            if let Some(file) = journal.as_mut() {
                let entry = json!({"test": names[resumed + written], "rows": rows});
                writeln!(file, "{entry}").map_err(|error| error.to_string())?;
            }
            results.extend(rows);
            written += 1;
            let done = resumed + written;
            if done % 1000 == 0 || done == names.len() {
                if let Some(file) = journal.as_mut() {
                    file.sync_data().map_err(|error| error.to_string())?;
                }
                let elapsed = started.elapsed().as_secs_f64();
                let remaining = elapsed / written as f64 * (names.len() - done) as f64;
                eprintln!(
                    "Test262: {done}/{} files, {elapsed:.0}s elapsed, ~{remaining:.0}s left",
                    names.len()
                );
            }
        }
    }
    for handle in handles {
        let _ = handle.join();
    }
    if let Some(file) = journal.as_mut() {
        file.sync_all().map_err(|error| error.to_string())?;
    }

    let counts = counts(&results);
    let passed = counts["pass"].as_u64().unwrap_or(0);
    let total = results.len();
    let mut report = serde_json::Map::new();
    for (key, value) in [
        ("domain", json!("ECMAScript")),
        ("suite", json!("Test262")),
        ("revision", configuration["revision"].clone()),
        ("selection", configuration["selection"].clone()),
        ("groups", configuration["groups"].clone()),
        ("engine_sha256", json!(engine.sha256)),
        ("skip_features", configuration["skip_features"].clone()),
        (
            "denominator",
            json!("all selected variants, including skips and errors"),
        ),
        ("worker_jobs", json!(jobs)),
        ("timeout_seconds", json!(timeout.as_secs_f64())),
        ("resumed_files", json!(resumed)),
        ("total", json!(total)),
        ("counts", Json::Object(counts.clone())),
        (
            "pass_percentage",
            if total == 0 {
                Json::Null
            } else {
                json!(100.0 * passed as f64 / total as f64)
            },
        ),
        (
            "limitations",
            json!([
                "Host capabilities depend on the selected worker; GC requests run at quiescent host boundaries"
            ]),
        ),
        ("results", json!(results)),
    ] {
        report.insert(key.into(), value);
    }
    let text =
        serde_json::to_string_pretty(&Json::Object(report)).map_err(|error| error.to_string())?;
    std::fs::write(&output, text + "\n")
        .map_err(|error| format!("{}: {error}", output.display()))?;
    println!(
        "{}: {passed}/{total} passed ({})",
        output.display(),
        describe_counts(&counts)
    );
    Ok(if total > 0 && passed as usize == total {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn describe_counts(counts: &serde_json::Map<String, Json>) -> String {
    STATUSES
        .iter()
        .map(|status| {
            format!(
                "{status} {}",
                counts.get(*status).and_then(Json::as_u64).unwrap_or(0)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn restore_checkpoint(
    path: &Path,
    configuration: &Json,
    names: &[String],
) -> Result<(usize, Vec<Json>), String> {
    let contents = std::fs::read(path).map_err(|error| format!("checkpoint: {error}"))?;
    // An interrupted final write is discarded and rerun.
    let complete = contents
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let mut lines = contents[..complete].lines();
    let header: Json = lines
        .next()
        .ok_or("empty checkpoint")?
        .map_err(|error| error.to_string())
        .and_then(|line| serde_json::from_str(&line).map_err(|error| error.to_string()))?;
    if &header != configuration {
        return Err("checkpoint configuration or worker digest mismatch".into());
    }
    let mut results = Vec::new();
    let mut count = 0;
    for line in lines {
        let line = line.map_err(|error| error.to_string())?;
        let entry: Json = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        if names.get(count).map(String::as_str) != entry["test"].as_str() {
            return Err("checkpoint is not an ordered selection prefix".into());
        }
        let rows = entry["rows"]
            .as_array()
            .ok_or("checkpoint entry has no rows")?;
        if rows.is_empty()
            || rows
                .iter()
                .any(|row| !STATUSES.contains(&row["status"].as_str().unwrap_or("")))
        {
            return Err("invalid checkpoint outcome".into());
        }
        results.extend(rows.iter().cloned());
        count += 1;
    }
    if complete != contents.len() {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .and_then(|file| file.set_len(complete as u64))
            .map_err(|error| error.to_string())?;
    }
    Ok((count, results))
}

fn command_time(arguments: &[String]) -> Result<ExitCode, String> {
    let options = Options::parse(
        arguments,
        &[&COMMON[..], &["timeout", "variant"]].concat(),
        &[],
    )?;
    if options.positional.is_empty() {
        return Err("time needs at least one test path".into());
    }
    let timeout = options.seconds("timeout", 600.0)?;
    let only = match options.one("variant") {
        Some(name) => Some(Variant::parse(name).ok_or_else(|| format!("unknown variant {name}"))?),
        None => None,
    };
    let (corpus, _) = corpus(&options, false)?;
    let engine = EngineSnapshot::take(&options)?;
    let tests = select(&corpus, &options.positional, &[])?;
    let mut failures = 0;
    for test in &tests {
        let name = relative(&corpus.test_dir(), test);
        let source = Corpus::read(test)?;
        let metadata = metadata::parse(&source).map_err(|error| format!("{name}: {error}"))?;
        for variant in metadata::variants(&metadata) {
            if only.is_some_and(|only| only != variant) {
                continue;
            }
            let request = corpus.request(&name, &source, &metadata, variant)?;
            let (execution, elapsed) = worker::execute(&engine.path, &request, timeout);
            let row = worker::row(&name, variant, execution, &metadata, elapsed);
            let status = row["status"].as_str().unwrap_or("?");
            if status != "pass" {
                failures += 1;
            }
            let message = row["engine"]["message"]
                .as_str()
                .or(row["message"].as_str())
                .unwrap_or("");
            let message: String = message.chars().take(160).collect();
            println!(
                "{status:<13} {:>8.2}s  {name} [{}] {message}",
                elapsed.as_secs_f64(),
                variant.name()
            );
        }
    }
    Ok(if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn read_report(path: &str) -> Result<Json, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    let bytes = if path.ends_with(".gz") {
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&bytes[..]), &mut decoded)
            .map_err(|error| format!("{path}: {error}"))?;
        decoded
    } else {
        bytes
    };
    serde_json::from_slice(&bytes).map_err(|error| format!("{path}: {error}"))
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

fn status_detail(row: &Json) -> String {
    let status = row["status"].as_str().unwrap_or("?");
    let message = row["engine"]["message"]
        .as_str()
        .or(row["message"].as_str())
        .unwrap_or("");
    let message: String = message.chars().take(140).collect();
    if message.is_empty() {
        status.to_string()
    } else {
        format!("{status}: {message}")
    }
}

fn command_compare(arguments: &[String]) -> Result<ExitCode, String> {
    let options = Options::parse(arguments, &["output"], &["allow-config-change"])?;
    let [baseline, current] = options.positional.as_slice() else {
        return Err("compare needs BASELINE and CURRENT reports".into());
    };
    let (baseline, current) = (read_report(baseline)?, read_report(current)?);
    if !options.flag("allow-config-change") {
        for field in ["revision", "worker_jobs", "timeout_seconds"] {
            if baseline[field] != current[field] {
                return Err(format!(
                    "incompatible {field}: {} vs {} (use --allow-config-change)",
                    baseline[field], current[field]
                ));
            }
        }
    }
    let (before, after) = (indexed(&baseline)?, indexed(&current)?);
    let (mut gains, mut losses, mut changed, mut added, mut removed) =
        (vec![], vec![], vec![], vec![], vec![]);
    for (key, new) in &after {
        let Some(old) = before.get(key) else {
            added.push(json!({"test": key.0, "variant": key.1, "after": new["status"]}));
            continue;
        };
        if old["status"] == new["status"] {
            continue;
        }
        let entry = json!({"test": key.0, "variant": key.1, "before": old["status"], "after": new["status"]});
        if new["status"] == "pass" {
            gains.push(entry);
        } else if old["status"] == "pass" {
            println!("LOSS {} [{}]: {}", key.0, key.1, status_detail(new));
            losses.push(entry);
        } else {
            changed.push(entry);
        }
    }
    for key in before.keys().filter(|key| !after.contains_key(*key)) {
        removed.push(json!({"test": key.0, "variant": key.1}));
    }
    for entry in &gains {
        println!(
            "GAIN {} [{}] (was {})",
            entry["test"].as_str().unwrap_or(""),
            entry["variant"].as_str().unwrap_or(""),
            entry["before"]
        );
    }
    println!(
        "baseline {}/{} -> current {}/{}: {} gains, {} losses, {} other changes, {} added, {} removed",
        baseline["counts"]["pass"],
        baseline["total"],
        current["counts"]["pass"],
        current["total"],
        gains.len(),
        losses.len(),
        changed.len(),
        added.len(),
        removed.len()
    );
    if let Some(output) = options.one("output") {
        let comparison = json!({"gains": gains, "losses": losses, "changed": changed, "added": added, "removed": removed});
        let text = serde_json::to_string_pretty(&comparison).map_err(|error| error.to_string())?;
        std::fs::write(output, text + "\n").map_err(|error| format!("{output}: {error}"))?;
    }
    Ok(if losses.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn command_summary(arguments: &[String]) -> Result<ExitCode, String> {
    let options = Options::parse(arguments, &["slowest"], &[])?;
    let [path] = options.positional.as_slice() else {
        return Err("summary needs one REPORT".into());
    };
    let report = read_report(path)?;
    let results = report["results"]
        .as_array()
        .ok_or("report has no results")?;
    let counts = counts(results);
    println!(
        "{}/{} passed ({}), revision {}, engine {}",
        counts["pass"],
        results.len(),
        describe_counts(&counts),
        report["revision"].as_str().unwrap_or("?"),
        report["engine_sha256"].as_str().unwrap_or("?")
    );
    for row in results.iter().filter(|row| row["status"] != "pass") {
        println!(
            "  {} [{}] {}",
            row["test"].as_str().unwrap_or(""),
            row["variant"].as_str().unwrap_or(""),
            status_detail(row)
        );
    }
    let slowest: usize = options.number("slowest", 10)?;
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
    if !timed.is_empty() && slowest > 0 {
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
    Ok(ExitCode::SUCCESS)
}

fn command_request(arguments: &[String]) -> Result<ExitCode, String> {
    let options = Options::parse(
        arguments,
        &[&COMMON[..], &["variant", "output"]].concat(),
        &[],
    )?;
    let [test] = options.positional.as_slice() else {
        return Err("request needs one TEST".into());
    };
    let (corpus, _) = corpus(&options, false)?;
    let path = corpus.test_dir().join(test);
    let source = Corpus::read(&path)?;
    let metadata = metadata::parse(&source)?;
    let variants = metadata::variants(&metadata);
    let variant = match options.one("variant") {
        Some(name) => Variant::parse(name).ok_or_else(|| format!("unknown variant {name}"))?,
        None => variants[0],
    };
    if !variants.contains(&variant) {
        return Err(format!("{test} does not run as {}", variant.name()));
    }
    let request = corpus
        .request(test, &source, &metadata, variant)?
        .to_string();
    match options.one("output") {
        Some(output) => {
            std::fs::write(output, request).map_err(|error| format!("{output}: {error}"))?
        }
        None => println!("{request}"),
    }
    Ok(ExitCode::SUCCESS)
}

fn command_bench(arguments: &[String]) -> Result<ExitCode, String> {
    let options = Options::parse(
        arguments,
        &[&COMMON[..], &["repeat", "timeout"]].concat(),
        &["harness"],
    )?;
    if options.positional.is_empty() {
        return Err("bench needs at least one JavaScript file".into());
    }
    let repeat: usize = options.number("repeat", 3)?;
    let timeout = options.seconds("timeout", 600.0)?;
    let harness = if options.flag("harness") {
        let (corpus, _) = corpus(&options, false)?;
        let request = corpus.request(
            "bench.js",
            "",
            &metadata::Metadata::default(),
            Variant::Script,
        )?;
        request["harness"].as_str().unwrap_or_default().to_string()
    } else {
        String::new()
    };
    let engine = EngineSnapshot::take(&options)?;
    for file in &options.positional {
        let source = std::fs::read_to_string(file).map_err(|error| format!("{file}: {error}"))?;
        let request = json!({"source": source, "harness": harness, "module": false, "asynchronous": false,
            "can_block": true, "modules": {}, "id": file, "corpus_root": null});
        let mut times = Vec::new();
        let mut status = String::new();
        for _ in 0..repeat.max(1) {
            let (execution, elapsed) = worker::execute(&engine.path, &request, timeout);
            times.push(elapsed.as_secs_f64());
            status = match execution {
                worker::Execution::Report(report) => {
                    report["status"].as_str().unwrap_or("?").to_string()
                        + report["message"]
                            .as_str()
                            .map(|m| format!(": {m}"))
                            .unwrap_or_default()
                            .as_str()
                }
                worker::Execution::Timeout => "timeout".into(),
                worker::Execution::Crash { code, .. } => format!("crash {code:?}"),
                worker::Execution::InvalidOutput => "invalid output".into(),
            };
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        println!(
            "min {:>7.3}s  median {:>7.3}s  {file}  {status}",
            times[0],
            times[times.len() / 2]
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = arguments.first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let rest = &arguments[1..];
    let result = match command.as_str() {
        "run" => command_run(rest),
        "time" => command_time(rest),
        "compare" => command_compare(rest),
        "summary" => command_summary(rest),
        "request" => command_request(rest),
        "bench" => command_bench(rest),
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        other => Err(format!("unknown command {other}\n\n{USAGE}")),
    };
    result.unwrap_or_else(|message| {
        eprintln!("test262: {message}");
        ExitCode::from(2)
    })
}
