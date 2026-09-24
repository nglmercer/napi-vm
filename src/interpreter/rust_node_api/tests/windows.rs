use super::{NodeApiShim, validate_native_addon_header};
use crate::interpreter::Interpreter;
use crate::value::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Mutex;

static SHIM_TEST_LOCK: Mutex<()> = Mutex::new(());

fn pe_image(machine: u16, optional_magic: u16, is_dll: bool) -> Vec<u8> {
    let pe_offset = 0x80;
    let mut image = vec![0_u8; pe_offset + 26];
    image[..2].copy_from_slice(b"MZ");
    image[0x3c..0x40].copy_from_slice(&(pe_offset as u32).to_le_bytes());
    image[pe_offset..pe_offset + 4].copy_from_slice(b"PE\0\0");
    image[pe_offset + 4..pe_offset + 6].copy_from_slice(&machine.to_le_bytes());
    let characteristics = if is_dll { 0x2000_u16 } else { 0x0002_u16 };
    image[pe_offset + 22..pe_offset + 24].copy_from_slice(&characteristics.to_le_bytes());
    image[pe_offset + 24..pe_offset + 26].copy_from_slice(&optional_magic.to_le_bytes());
    image
}

#[test]
fn pe_preflight_accepts_only_matching_windows_dlls() {
    let x64 = pe_image(0x8664, 0x020b, true);
    assert!(
        validate_native_addon_header(&x64, x64.len() as u64, "windows", "x86_64", true).is_ok()
    );
    let arm64 = pe_image(0xaa64, 0x020b, true);
    assert!(
        validate_native_addon_header(&arm64, arm64.len() as u64, "windows", "aarch64", true)
            .is_ok()
    );
    let arm32 = pe_image(0x01c4, 0x010b, true);
    assert!(
        validate_native_addon_header(&arm32, arm32.len() as u64, "windows", "arm", true).is_ok()
    );

    let wrong_machine = pe_image(0x014c, 0x010b, true);
    assert!(
        validate_native_addon_header(
            &wrong_machine,
            wrong_machine.len() as u64,
            "windows",
            "x86_64",
            true
        )
        .unwrap_err()
        .contains("does not match host architecture")
    );

    let executable = pe_image(0x8664, 0x020b, false);
    assert!(
        validate_native_addon_header(
            &executable,
            executable.len() as u64,
            "windows",
            "x86_64",
            true
        )
        .unwrap_err()
        .contains("not a DLL")
    );

    let wrong_format = pe_image(0x8664, 0x010b, true);
    assert!(
        validate_native_addon_header(
            &wrong_format,
            wrong_format.len() as u64,
            "windows",
            "x86_64",
            true
        )
        .unwrap_err()
        .contains("optional-header format")
    );
}

#[test]
fn pe_preflight_rejects_malformed_and_foreign_binary_formats() {
    let truncated = b"MZ";
    assert!(
        validate_native_addon_header(truncated, truncated.len() as u64, "windows", "x86_64", true)
            .unwrap_err()
            .contains("DOS header is truncated")
    );

    let elf = b"\x7fELF";
    assert!(
        validate_native_addon_header(elf, elf.len() as u64, "windows", "x86_64", true)
            .unwrap_err()
            .contains("requires PE")
    );
}

#[test]
fn windows_node_api_shim_loads_with_the_node_import_name() {
    let _guard = SHIM_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let shim = NodeApiShim::load().expect("load the Windows Node-API import provider");
    assert_eq!(shim.path.file_name().unwrap(), "node.exe");
}

#[test]
fn windows_node_api_shim_is_shared_for_the_process_lifetime() {
    let _guard = SHIM_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let first = NodeApiShim::load().expect("load the first Node-API import provider");
    let second = NodeApiShim::load().expect("load the second Node-API import provider");
    let first_root = first.path.parent().unwrap().to_path_buf();
    let second_root = second.path.parent().unwrap().to_path_buf();

    assert!(std::sync::Arc::ptr_eq(&first, &second));
    assert_eq!(first_root, second_root);
    drop(first);
    drop(second);
    assert!(
        first_root.exists(),
        "the process provider was removed early"
    );
}

#[test]
fn windows_node_api_addon_imports_from_the_shim() {
    let _guard = SHIM_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(addon) = std::env::var_os("NAPI_VM_WINDOWS_NODE_API_FIXTURE").map(PathBuf::from)
    else {
        eprintln!("skipping Windows addon load fixture: NAPI_VM_WINDOWS_NODE_API_FIXTURE is unset");
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
