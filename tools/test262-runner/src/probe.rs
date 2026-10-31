//! Single-test probes: `time` (uncapped timings), `request` (exact worker input),
//! and `bench` (JavaScript snippets through the worker).

use crate::corpus::{Corpus, read_source};
use crate::metadata::{self, Metadata, Variant};
use crate::options::{COMMON_VALUES, Options};
use crate::worker::{self, Execution, Snapshot};
use serde_json::json;
use std::process::ExitCode;
use std::time::Duration;

fn variant_option(options: &Options) -> Result<Option<Variant>, String> {
    match options.one("variant") {
        Some(name) => Variant::parse(name)
            .map(Some)
            .ok_or_else(|| format!("unknown variant {name}")),
        None => Ok(None),
    }
}

/// `time PATH... [--timeout S] [--variant V]`: one line per variant, uncapped by default.
pub fn time(arguments: &[String]) -> Result<ExitCode, String> {
    let options = Options::parse(
        arguments,
        &[&COMMON_VALUES[..], &["timeout", "variant"]].concat(),
        &[],
    )?;
    if options.positional.is_empty() {
        return Err("time needs at least one test path".into());
    }
    let timeout = options.seconds("timeout", 600.0)?;
    let only = variant_option(&options)?;
    let (corpus, _) = Corpus::from_options(&options, false)?;
    let snapshot = Snapshot::take(&options)?;
    let mut failures = 0;
    for test in corpus.select(&options.positional, &[])? {
        let name = corpus.name(&test);
        let source = read_source(&test)?;
        let metadata = metadata::parse(&source).map_err(|error| format!("{name}: {error}"))?;
        for variant in metadata::variants(&metadata) {
            if only.is_some_and(|only| only != variant) {
                continue;
            }
            let request = corpus.request(&name, &source, &metadata, variant)?;
            let (execution, elapsed) = worker::execute(&snapshot.path, &request, timeout);
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

/// `request TEST [--variant V] [--output FILE]`: the exact JSON the worker reads.
pub fn request(arguments: &[String]) -> Result<ExitCode, String> {
    let options = Options::parse(
        arguments,
        &[&COMMON_VALUES[..], &["variant", "output"]].concat(),
        &[],
    )?;
    let test = &options.require_positional(1, "one TEST")?[0];
    let (corpus, _) = Corpus::from_options(&options, false)?;
    let path = corpus.file(test)?;
    let source = read_source(&path)?;
    let metadata = metadata::parse(&source)?;
    let variants = metadata::variants(&metadata);
    let variant = variant_option(&options)?.unwrap_or(variants[0]);
    if !variants.contains(&variant) {
        return Err(format!("{test} does not run as {}", variant.name()));
    }
    let text = corpus
        .request(test, &source, &metadata, variant)?
        .to_string();
    match options.one("output") {
        Some(output) => {
            std::fs::write(output, text).map_err(|error| format!("{output}: {error}"))?
        }
        None => println!("{text}"),
    }
    Ok(ExitCode::SUCCESS)
}

/// `bench FILE... [--harness] [--repeat N] [--timeout S]`: min and median wall time.
pub fn bench(arguments: &[String]) -> Result<ExitCode, String> {
    let options = Options::parse(
        arguments,
        &[&COMMON_VALUES[..], &["repeat", "timeout"]].concat(),
        &["harness"],
    )?;
    if options.positional.is_empty() {
        return Err("bench needs at least one JavaScript file".into());
    }
    let repeat: usize = options.number("repeat", 3)?;
    let timeout: Duration = options.seconds("timeout", 600.0)?;
    let harness = if options.flag("harness") {
        let (corpus, _) = Corpus::from_options(&options, false)?;
        let request = corpus.request("bench.js", "", &Metadata::default(), Variant::Script)?;
        request["harness"].as_str().unwrap_or_default().to_string()
    } else {
        String::new()
    };
    let snapshot = Snapshot::take(&options)?;
    for file in &options.positional {
        let source = std::fs::read_to_string(file).map_err(|error| format!("{file}: {error}"))?;
        let request = json!({"source": source, "harness": harness, "module": false,
            "asynchronous": false, "can_block": true, "modules": {}, "id": file, "corpus_root": null});
        let mut times = Vec::new();
        let mut status = String::new();
        for _ in 0..repeat.max(1) {
            let (execution, elapsed) = worker::execute(&snapshot.path, &request, timeout);
            times.push(elapsed.as_secs_f64());
            status = match execution {
                Execution::Report(report) => match report["message"].as_str() {
                    Some(message) => {
                        format!("{}: {message}", report["status"].as_str().unwrap_or("?"))
                    }
                    None => report["status"].as_str().unwrap_or("?").to_string(),
                },
                Execution::Timeout => "timeout".into(),
                Execution::Crash { code, .. } => format!("crash {code:?}"),
                Execution::InvalidOutput => "invalid output".into(),
            };
        }
        times.sort_by(|a, b| a.total_cmp(b));
        println!(
            "min {:>7.3}s  median {:>7.3}s  {file}  {status}",
            times[0],
            times[times.len() / 2]
        );
    }
    Ok(ExitCode::SUCCESS)
}
