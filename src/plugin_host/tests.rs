use super::*;
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows"),
    any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")
))]
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestPluginDir(PathBuf);

impl TestPluginDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "napi-vm-rust-plugin-host-{name}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, source: &str) {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, source).unwrap();
    }

    fn manifest(&self, name: &str, entry: &str, permissions: &str) {
        self.write(
                "plugin.json",
                &format!(
                    r#"{{"name":{name:?},"version":"1.0.0","apiVersion":1,"entry":{entry:?},"permissions":{permissions}}}"#
                ),
            );
    }
}

impl Drop for TestPluginDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
#[test]
fn napi_prebuild_package_roots_stay_inside_the_plugin_directory() {
    let plugin = TestPluginDir::new("napi-package-root");
    let outside = TestPluginDir::new("napi-package-outside");
    fs::create_dir_all(plugin.0.join("node_modules/example-addon")).unwrap();

    let inside =
        plugin_napi_package_root(&plugin.0, Path::new("node_modules/example-addon")).unwrap();
    assert_eq!(inside, plugin.0.join("node_modules/example-addon"));

    let error = plugin_napi_package_root(&plugin.0, &outside.0).unwrap_err();
    assert!(error.to_string().contains("outside the plugin directory"));
}

fn string_value(_interp: &mut Interpreter, args: Vec<Value>) -> Result<Value, VmErr> {
    match args.first() {
        Some(Value::String(value)) => Ok(Value::String(format!("hello {value}"))),
        _ => Err(VmErr::Msg("expected one string argument".into())),
    }
}

#[test]
fn rust_host_loads_relative_esm_and_reloads_with_serialized_state() {
    let dir = TestPluginDir::new("lifecycle");
    dir.write("data.txt", "Ada");
    dir.write("sub/deep.mjs", "export const prefix = 'Ms. ';\n");
    dir.write("sub/index.mjs", "export { prefix } from './deep.mjs';\n");
    dir.write(
        "main.mjs",
        r#"
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { hello } from "greet";
import { prefix } from "./sub/index.mjs";
export default class Example {
  onLoad(context) {
    this.value = readFileSync("./data.txt", "utf8");
    this.message = hello(prefix + this.value);
    writeFileSync(join("./cache", "status.txt"), this.message);
    return { name: context.name, message: this.message };
  }
  onUnload(context) { return { value: this.value, reason: context.reason }; }
  onReload(context, state) { this.value = state.value; return { restored: this.value }; }
}
"#,
    );
    dir.write("cache/.keep", "");
    dir.manifest(
            "sample-plugin",
            "main.mjs",
            r#"{"fs":{"read":["data.txt","cache/**"],"write":"cache/**"},"path":true,"capabilities":{"greet":true}}"#,
        );

    let capability = RustPluginCapability::new("greet").export("hello", string_value);
    let mut host = RustPluginHost::new(RustPluginHostOptions {
        policy: RustPluginPolicy::default()
            .grant_fs_read("data.txt")
            .grant_fs_read("cache/**")
            .grant_fs_write("cache/**")
            .grant_path()
            .grant("greet", JsonValue::Bool(true)),
        ..RustPluginHostOptions::default()
    });
    host.define_capability(capability).unwrap();
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(plugin.manifest.name, "sample-plugin");
    assert_eq!(plugin.capabilities, ["greet"]);
    assert_eq!(
        plugin.load_result,
        Some(serde_json::json!({
            "name": "sample-plugin",
            "message": "hello Ms. Ada"
        }))
    );
    assert_eq!(
        fs::read_to_string(dir.0.join("cache/status.txt")).unwrap(),
        "hello Ms. Ada"
    );

    let reloaded = host.reload("sample-plugin").unwrap();
    assert_eq!(
        reloaded.load_result,
        Some(serde_json::json!({"restored":"Ada"}))
    );
    let state = host.unload("sample-plugin").unwrap();
    assert_eq!(
        state,
        Some(serde_json::json!({"value":"Ada","reason":"unload"}))
    );
    assert!(host.list().next().is_none());
}

#[test]
fn rust_host_resolves_in_root_esm_packages_with_exports_and_nested_dependencies() {
    let dir = TestPluginDir::new("npm-esm");
    dir.write(
        "main.mjs",
        r#"
import { value } from "tiny-lib";
import { nested as rootDependency } from "nested-dep";
export default class Example {
  onLoad() { return { value: value(), rootDependency }; }
}
"#,
    );
    dir.write(
            "node_modules/tiny-lib/package.json",
            r#"{"name":"tiny-lib","version":"1.0.0","exports":{".":{"import":"./esm/index.mjs","require":"./cjs/index.cjs","default":"./fallback.mjs"},"./feature":{"import":"./esm/feature.mjs"}}}"#,
        );
    dir.write(
        "node_modules/tiny-lib/esm/index.mjs",
        r#"
import { base } from "./helper";
import { nested } from "nested-dep";
import { extra } from "tiny-lib/feature";
export const value = () => base + nested + extra;
"#,
    );
    dir.write(
        "node_modules/tiny-lib/esm/helper.mjs",
        "export const base = 20;",
    );
    dir.write(
        "node_modules/tiny-lib/esm/feature.mjs",
        "export const extra = 2;",
    );
    dir.write(
        "node_modules/tiny-lib/node_modules/nested-dep/package.json",
        r#"{"name":"nested-dep","version":"1.0.0","module":"index.mjs"}"#,
    );
    dir.write(
        "node_modules/tiny-lib/node_modules/nested-dep/index.mjs",
        "export const nested = 20;",
    );
    dir.write(
        "node_modules/nested-dep/package.json",
        r#"{"name":"nested-dep","version":"2.0.0","module":"index.mjs"}"#,
    );
    dir.write(
        "node_modules/nested-dep/index.mjs",
        "export const nested = 100;",
    );
    dir.manifest("npm-plugin", "main.mjs", "{}");

    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(
        plugin.load_result,
        Some(serde_json::json!({"value":42,"rootDependency":100}))
    );
}

#[test]
fn rust_host_loads_commonjs_packages_without_native_addons() {
    let dir = TestPluginDir::new("npm-commonjs");
    dir.write("config.txt", "checked");
    dir.write(
        "main.mjs",
        r#"
const pkg = require("fixture-cjs");
export default {
  onLoad() {
    let nativeDenied = false;
    try { require("./native/fixture.node"); }
    catch (error) { nativeDenied = error.message.includes("not allowlisted"); }
    return {
      answer: pkg.answer,
      text: pkg.text,
      metadata: pkg.metadata,
      basename: pkg.basename,
      cached: pkg === require("fixture-cjs"),
      nestedCache: pkg.nested === require("fixture-cjs/nested"),
      nativeDenied,
    };
  }
};
"#,
    );
    dir.write(
            "node_modules/fixture-cjs/package.json",
            r#"{"name":"fixture-cjs","version":"1.0.0","exports":{".":{"require":"./index.cjs","default":"./index.cjs"},"./nested":"./nested.cjs"}}"#,
        );
    dir.write(
        "node_modules/fixture-cjs/index.cjs",
        r#"
const fs = require("node:fs");
const path = require("node:path");
const nested = require("./nested.cjs");
const metadata = require("./metadata.json");
module.exports = {
  answer: nested.answer,
  text: fs.readFileSync("./config.txt", "utf8"),
  metadata: metadata.kind,
  basename: path.basename(__dirname),
  nested,
};
"#,
    );
    dir.write(
        "node_modules/fixture-cjs/nested.cjs",
        "module.exports = { answer: 42 };\n",
    );
    dir.write(
        "node_modules/fixture-cjs/metadata.json",
        r#"{"kind":"json-module"}"#,
    );
    dir.write("native/fixture.node", "not a native library");
    dir.manifest(
        "commonjs-plugin",
        "main.mjs",
        r#"{"fs":{"read":"config.txt"},"path":true}"#,
    );

    let mut host = RustPluginHost::new(RustPluginHostOptions {
        policy: RustPluginPolicy::default()
            .grant_fs_read("config.txt")
            .grant_path(),
        ..RustPluginHostOptions::default()
    });
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(
        plugin.load_result,
        Some(serde_json::json!({
            "answer": 42,
            "text": "checked",
            "metadata": "json-module",
            "basename": "fixture-cjs",
            "cached": true,
            "nestedCache": true,
            "nativeDenied": true
        }))
    );
}

#[test]
fn rust_host_rejects_package_exports_that_escape_the_plugin_root() {
    let dir = TestPluginDir::new("npm-escape");
    dir.write(
        "main.mjs",
        r#"import { value } from "unsafe-pkg"; export default { value };"#,
    );
    dir.write(
        "node_modules/unsafe-pkg/package.json",
        r#"{"name":"unsafe-pkg","exports":"../../outside.mjs"}"#,
    );
    dir.manifest("npm-escape", "main.mjs", "{}");

    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    let error = match host.load(&dir.0) {
        Ok(_) => panic!("package export escaping the plugin root must be rejected"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("unsupported package exports target")
    );
}

#[test]
fn manifest_permission_denial_is_a_catchable_guest_error() {
    let dir = TestPluginDir::new("deny");
    dir.write("data.txt", "visible");
    dir.write("secret.txt", "hidden");
    dir.write(
        "main.mjs",
        r#"
import { readFileSync } from "node:fs";
export default { onLoad() {
  try { readFileSync("./secret.txt", "utf8"); }
  catch (error) { return { name: error.name, message: error.message }; }
} };
"#,
    );
    dir.manifest("deny-plugin", "main.mjs", r#"{"fs":{"read":"data.txt"}}"#);

    let mut host = RustPluginHost::new(RustPluginHostOptions {
        policy: RustPluginPolicy::default().grant_fs_read("**"),
        ..RustPluginHostOptions::default()
    });
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(
        plugin.load_result,
        Some(serde_json::json!({
            "name": "PermissionDenied",
            "message": "fs.read is not permitted for \"./secret.txt\""
        }))
    );
}

#[test]
fn host_filesystem_policy_intersects_manifest_requests() {
    let dir = TestPluginDir::new("host-fs-policy");
    dir.write("public.txt", "public");
    dir.write("secret.txt", "secret");
    dir.write(
        "main.mjs",
        r#"
import { readFileSync } from "node:fs";
export default { onLoad() {
  try { return readFileSync("./secret.txt", "utf8"); }
  catch (error) { return { name: error.name }; }
} };
"#,
    );
    dir.manifest("host-policy-plugin", "main.mjs", r#"{"fs":{"read":true}}"#);
    let mut host = RustPluginHost::new(RustPluginHostOptions {
        policy: RustPluginPolicy::default().grant_fs_read("public.txt"),
        ..RustPluginHostOptions::default()
    });
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(
        plugin.load_result,
        Some(serde_json::json!({"name":"PermissionDenied"}))
    );

    let no_host_grant = TestPluginDir::new("host-fs-default-deny");
    no_host_grant.write("public.txt", "public");
    no_host_grant.write(
        "main.mjs",
        r#"
import { readFileSync } from "node:fs";
export default { onLoad() {
  try { return readFileSync("./public.txt", "utf8"); }
  catch (error) { return { name: error.name }; }
} };
"#,
    );
    no_host_grant.manifest(
        "host-default-deny-plugin",
        "main.mjs",
        r#"{"fs":{"read":"public.txt"}}"#,
    );
    let mut default_host = RustPluginHost::new(RustPluginHostOptions::default());
    let plugin = default_host.load(&no_host_grant.0).unwrap();
    assert_eq!(
        plugin.load_result,
        Some(serde_json::json!({"name":"PermissionDenied"}))
    );
}

#[test]
fn resource_limits_and_missing_capability_requests_fail_closed() {
    let dir = TestPluginDir::new("limits");
    dir.write("data.txt", &"x".repeat(1200));
    dir.write(
        "main.mjs",
        r#"
import { readFileSync } from "node:fs";
export default { onLoad() {
  try { readFileSync("./data.txt", "utf8"); }
  catch (error) { return { name: error.name }; }
} };
"#,
    );
    dir.manifest("limit-plugin", "main.mjs", r#"{"fs":{"read":"data.txt"}}"#);
    let mut host = RustPluginHost::new(RustPluginHostOptions {
        policy: RustPluginPolicy::default().grant_fs_read("data.txt"),
        max_file_bytes: 1024,
    });
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(
        plugin.load_result,
        Some(serde_json::json!({"name":"ResourceLimit"}))
    );

    let unknown = TestPluginDir::new("unknown-cap");
    unknown.write(
        "main.mjs",
        "export default { onLoad() { return 'never'; } };",
    );
    unknown.manifest(
        "unknown-plugin",
        "main.mjs",
        r#"{"capabilities":{"not-registered":true}}"#,
    );
    assert!(matches!(
        host.load(&unknown.0),
        Err(PluginHostError::Load(message)) if message.contains("unknown capability")
    ));
}

#[test]
fn manifest_validation_and_glob_rules_reject_traversal() {
    assert!(compile_one_pattern("../outside", "permissions.fs.read").is_err());
    assert!(
        compile_one_pattern("assets/**", "permissions.fs.read").is_ok_and(|rule| {
            path_pattern_matches(&rule.pattern, "assets")
                && path_pattern_matches(&rule.pattern, "assets/icons/logo.svg")
                && !path_pattern_matches(&rule.pattern, "other/logo.svg")
        })
    );
    let dir = TestPluginDir::new("bad-manifest");
    dir.write("main.mjs", "export default {};\n");
    dir.manifest("bad", "../main.mjs", "{}");
    assert!(matches!(
        prepare_plugin(&dir.0, DEFAULT_MAX_PLUGIN_FILE_BYTES),
        Err(PluginHostError::Manifest(_))
    ));
}

#[cfg(all(
    feature = "node-api-host",
    target_os = "linux",
    any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")
))]
#[test]
fn plugin_host_composes_node_api_addons_with_checked_fs_facades() {
    use sha2::{Digest, Sha256};

    let dir = TestPluginDir::new("native-integration");
    let compiler = Command::new("cc").arg("--version").output();
    let include = [
        std::env::var_os("NODE_INCLUDE_DIR").map(PathBuf::from),
        Some(PathBuf::from("/usr/include/node")),
        Some(PathBuf::from("/usr/local/include/node")),
    ]
    .into_iter()
    .flatten()
    .find(|path| path.join("node_api.h").is_file());
    let (Ok(compiler), Some(include)) = (compiler, include) else {
        eprintln!("skipping plugin Node-API integration: cc or Node headers are unavailable");
        return;
    };
    assert!(compiler.status.success(), "cc --version failed");
    let source = dir.0.join("fixture.c");
    let addon = dir.0.join("fixture.node");
    fs::write(
        &source,
        r#"
#define NAPI_VERSION 8
#include <node_api.h>

static napi_value answer(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_create_string_utf8(env, "native", NAPI_AUTO_LENGTH, &result) != napi_ok)
    return NULL;
  return result;
}

NAPI_MODULE_INIT() {
  napi_value function;
  if (napi_create_function(env, "answer", NAPI_AUTO_LENGTH, answer, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "answer", function) != napi_ok)
    return NULL;
  return exports;
}
"#,
    )
    .unwrap();
    let built = Command::new("cc")
        .args(["-std=c11", "-O2", "-fPIC", "-shared", "-I"])
        .arg(&include)
        .arg(&source)
        .arg("-o")
        .arg(&addon)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "Node-API addon compilation failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();
    dir.write("data.txt", "checked");
    dir.write(
        "main.mjs",
        r#"
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
const addon = require("./fixture.node");
export default { onLoad() {
  const value = addon.answer() + ":" + readFileSync("./data.txt", "utf8");
  writeFileSync(join("./cache", "native.txt"), value);
  return value;
} };
"#,
    );
    dir.write("cache/.keep", "");
    dir.manifest(
        "native-plugin",
        "main.mjs",
        r#"{"fs":{"read":"data.txt","write":"cache/**"},"path":true}"#,
    );
    let mut host = RustPluginHost::new(RustPluginHostOptions {
        policy: RustPluginPolicy::default()
            .grant_fs_read("data.txt")
            .grant_fs_write("cache/**")
            .grant_path(),
        ..RustPluginHostOptions::default()
    });
    host.configure_napi_addons(
        "native-plugin",
        RustPluginNapiOptions::default().allow_addon_with_sha256(&addon, digest),
    )
    .unwrap();
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(
        plugin.load_result,
        Some(serde_json::json!("native:checked"))
    );
    assert_eq!(
        fs::read_to_string(dir.0.join("cache/native.txt")).unwrap(),
        "native:checked"
    );
    host.unload("native-plugin").unwrap();
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows"),
    any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")
))]
#[test]
fn plugin_host_loads_rust_authored_napi_rs_addon() {
    use sha2::{Digest, Sha256};

    let dir = TestPluginDir::new("napi-rs-plugin");
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/node-api/napi-rs/Cargo.toml");
    // This fixture is also built by a Node-API integration test in
    // `rust_node_api::tests`. Keep the outputs separate so parallel test
    // runs cannot copy the shared cdylib while Cargo is rebuilding it.
    let target_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/node-api-fixtures/napi-rs-plugin-host");
    let temp_dir = target_dir.join("tmp");
    fs::create_dir_all(&temp_dir).unwrap();
    let built = Command::new("cargo")
        .args(["build", "--offline", "--release", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target_dir)
        .env("TMPDIR", &temp_dir)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "napi-rs plugin fixture build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let cdylib_name = if cfg!(target_os = "windows") {
        "napi_vm_napi_rs_fixture.dll"
    } else if cfg!(target_os = "macos") {
        "libnapi_vm_napi_rs_fixture.dylib"
    } else {
        "libnapi_vm_napi_rs_fixture.so"
    };
    let compiled_addon = target_dir.join("release").join(cdylib_name);
    assert!(compiled_addon.is_file(), "napi-rs fixture was not built");
    let addon = dir
        .0
        .join("node_modules/fixture.node/build/Release/fixture.node");
    fs::create_dir_all(addon.parent().unwrap()).unwrap();
    fs::copy(&compiled_addon, &addon).unwrap();
    let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();
    dir.write(
        "node_modules/fixture.node/package.json",
        r#"{"name":"fixture.node","version":"1.0.0","main":"index.cjs"}"#,
    );
    dir.write(
        "node_modules/fixture.node/index.cjs",
        r#"
const fs = require("fs");
const path = require("node:path");
if (fs.readFileSync("./data.txt", "utf8") !== "checked" ||
    path.basename(__dirname) !== "fixture.node") {
  throw new Error("CommonJS facades were not installed");
}
module.exports = require("node-gyp-build")(__dirname);
"#,
    );

    dir.write("data.txt", "checked");
    dir.write("secret.txt", "not granted");
    dir.write("cache/.keep", "");
    dir.write(
        "main.mjs",
        r#"
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createRequire, isBuiltin } from "node:module";
const localRequire = createRequire(import.meta.url);
const aliasedAddon = localRequire("fixture-native");
const addon = localRequire("fixture.node");
const cjsFs = require("node:fs");
const cjsFsAlias = require("fs");
const cjsPath = require("path");
const cjsPathAlias = require("node:path");
export default {
async onLoad() {
  const counter = new addon.Counter(40);
  let failure;
  try { addon.fail(); }
  catch (error) { failure = { name: error.name, message: error.message }; }
  let deniedRead;
  try { cjsFs.readFileSync("./secret.txt", "utf8"); }
  catch (error) { deniedRead = error.name; }
  const result = {
    sameExports: aliasedAddon === addon,
    moduleFacade: isBuiltin("node:module") &&
      localRequire("fixture.node") === addon &&
      localRequire.resolve("fixture.node") === require.resolve("fixture.node"),
    commonJsFacades: cjsFs === cjsFsAlias && cjsPath === cjsPathAlias &&
      cjsFs.readFileSync === readFileSync &&
      cjsFsAlias.writeFileSync === writeFileSync &&
      cjsPath.join === join && cjsPathAlias.sep === cjsPath.sep,
    commonJsBuiltinResolve: require.resolve("fs") === "fs" &&
      require.resolve("node:path") === "node:path",
    deniedRead,
    sum: addon.add(19, 23),
    text: addon.concatenate("rust", "-napi"),
    counter: { initial: counter.value, incremented: counter.increment(), value: counter.value },
    bytes: Array.from(addon.reverseBytes(Buffer.from([1, 2, 3, 4]))),
    failure,
    file: readFileSync("./data.txt", "utf8"),
    asyncSum: await addon.addAsync(20, 22),
  };
  writeFileSync(join("./cache", "napi-rs.json"), JSON.stringify(result));
  return result;
},
async onUnload(context) {
  return { reason: context.reason, asyncSum: await addon.addAsync(1, 2) };
},
async onReload(context, previousState) {
  return { previousState, asyncSum: await addon.addAsync(20, 22) };
}
};
"#,
    );
    dir.manifest(
        "napi-rs-plugin",
        "main.mjs",
        r#"{"fs":{"read":"data.txt","write":"cache/**"},"path":true}"#,
    );

    let policy = RustPluginPolicy::default()
        .grant_fs_read("data.txt")
        .grant_fs_write("cache/**")
        .grant_path();
    let mut host = RustPluginHost::new(RustPluginHostOptions {
        policy,
        ..RustPluginHostOptions::default()
    });
    let package_root = dir.0.join("node_modules/fixture.node");
    host.configure_napi_addons(
        "napi-rs-plugin",
        RustPluginNapiOptions::default()
            .allow_native_prebuild_with_sha256("fixture-native", &package_root, digest)
            .allow_native_package_prebuild_with_sha256(&package_root, digest),
    )
    .unwrap();
    let expected = serde_json::json!({
        "sameExports": true,
        "moduleFacade": true,
        "commonJsFacades": true,
        "commonJsBuiltinResolve": true,
        "deniedRead": "PermissionDenied",
        "sum": 42,
        "text": "rust-napi",
        "counter": { "initial": 40, "incremented": 41, "value": 41 },
        "bytes": [4, 3, 2, 1],
        "failure": { "name": "Error", "message": "fixture failure" },
        "file": "checked",
        "asyncSum": 42
    });
    {
        let plugin = host.load(&dir.0).unwrap();
        assert_eq!(plugin.load_result, Some(expected.clone()));
    }
    assert_eq!(
        serde_json::from_str::<JsonValue>(
            &fs::read_to_string(dir.0.join("cache/napi-rs.json")).unwrap()
        )
        .unwrap(),
        expected
    );
    let reloaded = host.reload("napi-rs-plugin").unwrap();
    assert_eq!(
        reloaded.load_result,
        Some(serde_json::json!({
            "previousState": { "reason": "reload", "asyncSum": 3 },
            "asyncSum": 42
        }))
    );
    assert_eq!(
        host.unload("napi-rs-plugin").unwrap(),
        Some(serde_json::json!({ "reason": "unload", "asyncSum": 3 }))
    );
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows"),
    any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")
))]
#[test]
fn plugin_host_authorizes_addon_without_digest_and_aliases_bare_name() {
    use sha2::{Digest, Sha256};

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/node-api/napi-rs/Cargo.toml");
    // Dedicated output directory: parallel tests build the same fixture
    // into their own trees so Cargo never rebuilds under a copy.
    let target_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/node-api-fixtures/napi-alias-plugin-host");
    let temp_dir = target_dir.join("tmp");
    fs::create_dir_all(&temp_dir).unwrap();
    let built = Command::new("cargo")
        .args(["build", "--offline", "--release", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target_dir)
        .env("TMPDIR", &temp_dir)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "napi-rs plugin fixture build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let cdylib_name = if cfg!(target_os = "windows") {
        "napi_vm_napi_rs_fixture.dll"
    } else if cfg!(target_os = "macos") {
        "libnapi_vm_napi_rs_fixture.dylib"
    } else {
        "libnapi_vm_napi_rs_fixture.so"
    };
    let compiled_addon = target_dir.join("release").join(cdylib_name);
    assert!(compiled_addon.is_file(), "napi-rs fixture was not built");
    let addon_bytes = fs::read(&compiled_addon).unwrap();

    // One staged plugin per case, all sharing the built bytes. The
    // package ships a throwing `index.cjs` behind its `main` entry:
    // loading through the alias must never execute it.
    let stage = |label: &str| {
        let dir = TestPluginDir::new(label);
        let addon = dir.0.join("node_modules/bare-native/bare-native.node");
        fs::create_dir_all(addon.parent().unwrap()).unwrap();
        fs::write(&addon, &addon_bytes).unwrap();
        dir.write(
            "node_modules/bare-native/package.json",
            r#"{"name":"bare-native","version":"1.0.0","main":"index.cjs"}"#,
        );
        dir.write(
            "node_modules/bare-native/index.cjs",
            "throw new Error('package loader must not execute');",
        );
        dir.write(
            "main.mjs",
            r#"
import { createRequire } from "node:module";
const localRequire = createRequire(import.meta.url);
const addon = localRequire("bare-native");
const again = localRequire(localRequire.resolve("bare-native"));
const globalAddon = require("bare-native");
export default {
  async onLoad() {
    return {
      sum: addon.add(19, 23),
      cacheIdentity: addon === again,
      globalIdentity: addon === globalAddon,
    };
  },
};
"#,
        );
        dir.manifest("napi-alias-plugin", "main.mjs", "{}");
        (dir, addon)
    };

    let expected = serde_json::json!({ "sum": 42, "cacheIdentity": true, "globalIdentity": true });

    // Digest-optional allowlist plus bare-name alias: the guest loads
    // the addon through `createRequire` without touching `index.cjs`.
    let (dir, addon) = stage("napi-alias-happy");
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    host.configure_napi_addons(
        "napi-alias-plugin",
        RustPluginNapiOptions::default()
            .allow_addon(&addon)
            .allow_addon_alias("bare-native", &addon),
    )
    .unwrap();
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(plugin.load_result, Some(expected.clone()));

    // Explicit digests keep working through the same alias path.
    let (dir, addon) = stage("napi-alias-digest");
    let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    host.configure_napi_addons(
        "napi-alias-plugin",
        RustPluginNapiOptions::default()
            .allow_addon_with_sha256(&addon, digest)
            .allow_addon_alias("bare-native", &addon),
    )
    .unwrap();
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(plugin.load_result, Some(expected.clone()));

    // An alias without an allowlisted target fails closed at load.
    let (dir, addon) = stage("napi-alias-unlisted");
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    host.configure_napi_addons(
        "napi-alias-plugin",
        RustPluginNapiOptions::default().allow_addon_alias("bare-native", &addon),
    )
    .unwrap();
    let error = match host.load(&dir.0) {
        Ok(_) => panic!("alias without an allowlisted target must fail"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("not allowlisted"), "{error}");

    // Duplicate aliases fail instead of shadowing each other.
    let (dir, addon) = stage("napi-alias-duplicate");
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    host.configure_napi_addons(
        "napi-alias-plugin",
        RustPluginNapiOptions::default()
            .allow_addon(&addon)
            .allow_addon_alias("bare-native", &addon)
            .allow_addon_alias("bare-native", &addon),
    )
    .unwrap();
    let error = match host.load(&dir.0) {
        Ok(_) => panic!("duplicate alias must fail"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("already configured"), "{error}");

    // Only bare package names alias; subpaths keep package semantics.
    let (dir, addon) = stage("napi-alias-subpath");
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    host.configure_napi_addons(
        "napi-alias-plugin",
        RustPluginNapiOptions::default()
            .allow_addon(&addon)
            .allow_addon_alias("bare-native/sub", &addon),
    )
    .unwrap();
    let error = match host.load(&dir.0) {
        Ok(_) => panic!("non-bare alias request must fail"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("bare package request"), "{error}");

    // Alias targets outside the plugin root never load, even when an
    // inside file is allowlisted.
    let (dir, addon) = stage("napi-alias-outside");
    let outside = TestPluginDir::new("napi-alias-outside-addon");
    let escaped = outside.0.join("escaped.node");
    fs::write(&escaped, &addon_bytes).unwrap();
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    host.configure_napi_addons(
        "napi-alias-plugin",
        RustPluginNapiOptions::default()
            .allow_addon(&addon)
            .allow_addon_alias("bare-native", &escaped),
    )
    .unwrap();
    let error = match host.load(&dir.0) {
        Ok(_) => panic!("alias target outside the plugin root must fail"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("outside plugin root"), "{error}");
}

fn load_direct_call_plugin(dir: &TestPluginDir, source: &str) -> RustPluginHost {
    dir.write("main.mjs", source);
    dir.manifest("direct-call-plugin", "main.mjs", "{}");
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    host.load(&dir.0).unwrap();
    host
}

fn parse_count() -> u64 {
    crate::parser::PARSE_PROGRAM_COUNT.with(|count| count.get())
}

#[test]
fn direct_call_invokes_export_without_parsing() {
    let dir = TestPluginDir::new("direct-call");
    let mut host = load_direct_call_plugin(
        &dir,
        r#"
export default {
  prefix: "Hi ",
  call(request, context) {
    return { greeting: this.prefix + request.name, via: context.channel };
  },
};
"#,
    );
    let plugin = host.get_mut("direct-call-plugin").unwrap();
    let before = parse_count();
    for _ in 0..3 {
        let result = plugin
            .call_json(
                &serde_json::json!({"name": "Ada"}),
                &serde_json::json!({"channel": "test"}),
            )
            .unwrap();
        assert_eq!(
            result,
            serde_json::json!({"greeting": "Hi Ada", "via": "test"}),
        );
    }
    assert_eq!(
        parse_count(),
        before,
        "steady-state calls must not touch the parser"
    );
}

#[test]
fn direct_call_awaits_promises_and_drains_before_converting() {
    let dir = TestPluginDir::new("direct-call-async");
    let mut host = load_direct_call_plugin(
        &dir,
        r#"
export default {
  async call(request) {
    const seen = await Promise.resolve(request.depth + 1);
    return { seen };
  },
};
"#,
    );
    let plugin = host.get_mut("direct-call-plugin").unwrap();
    let result = plugin
        .call_json(&serde_json::json!({"depth": 41}), &serde_json::json!({}))
        .unwrap();
    assert_eq!(result, serde_json::json!({"seen": 42}));

    // A sync return still observes already-queued microtasks: the drain
    // runs before the result converts back to JSON.
    let dir = TestPluginDir::new("direct-call-drain");
    let mut host = load_direct_call_plugin(
        &dir,
        r#"
export default {
  call() {
    const out = { seen: [] };
    Promise.resolve(1).then((value) => out.seen.push(value));
    return out;
  },
};
"#,
    );
    let plugin = host.get_mut("direct-call-plugin").unwrap();
    let result = plugin
        .call_json(&serde_json::json!({}), &serde_json::json!({}))
        .unwrap();
    assert_eq!(result, serde_json::json!({"seen": [1]}));
}

#[test]
fn direct_call_propagates_exceptions() {
    let dir = TestPluginDir::new("direct-call-throw");
    let mut host = load_direct_call_plugin(
        &dir,
        r#"
export default {
  call() {
    throw new Error("boom-from-guest");
  },
};
"#,
    );
    let plugin = host.get_mut("direct-call-plugin").unwrap();
    let error = plugin
        .call_json(&serde_json::json!({}), &serde_json::json!({}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("boom-from-guest"), "{error}");
}

#[test]
fn direct_call_without_export_fails_closed() {
    let dir = TestPluginDir::new("direct-call-missing");
    let mut host = load_direct_call_plugin(&dir, "export default { onLoad() {} };\n");
    let plugin = host.get_mut("direct-call-plugin").unwrap();
    let error = plugin
        .call_json(&serde_json::json!({}), &serde_json::json!({}))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("must export call(request, context)"),
        "{error}"
    );
}

#[test]
fn direct_call_rejects_undefined_result() {
    let dir = TestPluginDir::new("direct-call-undefined");
    let mut host = load_direct_call_plugin(&dir, "export default { call() {} };\n");
    let plugin = host.get_mut("direct-call-plugin").unwrap();
    let error = plugin
        .call_json(&serde_json::json!({}), &serde_json::json!({}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("must return a result object"), "{error}");
}

#[test]
fn direct_call_value_returns_raw_values_with_this() {
    let dir = TestPluginDir::new("direct-call-value");
    let mut host = load_direct_call_plugin(
        &dir,
        r#"
export default {
  factor: 3,
  call(request) {
    return request.input * this.factor;
  },
};
"#,
    );
    let plugin = host.get_mut("direct-call-plugin").unwrap();
    let request = crate::convert::value_from_json(&serde_json::json!({"input": 14})).unwrap();
    let context = crate::convert::value_from_json(&serde_json::json!({})).unwrap();
    let result = plugin.call_value(request, context).unwrap();
    assert!(matches!(result, Value::Number(number) if number == 42.0));
}

#[test]
fn direct_lifecycle_awaits_async_on_load() {
    let dir = TestPluginDir::new("direct-lifecycle-async");
    dir.write(
        "main.mjs",
        r#"
export default {
  async onLoad(context) {
    const name = await Promise.resolve(context.name);
    return { loaded: name };
  },
};
"#,
    );
    dir.manifest("direct-lifecycle-async", "main.mjs", "{}");
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(
        plugin.load_result,
        Some(serde_json::json!({"loaded": "direct-lifecycle-async"}))
    );
    host.unload("direct-lifecycle-async").unwrap();
}

#[test]
fn colon_capability_installs_from_extra_requests_with_interpreter_access() {
    let dir = TestPluginDir::new("colon-capability");
    dir.write(
        "main.mjs",
        r#"
import { echo } from "host:echo";
export default {
  onLoad() { return { echoed: echo({ a: 1 }) }; },
};
"#,
    );
    // No manifest request: the embedder translates the plugin's ask.
    dir.manifest("colon-capability", "main.mjs", "{}");
    let capability = RustPluginCapability::new("host:echo").export("echo", |interp, args| {
        let value = args.first().cloned().unwrap_or(Value::Undefined);
        let json = crate::convert::value_to_json(interp, &value)?;
        Ok(Value::String(json.to_string()))
    });
    let mut host = RustPluginHost::new(RustPluginHostOptions {
        policy: RustPluginPolicy::default().grant("host:echo", JsonValue::Bool(true)),
        ..RustPluginHostOptions::default()
    });
    host.define_capability(capability).unwrap();
    host.set_extra_capability_requests(BTreeMap::from([(
        "host:echo".to_owned(),
        JsonValue::Bool(true),
    )]))
    .unwrap();
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(plugin.capabilities, ["host:echo"]);
    assert_eq!(
        plugin.load_result,
        Some(serde_json::json!({"echoed": r#"{"a":1}"#}))
    );
    host.unload("colon-capability").unwrap();
}

#[test]
fn extra_request_for_unknown_capability_fails_closed() {
    let dir = TestPluginDir::new("unknown-capability");
    dir.write("main.mjs", "export default { onLoad() { return {}; } };");
    dir.manifest("unknown-capability", "main.mjs", "{}");
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    host.set_extra_capability_requests(BTreeMap::from([(
        "nope".to_owned(),
        JsonValue::Bool(true),
    )]))
    .unwrap();
    let error = match host.load(&dir.0) {
        Ok(_) => panic!("load with an unknown capability request must fail"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("unknown capability"), "{error}");
}

#[test]
fn extra_request_without_grant_does_not_install() {
    let dir = TestPluginDir::new("ungranted-capability");
    dir.write(
        "main.mjs",
        r#"
export default {
  onLoad() {
    try {
      require("host:private");
      return { imported: true };
    } catch {
      return { imported: false };
    }
  },
};
"#,
    );
    dir.manifest("ungranted-capability", "main.mjs", "{}");
    let capability = RustPluginCapability::new("host:private").export("echo", string_value);
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    host.define_capability(capability).unwrap();
    // Requested, but the policy never grants it: the module must not exist.
    host.set_extra_capability_requests(BTreeMap::from([(
        "host:private".to_owned(),
        JsonValue::Bool(true),
    )]))
    .unwrap();
    let plugin = host.load(&dir.0).unwrap();
    assert!(plugin.capabilities.is_empty());
    assert_eq!(
        plugin.load_result,
        Some(serde_json::json!({"imported": false}))
    );
    host.unload("ungranted-capability").unwrap();
}

#[test]
fn repeated_plugin_loads_share_module_parses() {
    let dir = TestPluginDir::new("module-cache");
    dir.write(
        "main.mjs",
        r#"
import { depValue } from "./dep.mjs";
export default {
  onLoad() { return { v: depValue + 1 }; },
};
"#,
    );
    dir.write("dep.mjs", "export const depValue = 41;\n");
    dir.manifest("module-cache", "main.mjs", "{}");
    // First load warms the cache: the two modules plus host shims (fs
    // facade, require/resolve factories, entry wrapper, barrel). The exact
    // warm count varies with what other tests already cached, so assert the
    // invariant that matters: a fresh host reloading the same plugin parses
    // nothing, even though the import scan and evaluation both run again.
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    host.load(&dir.0).unwrap();
    host.unload("module-cache").unwrap();
    let before = parse_count();
    let mut host = RustPluginHost::new(RustPluginHostOptions::default());
    let plugin = host.load(&dir.0).unwrap();
    assert_eq!(parse_count() - before, 0);
    assert_eq!(plugin.load_result, Some(serde_json::json!({"v": 42})));
}
