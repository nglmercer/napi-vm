use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn worker(request: Value) -> Value {
    let mut child = Command::new(env!("CARGO_BIN_EXE_napi-vm-test262"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn worker_separates_parse_runtime_and_harness_errors() {
    assert_eq!(worker(json!({"source":"1+2"}))["status"], "ok");
    let parse = worker(json!({"source":"var = 1"}));
    assert_eq!(parse["phase"], "parse");
    assert_eq!(parse["error_type"], "SyntaxError");
    let runtime = worker(json!({"source":"throw new TypeError('expected')"}));
    assert_eq!(runtime["phase"], "runtime");
    assert_eq!(runtime["error_type"], "TypeError");
    assert_eq!(
        worker(json!({"source":"1", "harness":"throw new Error('broken harness')"}))["phase"],
        "harness"
    );
}
#[test]
fn worker_requires_exact_async_completion() {
    assert_eq!(
        worker(json!({"source":"Promise.resolve().then(() => $DONE());", "asynchronous":true}))["status"],
        "ok"
    );
    assert_eq!(
        worker(json!({"source":"Promise.resolve();", "asynchronous":true}))["error_type"],
        "Test262AsyncError"
    );
    assert_eq!(
        worker(json!({"source":"$DONE(); $DONE();", "asynchronous":true}))["status"],
        "error"
    );
    assert_eq!(
        worker(json!({"source":"$DONE(new TypeError('failed'));", "asynchronous":true}))["error_type"],
        "TypeError"
    );
}
#[test]
fn worker_loads_module_fixtures() {
    let result = worker(json!({"id":"pkg/test.js", "module":true,
        "source":"import { answer } from './value_FIXTURE.js'; if (answer !== 42) throw new Error('wrong');",
        "modules":{"pkg/value_FIXTURE.js":"export const answer = 42;"}}));
    assert_eq!(result["status"], "ok", "{result}");
}
