use super::*;
use crate::interpreter::{Interpreter, NodeAddonOptions};
use crate::value::TypedKind;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn node_addon_configuration_rejects_entry_outside_roots_before_startup() {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir().join(format!(
        "napi-vm-node-addon-config-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let root = base.join("root");
    let outside = base.join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let entry = outside.join("main.cjs");
    fs::write(&entry, "").unwrap();

    let mut interpreter = Interpreter::with_builtins();
    let error = interpreter
        .enable_node_addons(
            NodeAddonOptions::new("node-executable-must-not-start", [root.clone()]).entry(entry),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("CommonJS entry escapes configured roots")
    );
    assert!(
        interpreter
            .require_commonjs("./main.cjs", None)
            .unwrap_err()
            .to_string()
            .contains("configure a host CommonJS module loader")
    );

    let addon = root.join("fixture.node");
    let valid_entry = root.join("main.cjs");
    fs::write(&addon, "untrusted addon bytes").unwrap();
    fs::write(&valid_entry, "").unwrap();
    let error = interpreter
        .enable_node_addons(
            NodeAddonOptions::new("node-executable-must-not-start", [root.clone()])
                .allow_native_addon_with_sha256(addon, [0; 32])
                .entry(valid_entry),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("integrity check failed while configuring")
    );
    assert!(
        interpreter
            .require_commonjs("./main.cjs", None)
            .unwrap_err()
            .to_string()
            .contains("configure a host CommonJS module loader")
    );

    fs::remove_dir_all(base).unwrap();
}

#[test]
fn node_addon_sidecar_enforces_allowlist_and_digest_on_direct_loads() {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir().join(format!(
        "napi-vm-node-addon-policy-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let root = base.join("root");
    let outside = base.join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();

    let node = ProcessCommand::new("node").arg("--version").output();
    let Ok(node) = node else {
        eprintln!("skipping direct sidecar policy test: Node.js is unavailable");
        fs::remove_dir_all(base).unwrap();
        return;
    };
    if !node.status.success() {
        eprintln!("skipping direct sidecar policy test: node --version failed");
        fs::remove_dir_all(base).unwrap();
        return;
    }

    let pinned_addon = root.join("pinned.node");
    let untrusted_addon = root.join("untrusted.node");
    let invalid_binary = root.join("invalid.node");
    let outside_addon = outside.join("outside.node");
    let original = b"configured addon bytes";
    fs::write(&pinned_addon, original).unwrap();
    fs::write(&untrusted_addon, b"untrusted addon bytes").unwrap();
    fs::write(&invalid_binary, b"not a native library").unwrap();
    fs::write(&outside_addon, b"outside addon bytes").unwrap();
    let digest: [u8; 32] = Sha256::digest(original).into();

    let mut interpreter = Interpreter::with_builtins();
    let sidecar = interpreter
        .enable_node_addons(
            NodeAddonOptions::new("node", [root.clone()])
                .allow_native_addon_with_sha256(&pinned_addon, digest)
                .allow_native_addon(&invalid_binary),
        )
        .unwrap();

    let untrusted_error =
        crate::interpreter::NativeAddonLoader::load(sidecar.as_ref(), &untrusted_addon)
            .unwrap_err();
    assert!(untrusted_error.to_string().contains("not allowlisted"));

    let outside_error =
        crate::interpreter::NativeAddonLoader::load(sidecar.as_ref(), &outside_addon).unwrap_err();
    assert!(
        outside_error
            .to_string()
            .contains("escapes configured roots")
    );

    let wrong_extension = root.join("wrong-extension.so");
    fs::write(&wrong_extension, b"not a .node file").unwrap();
    let extension_error =
        crate::interpreter::NativeAddonLoader::load(sidecar.as_ref(), &wrong_extension)
            .unwrap_err();
    assert!(
        extension_error
            .to_string()
            .contains("must use the .node extension")
    );

    let invalid_binary_error =
        crate::interpreter::NativeAddonLoader::load(sidecar.as_ref(), &invalid_binary).unwrap_err();
    assert!(
        invalid_binary_error
            .to_string()
            .contains("incompatible native addon")
    );

    fs::write(&pinned_addon, b"changed after host configuration").unwrap();
    let integrity_error =
        crate::interpreter::NativeAddonLoader::load(sidecar.as_ref(), &pinned_addon).unwrap_err();
    assert!(
        integrity_error
            .to_string()
            .contains("integrity check failed")
    );

    sidecar.shutdown().unwrap();
    drop(interpreter);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn loads_and_invokes_a_real_node_api_addon() {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-node-addon-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();

    let node = ProcessCommand::new("node").arg("--version").output();
    let cc = ProcessCommand::new("cc").arg("--version").output();
    let mut include_dirs = Vec::new();
    if let Some(include) = std::env::var_os("NODE_INCLUDE_DIR") {
        include_dirs.push(PathBuf::from(include));
    }
    include_dirs.push(PathBuf::from("/usr/include/node"));
    include_dirs.push(PathBuf::from("/usr/local/include/node"));
    let include = include_dirs
        .into_iter()
        .find(|path| path.join("node_api.h").is_file());
    let (Ok(node), Ok(cc), Some(include)) = (node, cc, include) else {
        eprintln!("skipping real Node-API addon test: Node, cc, or Node headers are unavailable");
        let _ = fs::remove_dir_all(&root);
        return;
    };
    assert!(node.status.success(), "node --version failed");
    assert!(cc.status.success(), "cc --version failed");

    fs::write(root.join("main.cjs"), "").unwrap();
    fs::write(
        root.join("package.json"),
        r##"{"name":"fixture-runtime","imports":{"#native":"fixture"}}"##,
    )
    .unwrap();
    let source = root.join("fixture.c");
    let package_root = root.join("node_modules/fixture");
    let package_build = package_root.join("build/Release");
    fs::create_dir_all(&package_build).unwrap();
    fs::write(
            package_root.join("package.json"),
            r#"{"exports":{".":{"node-addons":"./build/Release/fixture.node","require":"./build/Release/fixture.node","default":"./build/Release/fixture.node"}}}"#,
        )
        .unwrap();
    let wrapper_root = root.join("node_modules/fixture-wrapper");
    fs::create_dir_all(&wrapper_root).unwrap();
    fs::write(
        wrapper_root.join("package.json"),
        r#"{"main":"./index.cjs"}"#,
    )
    .unwrap();
    fs::write(
        wrapper_root.join("index.cjs"),
        "module.exports = require('fixture');",
    )
    .unwrap();
    let addon = package_build.join("fixture.node");
    let direct_addon = root.join("fixture.node");
    std::os::unix::fs::symlink(&addon, &direct_addon).unwrap();
    fs::write(
            &source,
            r#"
#include <node_api.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static napi_value add(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2];
  double left = 0, right = 0;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2) return NULL;
  if (napi_get_value_double(env, argv[0], &left) != napi_ok) return NULL;
  if (napi_get_value_double(env, argv[1], &right) != napi_ok) return NULL;
  napi_value result;
  if (napi_create_double(env, left + right, &result) != napi_ok) return NULL;
  return result;
}

static napi_value echo(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1) return NULL;
  return argv[0];
}

static napi_value big(napi_env env, napi_callback_info info) {
  napi_value result;
  if (napi_create_bigint_int64(env, 9007199254740993LL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value date_value(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  double milliseconds;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_date_value(env, argv[0], &milliseconds) != napi_ok ||
      napi_create_double(env, milliseconds, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_date(napi_env env, napi_callback_info info) {
  napi_value result;
  if (napi_create_date(env, 123456.5, &result) != napi_ok) return NULL;
  return result;
}

static napi_value regex_source(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "source", &result) != napi_ok) return NULL;
  return result;
}

static napi_value regex_flags(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "flags", &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_regex(napi_env env, napi_callback_info info) {
  napi_value global, constructor, args[2], result;
  if (napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "RegExp", &constructor) != napi_ok ||
      napi_create_string_utf8(env, "a+", NAPI_AUTO_LENGTH, &args[0]) != napi_ok ||
      napi_create_string_utf8(env, "gi", NAPI_AUTO_LENGTH, &args[1]) != napi_ok ||
      napi_new_instance(env, constructor, 2, args, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_symbol(napi_env env, napi_callback_info info) {
  napi_value description, result;
  if (napi_create_string_utf8(env, "native", NAPI_AUTO_LENGTH, &description) != napi_ok ||
      napi_create_symbol(env, description, &result) != napi_ok) return NULL;
  return result;
}

static napi_value identity(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1) return NULL;
  return argv[0];
}

static napi_value is_symbol(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  napi_valuetype type;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_typeof(env, argv[0], &type) != napi_ok ||
      napi_create_int32(env, type == napi_symbol ? 1 : 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value is_node_iterator(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], global, symbol_constructor, iterator, result;
  bool equal = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Symbol", &symbol_constructor) != napi_ok ||
      napi_get_named_property(env, symbol_constructor, "iterator", &iterator) != napi_ok ||
      napi_strict_equals(env, argv[0], iterator, &equal) != napi_ok ||
      napi_create_int32(env, equal ? 1 : 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value same_object(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  bool equal = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_strict_equals(env, argv[0], argv[1], &equal) != napi_ok ||
      napi_create_int32(env, equal ? 1 : 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value set_prototype(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], global, object, setter, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Object", &object) != napi_ok ||
      napi_get_named_property(env, object, "setPrototypeOf", &setter) != napi_ok ||
      napi_call_function(env, object, setter, argc, argv, &result) != napi_ok) return NULL;
  return result;
}

static napi_value set_default_prototype(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], global, object, prototype, setter, args[2], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Object", &object) != napi_ok ||
      napi_get_named_property(env, object, "prototype", &prototype) != napi_ok ||
      napi_get_named_property(env, object, "setPrototypeOf", &setter) != napi_ok) return NULL;
  args[0] = argv[0];
  args[1] = prototype;
  if (napi_call_function(env, object, setter, 2, args, &result) != napi_ok) return NULL;
  return result;
}

static napi_value prototype_matches(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], prototype, result;
  bool equal = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_prototype(env, argv[0], &prototype) != napi_ok ||
      napi_strict_equals(env, prototype, argv[1], &equal) != napi_ok ||
      napi_get_boolean(env, equal, &result) != napi_ok) return NULL;
  return result;
}

static napi_value prototype_is_null(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], prototype, result;
  napi_valuetype type;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_prototype(env, argv[0], &prototype) != napi_ok ||
      napi_typeof(env, prototype, &type) != napi_ok ||
      napi_get_boolean(env, type == napi_null, &result) != napi_ok) return NULL;
  return result;
}

static napi_value prototype_is_default(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], global, object, expected, prototype, result;
  bool equal = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Object", &object) != napi_ok ||
      napi_get_named_property(env, object, "prototype", &expected) != napi_ok ||
      napi_get_prototype(env, argv[0], &prototype) != napi_ok ||
      napi_strict_equals(env, prototype, expected, &equal) != napi_ok ||
      napi_get_boolean(env, equal, &result) != napi_ok) return NULL;
  return result;
}

static napi_value read_property(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "value", &result) != napi_ok) return NULL;
  return result;
}

static napi_value call_inherited(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], method, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "read", &method) != napi_ok ||
      napi_call_function(env, argv[0], method, 0, NULL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value call_to_string(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], method, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "toString", &method) != napi_ok ||
      napi_call_function(env, argv[0], method, 0, NULL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value construct_and_read(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], instance, method, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_new_instance(env, argv[0], 1, &argv[1], &instance) != napi_ok ||
      napi_get_named_property(env, instance, "read", &method) != napi_ok ||
      napi_call_function(env, instance, method, 0, NULL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value write_property(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_set_named_property(env, argv[0], "value", argv[1]) != napi_ok) return NULL;
  return argv[1];
}

static napi_value write_then_read_property(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_set_named_property(env, argv[0], "value", argv[1]) != napi_ok ||
      napi_get_named_property(env, argv[0], "value", &result) != napi_ok) return NULL;
  return result;
}

static napi_value read_symbol_property(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_property(env, argv[0], argv[1], &result) != napi_ok) return NULL;
  return result;
}

static napi_value write_symbol_property(napi_env env, napi_callback_info info) {
  size_t argc = 3;
  napi_value argv[3];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 3 ||
      napi_set_property(env, argv[0], argv[1], argv[2]) != napi_ok) return NULL;
  return argv[2];
}

static napi_value delete_symbol_property(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  bool deleted = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_delete_property(env, argv[0], argv[1], &deleted) != napi_ok ||
      napi_get_boolean(env, deleted, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_symbol_object(napi_env env, napi_callback_info info) {
  napi_value object, description, key, value;
  if (napi_create_object(env, &object) != napi_ok ||
      napi_create_string_utf8(env, "native-key", NAPI_AUTO_LENGTH, &description) != napi_ok ||
      napi_create_symbol(env, description, &key) != napi_ok ||
      napi_create_int32(env, 89, &value) != napi_ok ||
      napi_set_property(env, object, key, value) != napi_ok ||
      napi_set_named_property(env, object, "key", key) != napi_ok) return NULL;
  return object;
}

static napi_value mutate_object(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], value, child, key;
  bool deleted = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_create_int32(env, 73, &value) != napi_ok ||
      napi_set_named_property(env, argv[0], "changed", value) != napi_ok ||
      napi_get_named_property(env, argv[0], "child", &child) != napi_ok ||
      napi_create_int32(env, 91, &value) != napi_ok ||
      napi_set_named_property(env, child, "value", value) != napi_ok ||
      napi_create_string_utf8(env, "removeMe", NAPI_AUTO_LENGTH, &key) != napi_ok ||
      napi_delete_property(env, argv[0], key, &deleted) != napi_ok || !deleted) return NULL;
  return argv[0];
}

static napi_value mutate_array(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], value;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_create_int32(env, 42, &value) != napi_ok ||
      napi_set_element(env, argv[0], 1, value) != napi_ok ||
      napi_create_int32(env, 84, &value) != napi_ok ||
      napi_set_element(env, argv[0], 2, value) != napi_ok ||
      napi_create_string_utf8(env, "native", NAPI_AUTO_LENGTH, &value) != napi_ok ||
      napi_set_named_property(env, argv[0], "tag", value) != napi_ok) return NULL;
  return argv[0];
}

static napi_value make_sparse_array(napi_env env, napi_callback_info info) {
  napi_value array, value;
  if (napi_create_array_with_length(env, 4, &array) != napi_ok ||
      napi_create_int32(env, 17, &value) != napi_ok ||
      napi_set_element(env, array, 1, value) != napi_ok ||
      napi_get_undefined(env, &value) != napi_ok ||
      napi_set_element(env, array, 3, value) != napi_ok) return NULL;
  return array;
}

static napi_value array_has_element(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  uint32_t index;
  bool has = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_value_uint32(env, argv[1], &index) != napi_ok ||
      napi_has_element(env, argv[0], index, &has) != napi_ok ||
      napi_get_boolean(env, has, &result) != napi_ok) return NULL;
  return result;
}

static napi_value delete_array_element(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  uint32_t index;
  bool deleted = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_value_uint32(env, argv[1], &index) != napi_ok ||
      napi_delete_element(env, argv[0], index, &deleted) != napi_ok ||
      napi_get_boolean(env, deleted, &result) != napi_ok) return NULL;
  return result;
}

static napi_value mutate_after_callback(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], receiver, value;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_global(env, &receiver) != napi_ok ||
      napi_call_function(env, receiver, argv[1], 0, NULL, NULL) != napi_ok ||
      napi_create_int32(env, 27, &value) != napi_ok ||
      napi_set_named_property(env, argv[0], "native", value) != napi_ok) return NULL;
  return argv[0];
}

static napi_value assign_and_return(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result, value;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, 15, &value) != napi_ok ||
      napi_set_named_property(env, result, "value", value) != napi_ok ||
      napi_set_named_property(env, argv[0], "created", result) != napi_ok) return NULL;
  return result;
}

static napi_value make_shared_array(napi_env env, napi_callback_info info) {
  napi_value outer, inner, value;
  if (napi_create_array_with_length(env, 2, &outer) != napi_ok ||
      napi_create_array_with_length(env, 1, &inner) != napi_ok ||
      napi_create_int32(env, 7, &value) != napi_ok ||
      napi_set_element(env, inner, 0, value) != napi_ok ||
      napi_set_element(env, outer, 0, inner) != napi_ok ||
      napi_set_element(env, outer, 1, inner) != napi_ok) return NULL;
  return outer;
}

static napi_value make_cycle(napi_env env, napi_callback_info info) {
  napi_value object, child, value;
  if (napi_create_object(env, &object) != napi_ok ||
      napi_create_object(env, &child) != napi_ok ||
      napi_create_int32(env, 1, &value) != napi_ok ||
      napi_set_named_property(env, child, "value", value) != napi_ok ||
      napi_set_named_property(env, object, "self", object) != napi_ok ||
      napi_set_named_property(env, object, "child", child) != napi_ok) return NULL;
  return object;
}

static napi_value make_cyclic_array(napi_env env, napi_callback_info info) {
  napi_value array;
  if (napi_create_array_with_length(env, 1, &array) != napi_ok ||
      napi_set_element(env, array, 0, array) != napi_ok) return NULL;
  return array;
}

static napi_value make_buffer(napi_env env, napi_callback_info info) {
  const char bytes[] = {'a', 'b', 'c'};
  napi_value result;
  if (napi_create_buffer_copy(env, sizeof(bytes), bytes, NULL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value is_buffer(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  bool is_buffer = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_is_buffer(env, argv[0], &is_buffer) != napi_ok ||
      napi_create_int32(env, is_buffer ? 1 : 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value typed_array_length(napi_env env, napi_callback_info info) {
  size_t argc = 1, length = 0;
  napi_value argv[1], result;
  napi_typedarray_type kind;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_typedarray_info(env, argv[0], &kind, &length, NULL, NULL, NULL) != napi_ok ||
      napi_create_int32(env, kind == napi_uint16_array ? (int32_t)length : -1, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_typed_array(napi_env env, napi_callback_info info) {
  napi_value buffer, result;
  void *data = NULL;
  if (napi_create_arraybuffer(env, 4, &data, &buffer) != napi_ok) return NULL;
  uint16_t *items = (uint16_t *)data;
  items[0] = 300;
  items[1] = 400;
  if (napi_create_typedarray(env, napi_uint16_array, 2, buffer, 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_array_buffer(napi_env env, napi_callback_info info) {
  napi_value result;
  void *data = NULL;
  if (napi_create_arraybuffer(env, 3, &data, &result) != napi_ok) return NULL;
  memcpy(data, "xyz", 3);
  return result;
}

static napi_value array_buffer_length(napi_env env, napi_callback_info info) {
  size_t argc = 1, length = 0;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_arraybuffer_info(env, argv[0], NULL, &length) != napi_ok ||
      napi_create_int32(env, (int32_t)length, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_data_view(napi_env env, napi_callback_info info) {
  napi_value buffer, result;
  void *data = NULL;
  if (napi_create_arraybuffer(env, 2, &data, &buffer) != napi_ok) return NULL;
  ((uint8_t *)data)[0] = 17;
  ((uint8_t *)data)[1] = 29;
  if (napi_create_dataview(env, 2, buffer, 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value data_view_byte(napi_env env, napi_callback_info info) {
  size_t argc = 1, length = 0, offset = 0;
  napi_value argv[1], result;
  void *data = NULL;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_dataview_info(env, argv[0], &length, &data, NULL, &offset) != napi_ok || length < 2 ||
      napi_create_int32(env, ((uint8_t *)data)[1], &result) != napi_ok) return NULL;
  return result;
}

static napi_value fail(napi_env env, napi_callback_info info) {
  napi_throw_type_error(env, "E_FIXTURE", "fixture failure");
  return NULL;
}

static napi_value mutate_then_throw(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], value;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_create_int32(env, 88, &value) != napi_ok ||
      napi_set_named_property(env, argv[0], "afterThrow", value) != napi_ok ||
      napi_throw_type_error(env, "E_AFTER_MUTATION", "mutation happened") != napi_ok) return NULL;
  return NULL;
}

static napi_value promise_result(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, result;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok) return NULL;
  if (napi_create_string_utf8(env, "native-promise-value", NAPI_AUTO_LENGTH, &result) != napi_ok) return NULL;
  if (napi_resolve_deferred(env, deferred, result) != napi_ok) return NULL;
  return promise;
}

static napi_value promise_reject(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, reason;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok) return NULL;
  if (napi_create_string_utf8(env, "native-rejection", NAPI_AUTO_LENGTH, &reason) != napi_ok) return NULL;
  if (napi_reject_deferred(env, deferred, reason) != napi_ok) return NULL;
  return promise;
}

static napi_value promise_reject_error(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, message, reason, code;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok ||
      napi_create_string_utf8(env, "native error rejection", NAPI_AUTO_LENGTH, &message) != napi_ok ||
      napi_create_error(env, NULL, message, &reason) != napi_ok ||
      napi_create_string_utf8(env, "E_NATIVE_REJECTION", NAPI_AUTO_LENGTH, &code) != napi_ok ||
      napi_set_named_property(env, reason, "code", code) != napi_ok ||
      napi_reject_deferred(env, deferred, reason) != napi_ok) return NULL;
  return promise;
}

static napi_value promise_pending(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok) return NULL;
  return promise;
}

static napi_value counter_constructor(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], self;
  if (napi_get_cb_info(env, info, &argc, argv, &self, NULL) != napi_ok) return NULL;
  napi_value initial;
  if (argc > 0) initial = argv[0];
  else if (napi_create_double(env, 0, &initial) != napi_ok) return NULL;
  if (napi_set_named_property(env, self, "count", initial) != napi_ok) return NULL;
  return self;
}

static napi_value counter_increment(napi_env env, napi_callback_info info) {
  napi_value self, value, next;
  if (napi_get_cb_info(env, info, NULL, NULL, &self, NULL) != napi_ok) return NULL;
  double count = 0;
  if (napi_get_named_property(env, self, "count", &value) != napi_ok) return NULL;
  if (napi_get_value_double(env, value, &count) != napi_ok) return NULL;
  if (napi_create_double(env, count + 1, &next) != napi_ok) return NULL;
  if (napi_set_named_property(env, self, "count", next) != napi_ok) return NULL;
  return next;
}

static napi_value counter_self(napi_env env, napi_callback_info info) {
  napi_value self;
  if (napi_get_cb_info(env, info, NULL, NULL, &self, NULL) != napi_ok) return NULL;
  return self;
}

typedef struct {
  napi_async_work work;
  napi_ref callback;
  napi_ref value;
} callback_work;

static void execute_callback_work(napi_env env, void *data) {
  usleep(50000);
}

static void complete_callback_work(napi_env env, napi_status status, void *data) {
  callback_work *work = (callback_work *)data;
  napi_value callback, receiver, value;
  napi_status value_status = work->value != NULL
      ? napi_get_reference_value(env, work->value, &value)
      : napi_create_string_utf8(env, "async-value", NAPI_AUTO_LENGTH, &value);
  if (napi_get_reference_value(env, work->callback, &callback) == napi_ok &&
      napi_get_global(env, &receiver) == napi_ok && value_status == napi_ok) {
    napi_call_function(env, receiver, callback, 1, &value, NULL);
  }
  napi_delete_reference(env, work->callback);
  if (work->value != NULL) napi_delete_reference(env, work->value);
  napi_delete_async_work(env, work->work);
  free(work);
}

static napi_value on_later(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], resource_name;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc < 1) return NULL;
  callback_work *work = (callback_work *)calloc(1, sizeof(callback_work));
  if (!work) return NULL;
  if (napi_create_reference(env, argv[0], 1, &work->callback) != napi_ok) { free(work); return NULL; }
  if (argc == 2 && napi_create_reference(env, argv[1], 1, &work->value) != napi_ok) {
    napi_delete_reference(env, work->callback);
    free(work);
    return NULL;
  }
  if (napi_create_string_utf8(env, "fixture callback", NAPI_AUTO_LENGTH, &resource_name) != napi_ok ||
      napi_create_async_work(env, NULL, resource_name, execute_callback_work,
                             complete_callback_work, work, &work->work) != napi_ok ||
      napi_queue_async_work(env, work->work) != napi_ok) {
    napi_delete_reference(env, work->callback);
    if (work->value != NULL) napi_delete_reference(env, work->value);
    free(work);
    return NULL;
  }
  napi_value result;
  napi_get_undefined(env, &result);
  return result;
}

typedef struct {
  napi_threadsafe_function function;
} threadsafe_work;

static void call_threadsafe_js(napi_env env, napi_value callback,
                               void *context, void *data) {
  (void)context;
  char *message = (char *)data;
  if (env != NULL && callback != NULL && message != NULL) {
    napi_value receiver, argument;
    if (napi_get_global(env, &receiver) == napi_ok &&
        napi_create_string_utf8(env, message, NAPI_AUTO_LENGTH, &argument) == napi_ok) {
      napi_call_function(env, receiver, callback, 1, &argument, NULL);
    }
  }
  free(message);
}

static void *threadsafe_thread_main(void *data) {
  threadsafe_work *work = (threadsafe_work *)data;
  char *message = strdup("threadsafe-value");
  if (message == NULL ||
      napi_call_threadsafe_function(work->function, message, napi_tsfn_nonblocking) != napi_ok) {
    free(message);
  }
  napi_release_threadsafe_function(work->function, napi_tsfn_release);
  free(work);
  return NULL;
}

static napi_value on_threadsafe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], resource_name, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_create_string_utf8(env, "fixture threadsafe callback", NAPI_AUTO_LENGTH,
                              &resource_name) != napi_ok ||
      napi_get_undefined(env, &result) != napi_ok) return NULL;
  threadsafe_work *work = (threadsafe_work *)calloc(1, sizeof(threadsafe_work));
  if (work == NULL) return NULL;
  if (napi_create_threadsafe_function(env, argv[0], NULL, resource_name,
                                      1, 1, NULL, NULL, NULL,
                                      call_threadsafe_js, &work->function) != napi_ok) {
    free(work);
    return NULL;
  }
  pthread_t thread;
  if (pthread_create(&thread, NULL, threadsafe_thread_main, work) != 0) {
    napi_release_threadsafe_function(work->function, napi_tsfn_abort);
    free(work);
    return NULL;
  }
  pthread_detach(thread);
  return result;
}

static napi_value on_sync(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], receiver, value, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1) return NULL;
  if (napi_get_global(env, &receiver) != napi_ok ||
      napi_create_string_utf8(env, "sync-value", NAPI_AUTO_LENGTH, &value) != napi_ok) return NULL;
  if (napi_call_function(env, receiver, argv[0], 1, &value, &result) != napi_ok) return NULL;
  return result;
}

static napi_value init(napi_env env, napi_value exports) {
  napi_value fn;
  if (napi_create_function(env, "add", NAPI_AUTO_LENGTH, add, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "add", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "echo", NAPI_AUTO_LENGTH, echo, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "echo", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "big", NAPI_AUTO_LENGTH, big, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "big", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "dateValue", NAPI_AUTO_LENGTH, date_value, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "dateValue", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeDate", NAPI_AUTO_LENGTH, make_date, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeDate", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "regexSource", NAPI_AUTO_LENGTH, regex_source, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "regexSource", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "regexFlags", NAPI_AUTO_LENGTH, regex_flags, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "regexFlags", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeRegex", NAPI_AUTO_LENGTH, make_regex, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeRegex", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeSymbol", NAPI_AUTO_LENGTH, make_symbol, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeSymbol", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "identity", NAPI_AUTO_LENGTH, identity, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "identity", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "isSymbol", NAPI_AUTO_LENGTH, is_symbol, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "isSymbol", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "isNodeIterator", NAPI_AUTO_LENGTH, is_node_iterator, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "isNodeIterator", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "sameObject", NAPI_AUTO_LENGTH, same_object, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "sameObject", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "setPrototype", NAPI_AUTO_LENGTH, set_prototype, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "setPrototype", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "setDefaultPrototype", NAPI_AUTO_LENGTH, set_default_prototype, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "setDefaultPrototype", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "prototypeMatches", NAPI_AUTO_LENGTH, prototype_matches, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "prototypeMatches", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "prototypeIsNull", NAPI_AUTO_LENGTH, prototype_is_null, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "prototypeIsNull", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "prototypeIsDefault", NAPI_AUTO_LENGTH, prototype_is_default, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "prototypeIsDefault", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "readProperty", NAPI_AUTO_LENGTH, read_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "readProperty", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "callInherited", NAPI_AUTO_LENGTH, call_inherited, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "callInherited", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "callToString", NAPI_AUTO_LENGTH, call_to_string, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "callToString", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "constructAndRead", NAPI_AUTO_LENGTH, construct_and_read, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "constructAndRead", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "writeProperty", NAPI_AUTO_LENGTH, write_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "writeProperty", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "writeThenRead", NAPI_AUTO_LENGTH, write_then_read_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "writeThenRead", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "readSymbolProperty", NAPI_AUTO_LENGTH, read_symbol_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "readSymbolProperty", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "writeSymbolProperty", NAPI_AUTO_LENGTH, write_symbol_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "writeSymbolProperty", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "deleteSymbolProperty", NAPI_AUTO_LENGTH, delete_symbol_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "deleteSymbolProperty", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeSymbolObject", NAPI_AUTO_LENGTH, make_symbol_object, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeSymbolObject", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "mutateObject", NAPI_AUTO_LENGTH, mutate_object, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "mutateObject", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "mutateArray", NAPI_AUTO_LENGTH, mutate_array, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "mutateArray", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeSparseArray", NAPI_AUTO_LENGTH, make_sparse_array, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeSparseArray", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "arrayHasElement", NAPI_AUTO_LENGTH, array_has_element, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "arrayHasElement", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "deleteArrayElement", NAPI_AUTO_LENGTH, delete_array_element, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "deleteArrayElement", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "mutateAfterCallback", NAPI_AUTO_LENGTH, mutate_after_callback, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "mutateAfterCallback", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "assignAndReturn", NAPI_AUTO_LENGTH, assign_and_return, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "assignAndReturn", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeSharedArray", NAPI_AUTO_LENGTH, make_shared_array, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeSharedArray", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeCycle", NAPI_AUTO_LENGTH, make_cycle, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeCycle", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeCyclicArray", NAPI_AUTO_LENGTH, make_cyclic_array, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeCyclicArray", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeBuffer", NAPI_AUTO_LENGTH, make_buffer, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeBuffer", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "isBuffer", NAPI_AUTO_LENGTH, is_buffer, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "isBuffer", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "typedArrayLength", NAPI_AUTO_LENGTH, typed_array_length, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "typedArrayLength", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeTypedArray", NAPI_AUTO_LENGTH, make_typed_array, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeTypedArray", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeArrayBuffer", NAPI_AUTO_LENGTH, make_array_buffer, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeArrayBuffer", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "arrayBufferLength", NAPI_AUTO_LENGTH, array_buffer_length, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "arrayBufferLength", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeDataView", NAPI_AUTO_LENGTH, make_data_view, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeDataView", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "dataViewByte", NAPI_AUTO_LENGTH, data_view_byte, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "dataViewByte", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "fail", NAPI_AUTO_LENGTH, fail, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "fail", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "mutateThenThrow", NAPI_AUTO_LENGTH, mutate_then_throw, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "mutateThenThrow", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseResult", NAPI_AUTO_LENGTH, promise_result, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseResult", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseReject", NAPI_AUTO_LENGTH, promise_reject, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseReject", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseRejectError", NAPI_AUTO_LENGTH, promise_reject_error, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseRejectError", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promisePending", NAPI_AUTO_LENGTH, promise_pending, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promisePending", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "onLater", NAPI_AUTO_LENGTH, on_later, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "onLater", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "onThreadsafe", NAPI_AUTO_LENGTH, on_threadsafe, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "onThreadsafe", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "onSync", NAPI_AUTO_LENGTH, on_sync, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "onSync", fn) != napi_ok) return NULL;
  napi_property_descriptor counter_methods[] = {
    {"increment", NULL, counter_increment, NULL, NULL, NULL, napi_default, NULL},
    {"self", NULL, counter_self, NULL, NULL, NULL, napi_default, NULL},
  };
  napi_value counter;
  if (napi_define_class(env, "Counter", NAPI_AUTO_LENGTH, counter_constructor, NULL,
                        2, counter_methods, &counter) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "Counter", counter) != napi_ok) return NULL;
  return exports;
}

NAPI_MODULE(NODE_GYP_MODULE_NAME, init)
"#,
        )
        .unwrap();
    let compile = ProcessCommand::new("cc")
        .arg("-shared")
        .arg("-fPIC")
        .arg("-pthread")
        .arg("-DNODE_GYP_MODULE_NAME=fixture")
        .arg(format!("-I{}", include.display()))
        .arg(&source)
        .arg("-o")
        .arg(&addon)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "could not compile Node-API fixture: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let sparse_fixture = "const addon = require('./fixture.node'); const sparse = addon.makeSparseArray(); const initial = {hole: Object.hasOwn(sparse, 0), value: Object.hasOwn(sparse, 1), explicitUndefined: Object.hasOwn(sparse, 3), nativeHole: addon.arrayHasElement(sparse, 0), nativeValue: addon.arrayHasElement(sparse, 1)}; sparse[0] = 5; delete sparse[1]; const guestDeleted = !Object.hasOwn(sparse, 1); const guestDeleteReachedNative = !addon.arrayHasElement(sparse, 1); const hostDeleteResult = addon.deleteArrayElement(sparse, 3); const hostDeleteVisible = !Object.hasOwn(sparse, 3); addon.mutateArray(sparse); const mapped = sparse.map(value => value); ({length: sparse.length, initialHole: initial.hole, initialValue: initial.value, initialExplicitUndefined: initial.explicitUndefined, initialNativeHole: initial.nativeHole, initialNativeValue: initial.nativeValue, written: Object.hasOwn(sparse, 0), guestDeleted, guestDeleteReachedNative, hostDeleteResult, hostDeleteVisible, nativeDeleted: !addon.arrayHasElement(sparse, 3), restored: Object.hasOwn(sparse, 1), nativeRestored: addon.arrayHasElement(sparse, 1), keys: Object.keys(sparse).join(','), mappedLength: mapped.length, mappedIndex0: Object.hasOwn(mapped, 0), mappedIndex1: Object.hasOwn(mapped, 1), mappedIndex2: Object.hasOwn(mapped, 2), mappedIndex3: Object.hasOwn(mapped, 3)});";
    let sparse_reference = ProcessCommand::new("node")
        .arg("-e")
        .arg(format!(
            "process.stdout.write(JSON.stringify(eval({})))",
            serde_json::to_string(sparse_fixture).unwrap()
        ))
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        sparse_reference.status.success(),
        "Node sparse array reference failed: {}",
        String::from_utf8_lossy(&sparse_reference.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&sparse_reference.stdout),
        r#"{"length":4,"initialHole":false,"initialValue":true,"initialExplicitUndefined":true,"initialNativeHole":false,"initialNativeValue":true,"written":true,"guestDeleted":true,"guestDeleteReachedNative":true,"hostDeleteResult":true,"hostDeleteVisible":true,"nativeDeleted":true,"restored":true,"nativeRestored":true,"keys":"0,1,2,tag","mappedLength":4,"mappedIndex0":true,"mappedIndex1":true,"mappedIndex2":true,"mappedIndex3":false}"#
    );

    let node_reference = ProcessCommand::new("node")
            .arg("-e")
            .arg("const {createRequire}=require('node:module');const req=createRequire(process.argv[1]);const a=req('fixture');const b=req('#native');const c=req('./fixture.node');const d=req('fixture-wrapper');const target={value:40};let ownKeysCalls=0;const proxy=new Proxy(target,{get:(t,k)=>k==='value'?t.value+2:Reflect.get(t,k),set:(t,k,v)=>{t[k]=v;return true},ownKeys:()=>{ownKeysCalls++;return ['value']}});const proxyBefore=a.readProperty(proxy);a.writeProperty(proxy,9);const simpleTarget={value:1};const setOnlyProxy=new Proxy(simpleTarget,{set:(t,k,v)=>{t[k]=v;return true}});const proxyWriteRead=a.writeThenRead(setOnlyProxy,17);const inheritedValue=a.readProperty(Object.create({value:29}));const inheritedObject=Object.create({read(){return this.value+3}});inheritedObject.value=40;const inheritedCall=a.callInherited(inheritedObject);const prototypeTarget={value:40};const customPrototype={read(){return this.value+3}};const initialDefault=a.prototypeIsDefault(prototypeTarget);a.setPrototype(prototypeTarget,customPrototype);const customPrototypeMatch=a.prototypeMatches(prototypeTarget,customPrototype);const inheritedPrototypeValue=a.callInherited(prototypeTarget);a.setPrototype(prototypeTarget,null);const nullPrototype=a.prototypeIsNull(prototypeTarget);a.setDefaultPrototype(prototypeTarget);const restoredDefault=a.prototypeIsDefault(prototypeTarget);const guestErrorText=a.callToString(new TypeError('bridge'));const nested=a.onSync(value=>a.onSync(inner=>a.add(19,23)));process.stdout.write(JSON.stringify({sum:a.add(19,23),same:a===c,importSame:a===b,wrapperSame:a===d,proxyBefore,proxyAfter:a.readProperty(proxy),targetValue:target.value,ownKeysCalls,proxyWriteRead,inheritedValue,inheritedCall,initialDefault,customPrototypeMatch,inheritedPrototypeValue,nullPrototype,restoredDefault,guestErrorText,nested}));")
            .arg(root.join("main.cjs"))
            .output()
            .unwrap();
    assert!(
        node_reference.status.success(),
        "Node reference could not load the native addon through package exports and imports: {}",
        String::from_utf8_lossy(&node_reference.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&node_reference.stdout),
        r#"{"sum":42,"same":true,"importSame":true,"wrapperSame":true,"proxyBefore":42,"proxyAfter":11,"targetValue":9,"ownKeysCalls":0,"proxyWriteRead":17,"inheritedValue":29,"inheritedCall":43,"initialDefault":true,"customPrototypeMatch":true,"inheritedPrototypeValue":43,"nullPrototype":true,"restoredDefault":true,"guestErrorText":"TypeError: bridge","nested":42}"#
    );

    let class_reference = ProcessCommand::new("node")
            .arg("-e")
            .arg("const {createRequire}=require('node:module');const req=createRequire(process.argv[1]);const addon=req('./fixture.node');class Box{static value=2;constructor(value){this.value=value}read(){return this.value+Box.value}}const direct=addon.constructAndRead(Box,40);const proxiedBox=new Proxy(Box,{construct:(target,args)=>Reflect.construct(target,args)});const proxied=addon.constructAndRead(proxiedBox,40);const classStatic=addon.readProperty(Box);const classWriteRead=addon.writeThenRead(Box,23);process.stdout.write(JSON.stringify({direct,proxied,classStatic,classWriteRead,classValue:Box.value}));")
            .arg(root.join("main.cjs"))
            .output()
            .unwrap();
    assert!(
        class_reference.status.success(),
        "Node reference could not construct the guest class fixture: {}",
        String::from_utf8_lossy(&class_reference.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&class_reference.stdout),
        r#"{"direct":42,"proxied":42,"classStatic":2,"classWriteRead":23,"classValue":23}"#
    );

    let mut interpreter = Interpreter::with_builtins();
    let expected_sha256: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();
    let runtime = interpreter
        .enable_native_addons(
            NodeAddonOptions::new("node", [root.clone()])
                .allow_native_addon_with_sha256(addon.clone(), expected_sha256)
                .minimum_napi_version(1)
                .entry(root.join("main.cjs")),
        )
        .unwrap();
    assert_eq!(runtime.backend_name(), "node-sidecar");
    runtime.preflight_addon(&addon).unwrap();
    let bridge = runtime.node_sidecar().expect("Node sidecar backend");
    assert!(!bridge.runtime_info().node_version.is_empty());
    assert!(bridge.runtime_info().napi_version >= 1);
    let mut incompatible_interpreter = Interpreter::with_builtins();
    let version_error = incompatible_interpreter
        .enable_node_addons(
            NodeAddonOptions::new("node", [root.clone()])
                .allow_native_addon_with_sha256(addon.clone(), expected_sha256)
                .minimum_napi_version(u32::MAX),
        )
        .unwrap_err();
    assert!(version_error.to_string().contains("Node-API v4294967295"));
    let package_result = interpreter
            .eval_source(
                "const packageAddon = require('fixture'); const importAddon = require('#native'); const wrapperAddon = require('fixture-wrapper'); const target = {value:40}; let ownKeysCalls=0; const proxy = new Proxy(target, {get:(t,k)=>k==='value'?t.value+2:Reflect.get(t,k),set:(t,k,v)=>{t[k]=v;return true},ownKeys:()=>{ownKeysCalls++;return ['value']}}); const proxyBefore = packageAddon.readProperty(proxy); packageAddon.writeProperty(proxy,9); const simpleTarget={value:1}; const setOnlyProxy=new Proxy(simpleTarget,{set:(t,k,v)=>{t[k]=v;return true}}); const proxyWriteRead=packageAddon.writeThenRead(setOnlyProxy,17); const inheritedValue=packageAddon.readProperty(Object.create({value:29})); const inheritedObject=Object.create({read:function(){return this.value+3}}); inheritedObject.value=40; const inheritedCall=packageAddon.callInherited(inheritedObject); const prototypeTarget={value:40}; const customPrototype={read:function(){return this.value+3}}; const initialDefault=packageAddon.prototypeIsDefault(prototypeTarget); packageAddon.setPrototype(prototypeTarget,customPrototype); const customPrototypeMatch=packageAddon.prototypeMatches(prototypeTarget,customPrototype); const inheritedPrototypeValue=packageAddon.callInherited(prototypeTarget); packageAddon.setPrototype(prototypeTarget,null); const nullPrototype=packageAddon.prototypeIsNull(prototypeTarget); packageAddon.setDefaultPrototype(prototypeTarget); const restoredDefault=packageAddon.prototypeIsDefault(prototypeTarget); const guestErrorText=packageAddon.callToString(new TypeError('bridge')); ({sum: packageAddon.add(19, 23), same: packageAddon === require('./fixture.node'), importSame: packageAddon === importAddon, wrapperSame: packageAddon === wrapperAddon, proxyBefore, proxyAfter: packageAddon.readProperty(proxy), targetValue: target.value, ownKeysCalls, proxyWriteRead, inheritedValue, inheritedCall, initialDefault, customPrototypeMatch, inheritedPrototypeValue, nullPrototype, restoredDefault, guestErrorText});",
            )
            .unwrap();
    assert!(matches!(
        package_result.get_prop("sum"),
        Some(Value::Number(value)) if value == 42.0
    ));
    assert!(matches!(
        package_result.get_prop("same"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        package_result.get_prop("importSame"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        package_result.get_prop("wrapperSame"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        package_result.get_prop("proxyBefore"),
        Some(Value::Number(value)) if value == 42.0
    ));
    assert!(matches!(
        package_result.get_prop("proxyAfter"),
        Some(Value::Number(value)) if value == 11.0
    ));
    assert!(matches!(
        package_result.get_prop("targetValue"),
        Some(Value::Number(value)) if value == 9.0
    ));
    assert!(matches!(
        package_result.get_prop("ownKeysCalls"),
        Some(Value::Number(value)) if value == 0.0
    ));
    assert!(matches!(
        package_result.get_prop("proxyWriteRead"),
        Some(Value::Number(value)) if value == 17.0
    ));
    assert!(matches!(
        package_result.get_prop("inheritedValue"),
        Some(Value::Number(value)) if value == 29.0
    ));
    assert!(matches!(
        package_result.get_prop("inheritedCall"),
        Some(Value::Number(value)) if value == 43.0
    ));
    assert!(matches!(
        package_result.get_prop("initialDefault"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        package_result.get_prop("customPrototypeMatch"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        package_result.get_prop("inheritedPrototypeValue"),
        Some(Value::Number(value)) if value == 43.0
    ));
    assert!(matches!(
        package_result.get_prop("nullPrototype"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        package_result.get_prop("restoredDefault"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        package_result.get_prop("guestErrorText"),
        Some(Value::String(ref value)) if value == "TypeError: bridge"
    ));
    let class_result = interpreter
            .eval_source(
                "class Box { static value = 2; constructor(value) { this.value = value; } read() { return this.value + Box.value; } } const direct = packageAddon.constructAndRead(Box, 40); const proxiedBox = new Proxy(Box, {construct:(target,args) => Reflect.construct(target,args)}); const proxied = packageAddon.constructAndRead(proxiedBox, 40); const classStatic = packageAddon.readProperty(Box); const classWriteRead = packageAddon.writeThenRead(Box, 23); ({direct, proxied, classStatic, classWriteRead, classValue: Box.value});",
            )
            .unwrap();
    assert!(matches!(
        class_result.get_prop("direct"),
        Some(Value::Number(value)) if value == 42.0
    ));
    assert!(matches!(
        class_result.get_prop("proxied"),
        Some(Value::Number(value)) if value == 42.0
    ));
    assert!(matches!(
        class_result.get_prop("classStatic"),
        Some(Value::Number(value)) if value == 2.0
    ));
    assert!(matches!(
        class_result.get_prop("classWriteRead"),
        Some(Value::Number(value)) if value == 23.0
    ));
    assert!(matches!(
        class_result.get_prop("classValue"),
        Some(Value::Number(value)) if value == 23.0
    ));
    let result = interpreter
        .eval_source("require('./fixture.node').add(19, 23);")
        .unwrap();
    assert!(matches!(result, Value::Number(value) if value == 42.0));
    let bigint = interpreter
        .eval_source("String(require('./fixture.node').big());")
        .unwrap();
    assert!(matches!(bigint, Value::String(ref value) if value == "9007199254740993"));
    let builtins = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const nativeDate = addon.makeDate(); const nativeRegex = addon.makeRegex(); const nativeSymbol = addon.makeSymbol(); const guestSymbol = Symbol('guest'); const nativeRegexMatched = nativeRegex.test('AAA'); ({inputDate:addon.dateValue(new Date(1700000000123)), nativeDate:nativeDate.getTime(), inputRegex:addon.regexSource(/a+/gi) + '/' + addon.regexFlags(/a+/gi), nativeRegex:nativeRegex.source + '/' + nativeRegex.flags, nativeRegexMatched, nativeRegexLastIndex:nativeRegex.lastIndex, nativeSymbolType:typeof nativeSymbol, nativeSymbolDescription:nativeSymbol.description, nativeSymbolNapiType:addon.isSymbol(nativeSymbol), guestSymbolNapiType:addon.isSymbol(guestSymbol), iteratorSymbolNapiType:addon.isNodeIterator(Symbol.iterator), nativeSymbolIdentity:addon.identity(nativeSymbol) === nativeSymbol, guestSymbolIdentity:addon.identity(guestSymbol) === guestSymbol});",
            )
            .unwrap();
    assert!(matches!(
        builtins.get_prop("inputDate"),
        Some(Value::Number(value)) if value == 1_700_000_000_123.0
    ));
    assert!(matches!(
        builtins.get_prop("nativeDate"),
        Some(Value::Number(value)) if value == 123_456.0
    ));
    assert!(matches!(
        builtins.get_prop("inputRegex"),
        Some(Value::String(ref value)) if value == "a+/gi"
    ));
    assert!(matches!(
        builtins.get_prop("nativeRegex"),
        Some(Value::String(ref value)) if value == "a+/gi"
    ));
    assert!(matches!(
        builtins.get_prop("nativeRegexMatched"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        builtins.get_prop("nativeRegexLastIndex"),
        Some(Value::Number(value)) if value == 3.0
    ));
    assert!(matches!(
        builtins.get_prop("nativeSymbolType"),
        Some(Value::String(ref value)) if value == "symbol"
    ));
    assert!(matches!(
        builtins.get_prop("nativeSymbolDescription"),
        Some(Value::String(ref value)) if value == "native"
    ));
    assert!(matches!(
        builtins.get_prop("nativeSymbolNapiType"),
        Some(Value::Number(value)) if value == 1.0
    ));
    assert!(matches!(
        builtins.get_prop("guestSymbolNapiType"),
        Some(Value::Number(value)) if value == 1.0
    ));
    assert!(matches!(
        builtins.get_prop("iteratorSymbolNapiType"),
        Some(Value::Number(value)) if value == 1.0
    ));
    assert!(matches!(
        builtins.get_prop("nativeSymbolIdentity"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        builtins.get_prop("guestSymbolIdentity"),
        Some(Value::Bool(true))
    ));
    let identities = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const child = {value:1}; const parent = {left:child, right:child}; const nativeArray = addon.makeSharedArray(); ({nested:addon.sameObject(parent.left, parent.right), topLevel:addon.sameObject(child, child), distinct:addon.sameObject({}, {}), nativeArray:nativeArray[0] === nativeArray[1]});",
            )
            .unwrap();
    assert!(matches!(
        identities.get_prop("nested"),
        Some(Value::Number(value)) if value == 1.0
    ));
    assert!(matches!(
        identities.get_prop("topLevel"),
        Some(Value::Number(value)) if value == 1.0
    ));
    assert!(matches!(
        identities.get_prop("distinct"),
        Some(Value::Number(value)) if value == 0.0
    ));
    assert!(matches!(
        identities.get_prop("nativeArray"),
        Some(Value::Bool(true))
    ));
    let cycles = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const guestObject = {value:1}; guestObject.self = guestObject; const guestArray = []; guestArray.push(guestArray); const nativeObject = addon.makeCycle(); const nativeArray = addon.makeCyclicArray(); ({guestObject:guestObject.self === addon.echo(guestObject), guestArray:guestArray[0] === addon.echo(guestArray), nativeObject:nativeObject.self === nativeObject, nativeArray:nativeArray[0] === nativeArray});",
            )
            .unwrap();
    for key in ["guestObject", "guestArray", "nativeObject", "nativeArray"] {
        assert!(
            matches!(cycles.get_prop(key), Some(Value::Bool(true))),
            "{key}"
        );
    }
    let cyclic_writeback = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const object = addon.makeCycle(); const result = addon.mutateObject(object); ({same:result === object, self:object.self === object, changed:object.changed, child:object.child.value});",
            )
            .unwrap();
    assert!(matches!(
        cyclic_writeback.get_prop("same"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        cyclic_writeback.get_prop("self"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        cyclic_writeback.get_prop("changed"),
        Some(Value::Number(value)) if value == 73.0
    ));
    assert!(matches!(
        cyclic_writeback.get_prop("child"),
        Some(Value::Number(value)) if value == 91.0
    ));
    let mutations = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const child = {value:1}; const object = {child, removeMe:5}; const array = [1,2]; const objectResult = addon.mutateObject(object); const arrayResult = addon.mutateArray(array); ({objectIdentity:objectResult === object, direct:object.changed, nested:child.value, removed:'removeMe' in object, arrayIdentity:arrayResult === array, item:array[1], appended:array[2], length:array.length, named:array.tag});",
            )
            .unwrap();
    assert!(matches!(
        mutations.get_prop("objectIdentity"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        mutations.get_prop("direct"),
        Some(Value::Number(value)) if value == 73.0
    ));
    assert!(matches!(
        mutations.get_prop("nested"),
        Some(Value::Number(value)) if value == 91.0
    ));
    assert!(matches!(
        mutations.get_prop("removed"),
        Some(Value::Bool(false))
    ));
    assert!(matches!(
        mutations.get_prop("arrayIdentity"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        mutations.get_prop("item"),
        Some(Value::Number(value)) if value == 42.0
    ));
    assert!(matches!(
        mutations.get_prop("appended"),
        Some(Value::Number(value)) if value == 84.0
    ));
    assert!(matches!(
        mutations.get_prop("length"),
        Some(Value::Number(value)) if value == 3.0
    ));
    assert!(matches!(
        mutations.get_prop("named"),
        Some(Value::String(ref value)) if value == "native"
    ));
    let accessor_bridge = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); let captured = 14; const object = {}; Object.defineProperty(object, 'value', {get: () => captured, set: (value) => { captured = value; }, enumerable: true, configurable: true}); const before = addon.readProperty(object); addon.writeProperty(object, 37); ({before, captured, after: addon.readProperty(object)});",
            )
            .unwrap();
    assert!(matches!(
        accessor_bridge.get_prop("before"),
        Some(Value::Number(value)) if value == 14.0
    ));
    assert!(matches!(
        accessor_bridge.get_prop("captured"),
        Some(Value::Number(value)) if value == 37.0
    ));
    assert!(matches!(
        accessor_bridge.get_prop("after"),
        Some(Value::Number(value)) if value == 37.0
    ));
    let symbol_bridge = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const key = Symbol('addon-key'); const defined = Symbol('defined-key'); const accessKey = Symbol('accessor-key'); const object = {[key]:18}; Object.defineProperty(object, defined, {value:29, writable:true, enumerable:true, configurable:true}); let captured = 6; Object.defineProperty(object, accessKey, {get: () => captured, set: (value) => { captured = value; }, enumerable:true, configurable:true}); const accessorBefore = addon.readSymbolProperty(object, accessKey); addon.writeSymbolProperty(object, accessKey, 61); const accessorAfter = addon.readSymbolProperty(object, accessKey); const fromKey = Symbol('from-entries'); const fromObject = Object.fromEntries([[fromKey, 71]]); const fromEntries = addon.readSymbolProperty(fromObject, fromKey); const before = addon.readSymbolProperty(object, key); addon.writeSymbolProperty(object, key, 53); const after = object[key]; const deleted = addon.deleteSymbolProperty(object, key); ({before, after, deleted, final: object[key], defined: object[defined], fromEntries, accessorBefore, accessorAfter, captured});",
            )
            .unwrap();
    assert!(matches!(
        symbol_bridge.get_prop("before"),
        Some(Value::Number(value)) if value == 18.0
    ));
    assert!(matches!(
        symbol_bridge.get_prop("after"),
        Some(Value::Number(value)) if value == 53.0
    ));
    assert!(matches!(
        symbol_bridge.get_prop("deleted"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        symbol_bridge.get_prop("final"),
        Some(Value::Undefined)
    ));
    assert!(matches!(
        symbol_bridge.get_prop("defined"),
        Some(Value::Number(value)) if value == 29.0
    ));
    assert!(matches!(
        symbol_bridge.get_prop("fromEntries"),
        Some(Value::Number(value)) if value == 71.0
    ));
    assert!(matches!(
        symbol_bridge.get_prop("accessorBefore"),
        Some(Value::Number(value)) if value == 6.0
    ));
    assert!(matches!(
        symbol_bridge.get_prop("accessorAfter"),
        Some(Value::Number(value)) if value == 61.0
    ));
    assert!(matches!(
        symbol_bridge.get_prop("captured"),
        Some(Value::Number(value)) if value == 61.0
    ));
    let returned_symbol_bridge = interpreter
            .eval_source(
                "const native = require('./fixture.node').makeSymbolObject(); ({description: native.key.description, value: native[native.key]});",
            )
            .unwrap();
    assert!(matches!(
        returned_symbol_bridge.get_prop("description"),
        Some(Value::String(ref value)) if value == "native-key"
    ));
    assert!(matches!(
        returned_symbol_bridge.get_prop("value"),
        Some(Value::Number(value)) if value == 89.0
    ));
    let callback_mutations = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const object = {value:1}; const returned = addon.mutateAfterCallback(object, () => { object.guest = 19; }); ({same:returned === object, guest:object.guest, native:object.native});",
            )
            .unwrap();
    assert!(matches!(
        callback_mutations.get_prop("same"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        callback_mutations.get_prop("guest"),
        Some(Value::Number(value)) if value == 19.0
    ));
    assert!(matches!(
        callback_mutations.get_prop("native"),
        Some(Value::Number(value)) if value == 27.0
    ));
    let returned_alias = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const holder = {}; const result = addon.assignAndReturn(holder); ({same:result === holder.created, value:holder.created.value});",
            )
            .unwrap();
    assert!(matches!(
        returned_alias.get_prop("same"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(
        returned_alias.get_prop("value"),
        Some(Value::Number(value)) if value == 15.0
    ));
    let buffers = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const buffer = addon.makeBuffer(); const view = addon.makeDataView(); ({text:buffer.toString('utf8'), first:buffer[0], roundTrip:addon.isBuffer(buffer), guestBufferIsBuffer:addon.isBuffer(Buffer.from([1, 2])), inputLength:addon.typedArrayLength(new Uint16Array([300, 400])), typed:addon.makeTypedArray(), arrayBufferLength:addon.arrayBufferLength(new Uint8Array([1, 2, 3]).buffer), arrayBufferByte:new Uint8Array(addon.makeArrayBuffer())[1], dataViewLength:view.byteLength, dataViewByte:view.getUint8(1), dataViewRoundTrip:addon.dataViewByte(new DataView(new Uint8Array([4, 5]).buffer))});",
            )
            .unwrap();
    assert!(matches!(buffers.get_prop("text"), Some(Value::String(ref value)) if value == "abc"));
    assert!(matches!(buffers.get_prop("first"), Some(Value::Number(value)) if value == 97.0));
    assert!(matches!(buffers.get_prop("roundTrip"), Some(Value::Number(value)) if value == 1.0));
    assert!(matches!(
        buffers.get_prop("guestBufferIsBuffer"),
        Some(Value::Number(value)) if value == 1.0
    ));
    assert!(matches!(buffers.get_prop("inputLength"), Some(Value::Number(value)) if value == 2.0));
    assert!(
        matches!(buffers.get_prop("arrayBufferLength"), Some(Value::Number(value)) if value == 3.0)
    );
    assert!(
        matches!(buffers.get_prop("arrayBufferByte"), Some(Value::Number(value)) if value == 121.0)
    );
    assert!(
        matches!(buffers.get_prop("dataViewLength"), Some(Value::Number(value)) if value == 2.0)
    );
    assert!(
        matches!(buffers.get_prop("dataViewByte"), Some(Value::Number(value)) if value == 29.0)
    );
    assert!(
        matches!(buffers.get_prop("dataViewRoundTrip"), Some(Value::Number(value)) if value == 5.0)
    );
    let typed_value = buffers.get_prop("typed").unwrap_or(Value::Undefined);
    let Value::TypedArray(typed) = &typed_value else {
        panic!("native Uint16Array was not preserved as a typed array");
    };
    assert_eq!(typed.kind, TypedKind::Uint16);
    assert_eq!(typed.length, 2);
    assert!(matches!(
        crate::builtins::read_element(typed, 0),
        Some(Value::Number(value)) if value == 300.0
    ));
    let error = interpreter
            .eval_source(
                "try { require('./fixture.node').fail(); } catch (error) { ({name:error.name, message:error.message, code:error.code}); }",
            )
            .unwrap();
    assert!(matches!(error.get_prop("name"), Some(Value::String(ref name)) if name == "TypeError"));
    assert!(
        matches!(error.get_prop("message"), Some(Value::String(ref message)) if message == "fixture failure")
    );
    assert!(matches!(error.get_prop("code"), Some(Value::String(ref code)) if code == "E_FIXTURE"));
    let error_after_mutation = interpreter
            .eval_source(
                "const object = {}; let observed; try { require('./fixture.node').mutateThenThrow(object); } catch (error) { observed = {name:error.name, code:error.code, message:error.message, value:object.afterThrow}; } observed;",
            )
            .unwrap();
    assert!(matches!(
        error_after_mutation.get_prop("name"),
        Some(Value::String(ref name)) if name == "TypeError"
    ));
    assert!(matches!(
        error_after_mutation.get_prop("code"),
        Some(Value::String(ref code)) if code == "E_AFTER_MUTATION"
    ));
    assert!(matches!(
        error_after_mutation.get_prop("message"),
        Some(Value::String(ref message)) if message == "mutation happened"
    ));
    assert!(matches!(
        error_after_mutation.get_prop("value"),
        Some(Value::Number(value)) if value == 88.0
    ));
    let async_result = interpreter
        .eval_source(
            "await require('./fixture.node').promiseResult().then(value => value + '-chained');",
        )
        .unwrap();
    assert!(
        matches!(async_result, Value::String(ref value) if value == "native-promise-value-chained")
    );
    let async_function_result = interpreter
            .eval_source(
                "async function readNativePromise() { return await require('./fixture.node').promiseResult(); } await readNativePromise();",
            )
            .unwrap();
    assert!(
        matches!(async_function_result, Value::String(ref value) if value == "native-promise-value")
    );
    let unrelated_await = interpreter
            .eval_source(
                "globalThis.pendingNativePromise = require('./fixture.node').promisePending(); await Promise.resolve(); 'unrelated-await-completed';",
            )
            .unwrap();
    assert!(
        matches!(unrelated_await, Value::String(ref value) if value == "unrelated-await-completed")
    );
    let async_rejection = interpreter
        .eval_source(
            "try { await require('./fixture.node').promiseReject(); } catch (reason) { reason; }",
        )
        .unwrap();
    assert!(matches!(async_rejection, Value::String(ref reason) if reason == "native-rejection"));
    let async_error_rejection = interpreter
            .eval_source(
                "try { await require('./fixture.node').promiseRejectError(); } catch (error) { ({name:error.name, message:error.message, code:error.code}); }",
            )
            .unwrap();
    assert!(matches!(
        async_error_rejection.get_prop("name"),
        Some(Value::String(ref name)) if name == "Error"
    ));
    assert!(matches!(
        async_error_rejection.get_prop("message"),
        Some(Value::String(ref message)) if message == "native error rejection"
    ));
    assert!(matches!(
        async_error_rejection.get_prop("code"),
        Some(Value::String(ref code)) if code == "E_NATIVE_REJECTION"
    ));
    let sync_result = interpreter
            .eval_source(
                "globalThis.syncCallbackThisType = ''; const syncCallbackResult = require('./fixture.node').onSync(function(value) { syncCallbackThisType = typeof this; return value + '-reply'; }); ({value:syncCallbackResult, thisType:syncCallbackThisType});",
            )
            .unwrap();
    assert!(
        matches!(sync_result.get_prop("value"), Some(Value::String(ref value)) if value == "sync-value-reply")
    );
    assert!(
        matches!(sync_result.get_prop("thisType"), Some(Value::String(ref value)) if value == "object")
    );
    let sync_throw = interpreter
            .eval_source(
                "try { require('./fixture.node').onSync(() => { throw new TypeError('guest callback failure'); }); } catch (error) { ({name:error.name, message:error.message}); }",
            )
            .unwrap();
    assert!(
        matches!(sync_throw.get_prop("name"), Some(Value::String(ref name)) if name == "TypeError"),
        "unexpected sync callback throw: {sync_throw:?}"
    );
    assert!(
        matches!(sync_throw.get_prop("message"), Some(Value::String(ref message)) if message == "guest callback failure"),
        "unexpected sync callback throw: {sync_throw:?}"
    );
    let nested_addon_call = interpreter
            .eval_source(
                "require('./fixture.node').onSync(value => require('./fixture.node').onSync(inner => require('./fixture.node').add(19, 23)));",
            )
            .unwrap();
    assert!(matches!(
        nested_addon_call,
        Value::Number(value) if value == 42.0
    ));
    interpreter
            .eval_source(
                "globalThis.callbackValues = []; globalThis.callbackThisType = ''; require('./fixture.node').onLater(function(value) { callbackValues.push(value); callbackThisType = typeof this; });",
            )
            .unwrap();
    assert!(
        interpreter
            .run_event_loop_once(Duration::from_secs(2))
            .unwrap()
    );
    let callback_value = interpreter.eval_source("callbackValues[0];").unwrap();
    assert!(matches!(callback_value, Value::String(ref value) if value == "async-value"));
    let callback_this = interpreter.eval_source("callbackThisType;").unwrap();
    assert!(matches!(callback_this, Value::String(ref value) if value == "object"));
    interpreter
            .eval_source(
                "globalThis.asyncReferenceTarget = {value:1}; globalThis.asyncCallbackIdentity = false; require('./fixture.node').onLater(value => { value.changed = 23; asyncCallbackIdentity = value === asyncReferenceTarget; }, asyncReferenceTarget);",
            )
            .unwrap();
    assert!(
        interpreter
            .run_event_loop_once(Duration::from_secs(2))
            .unwrap()
    );
    assert!(matches!(
        interpreter
            .eval_source("asyncReferenceTarget.changed;")
            .unwrap(),
        Value::Number(23.0)
    ));
    assert!(matches!(
        interpreter.eval_source("asyncCallbackIdentity;").unwrap(),
        Value::Bool(true)
    ));
    interpreter
            .eval_source(
                "globalThis.threadsafeValues = []; require('./fixture.node').onThreadsafe(value => { threadsafeValues.push(value); queueMicrotask(() => threadsafeValues.push('microtask')); });",
            )
            .unwrap();
    assert!(matches!(
        interpreter.eval_source("threadsafeValues.length;").unwrap(),
        Value::Number(0.0)
    ));
    let mut threadsafe_callback_received = false;
    for _ in 0..10 {
        if interpreter
            .run_event_loop_once(Duration::from_millis(250))
            .unwrap()
        {
            let values = interpreter
                .eval_source("threadsafeValues.join(',');")
                .unwrap();
            if matches!(values, Value::String(ref values) if values == "threadsafe-value,microtask")
            {
                threadsafe_callback_received = true;
                break;
            }
        }
    }
    assert!(
        threadsafe_callback_received,
        "Node-API threadsafe callback did not reach the guest event loop"
    );
    let counter = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const counter = new addon.Counter(19); counter.increment(); counter.count = 41; counter.increment(); const other = new addon.Counter(5); counter.increment.call(other); const spread = {...counter}; const assigned = Object.assign({}, counter); let iterated = ''; for (const key in counter) iterated += key; ({count:counter.count, receiver:other.count, same:counter === counter.self(), keys:Object.keys(counter).join(','), values:Object.values(counter).join(','), entries:Object.entries(counter)[0][0] + ':' + Object.entries(counter)[0][1], spread:spread.count, assigned:assigned.count, iterated, has:'count' in counter});",
            )
            .unwrap();
    assert!(matches!(counter.get_prop("count"), Some(Value::Number(count)) if count == 42.0));
    assert!(matches!(counter.get_prop("receiver"), Some(Value::Number(value)) if value == 6.0));
    assert!(matches!(counter.get_prop("same"), Some(Value::Bool(true))));
    assert!(matches!(counter.get_prop("keys"), Some(Value::String(ref keys)) if keys == "count"));
    assert!(
        matches!(counter.get_prop("values"), Some(Value::String(ref values)) if values == "42")
    );
    assert!(
        matches!(counter.get_prop("entries"), Some(Value::String(ref entries)) if entries == "count:42")
    );
    assert!(matches!(counter.get_prop("spread"), Some(Value::Number(value)) if value == 42.0));
    assert!(matches!(counter.get_prop("assigned"), Some(Value::Number(value)) if value == 42.0));
    assert!(
        matches!(counter.get_prop("iterated"), Some(Value::String(ref iterated)) if iterated == "count")
    );
    assert!(matches!(counter.get_prop("has"), Some(Value::Bool(true))));
    let roundtrip = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const counter = new addon.Counter(5); addon.echo(counter) === counter;",
            )
            .unwrap();
    assert!(matches!(roundtrip, Value::Bool(true)));

    runtime.shutdown().unwrap();
    assert!(runtime.is_shutdown());
    runtime.shutdown().unwrap();
    let after_shutdown = interpreter
        .eval_source("require('fixture').add(1, 2);")
        .unwrap_err();
    assert!(
        after_shutdown
            .to_string()
            .contains("Node sidecar has been shut down")
    );

    drop(interpreter);
    drop(runtime);
    let mut sparse_interpreter = Interpreter::with_builtins();
    let _sparse_bridge = sparse_interpreter
        .enable_node_addons(
            NodeAddonOptions::new("node", [root.clone()])
                .allow_native_addon_with_sha256(addon.clone(), expected_sha256)
                .entry(root.join("main.cjs")),
        )
        .unwrap();
    let sparse_result = sparse_interpreter.eval_source(sparse_fixture).unwrap();
    for (key, expected) in [
        ("initialHole", false),
        ("initialValue", true),
        ("initialExplicitUndefined", true),
        ("initialNativeHole", false),
        ("initialNativeValue", true),
        ("written", true),
        ("guestDeleted", true),
        ("guestDeleteReachedNative", true),
        ("hostDeleteResult", true),
        ("hostDeleteVisible", true),
        ("nativeDeleted", true),
        ("restored", true),
        ("nativeRestored", true),
        ("mappedIndex0", true),
        ("mappedIndex1", true),
        ("mappedIndex2", true),
        ("mappedIndex3", false),
    ] {
        assert!(
            matches!(sparse_result.get_prop(key), Some(Value::Bool(value)) if value == expected),
            "sparse array result mismatch for {key}: {:?}",
            sparse_result.get_prop(key)
        );
    }
    for key in ["length", "mappedLength"] {
        assert!(
            matches!(sparse_result.get_prop(key), Some(Value::Number(value)) if value == 4.0),
            "sparse array length mismatch for {key}"
        );
    }
    assert!(matches!(
        sparse_result.get_prop("keys"),
        Some(Value::String(ref value)) if value == "0,1,2,tag"
    ));
    drop(sparse_interpreter);
    drop(_sparse_bridge);
    fs::remove_dir_all(root).unwrap();
}
