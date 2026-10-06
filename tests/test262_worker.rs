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
        worker(
            json!({"source":"Promise.resolve().then(function() { $DONE(); });", "asynchronous":true})
        )["status"],
        "ok"
    );
    assert_eq!(
        worker(
            json!({"source":"Promise.resolve().then(function() { $DONE(new TypeError('failed')); }).catch(function() {});", "asynchronous":true})
        )["error_type"],
        "TypeError"
    );
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
#[test]
fn worker_distinguishes_module_linking_and_evaluation_errors() {
    let linked = worker(
        json!({"id":"pkg/test.js","module":true,"source":"throw 1; import {missing} from './dep_FIXTURE.js';","modules":{"pkg/dep_FIXTURE.js":"export const available=1;"}}),
    );
    assert_eq!(linked["phase"], "resolution", "{linked}");
    assert_eq!(linked["error_type"], "SyntaxError");
    let evaluated = worker(
        json!({"id":"pkg/test.js","module":true,"source":"import {available} from './dep_FIXTURE.js';throw new TypeError('body');","modules":{"pkg/dep_FIXTURE.js":"export const available=1;"}}),
    );
    assert_eq!(evaluated["phase"], "runtime", "{evaluated}");
    assert_eq!(evaluated["error_type"], "TypeError");
}

#[test]
fn worker_eval_script_uses_the_global_environment() {
    let report = worker(
        json!({"source":"function local() { var hidden = 1; $262.evalScript('var installed=42;'); } local(); if(installed!==42 || $262.global!==globalThis) throw new Error('wrong realm global');"}),
    );
    assert_eq!(report["status"], "ok", "{report}");
}

#[test]
fn worker_loads_nested_and_dynamic_fixtures_within_the_explicit_root() {
    let root =
        std::env::temp_dir().join(format!("napi-vm-test262-fixtures-{}", std::process::id()));
    std::fs::create_dir_all(root.join("nested/deeper")).unwrap();
    std::fs::write(root.join("value.js"), "export const answer=42;").unwrap();
    std::fs::write(
        root.join("nested/deeper/reexport.js"),
        "export {answer} from '../../value.js';",
    )
    .unwrap();
    let report = worker(json!({"id":"nested/test.js", "corpus_root":root,
        "source":"import('./deeper/reexport.js').then(function(m) { if(m.answer!==42) $DONE(new Error('wrong')); else $DONE(); }, $DONE);", "asynchronous":true}));
    assert_eq!(report["status"], "ok", "{report}");
    let outside = root.with_extension("outside.js");
    std::fs::write(&outside, "export default 42;").unwrap();
    let denied = worker(json!({"id":"test.js", "corpus_root":root, "module":true,
        "source":format!("import value from {};", serde_json::to_string(&outside.to_string_lossy()).unwrap())}));
    assert_eq!(denied["status"], "error", "{denied}");
    assert_eq!(denied["phase"], "resolution");
    std::fs::remove_file(outside).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
