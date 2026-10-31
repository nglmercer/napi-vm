//! Suite execution: `run` a selection, or `retest` a report's non-passing tests.

use crate::corpus::{Corpus, default_corpus, read_source};
use crate::metadata;
use crate::options::Options;
use crate::report::{self, Header};
use crate::worker::{self, Snapshot};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

pub struct Plan {
    pub jobs: usize,
    pub timeout: Duration,
    pub skip: Vec<String>,
}

/// Every variant row for one test file, in variant order.
fn run_file(
    corpus: &Corpus,
    engine: &Path,
    test: &Path,
    timeout: Duration,
    skip: &[String],
) -> Vec<Json> {
    let name = corpus.name(test);
    let prepared = read_source(test).and_then(|source| {
        let metadata = metadata::parse(&source)?;
        Ok((source, metadata))
    });
    let (source, metadata) = match prepared {
        Ok(prepared) => prepared,
        Err(message) => {
            let row = json!({"test": name, "variant": "metadata",
                "status": "harness_error", "message": message});
            return vec![row];
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
                return json!({"test": name, "variant": variant.name(),
                    "status": "skip", "features": skipped});
            }
            match corpus.request(&name, &source, &metadata, variant) {
                Ok(request) => {
                    let (execution, elapsed) = worker::execute(engine, &request, timeout);
                    worker::row(&name, variant, execution, &metadata, elapsed)
                }
                Err(message) => json!({"test": name, "variant": "metadata",
                    "status": "harness_error", "message": message}),
            }
        })
        .collect()
}

/// Run `tests` on `plan.jobs` workers. `sink` receives each file's rows in
/// selection order, so checkpoints are always an ordered prefix.
pub fn execute(
    corpus: &Arc<Corpus>,
    engine: &Path,
    tests: &[PathBuf],
    plan: &Plan,
    mut sink: impl FnMut(usize, &[Json]) -> Result<(), String>,
) -> Result<Vec<Json>, String> {
    let tests = Arc::new(tests.to_vec());
    let next = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::channel::<(usize, Vec<Json>)>();
    let handles: Vec<_> = (0..plan.jobs)
        .map(|_| {
            let (tests, next, stop, corpus) =
                (tests.clone(), next.clone(), stop.clone(), corpus.clone());
            let (sender, engine) = (sender.clone(), engine.to_path_buf());
            let (timeout, skip) = (plan.timeout, plan.skip.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(test) = tests.get(index) else {
                        break;
                    };
                    let rows = run_file(&corpus, &engine, test, timeout, &skip);
                    if sender.send((index, rows)).is_err() {
                        break;
                    }
                }
            })
        })
        .collect();
    drop(sender);

    let mut buffered: BTreeMap<usize, Vec<Json>> = BTreeMap::new();
    let mut results = Vec::new();
    let mut written = 0;
    let mut failure = None;
    'receive: for (index, rows) in receiver {
        buffered.insert(index, rows);
        while let Some(rows) = buffered.remove(&written) {
            if let Err(error) = sink(written, &rows) {
                failure = Some(error);
                stop.store(true, Ordering::Relaxed);
                break 'receive;
            }
            results.extend(rows);
            written += 1;
        }
    }
    for handle in handles {
        let _ = handle.join();
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(results),
    }
}

fn fingerprint(
    revision: &str,
    engine_sha256: &str,
    plan: &Plan,
    selection: &[String],
    groups: &[String],
    names: &[String],
) -> Json {
    json!({
        "revision": revision, "engine_sha256": engine_sha256,
        "worker_jobs": plan.jobs, "timeout_seconds": plan.timeout.as_secs_f64(),
        "skip_features": plan.skip, "selection": selection, "groups": groups,
        "selection_sha256": format!("{:x}", Sha256::digest(json!(names).to_string())),
    })
}

/// `run [--path P]... [--group G]... [--skip-feature F]... [--jobs N] [--timeout S]
///      [--output FILE] [--checkpoint FILE [--resume]]`
pub fn run(options: &Options) -> Result<ExitCode, String> {
    options.require_positional(
        0,
        "no positional arguments; select tests with --path or --group",
    )?;
    let jobs: usize = options.number("jobs", 2)?;
    if jobs == 0 {
        return Err("--jobs must be positive".into());
    }
    let plan = Plan {
        jobs,
        timeout: options.seconds("timeout", 5.0)?,
        skip: options.all("skip-feature"),
    };
    let resume = options.flag("resume");
    let checkpoint = options.one("checkpoint").map(PathBuf::from);
    if resume && checkpoint.is_none() {
        return Err("--resume requires --checkpoint".into());
    }
    let output = PathBuf::from(options.one("output").unwrap_or("test262-results.json"));
    let (corpus, revision) = Corpus::from_options(options, true)?;
    let corpus = Arc::new(corpus);
    let paths = options.all("path");
    let groups = options.all("group");
    let tests = corpus.select(&paths, &groups)?;
    let names: Vec<String> = tests.iter().map(|test| corpus.name(test)).collect();
    let snapshot = Snapshot::take(options)?;
    let selection = if paths.is_empty() && groups.is_empty() {
        vec![".".to_string()]
    } else {
        paths
    };
    let expected = fingerprint(
        &revision,
        &snapshot.sha256,
        &plan,
        &selection,
        &groups,
        &names,
    );

    let mut results = Vec::new();
    let mut resumed = 0;
    let mut journal = None;
    if let Some(path) = &checkpoint {
        if resume {
            let (count, rows) = restore(path, &expected, &names)?;
            resumed = count;
            results = rows;
            eprintln!("Test262: restored {resumed}/{} files", names.len());
        }
        journal = Some(open_journal(path, resume, &expected)?);
    }

    let started = Instant::now();
    let fresh = execute(
        &corpus,
        &snapshot.path,
        &tests[resumed..],
        &plan,
        |index, rows| {
            let done = resumed + index + 1;
            if let Some(file) = journal.as_mut() {
                let entry = json!({"test": names[done - 1], "rows": rows});
                writeln!(file, "{entry}").map_err(|error| error.to_string())?;
                if done % 1000 == 0 || done == names.len() {
                    file.sync_data().map_err(|error| error.to_string())?;
                }
            }
            if done % 1000 == 0 || done == names.len() {
                let elapsed = started.elapsed().as_secs_f64();
                let fresh_done = (done - resumed) as f64;
                let remaining = elapsed / fresh_done * (names.len() - done) as f64;
                eprintln!(
                    "Test262: {done}/{} files, {elapsed:.0}s elapsed, ~{remaining:.0}s left",
                    names.len()
                );
            }
            Ok(())
        },
    )?;
    if let Some(file) = journal.as_mut() {
        file.sync_all().map_err(|error| error.to_string())?;
    }
    results.extend(fresh);

    let header = Header {
        revision,
        selection,
        groups,
        engine_sha256: snapshot.sha256.clone(),
        skip_features: plan.skip.clone(),
        worker_jobs: plan.jobs,
        timeout_seconds: plan.timeout.as_secs_f64(),
        resumed_files: resumed,
    };
    finish(&output, &header, results)
}

/// `retest BASE [--output FILE] [--jobs N] [--timeout S]`: rerun only the tests
/// that were not passing in BASE, against the same revision. Fast enough for
/// an edit-test loop; a full `run` remains the gate because it also catches
/// regressions in tests that passed before.
pub fn retest(options: &Options) -> Result<ExitCode, String> {
    let base_path = options.require_positional(1, "one BASE report")?[0].clone();
    let base = report::read(&base_path)?;
    let revision = base["revision"]
        .as_str()
        .ok_or("base report has no revision")?
        .to_string();
    let base_jobs = base["worker_jobs"].as_u64().unwrap_or(2) as usize;
    let base_timeout = base["timeout_seconds"].as_f64().unwrap_or(5.0);
    let base_skip: Vec<String> = base["skip_features"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|feature| feature.as_str().map(str::to_string))
        .collect();
    let plan = Plan {
        jobs: options.number("jobs", base_jobs)?,
        timeout: options.seconds("timeout", base_timeout)?,
        skip: base_skip,
    };
    if plan.jobs == 0 {
        return Err("--jobs must be positive".into());
    }
    let path = options
        .one("corpus")
        .map(PathBuf::from)
        .unwrap_or_else(default_corpus);
    let (corpus, _) = Corpus::open(&path, Some(&revision))?;
    let corpus = Arc::new(corpus);

    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for row in base["results"]
        .as_array()
        .ok_or("base report has no results")?
    {
        let test = row["test"].as_str().unwrap_or_default();
        if row["status"] != "pass" && seen.insert(test.to_string()) {
            names.push(test.to_string());
        }
    }
    let tests = names
        .iter()
        .map(|name| corpus.file(name))
        .collect::<Result<Vec<_>, _>>()?;
    let snapshot = Snapshot::take(options)?;
    let selection = vec![format!("retest of {base_path}")];
    eprintln!("Test262: retesting {} files", names.len());
    let results = execute(&corpus, &snapshot.path, &tests, &plan, |_, _| Ok(()))?;
    let header = Header {
        revision,
        selection,
        groups: Vec::new(),
        engine_sha256: snapshot.sha256.clone(),
        skip_features: plan.skip.clone(),
        worker_jobs: plan.jobs,
        timeout_seconds: plan.timeout.as_secs_f64(),
        resumed_files: 0,
    };
    let output = PathBuf::from(options.one("output").unwrap_or("retest.json"));
    finish(&output, &header, results)
}

fn finish(output: &Path, header: &Header, results: Vec<Json>) -> Result<ExitCode, String> {
    let document = report::document(header, results);
    report::write(output, &document)?;
    let counts = &document["counts"];
    let total = document["total"].as_u64().unwrap_or(0);
    let passed = counts["pass"].as_u64().unwrap_or(0);
    println!(
        "{}: {passed}/{total} passed ({})",
        output.display(),
        report::describe(counts.as_object().expect("counts object"))
    );
    Ok(if total > 0 && passed == total {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn open_journal(path: &Path, resume: bool, header: &Json) -> Result<std::fs::File, String> {
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
        writeln!(file, "{header}").map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
    }
    Ok(file)
}

/// Restore an ordered prefix of a journal. A torn final line is cut off and
/// its file is rerun.
fn restore(path: &Path, expected: &Json, names: &[String]) -> Result<(usize, Vec<Json>), String> {
    let contents = std::fs::read(path).map_err(|error| format!("checkpoint: {error}"))?;
    let complete = contents
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let text = std::str::from_utf8(&contents[..complete]).map_err(|error| error.to_string())?;
    let mut lines = text.lines();
    let header: Json = serde_json::from_str(lines.next().ok_or("empty checkpoint")?)
        .map_err(|error| error.to_string())?;
    if &header != expected {
        return Err("checkpoint configuration or worker digest mismatch".into());
    }
    let mut results = Vec::new();
    let mut count = 0;
    for line in lines {
        let entry: Json = serde_json::from_str(line).map_err(|error| error.to_string())?;
        if names.get(count).map(String::as_str) != entry["test"].as_str() {
            return Err("checkpoint is not an ordered selection prefix".into());
        }
        let rows = entry["rows"]
            .as_array()
            .ok_or("checkpoint entry has no rows")?;
        let valid = |row: &Json| report::STATUSES.contains(&row["status"].as_str().unwrap_or(""));
        if rows.is_empty() || !rows.iter().all(valid) {
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
