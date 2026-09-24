use crate::interpreter::Interpreter;
use crate::value::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[test]
fn macos_node_api_addon_imports_from_the_shim() {
    let Some(addon) = std::env::var_os("NAPI_VM_MACOS_NODE_API_FIXTURE").map(PathBuf::from) else {
        eprintln!("skipping macOS addon load fixture: NAPI_VM_MACOS_NODE_API_FIXTURE is unset");
        return;
    };
    let source = std::fs::read(&addon).unwrap();
    let digest: [u8; 32] = Sha256::digest(&source).into();
    let root = addon
        .parent()
        .expect("fixture must have a parent directory");
    let mut interpreter = Interpreter::with_builtins();
    interpreter
        .enable_rust_node_api_addons(
            super::RustNodeApiOptions::new([root]).allow_native_addon_with_sha256(&addon, digest),
        )
        .unwrap();
    let result = interpreter
            .run_script_source(
                "const explicit = require('./fixture.node'); const omitted = require('./fixture'); [explicit === omitted, omitted.answer]",
            )
            .unwrap();
    let Value::Array(ref result) = result else {
        panic!("expected extension resolution and cache results");
    };
    let result = result.borrow();
    assert!(matches!(result.first(), Some(Value::Bool(true))));
    assert!(matches!(result.get(1), Some(Value::Number(42.0))));
}
