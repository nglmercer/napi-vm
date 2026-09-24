use super::*;
use crate::interpreter::{Interpreter, NativeAddonRuntime};
use sha2::{Digest, Sha256};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn assert_error_fields(value: &Value, name: &str, message: &str, code: Option<&str>) {
    assert!(matches!(
        value.get_prop("name"),
        Some(Value::String(ref actual)) if actual == name
    ));
    assert!(matches!(
        value.get_prop("message"),
        Some(Value::String(ref actual)) if actual == message
    ));
    match code {
        Some(code) => assert!(matches!(
            value.get_prop("code"),
            Some(Value::String(ref actual)) if actual == code
        )),
        None => assert!(matches!(
            value.get_prop("code"),
            None | Some(Value::Undefined)
        )),
    }
}

fn number_array(value: Value) -> Vec<u32> {
    let Value::Array(values) = &value else {
        panic!("expected a numeric array");
    };
    values
        .borrow()
        .iter()
        .map(|value| match value {
            Value::Number(number) => *number as u32,
            _ => panic!("expected a number in array"),
        })
        .collect()
}

#[test]
fn node_api_shim_is_shared_across_host_generations() {
    let first = NodeApiShim::load().expect("load the process Node-API shim");
    let second = NodeApiShim::load().expect("reuse the process Node-API shim");
    assert!(std::sync::Arc::ptr_eq(&first, &second));
    assert_eq!(first.path, second.path);
    #[cfg(unix)]
    assert!(!first.path.exists(), "the mapped shim should be unlinked");
}

include!("cases_01.rs");
include!("cases_02.rs");

#[cfg(all(test, target_os = "windows"))]
#[path = "windows.rs"]
mod windows_tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "macos.rs"]
mod macos_tests;
