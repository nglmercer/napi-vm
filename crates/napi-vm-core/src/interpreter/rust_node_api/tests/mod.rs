use super::api::{settle_deferred_without_interpreter, to_int32};
use super::*;
use crate::interpreter::{Interpreter, NativeAddonRuntime};
#[cfg(target_os = "linux")]
use libloading::os::unix::RTLD_GLOBAL;
use libloading::os::unix::RTLD_NOW;
use sha2::{Digest, Sha256};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn node_addon_compiler(compiler: &str) -> Command {
    let mut command = Command::new(compiler);
    #[cfg(target_os = "linux")]
    command.arg("-shared");
    // Node-API symbols are supplied by the loading host, not a link-time
    // library. Darwin needs explicit dynamic lookup for both C and C++ addons.
    #[cfg(target_os = "macos")]
    command.args(["-dynamiclib", "-undefined", "dynamic_lookup"]);
    command
}

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
