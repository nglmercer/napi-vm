//! One isolated worker process per variant, with a hard deadline.

use crate::metadata::{Metadata, Variant};
use crate::options::Options;
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Outcome of running one request in a fresh process.
pub enum Execution {
    Report(Json),
    Timeout,
    Crash { code: Option<i32>, stderr: String },
    InvalidOutput,
}

/// A private copy of the worker, so a concurrent `cargo build` cannot change
/// the implementation under a running comparison.
pub struct Snapshot {
    pub path: PathBuf,
    pub sha256: String,
    directory: PathBuf,
}

impl Snapshot {
    pub fn take(options: &Options) -> Result<Self, String> {
        let source = options
            .one("engine")
            .map(PathBuf::from)
            .unwrap_or_else(crate::corpus::default_engine);
        let bytes = std::fs::read(&source)
            .map_err(|error| format!("engine {}: {error}", source.display()))?;
        let sha256 = hex(&Sha256::digest(&bytes));
        let directory =
            std::env::temp_dir().join(format!("napi-vm-test262-{}", std::process::id()));
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let path = directory.join("worker");
        std::fs::write(&path, &bytes).map_err(|error| format!("snapshot engine: {error}"))?;
        set_executable(&path)?;
        Ok(Self {
            path,
            sha256,
            directory,
        })
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn set_executable(_: &Path) -> Result<(), String> {
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Run `engine` on `request`, killing it at `timeout`.
pub fn execute(engine: &Path, request: &Json, timeout: Duration) -> (Execution, Duration) {
    let started = Instant::now();
    let mut child = match Command::new(engine)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let crash = Execution::Crash {
                code: None,
                stderr: format!("spawn failed: {error}"),
            };
            return (crash, started.elapsed());
        }
    };
    let input = request.to_string();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let err_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });
    // Short waits first: most variants finish in milliseconds, and a fixed
    // long sleep would dominate their wall time.
    let deadline = started + timeout;
    let mut pause = Duration::from_micros(100);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(2));
            }
            Err(_) => break None,
        }
    };
    let elapsed = started.elapsed();
    let _ = writer.join();
    let out = out_reader.join().unwrap_or_default();
    let err = err_reader.join().unwrap_or_default();
    let Some(status) = status else {
        return (Execution::Timeout, elapsed);
    };
    if !status.success() {
        let text = String::from_utf8_lossy(&err);
        let tail = text
            .char_indices()
            .rev()
            .nth(3999)
            .map_or(&text[..], |(index, _)| &text[index..]);
        let crash = Execution::Crash {
            code: status.code(),
            stderr: tail.to_string(),
        };
        return (crash, elapsed);
    }
    match serde_json::from_slice(&out) {
        Ok(report) => (Execution::Report(report), elapsed),
        Err(_) => (Execution::InvalidOutput, elapsed),
    }
}

/// The reported status of an engine result (the rules `run.py` used).
pub fn outcome(report: &Json, metadata: &Metadata) -> &'static str {
    let phase = report.get("phase").and_then(Json::as_str);
    if matches!(phase, Some("driver" | "harness")) {
        return "harness_error";
    }
    let status = report.get("status").and_then(Json::as_str);
    let Some(negative) = &metadata.negative else {
        return if status == Some("ok") { "pass" } else { "fail" };
    };
    if status != Some("error") {
        return "fail";
    }
    let error_type = report.get("error_type").and_then(Json::as_str);
    if phase == Some(negative.phase.as_str()) && error_type == Some(negative.error_type.as_str()) {
        "pass"
    } else {
        "fail"
    }
}

/// One report row for a variant run.
pub fn row(
    test: &str,
    variant: Variant,
    execution: Execution,
    metadata: &Metadata,
    elapsed: Duration,
) -> Json {
    let seconds = (elapsed.as_secs_f64() * 1000.0).round() / 1000.0;
    let body = match execution {
        Execution::Timeout => json!({"status": "timeout"}),
        Execution::Crash { code, stderr } => {
            json!({"status": "crash", "exit_code": code, "message": stderr})
        }
        Execution::InvalidOutput => {
            json!({"status": "harness_error", "message": "worker returned invalid JSON"})
        }
        Execution::Report(report) => {
            json!({"status": outcome(&report, metadata), "engine": report})
        }
    };
    let mut ordered = serde_json::Map::new();
    ordered.insert("test".into(), json!(test));
    ordered.insert("variant".into(), json!(variant.name()));
    if let Json::Object(fields) = body {
        ordered.extend(fields);
    }
    ordered.insert("seconds".into(), json!(seconds));
    Json::Object(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::Negative;

    #[test]
    fn outcomes_follow_negative_expectations() {
        let plain = Metadata::default();
        let ok = json!({"status": "ok", "phase": "runtime"});
        assert_eq!(outcome(&ok, &plain), "pass");
        let error = json!({"status": "error", "phase": "runtime"});
        assert_eq!(outcome(&error, &plain), "fail");
        let driver = json!({"status": "error", "phase": "driver"});
        assert_eq!(outcome(&driver, &plain), "harness_error");
        let negative = Metadata {
            negative: Some(Negative {
                phase: "parse".into(),
                error_type: "SyntaxError".into(),
            }),
            ..Metadata::default()
        };
        let parse_error = json!({"status": "error", "phase": "parse", "error_type": "SyntaxError"});
        assert_eq!(outcome(&parse_error, &negative), "pass");
        let runtime_error =
            json!({"status": "error", "phase": "runtime", "error_type": "SyntaxError"});
        assert_eq!(outcome(&runtime_error, &negative), "fail");
        assert_eq!(outcome(&ok, &negative), "fail");
    }
}
