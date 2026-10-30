//! One isolated worker process per variant, with a hard deadline.

use crate::metadata::{Metadata, Variant};
use serde_json::{Value as Json, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Corpus layout and the harness files a test includes.
pub struct Corpus {
    pub root: PathBuf,
}

impl Corpus {
    pub fn test_dir(&self) -> PathBuf {
        self.root.join("test")
    }

    /// Source text exactly as stored: Function.prototype.toString observes it,
    /// so line endings are never normalized.
    pub fn read(path: &Path) -> Result<String, String> {
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn harness(&self, name: &str) -> Result<String, String> {
        let directory = self.root.join("harness");
        let path = directory.join(name);
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("harness {name}: {error}"))?;
        if !canonical.starts_with(directory.canonicalize().map_err(|e| e.to_string())?) {
            return Err(format!("harness include escapes corpus: {name}"));
        }
        Self::read(&canonical)
    }

    /// The worker request the runner sends for one variant.
    pub fn request(
        &self,
        id: &str,
        source: &str,
        metadata: &Metadata,
        variant: Variant,
    ) -> Result<Json, String> {
        let mut names: Vec<String> = if metadata.has_flag("raw") {
            Vec::new()
        } else {
            vec!["sta.js".into(), "assert.js".into()]
        };
        for include in &metadata.includes {
            if !names.contains(include) {
                names.push(include.clone());
            }
        }
        // The worker provides $DONE directly; doneprintHandle.js needs print.
        names.retain(|name| name != "doneprintHandle.js");
        let harness = names
            .iter()
            .map(|name| self.harness(name))
            .collect::<Result<Vec<_>, _>>()?
            .join("\n");
        let source = if variant == Variant::Strict {
            format!("\"use strict\";\n{source}")
        } else {
            source.to_string()
        };
        Ok(json!({
            "source": source,
            "harness": harness,
            "module": variant == Variant::Module,
            "asynchronous": metadata.has_flag("async"),
            "can_block": !metadata.has_flag("CanBlockIsFalse"),
            "modules": {},
            "id": id,
            "corpus_root": self.test_dir().to_string_lossy(),
        }))
    }
}

/// Raw result of running a request.
pub enum Execution {
    Report(Json),
    Timeout,
    Crash { code: Option<i32>, stderr: String },
    InvalidOutput,
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
            return (
                Execution::Crash {
                    code: None,
                    stderr: format!("spawn failed: {error}"),
                },
                started.elapsed(),
            );
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
    let deadline = started + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(2)),
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
        return (
            Execution::Crash {
                code: status.code(),
                stderr: tail.to_string(),
            },
            elapsed,
        );
    }
    match serde_json::from_slice(&out) {
        Ok(report) => (Execution::Report(report), elapsed),
        Err(_) => (Execution::InvalidOutput, elapsed),
    }
}

/// The reported status of an engine result, mirroring `run.py`.
pub fn outcome(report: &Json, metadata: &Metadata) -> &'static str {
    let phase = report.get("phase").and_then(Json::as_str);
    if matches!(phase, Some("driver" | "harness")) {
        return "harness_error";
    }
    let status = report.get("status").and_then(Json::as_str);
    if let Some(negative) = &metadata.negative {
        if status != Some("error") {
            return "fail";
        }
        let error_type = report.get("error_type").and_then(Json::as_str);
        return if phase == Some(negative.phase.as_str())
            && error_type == Some(negative.error_type.as_str())
        {
            "pass"
        } else {
            "fail"
        };
    }
    if status == Some("ok") { "pass" } else { "fail" }
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
    let mut row = match execution {
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
    let object = row.as_object_mut().expect("row object");
    let mut ordered = serde_json::Map::new();
    ordered.insert("test".into(), json!(test));
    ordered.insert("variant".into(), json!(variant.name()));
    ordered.append(object);
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
        assert_eq!(
            outcome(&json!({"status": "ok", "phase": "runtime"}), &plain),
            "pass"
        );
        assert_eq!(
            outcome(&json!({"status": "error", "phase": "runtime"}), &plain),
            "fail"
        );
        assert_eq!(
            outcome(&json!({"status": "error", "phase": "driver"}), &plain),
            "harness_error"
        );
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
        assert_eq!(
            outcome(&json!({"status": "ok", "phase": "runtime"}), &negative),
            "fail"
        );
    }
}
