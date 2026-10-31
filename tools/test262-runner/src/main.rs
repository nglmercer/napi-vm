//! Test262 validation for the isolated `napi-vm-test262` worker.
//!
//! Build once, then run from anywhere in the repository:
//!
//! ```text
//! cargo build --release -p napi-vm-test262-runner
//! target/release/test262 help
//! ```
//!
//! `tools/test262/README.md` documents the workflow and the reports.

mod corpus;
mod metadata;
mod options;
mod probe;
mod report;
mod run;
mod worker;

use options::{COMMON_VALUES, Options};
use std::process::ExitCode;

const USAGE: &str = "\
usage: test262 <command> [options]

Setup once:  test262 fetch

Validation loop, cheapest first:
  test262 run --group smoke                 seconds: a fixed cross-section
  test262 retest base.json --output r.json  only the base report's non-passing tests
  test262 compare base.json r.json --subset exit 1 if any retested pass was lost
  test262 run --output full.json            the complete gate (about 25 minutes, 2 cores)

commands:
  run [--path P]... [--group G]... [--skip-feature F]... [--jobs N] [--timeout S]
      [--output FILE] [--checkpoint FILE [--resume]]
        Run a selection (default: the whole corpus). --jobs defaults to 2 to match
        the archived runs; --timeout defaults to 5 seconds per variant.
  retest BASE [--output FILE] [--jobs N] [--timeout S]
        Rerun only BASE's non-passing tests, with BASE's worker count, timeout and
        skip list. Writes a report covering just those tests.
  compare BASELINE CURRENT [--subset] [--output FILE] [--allow-config-change]
        Gains, losses, changes, additions and removals. --subset compares only the
        variants present in CURRENT. Exits 1 when any variant lost its pass.
  summary REPORT [--limit N] [--show N] [--slowest N] [--json FILE]
        Counts, non-passing tests grouped by area, and optionally the slowest variants
        and a grouped JSON document for CI artifacts.
  fetch [--corpus DIR]
        Clone the pinned Test262 revision (corpus.lock.json) into DIR; no-op if present.
  time PATH... [--timeout S] [--variant script|strict|module]
        Time each variant of the given tests or directories, uncapped by default.
  request TEST [--variant script|strict|module] [--output FILE]
        Write the exact worker request for one variant, for profiling.
  bench FILE... [--harness] [--repeat N] [--timeout S]
        Time JavaScript snippet files through the worker (--harness adds Test262 harness).

common options:
  --corpus DIR       Test262 checkout (default .napi-vm/test262)
  --revision SHA     required corpus commit (default tools/test262/corpus.lock.json)
  --engine FILE      worker binary (default target/release/napi-vm-test262)
";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = arguments.split_first() else {
        eprint!("{USAGE}");
        return ExitCode::from(2);
    };
    let result = match command.as_str() {
        "run" => parse(
            rest,
            &[
                &COMMON_VALUES[..],
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
        )
        .and_then(|options| run::run(&options)),
        "retest" => parse(
            rest,
            &[&COMMON_VALUES[..], &["jobs", "timeout", "output"]].concat(),
            &[],
        )
        .and_then(|options| run::retest(&options)),
        "compare" => parse(rest, &["output"], &["subset", "allow-config-change"])
            .and_then(|options| report::compare(&options)),
        "summary" => parse(rest, &["limit", "show", "slowest", "json"], &[])
            .and_then(|options| report::summary(&options)),
        "fetch" => parse(rest, &COMMON_VALUES, &[])
            .and_then(|options| corpus::fetch(&options).map(|()| ExitCode::SUCCESS)),
        "time" => probe::time(rest),
        "request" => probe::request(rest),
        "bench" => probe::bench(rest),
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

fn parse(arguments: &[String], values: &[&str], flags: &[&str]) -> Result<Options, String> {
    Options::parse(arguments, values, flags)
}
