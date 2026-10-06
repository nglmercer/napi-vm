#![cfg(feature = "runtime-cli")]
use std::io::Write;
use std::process::{Command, Stdio};

fn cli(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_napi-vm"))
        .args(arguments)
        .output()
        .unwrap()
}
#[test]
fn eval_info_and_invalid_options_have_clear_exit_status() {
    let eval = cli(&["eval", "1 + 2"]);
    assert!(eval.status.success());
    assert_eq!(String::from_utf8(eval.stdout).unwrap().trim(), "3");
    assert!(cli(&["info"]).status.success());
    assert!(!cli(&["eval"]).status.success());
    assert!(!cli(&["eval", "--allow-everything", "1"]).status.success());
    assert!(!cli(&["eval", "--max-time=bad", "1"]).status.success());
    assert!(!cli(&["eval", "napiVm.env('PATH')"]).status.success());
    assert!(
        cli(&["eval", "--allow-env=PATH", "typeof napiVm.env('PATH')"])
            .status
            .success()
    );
    assert!(
        !cli(&["eval", "--max-fuel=10", "var n=0; while(true) n++;"])
            .status
            .success()
    );
}
#[test]
fn repl_keeps_bindings_between_lines() {
    let mut process = Command::new(env!("CARGO_BIN_EXE_napi-vm"))
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    process
        .stdin
        .take()
        .unwrap()
        .write_all(b"var answer = 40\nanswer + 2\n.exit\n")
        .unwrap();
    let output = process.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains("42"));
}
#[cfg(all(feature = "runtime-fs", unix))]
#[test]
fn module_dependencies_require_an_explicit_file_grant() {
    let root = std::env::temp_dir().join(format!("napi-vm-cli-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let entry = root.join("main.js");
    std::fs::write(
        &entry,
        "import { answer } from './value.js'; console.log(answer);",
    )
    .unwrap();
    std::fs::write(root.join("value.js"), "export const answer = 42;").unwrap();
    let denied = cli(&["run", entry.to_str().unwrap()]);
    assert!(!denied.status.success());
    assert!(
        String::from_utf8(denied.stderr)
            .unwrap()
            .contains("PermissionDenied")
    );
    let grant = format!("--allow-read={}", root.display());
    let permitted = cli(&["run", &grant, entry.to_str().unwrap()]);
    assert!(
        permitted.status.success(),
        "{}",
        String::from_utf8_lossy(&permitted.stderr)
    );
    assert_eq!(String::from_utf8(permitted.stdout).unwrap().trim(), "42");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn process_isolation_enforces_supervisor_limits() {
    assert!(
        cli(&[
            "eval",
            "--isolate=process",
            "--max-memory=256M",
            "--max-cpu=2s",
            "1+2"
        ])
        .status
        .success()
    );
    assert!(!cli(&["eval", "--max-memory=0", "1"]).status.success());
    assert!(
        !cli(&[
            "eval",
            "--isolate=process",
            "--max-time=0ms",
            "while(true) {}"
        ])
        .status
        .success()
    );
    assert!(!cli(&["repl", "--isolate=process"]).status.success());
}
