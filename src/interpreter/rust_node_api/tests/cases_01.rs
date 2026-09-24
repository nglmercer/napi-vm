    #[test]
    fn napi_run_script_preserves_the_outer_loop_budget_and_source_context() {
        let mut interpreter = Interpreter::with_builtins();
        interpreter.set_source("outer script");
        interpreter.set_loop_budget(1);
        interpreter.consume_loop().unwrap();

        let error = interpreter
            .run_script_source("while (true) {}")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Maximum loop iterations exceeded")
        );
        assert_eq!(interpreter.get_source_line(1), Some("outer script"));
    }

    #[test]
    fn integer_conversion_matches_ecmascript_int32_wraparound() {
        assert_eq!(to_int32(4_294_967_297.0), 1);
        assert_eq!(to_int32(-1.0), -1);
        assert_eq!(to_int32(f64::NAN), 0);
        assert_eq!(to_int32(f64::INFINITY), 0);
    }

    #[test]
    fn missing_node_api_imports_have_a_specific_loader_diagnostic() {
        assert_eq!(
            missing_node_api_import_from_loader_error(
                "dlopen failed: undefined symbol: napi_vm_missing_test_import"
            ),
            Some("napi_vm_missing_test_import")
        );
        assert_eq!(
            missing_node_api_import_from_loader_error(
                "dlopen failed: Symbol not found: _node_api_missing_test_import"
            ),
            Some("node_api_missing_test_import")
        );
        assert_eq!(
            missing_node_api_import_from_loader_error("cannot open dependency libnapi-helper.so"),
            None
        );
        assert_eq!(
            missing_node_api_import_from_loader_error("undefined symbol: _Z12napi_helperv"),
            None
        );

        let message = native_addon_loader_error_message(
            "fixture.node",
            "undefined symbol: napi_vm_missing_test_import",
            10,
        );
        assert!(message.contains("[UNSUPPORTED_NODE_API]"));
        assert!(message.contains("`napi_vm_missing_test_import`"));
        assert!(message.contains("versions 1 through 10"));
        assert!(message.contains("declared version could not be read"));
        assert!(message.contains("Node sidecar backend"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn unresolved_node_api_import_fails_before_addon_initialization() {
        const ROOT_ENV: &str = "NAPI_VM_MISSING_NODE_API_FIXTURE_ROOT";
        if let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) {
            let addon = root.join("missing.node");
            let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();
            let mut interpreter = Interpreter::with_builtins();
            interpreter
                .enable_rust_node_api_addons(
                    RustNodeApiOptions::new([root]).allow_native_addon_with_sha256(&addon, digest),
                )
                .unwrap();
            let error = interpreter
                .eval_source("require('./missing.node')")
                .unwrap_err()
                .to_string();
            assert!(error.contains("[UNSUPPORTED_NODE_API]"), "{error}");
            assert!(error.contains("napi_vm_missing_test_import"), "{error}");
            assert!(error.contains("versions 1 through 10"), "{error}");
            return;
        }

        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-missing-import-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
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
            eprintln!("skipping missing-symbol fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");

        let source = root.join("missing.c");
        let addon = root.join("missing.node");
        fs::write(
            &source,
            r#"
#define NAPI_VERSION 1
#include <node_api.h>

extern napi_status napi_vm_missing_test_import(napi_env env);

NAPI_MODULE_INIT() {
  (void)napi_vm_missing_test_import(env);
  return exports;
}
"#,
        )
        .unwrap();
        let built = Command::new("cc")
            .args(["-std=c11", "-O2", "-fPIC", "-shared", "-I"])
            .arg(include)
            .arg(&source)
            .arg("-o")
            .arg(&addon)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "missing-symbol fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );

        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();
        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest),
            )
            .unwrap();
        let error = interpreter
            .eval_source("require('./missing.node')")
            .unwrap_err()
            .to_string();
        assert!(error.contains("[UNSUPPORTED_NODE_API]"), "{error}");
        assert!(error.contains("napi_vm_missing_test_import"), "{error}");
        assert!(error.contains("versions 1 through 10"), "{error}");
        drop(interpreter);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_handles_expire_at_scope_close_without_reusing_their_pointer() {
        let mut arena = NapiHandleArena::default();
        let outer = arena.create(Value::Number(1.0)).unwrap();
        let scope = arena.open_scope().unwrap();
        let inner = arena.create(Value::Number(2.0)).unwrap();
        assert!(matches!(arena.get(inner), Ok(Value::Number(value)) if value == 2.0));
        arena.close_scope(scope).unwrap();
        assert_eq!(arena.get(inner).unwrap_err(), NAPI_INVALID_ARG);
        assert!(matches!(arena.get(outer), Ok(Value::Number(value)) if value == 1.0));
        let replacement = arena.create(Value::Number(3.0)).unwrap();
        assert_ne!(replacement, inner);
        assert!(matches!(arena.get(replacement), Ok(Value::Number(value)) if value == 3.0));
    }

    #[test]
    fn repeated_local_scopes_release_handle_table_entries_and_reuse_slots() {
        let mut arena = NapiHandleArena::default();
        let outer = arena.create(Value::Number(0.0)).unwrap();
        for index in 0..64 {
            let scope = arena.open_scope().unwrap();
            let local = arena.create(Value::Number(index as f64)).unwrap();
            assert_eq!(arena.handles.len(), 2);
            arena.close_scope(scope).unwrap();
            assert_eq!(arena.handles.len(), 1);
            assert_eq!(arena.get(local).unwrap_err(), NAPI_INVALID_ARG);
            assert!(matches!(arena.get(outer), Ok(Value::Number(0.0))));
        }
        assert_eq!(arena.slots.len(), 2);
    }

    #[test]
    fn handle_scopes_must_close_in_lifo_order() {
        let mut arena = NapiHandleArena::default();
        let outer = arena.open_scope().unwrap();
        let inner = arena.open_scope().unwrap();
        assert_eq!(arena.close_scope(outer).unwrap_err(), NAPI_INVALID_ARG);
        arena.close_scope(inner).unwrap();
        arena.close_scope(outer).unwrap();
        assert_eq!(arena.close_scope(0).unwrap_err(), NAPI_INVALID_ARG);
    }

    #[test]
    fn native_handles_are_bound_to_one_environment() {
        let mut owner = NapiHandleArena::default();
        let other = NapiHandleArena::default();
        let handle = owner.create(Value::Number(7.0)).unwrap();
        assert_eq!(other.get(handle).unwrap_err(), NAPI_INVALID_ARG);
        assert!(matches!(owner.get(handle), Ok(Value::Number(value)) if value == 7.0));
    }

    #[test]
    fn closed_scope_handles_become_stale() {
        let mut arena = NapiHandleArena::default();
        let scope_id = arena.open_scope().unwrap();
        let scope_handle = arena.create_scope_handle(scope_id, false).unwrap();
        arena.close_scope_handle(scope_handle).unwrap();
        assert_eq!(
            arena.close_scope_handle(scope_handle).unwrap_err(),
            NAPI_INVALID_ARG
        );
    }

    #[test]
    fn escapable_scope_promotes_one_local_handle_to_its_parent() {
        let mut arena = NapiHandleArena::default();
        let outer = arena.create(Value::Number(7.0)).unwrap();
        let scope_id = arena.open_escapable_scope().unwrap();
        let scope_handle = arena.create_scope_handle(scope_id, true).unwrap();
        let escapee = arena.create(Value::String("escaped".into())).unwrap();
        let escaped = arena.escape_handle(scope_handle, escapee).unwrap();

        assert!(matches!(arena.get(escaped), Ok(Value::String(ref value)) if value == "escaped"));
        assert_eq!(
            arena.escape_handle(scope_handle, escapee).unwrap_err(),
            NAPI_ESCAPE_CALLED_TWICE
        );
        arena.close_escapable_scope_handle(scope_handle).unwrap();
        assert_eq!(arena.get(escapee).unwrap_err(), NAPI_INVALID_ARG);
        assert!(matches!(arena.get(escaped), Ok(Value::String(ref value)) if value == "escaped"));
        assert!(matches!(arena.get(outer), Ok(Value::Number(7.0))));
    }

    #[test]
    fn escapable_scope_rejects_parent_handles_and_regular_close_calls() {
        let mut arena = NapiHandleArena::default();
        let parent_handle = arena.create(Value::Number(7.0)).unwrap();
        let scope_id = arena.open_escapable_scope().unwrap();
        let scope_handle = arena.create_scope_handle(scope_id, true).unwrap();

        assert_eq!(
            arena
                .escape_handle(scope_handle, parent_handle)
                .unwrap_err(),
            NAPI_HANDLE_SCOPE_MISMATCH
        );
        assert_eq!(
            arena.close_scope_handle(scope_handle).unwrap_err(),
            NAPI_HANDLE_SCOPE_MISMATCH
        );

        let local = arena.create(Value::Null).unwrap();
        let escaped = arena.escape_handle(scope_handle, local).unwrap();
        arena.close_escapable_scope_handle(scope_handle).unwrap();
        assert!(matches!(arena.get(escaped), Ok(Value::Null)));
    }

    #[test]
    fn deferred_resolution_without_interpreter_rejects_objects_but_accepts_undefined() {
        let promise = Value::pending_promise();

        assert_eq!(
            settle_deferred_without_interpreter(&promise, Value::object(Vec::new()), false),
            Err(NAPI_GENERIC_FAILURE)
        );
        assert_eq!(promise.borrow().state, PromiseState::Pending);

        settle_deferred_without_interpreter(&promise, Value::Undefined, false).unwrap();
        let promise = promise.borrow();
        assert_eq!(promise.state, PromiseState::Fulfilled);
        assert!(matches!(promise.value, Value::Undefined));
    }

    #[test]
    fn napi_fatal_error_terminates_only_its_child_process() {
        const CHILD_ROOT_ENV: &str = "NAPI_VM_FATAL_ERROR_FIXTURE_ROOT";
        if let Some(root) = std::env::var_os(CHILD_ROOT_ENV).map(PathBuf::from) {
            let addon = root.join("fixture.node");
            let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();
            let mut interpreter = Interpreter::with_builtins();
            interpreter
                .enable_rust_node_api_addons(
                    RustNodeApiOptions::new([root]).allow_native_addon_with_sha256(&addon, digest),
                )
                .unwrap();
            let _ = interpreter.eval_source("require('./fixture.node').fatal();");
            panic!("napi_fatal_error unexpectedly returned");
        }

        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-fatal-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let compiler = Command::new("cc").arg("--version").output();
        let include_dirs = [
            std::env::var_os("NODE_INCLUDE_DIR").map(PathBuf::from),
            Some(PathBuf::from("/usr/include/node")),
            Some(PathBuf::from("/usr/local/include/node")),
        ];
        let include = include_dirs
            .into_iter()
            .flatten()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(compiler), Some(include)) = (compiler, include) else {
            eprintln!("skipping fatal Node-API fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");

        let source = root.join("fixture.c");
        let addon = root.join("fixture.node");
        let c_source = r#"
#define NAPI_VERSION 1
#include <node_api.h>

static napi_value fatal_probe(napi_env env, napi_callback_info info) {
  (void)env;
  (void)info;
  napi_fatal_error("fixture", NAPI_AUTO_LENGTH, "fatal message", NAPI_AUTO_LENGTH);
}

NAPI_MODULE_INIT() {
  napi_value function;
  if (napi_create_function(env, "fatal", NAPI_AUTO_LENGTH, fatal_probe,
                           NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "fatal", function) != napi_ok)
    return NULL;
  return exports;
}
"#;
        fs::write(&source, c_source).unwrap();
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
            "fatal Node-API fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );

        let current_exe = std::env::current_exe().unwrap();
        let child = Command::new("sh")
            .args(["-c", "ulimit -c 0; exec \"$@\"", "napi-vm-fatal-child"])
            .arg(current_exe)
            .args([
                "--exact",
                "interpreter::rust_node_api::tests::napi_fatal_error_terminates_only_its_child_process",
                "--nocapture",
            ])
            .env(CHILD_ROOT_ENV, &root)
            .output()
            .unwrap();
        assert!(
            !child.status.success(),
            "fatal error child unexpectedly passed"
        );
        let stderr = String::from_utf8_lossy(&child.stderr);
        assert!(
            stderr.contains("FATAL ERROR: fixture: fatal message"),
            "fatal diagnostic was missing: {stderr}"
        );
        drop(child);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn require_module_node_loads_an_allowlisted_bare_napi_package() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-legacy-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let compiler = Command::new("cc").arg("--version").output();
        let include_dirs = [
            std::env::var_os("NODE_INCLUDE_DIR").map(PathBuf::from),
            Some(PathBuf::from("/usr/include/node")),
            Some(PathBuf::from("/usr/local/include/node")),
        ];
        let include = include_dirs
            .into_iter()
            .flatten()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(compiler), Some(include)) = (compiler, include) else {
            eprintln!("skipping legacy Node-API fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");

        let package_root = root.join("node_modules/module.node");
        let release_dir = package_root.join("build/Release");
        fs::create_dir_all(&release_dir).unwrap();
        let source = root.join("fixture.c");
        let addon = release_dir.join("fixture.node");
        let c_source = r#"
#define NAPI_VERSION 1
#include <node_api.h>

static napi_value initialize(napi_env env, napi_value exports) {
  napi_value value, global, script, ignored;
  napi_status run_script_status;
  if (napi_create_string_utf8(env, "legacy registration", NAPI_AUTO_LENGTH,
                              &value) != napi_ok ||
      napi_set_named_property(env, exports, "kind", value) != napi_ok ||
      napi_get_global(env, &global) != napi_ok ||
      napi_set_named_property(env, global, "expectedAddonExports", exports) != napi_ok ||
      napi_create_string_utf8(env,
        "globalThis.partialAddon = globalThis.reenterAddonRequire()",
        NAPI_AUTO_LENGTH, &script) != napi_ok)
    return NULL;
  run_script_status = napi_run_script(env, script, &ignored);
  if (napi_create_int32(env, run_script_status, &value) != napi_ok ||
      napi_set_named_property(env, exports, "runScriptStatus", value) != napi_ok)
    return NULL;
  return exports;
}

static napi_module module = {
  NAPI_MODULE_VERSION, 0, __FILE__, initialize, "legacy_fixture", NULL,
  {NULL, NULL, NULL, NULL}
};

__attribute__((constructor)) static void register_module(void) {
  napi_module_register(&module);
}
"#;
        fs::write(&source, c_source).unwrap();
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
            "legacy Node-API fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        fs::write(
            package_root.join("package.json"),
            r#"{"name":"module.node","exports":{".":{"node-addons":"./build/Release/fixture.node","default":"./fallback.cjs"}}}"#,
        )
        .unwrap();
        fs::write(
            package_root.join("fallback.cjs"),
            "module.exports = {kind: 'javascript fallback'};",
        )
        .unwrap();
        let main = root.join("main.cjs");
        fs::write(
            &main,
            "globalThis.reenterAddonRequire = () => require('module.node'); const addon = require('module.node'); module.exports = {kind: addon.kind, cached: addon === require('module.node'), sameByPath: addon === require('./node_modules/module.node/build/Release/fixture.node'), reentrant: globalThis.partialAddon === addon, expectedExports: globalThis.expectedAddonExports === addon, runScriptStatus: addon.runScriptStatus};",
        )
        .unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut interpreter = Interpreter::with_builtins();
        let runtime = interpreter
            .enable_native_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest)
                    .entry(&main),
            )
            .unwrap();
        assert_eq!(runtime.backend_name(), "rust-node-api");
        assert!(matches!(&runtime, NativeAddonRuntime::RustNodeApi(_)));
        runtime.preflight_addon(&addon).unwrap();
        let value = interpreter
            .eval_source("JSON.stringify(require('./main.cjs'))")
            .unwrap();
        assert!(
            matches!(value, Value::String(ref value) if value == r#"{"kind":"legacy registration","cached":true,"sameByPath":true,"reentrant":true,"expectedExports":true,"runScriptStatus":0}"#)
        );

        let node = Command::new("node")
            .current_dir(&root)
            .args([
                "-e",
                "process.stdout.write(JSON.stringify(require('./main.cjs')))",
            ])
            .output()
            .unwrap();
        assert!(
            node.status.success(),
            "Node bare-package fixture failed: {}",
            String::from_utf8_lossy(&node.stderr)
        );
        assert_eq!(node.stdout, br#"{"kind":"legacy registration","cached":true,"sameByPath":true,"reentrant":true,"expectedExports":true,"runScriptStatus":0}"#);

        let bun_version = Command::new("bun")
            .arg("--version")
            .output()
            .ok()
            .filter(|version| version.status.success());
        if let Some(version) = bun_version {
            let bun = Command::new("bun")
                .current_dir(&root)
                .args([
                    "-e",
                    "process.stdout.write(JSON.stringify(require('./main.cjs')))",
                ])
                .output()
                .unwrap();
            if bun.status.success() {
                let bun_value: serde_json::Value = serde_json::from_slice(&bun.stdout).unwrap();
                assert_eq!(bun_value["runScriptStatus"].as_i64(), Some(0));
                assert_eq!(bun.stdout, node.stdout, "Node and Bun results differ");
            } else {
                let stderr = String::from_utf8_lossy(&bun.stderr);
                assert!(
                    stderr.contains("Node-API module \"legacy_fixture\" returned an error"),
                    "unexpected Bun failure for reentrant initializer fixture: {stderr}"
                );
                eprintln!(
                    "HOST_BRIDGE: Bun {} rejected guest re-entry from a native initializer; Node and napi-vm were compared for this fixture",
                    String::from_utf8_lossy(&version.stdout).trim()
                );
            }
        }

        runtime.shutdown().unwrap();
        assert!(runtime.is_shutdown());
        runtime.shutdown().unwrap();
        let host = runtime.rust_node_api().expect("Rust Node-API backend");
        let after_shutdown = crate::interpreter::NativeAddonLoader::load(host, &addon).unwrap_err();
        assert!(
            after_shutdown
                .to_string()
                .contains("Rust Node-API host has been shut down")
        );

        drop(interpreter);
        fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rust_host_selects_and_loads_a_napi_tagged_prebuild_through_bare_require() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-prebuild-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let package_root = root.join("node_modules/prebuilt");
        let platform = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        };
        let architecture = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "x86" => "ia32",
            "aarch64" => "arm64",
            other => other,
        };
        let tuple = format!("{platform}-{architecture}");
        let prebuild_dir = package_root.join("prebuilds").join(&tuple);
        fs::create_dir_all(&prebuild_dir).unwrap();

        let compiler = Command::new("cc").arg("--version").output();
        let include_dirs = [
            std::env::var_os("NODE_INCLUDE_DIR").map(PathBuf::from),
            Some(PathBuf::from("/usr/include/node")),
            Some(PathBuf::from("/usr/local/include/node")),
        ];
        let include = include_dirs
            .into_iter()
            .flatten()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(compiler), Some(include)) = (compiler, include) else {
            eprintln!("skipping Node-API prebuild fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");

        let source = root.join("fixture.c");
        let addon = prebuild_dir.join("node.napi.node");
        fs::write(
            &source,
            r#"
#define NAPI_VERSION 1
#include <node_api.h>

static napi_value initialize(napi_env env, napi_value exports) {
  napi_value value;
  if (napi_create_string_utf8(env, "selected prebuild", NAPI_AUTO_LENGTH,
                              &value) != napi_ok ||
      napi_set_named_property(env, exports, "kind", value) != napi_ok)
    return NULL;
  return exports;
}

NAPI_MODULE(prebuild_fixture, initialize)
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
            "Node-API prebuild fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );

        fs::write(
            package_root.join("package.json"),
            r#"{"name":"prebuilt","main":"index.cjs"}"#,
        )
        .unwrap();
        let node_gyp_build = root.join("node_modules/node-gyp-build");
        fs::create_dir_all(&node_gyp_build).unwrap();
        fs::write(
            node_gyp_build.join("index.cjs"),
            format!(
                "const path = require('node:path'); const tuple = {tuple:?}; function resolve(dir) {{ return path.join(dir, 'prebuilds', tuple, 'node.napi.node'); }} function load(dir) {{ return require(resolve(dir)); }} load.path = load.resolve = resolve; module.exports = load;"
            ),
        )
        .unwrap();
        fs::write(
            node_gyp_build.join("package.json"),
            r#"{"name":"node-gyp-build","main":"index.cjs"}"#,
        )
        .unwrap();
        fs::write(
            package_root.join("index.cjs"),
            "const load = require('node-gyp-build'); const filename = load.path(__dirname); const addon = load(__dirname); module.exports = {kind: addon.kind, filename, helperPath: require.resolve('node-gyp-build'), resolved: load.resolve(__dirname) === filename, aliases: load.path === load.resolve, nativeCached: addon === require(filename)};",
        )
        .unwrap();
        let main = root.join("main.cjs");
        fs::write(
            &main,
            "const addon = require('prebuilt'); module.exports = {...addon, packageCached: addon === require('prebuilt')};",
        )
        .unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_package_prebuild_with_sha256(&package_root, digest)
                    .entry(&main),
            )
            .unwrap();
        let vm_value = interpreter
            .eval_source("JSON.stringify(require('./main.cjs'))")
            .unwrap();
        let Value::String(vm_json) = &vm_value else {
            panic!("Node-API prebuild fixture did not return JSON: {vm_value:?}");
        };
        let vm_json: serde_json::Value = serde_json::from_str(vm_json).unwrap();

        let runner = "process.stdout.write(JSON.stringify(require('./main.cjs')))";
        for runtime in ["node", "bun"] {
            let available = Command::new(runtime).arg("--version").output();
            let Ok(version) = available else {
                continue;
            };
            if !version.status.success() {
                continue;
            }
            let reference = Command::new(runtime)
                .current_dir(&root)
                .args(["-e", runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "{runtime} N-API prebuild fixture failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let reference: serde_json::Value = serde_json::from_slice(&reference.stdout).unwrap();
            assert_eq!(vm_json, reference, "{runtime} and napi-vm differ");
        }

        drop(interpreter);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_napi_v10_external_strings_property_keys_and_arraybuffer_buffers() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-v10-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let compiler = Command::new("cc").arg("--version").output();
        let include_dirs = [
            std::env::var_os("NODE_INCLUDE_DIR").map(PathBuf::from),
            Some(PathBuf::from("/usr/include/node")),
            Some(PathBuf::from("/usr/local/include/node")),
        ];
        let include = include_dirs
            .into_iter()
            .flatten()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(compiler), Some(include)) = (compiler, include) else {
            eprintln!("skipping Node-API v10 fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");

        let source = root.join("fixture.c");
        let addon = root.join("fixture.node");
        let c_source = r#"
#define NAPI_VERSION 10
#include <node_api.h>
#include <stdlib.h>

static int external_string_finalizers;
static void external_string_finalize(napi_env env, void* data, void* hint) {
  (void)env;
  (void)hint;
  free(data);
  external_string_finalizers++;
}

static napi_value external_strings(napi_env env, napi_callback_info info) {
  char* latin1 = (char*)malloc(3);
  char16_t* utf16 = (char16_t*)malloc(3 * sizeof(char16_t));
  napi_value latin_value, utf16_value, result, field;
  bool latin1_copied = false, utf16_copied = false;
  (void)info;
  if (latin1 == NULL || utf16 == NULL) {
    free(latin1);
    free(utf16);
    return NULL;
  }
  latin1[0] = 'L'; latin1[1] = (char)0xe9; latin1[2] = 'X';
  utf16[0] = 'O'; utf16[1] = 0x03a9; utf16[2] = 'K';
  if (node_api_create_external_string_latin1(
          env, latin1, 3, external_string_finalize, NULL, &latin_value,
          &latin1_copied) != napi_ok ||
      node_api_create_external_string_utf16(
          env, utf16, 3, external_string_finalize, NULL, &utf16_value,
          &utf16_copied) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "latin1", latin_value) != napi_ok ||
      napi_set_named_property(env, result, "utf16", utf16_value) != napi_ok ||
      napi_get_boolean(env, latin1_copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "latin1Copied", field) != napi_ok ||
      napi_get_boolean(env, utf16_copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "utf16Copied", field) != napi_ok ||
      napi_create_int32(env, external_string_finalizers, &field) != napi_ok ||
      napi_set_named_property(env, result, "finalizersAtReturn", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value property_keys(napi_env env, napi_callback_info info) {
  static const char latin1_key[] = {'k', 'e', 'y', (char)0xe9};
  static const char utf8_key[] = "utf8-雪";
  static const char16_t utf16_key[] = {'u', '1', '6', 0x03a9};
  napi_value result, key, value;
  (void)info;
  if (napi_create_object(env, &result) != napi_ok ||
      node_api_create_property_key_latin1(env, latin1_key,
                                          sizeof(latin1_key), &key) != napi_ok ||
      napi_create_string_utf8(env, "latin1-value", NAPI_AUTO_LENGTH,
                              &value) != napi_ok ||
      napi_set_property(env, result, key, value) != napi_ok ||
      node_api_create_property_key_utf8(env, utf8_key, sizeof(utf8_key) - 1,
                                        &key) != napi_ok ||
      napi_create_string_utf8(env, "utf8-value", NAPI_AUTO_LENGTH,
                              &value) != napi_ok ||
      napi_set_property(env, result, key, value) != napi_ok ||
      node_api_create_property_key_utf16(env, utf16_key,
                                         sizeof(utf16_key) / sizeof(char16_t),
                                         &key) != napi_ok ||
      napi_create_string_utf8(env, "utf16-value", NAPI_AUTO_LENGTH,
                              &value) != napi_ok ||
      napi_set_property(env, result, key, value) != napi_ok)
    return NULL;
  return result;
}

static napi_value buffer_from_arraybuffer(napi_env env, napi_callback_info info) {
  napi_value arraybuffer, backing, buffer, result, field;
  void* bytes = NULL;
  void* buffer_bytes = NULL;
  size_t buffer_length = 0;
  bool is_buffer = false;
  (void)info;
  if (napi_create_arraybuffer(env, 6, &bytes, &arraybuffer) != napi_ok ||
      bytes == NULL)
    return NULL;
  for (size_t i = 0; i < 6; i++) ((uint8_t*)bytes)[i] = (uint8_t)(10 + i);
  if (napi_create_typedarray(env, napi_uint8_array, 6, arraybuffer, 0,
                             &backing) != napi_ok ||
      node_api_create_buffer_from_arraybuffer(env, arraybuffer, 2, 3,
                                             &buffer) != napi_ok ||
      napi_is_buffer(env, buffer, &is_buffer) != napi_ok ||
      napi_get_buffer_info(env, buffer, &buffer_bytes, &buffer_length) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "backing", backing) != napi_ok ||
      napi_set_named_property(env, result, "buffer", buffer) != napi_ok ||
      napi_get_boolean(env, is_buffer, &field) != napi_ok ||
      napi_set_named_property(env, result, "isBuffer", field) != napi_ok ||
      napi_get_boolean(env, buffer_bytes == (uint8_t*)bytes + 2, &field) != napi_ok ||
      napi_set_named_property(env, result, "sharesBytes", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)buffer_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "length", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value buffer_range_error(napi_env env, napi_callback_info info) {
  napi_value arraybuffer, result;
  (void)info;
  if (napi_create_arraybuffer(env, 4, NULL, &arraybuffer) != napi_ok)
    return NULL;
  (void)node_api_create_buffer_from_arraybuffer(env, arraybuffer, 3, 2,
                                                &result);
  return NULL;
}

static napi_value node_version_probe(napi_env env, napi_callback_info info) {
  const napi_node_version* version = NULL;
  napi_value result, field;
  (void)info;
  if (napi_get_node_version(env, &version) != napi_ok || version == NULL ||
      napi_get_node_version(env, NULL) != napi_invalid_arg ||
      napi_create_object(env, &result) != napi_ok ||
      napi_create_uint32(env, version->major, &field) != napi_ok ||
      napi_set_named_property(env, result, "major", field) != napi_ok ||
      napi_create_uint32(env, version->minor, &field) != napi_ok ||
      napi_set_named_property(env, result, "minor", field) != napi_ok ||
      napi_create_uint32(env, version->patch, &field) != napi_ok ||
      napi_set_named_property(env, result, "patch", field) != napi_ok ||
      napi_create_string_utf8(env, version->release, NAPI_AUTO_LENGTH, &field) != napi_ok ||
      napi_set_named_property(env, result, "release", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value uv_loop_probe(napi_env env, napi_callback_info info) {
  struct uv_loop_s* loop = NULL;
  const napi_extended_error_info* error_info = NULL;
  napi_status status;
  napi_status invalid_status;
  napi_value result, field, error_message;
  const char* message = "napi_get_uv_event_loop succeeded";
  (void)info;
  invalid_status = napi_get_uv_event_loop(env, NULL);
  status = napi_get_uv_event_loop(env, &loop);
  if (status != napi_ok) {
    if (napi_get_last_error_info(env, &error_info) != napi_ok ||
        error_info == NULL || error_info->error_message == NULL)
      return NULL;
    message = error_info->error_message;
  }
  if (napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, status, &field) != napi_ok ||
      napi_set_named_property(env, result, "status", field) != napi_ok ||
      napi_get_boolean(env, loop == NULL, &field) != napi_ok ||
      napi_set_named_property(env, result, "isNull", field) != napi_ok ||
      napi_create_string_utf8(env, message, NAPI_AUTO_LENGTH, &error_message) != napi_ok ||
      napi_set_named_property(env, result, "errorMessage", error_message) != napi_ok ||
      napi_create_int32(env, invalid_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "invalidStatus", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value fatal_exception_probe(napi_env env, napi_callback_info info) {
  napi_value message, error;
  (void)info;
  if (napi_create_string_utf8(env, "fatal exception", NAPI_AUTO_LENGTH,
                              &message) != napi_ok ||
      napi_create_error(env, NULL, message, &error) != napi_ok ||
      napi_fatal_exception(env, error) != napi_ok)
    return NULL;
  return NULL;
}

NAPI_MODULE_INIT() {
  napi_value function;
  if (napi_create_function(env, "externalStrings", NAPI_AUTO_LENGTH,
                           external_strings, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "externalStrings", function) != napi_ok ||
      napi_create_function(env, "propertyKeys", NAPI_AUTO_LENGTH,
                           property_keys, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "propertyKeys", function) != napi_ok ||
      napi_create_function(env, "bufferFromArrayBuffer", NAPI_AUTO_LENGTH,
                           buffer_from_arraybuffer, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "bufferFromArrayBuffer", function) != napi_ok ||
      napi_create_function(env, "bufferRangeError", NAPI_AUTO_LENGTH,
                           buffer_range_error, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "bufferRangeError", function) != napi_ok ||
      napi_create_function(env, "nodeVersion", NAPI_AUTO_LENGTH,
                           node_version_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "nodeVersion", function) != napi_ok ||
      napi_create_function(env, "uvLoop", NAPI_AUTO_LENGTH,
                           uv_loop_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "uvLoop", function) != napi_ok ||
      napi_create_function(env, "fatalException", NAPI_AUTO_LENGTH,
                           fatal_exception_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "fatalException", function) != napi_ok)
    return NULL;
  return exports;
}
"#;
        fs::write(&source, c_source).unwrap();
        let built = Command::new("cc")
            .args([
                "-std=c11",
                "-O2",
                "-fPIC",
                "-shared",
                "-DNAPI_VERSION=10",
                "-I",
            ])
            .arg(&include)
            .arg(&source)
            .arg("-o")
            .arg(&addon)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "Node-API v10 fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        fs::write(
            root.join("main.cjs"),
            r#"
const addon = require('./fixture.node');
const external = addon.externalStrings();
const nodeVersion = addon.nodeVersion();
const uvLoop = addon.uvLoop();
const properties = addon.propertyKeys();
const buffer = addon.bufferFromArrayBuffer();
const backing = buffer.backing;
const view = buffer.buffer;
const before = [view[0], view[1], view[2]];
view[1] = 99;
let rangeErrorName = 'none';
try { addon.bufferRangeError(); } catch (error) { rangeErrorName = error.name; }
module.exports = {
  external,
  nodeVersion,
  uvLoop,
  propertyKeys: Object.keys(properties),
  propertyValues: [properties['keyé'], properties['utf8-雪'], properties['u16Ω']],
  buffer: {
    isBuffer: buffer.isBuffer,
    sharesBytes: buffer.sharesBytes,
    length: buffer.length,
    before,
    after: [backing[0], backing[1], backing[2], backing[3], backing[4], backing[5]],
  },
  rangeErrorName,
};
"#,
        )
        .unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest)
                    .reported_node_version(ReportedNodeVersion::new(22, 17, 3))
                    .entry(root.join("main.cjs")),
            )
            .unwrap();
        let observer = unsafe { Library::open(Some(addon.as_os_str()), RTLD_NOW) }.unwrap();
        let vm_report = interpreter
            .eval_source("JSON.stringify(require('./main.cjs'));")
            .unwrap();
        let Value::String(ref vm_report) = vm_report else {
            panic!("Node-API v10 VM fixture did not return JSON");
        };
        let mut vm_report: serde_json::Value = serde_json::from_str(vm_report).unwrap();
        assert_eq!(vm_report["external"]["latin1"], "LéX");
        assert_eq!(vm_report["external"]["utf16"], "OΩK");
        assert_eq!(vm_report["external"]["latin1Copied"], true);
        assert_eq!(vm_report["external"]["utf16Copied"], true);
        assert_eq!(vm_report["external"]["finalizersAtReturn"], 2);
        assert_eq!(
            vm_report["nodeVersion"],
            serde_json::json!({"major": 22, "minor": 17, "patch": 3, "release": "napi-vm"})
        );
        assert_eq!(
            vm_report["uvLoop"],
            serde_json::json!({
                "status": NAPI_GENERIC_FAILURE,
                "isNull": true,
                "errorMessage": "napi_get_uv_event_loop is unsupported by the Rust Node-API backend: libuv is not embedded",
                "invalidStatus": NAPI_INVALID_ARG
            })
        );
        assert_eq!(vm_report["propertyKeys"].as_array().unwrap().len(), 3);
        assert_eq!(
            vm_report["propertyValues"],
            serde_json::json!(["latin1-value", "utf8-value", "utf16-value"])
        );
        assert_eq!(vm_report["buffer"]["isBuffer"], true);
        assert_eq!(vm_report["buffer"]["sharesBytes"], true);
        assert_eq!(vm_report["buffer"]["length"], 3);
        assert_eq!(
            vm_report["buffer"]["before"],
            serde_json::json!([12, 13, 14])
        );
        assert_eq!(
            vm_report["buffer"]["after"],
            serde_json::json!([10, 11, 12, 99, 14, 15])
        );
        assert_eq!(vm_report["rangeErrorName"], "RangeError");

        interpreter
            .eval_source(
                "globalThis.process = { emit: function(name, error) { globalThis.fatalEvent = name + ':' + error.message; return true; } }; require('./fixture.node').fatalException();",
            )
            .unwrap();
        let Value::String(ref fatal_report) = interpreter.eval_source("fatalEvent").unwrap() else {
            panic!("fatal exception event did not reach the guest process handler");
        };
        let unhandled = interpreter
            .eval_source(
                "globalThis.process.emit = function() { return false; }; require('./fixture.node').fatalException();",
            )
            .unwrap_err();
        assert!(matches!(
            unhandled,
            VmErr::Throw(Value::Error(ref error)) if error.message == "fatal exception"
        ));
        let fatal_runner = r#"const addon = require('./fixture.node');
let event = '';
process.once('uncaughtException', error => { event = `uncaughtException:${error.message}`; });
addon.fatalException();
setImmediate(() => {
  if (!event) { process.stderr.write('fatal exception event was not delivered'); process.exitCode = 1; }
  else process.stdout.write(event);
});"#;
        for runtime in ["node", "bun"] {
            if !Command::new(runtime)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
            {
                continue;
            }
            let reference = Command::new(runtime)
                .current_dir(&root)
                .args(["-e", fatal_runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "{runtime} fatal exception fixture failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&reference.stdout),
                fatal_report.as_str(),
                "fatal exception behavior differs from {runtime}"
            );
        }

        // Whether a runtime can retain an external string is an engine choice.
        // Compare the actual JavaScript strings and byte-view behavior while
        // checking each engine's copied/finalizer contract separately.
        let external = vm_report["external"].as_object_mut().unwrap();
        external.remove("latin1Copied");
        external.remove("utf16Copied");
        external.remove("finalizersAtReturn");
        vm_report.as_object_mut().unwrap().remove("nodeVersion");
        vm_report.as_object_mut().unwrap().remove("uvLoop");
        let runner = r#"const value = require('./main.cjs');
const e = value.external;
const copied = Number(e.latin1Copied) + Number(e.utf16Copied);
if (e.finalizersAtReturn !== copied) throw new Error('external string finalizer contract violated');
delete e.latin1Copied; delete e.utf16Copied; delete e.finalizersAtReturn;
delete value.nodeVersion;
delete value.uvLoop;
process.stdout.write(JSON.stringify(value));"#;
        let mut reference_reports = Vec::new();
        for runtime in ["node", "bun"] {
            if !Command::new(runtime)
                .arg("--version")
                .output()
                .is_ok_and(|out| out.status.success())
            {
                continue;
            }
            let reference = Command::new(runtime)
                .current_dir(&root)
                .args(["-e", runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "{runtime} Node-API v10 fixture failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let report: serde_json::Value = serde_json::from_slice(&reference.stdout)
                .unwrap_or_else(|_| {
                    panic!(
                        "{runtime} v10 result was not JSON: {}",
                        String::from_utf8_lossy(&reference.stdout)
                    )
                });
            reference_reports.push((runtime, report));
        }
        assert!(
            !reference_reports.is_empty(),
            "Node or Bun is required for the Node-API v10 differential fixture"
        );
        for (runtime, report) in reference_reports {
            assert_eq!(
                vm_report, report,
                "Node-API v10 result differs from {runtime}"
            );
        }
        drop(interpreter);
        drop(observer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_napi_v9_symbols_syntax_errors_and_module_file_url() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-v9-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let compiler = Command::new("cc").arg("--version").output();
        let include_dirs = [
            std::env::var_os("NODE_INCLUDE_DIR").map(PathBuf::from),
            Some(PathBuf::from("/usr/include/node")),
            Some(PathBuf::from("/usr/local/include/node")),
        ];
        let include = include_dirs
            .into_iter()
            .flatten()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(compiler), Some(include)) = (compiler, include) else {
            eprintln!("skipping Node-API v9 fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");

        let source = root.join("fixture.c");
        let addon = root.join("fixture.node");
        let c_source = r#"
#define NAPI_VERSION 9
#include <node_api.h>

static napi_value global_symbol(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (node_api_symbol_for(env, "napi-vm-v9-global", NAPI_AUTO_LENGTH,
                          &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value module_file_name(napi_env env, napi_callback_info info) {
  const char* file_name = NULL;
  napi_value result;
  (void)info;
  if (node_api_get_module_file_name(env, &file_name) != napi_ok ||
      file_name == NULL ||
      napi_create_string_utf8(env, file_name, NAPI_AUTO_LENGTH, &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value create_syntax_error(napi_env env, napi_callback_info info) {
  napi_value code, message, result;
  (void)info;
  if (napi_create_string_utf8(env, "E_CREATED_SYNTAX", NAPI_AUTO_LENGTH,
                              &code) != napi_ok ||
      napi_create_string_utf8(env, "created syntax failure", NAPI_AUTO_LENGTH,
                              &message) != napi_ok ||
      node_api_create_syntax_error(env, code, message, &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value throw_syntax_error(napi_env env, napi_callback_info info) {
  (void)info;
  if (node_api_throw_syntax_error(env, "E_THROWN_SYNTAX",
                                  "thrown syntax failure") != napi_ok)
    return NULL;
  return NULL;
}

NAPI_MODULE_INIT() {
  napi_value function;
  if (napi_create_function(env, "globalSymbol", NAPI_AUTO_LENGTH,
                           global_symbol, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "globalSymbol", function) != napi_ok ||
      napi_create_function(env, "moduleFileName", NAPI_AUTO_LENGTH,
                           module_file_name, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "moduleFileName", function) != napi_ok ||
      napi_create_function(env, "createSyntaxError", NAPI_AUTO_LENGTH,
                           create_syntax_error, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "createSyntaxError", function) != napi_ok ||
      napi_create_function(env, "throwSyntaxError", NAPI_AUTO_LENGTH,
                           throw_syntax_error, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "throwSyntaxError", function) != napi_ok)
    return NULL;
  return exports;
}
"#;
        fs::write(&source, c_source).unwrap();
        let built = Command::new("cc")
            .args([
                "-std=c11",
                "-O2",
                "-fPIC",
                "-shared",
                "-DNAPI_VERSION=9",
                "-I",
            ])
            .arg(&include)
            .arg(&source)
            .arg("-o")
            .arg(&addon)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "Node-API v9 fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        fs::write(
            root.join("main.cjs"),
            r#"
const addon = require('./fixture.node');
const firstSymbol = addon.globalSymbol();
const secondSymbol = addon.globalSymbol();
const created = addon.createSyntaxError();
let thrown;
try {
  addon.throwSyntaxError();
} catch (error) {
  thrown = {name: error.name, message: error.message, code: error.code};
}
const moduleFileName = addon.moduleFileName();
module.exports = {
  symbolIdentity: firstSymbol === secondSymbol,
  symbolMatchesGuestRegistry: firstSymbol === Symbol.for('napi-vm-v9-global'),
  symbolKey: Symbol.keyFor(firstSymbol),
  moduleFileName,
  moduleFileNameIsUrl: moduleFileName.startsWith('file://'),
  created: {name: created.name, message: created.message, code: created.code},
  thrown,
};
"#,
        )
        .unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest)
                    .entry(root.join("main.cjs")),
            )
            .unwrap();
        let observer = unsafe { Library::open(Some(addon.as_os_str()), RTLD_NOW) }.unwrap();
        let vm_report = interpreter
            .eval_source("JSON.stringify(require('./main.cjs'));")
            .unwrap();
        let Value::String(ref vm_report) = vm_report else {
            panic!("Node-API v9 VM fixture did not return JSON");
        };
        let vm_report: serde_json::Value = serde_json::from_str(vm_report).unwrap();

        let runner = "process.stdout.write(JSON.stringify(require('./main.cjs')))";
        let mut reference_reports = Vec::new();
        for runtime in ["node", "bun"] {
            if !Command::new(runtime)
                .arg("--version")
                .output()
                .is_ok_and(|out| out.status.success())
            {
                continue;
            }
            let reference = Command::new(runtime)
                .current_dir(&root)
                .args(["-e", runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "{runtime} Node-API v9 fixture failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let report: serde_json::Value = serde_json::from_slice(&reference.stdout)
                .unwrap_or_else(|_| {
                    panic!(
                        "{runtime} v9 result was not JSON: {}",
                        String::from_utf8_lossy(&reference.stdout)
                    )
                });
            reference_reports.push((runtime, report));
        }
        assert!(
            !reference_reports.is_empty(),
            "Node or Bun is required for the Node-API v9 differential fixture"
        );
        for (runtime, report) in reference_reports {
            assert_eq!(
                vm_report, report,
                "Node-API v9 result differs from {runtime}"
            );
        }
        assert_eq!(vm_report["symbolIdentity"], true);
        assert_eq!(vm_report["symbolMatchesGuestRegistry"], true);
        assert_eq!(vm_report["symbolKey"], "napi-vm-v9-global");
        assert_eq!(vm_report["moduleFileNameIsUrl"], true);
        assert_eq!(vm_report["created"]["name"], "SyntaxError");
        assert_eq!(vm_report["created"]["message"], "created syntax failure");
        assert_eq!(vm_report["created"]["code"], "E_CREATED_SYNTAX");
        assert_eq!(vm_report["thrown"]["name"], "SyntaxError");
        assert_eq!(vm_report["thrown"]["message"], "thrown syntax failure");
        assert_eq!(vm_report["thrown"]["code"], "E_THROWN_SYNTAX");
        drop(interpreter);
        drop(observer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_napi_v8_type_tags_integrity_and_async_cleanup_hooks() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-v8-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let compiler = Command::new("cc").arg("--version").output();
        let include_dirs = [
            std::env::var_os("NODE_INCLUDE_DIR").map(PathBuf::from),
            Some(PathBuf::from("/usr/include/node")),
            Some(PathBuf::from("/usr/local/include/node")),
        ];
        let include = include_dirs
            .into_iter()
            .flatten()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(compiler), Some(include)) = (compiler, include) else {
            eprintln!("skipping Node-API v8 fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");

        let source = root.join("fixture.c");
        let addon = root.join("fixture.node");
        let marker = root.join("async-cleanup.marker");
        let c_source = r#"
#define _POSIX_C_SOURCE 200809L
#define NAPI_VERSION 8
#include <node_api.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <time.h>

static const napi_type_tag fixture_tag = { UINT64_C(0x123456789abcdef0), UINT64_C(0xfedcba9876543210) };
static const napi_type_tag other_tag = { UINT64_C(0x1111111111111111), UINT64_C(0x2222222222222222) };

static napi_value get_array_accessor(napi_env env, napi_callback_info info) {
  napi_value value;
  (void)info;
  if (napi_create_string_utf8(env, "native-accessor", NAPI_AUTO_LENGTH, &value) != napi_ok)
    return NULL;
  return value;
}

static napi_value constructor_callback(napi_env env, napi_callback_info info) {
  napi_value this_value, constructed_value;
  size_t argc = 0;
  if (napi_get_cb_info(env, info, &argc, NULL, &this_value, NULL) != napi_ok ||
      napi_create_int32(env, 73, &constructed_value) != napi_ok ||
      napi_set_named_property(env, this_value, "constructed", constructed_value) != napi_ok)
    return NULL;
  return NULL;
}

static napi_value construct_and_check(napi_env env, napi_callback_info info) {
  napi_value args[1], instance, result;
  bool is_instance = false;
  size_t argc = 1;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc != 1 ||
      napi_new_instance(env, args[0], 0, NULL, &instance) != napi_ok ||
      napi_instanceof(env, instance, args[0], &is_instance) != napi_ok ||
      napi_get_boolean(env, is_instance, &result) != napi_ok)
    return NULL;
  return result;
}

static void append_cleanup_event(const char* event) {
  FILE* file = fopen("__MARKER_PATH__", "a");
  if (file != NULL) { fputs(event, file); fclose(file); }
}

static void sync_cleanup(void* data) {
  (void)data;
  append_cleanup_event("sync|");
}

static void* finish_async_cleanup(void* data) {
  napi_async_cleanup_hook_handle handle = (napi_async_cleanup_hook_handle)data;
  struct timespec delay = {0, 10000000};
  nanosleep(&delay, NULL);
  append_cleanup_event("async-done|");
  napi_remove_async_cleanup_hook(handle);
  return NULL;
}

static void async_cleanup(napi_async_cleanup_hook_handle handle, void* data) {
  pthread_t worker;
  (void)data;
  append_cleanup_event("async-start|");
  if (pthread_create(&worker, NULL, finish_async_cleanup, handle) == 0)
    pthread_detach(worker);
  else
    napi_remove_async_cleanup_hook(handle);
}

static napi_value probe(napi_env env, napi_callback_info info) {
  napi_value args[5], result, field, array_index_value, array_tag_value;
  napi_value accessor_before_value, accessor_after_value;
  bool matches = false, wrong_matches = true;
  napi_status duplicate_tag_status, freeze_status, seal_status, function_freeze_status;
  napi_status array_define_status, array_freeze_status, frozen_array_set_status;
  napi_status array_accessor_read_status, array_accessor_write_status;
  napi_property_descriptor array_properties[3] = {0};
  size_t argc = 5;
  int32_t accessor_before, accessor_after;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc != 5 ||
      napi_type_tag_object(env, args[0], &fixture_tag) != napi_ok ||
      napi_check_object_type_tag(env, args[0], &fixture_tag, &matches) != napi_ok ||
      napi_check_object_type_tag(env, args[0], &other_tag, &wrong_matches) != napi_ok)
    return NULL;
  duplicate_tag_status = napi_type_tag_object(env, args[0], &fixture_tag);
  freeze_status = napi_object_freeze(env, args[0]);
  seal_status = napi_object_seal(env, args[1]);
  function_freeze_status = napi_object_freeze(env, args[2]);
  if (napi_create_int32(env, 23, &array_index_value) != napi_ok ||
      napi_create_string_utf8(env, "native-tag", NAPI_AUTO_LENGTH, &array_tag_value) != napi_ok)
    return NULL;
  array_properties[0].utf8name = "1";
  array_properties[0].value = array_index_value;
  array_properties[0].attributes = napi_default;
  array_properties[1].utf8name = "tag";
  array_properties[1].value = array_tag_value;
  array_properties[1].attributes = napi_default;
  array_properties[2].utf8name = "nativeAccessor";
  array_properties[2].getter = get_array_accessor;
  array_properties[2].attributes = napi_enumerable;
  array_define_status = napi_define_properties(env, args[3], 3, array_properties);
  if (array_define_status != napi_ok) return NULL;
  array_freeze_status = napi_object_freeze(env, args[3]);
  if (napi_create_int32(env, 77, &field) != napi_ok) return NULL;
  frozen_array_set_status = napi_set_element(env, args[3], 0, field);
  array_accessor_read_status = napi_get_element(env, args[4], 0, &accessor_before_value);
  if (array_accessor_read_status != napi_ok ||
      napi_get_value_int32(env, accessor_before_value, &accessor_before) != napi_ok ||
      napi_create_int32(env, 41, &field) != napi_ok)
    return NULL;
  array_accessor_write_status = napi_set_element(env, args[4], 0, field);
  if (array_accessor_write_status != napi_ok ||
      napi_get_element(env, args[4], 0, &accessor_after_value) != napi_ok ||
      napi_get_value_int32(env, accessor_after_value, &accessor_after) != napi_ok)
    return NULL;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_get_boolean(env, matches, &field) != napi_ok ||
      napi_set_named_property(env, result, "tagMatches", field) != napi_ok ||
      napi_get_boolean(env, wrong_matches, &field) != napi_ok ||
      napi_set_named_property(env, result, "wrongTagMatches", field) != napi_ok ||
      napi_create_int32(env, duplicate_tag_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "duplicateTagStatus", field) != napi_ok ||
      napi_create_int32(env, freeze_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "freezeStatus", field) != napi_ok ||
      napi_create_int32(env, seal_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "sealStatus", field) != napi_ok ||
      napi_create_int32(env, function_freeze_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "functionFreezeStatus", field) != napi_ok ||
      napi_create_int32(env, array_define_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "arrayDefineStatus", field) != napi_ok ||
      napi_create_int32(env, array_freeze_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "arrayFreezeStatus", field) != napi_ok ||
      napi_create_int32(env, frozen_array_set_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "frozenArraySetStatus", field) != napi_ok ||
      napi_create_int32(env, array_accessor_read_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "arrayAccessorReadStatus", field) != napi_ok ||
      napi_create_int32(env, array_accessor_write_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "arrayAccessorWriteStatus", field) != napi_ok ||
      napi_create_int32(env, accessor_before, &field) != napi_ok ||
      napi_set_named_property(env, result, "arrayAccessorBefore", field) != napi_ok ||
      napi_create_int32(env, accessor_after, &field) != napi_ok ||
      napi_set_named_property(env, result, "arrayAccessorAfter", field) != napi_ok)
    return NULL;
  return result;
}

NAPI_MODULE_INIT() {
  napi_value function, marker, status_value, constructor, construct_check;
  napi_property_descriptor defined_descriptor = {0};
  napi_status marker_status, define_status, freeze_status;
  if (napi_add_env_cleanup_hook(env, sync_cleanup, NULL) != napi_ok ||
      napi_add_async_cleanup_hook(env, async_cleanup, NULL, NULL) != napi_ok ||
      napi_create_function(env, "probe", NAPI_AUTO_LENGTH, probe, NULL, &function) != napi_ok ||
      napi_create_int32(env, 64, &marker) != napi_ok)
    return NULL;
  marker_status = napi_set_named_property(env, function, "nativeMarker", marker);
  defined_descriptor.utf8name = "definedMarker";
  defined_descriptor.value = marker;
  defined_descriptor.attributes = napi_writable | napi_enumerable | napi_configurable;
  define_status = napi_define_properties(env, function, 1, &defined_descriptor);
  freeze_status = napi_object_freeze(env, function);
  if (napi_set_named_property(env, exports, "probe", function) != napi_ok ||
      napi_create_int32(env, marker_status, &status_value) != napi_ok ||
      napi_set_named_property(env, exports, "markerStatus", status_value) != napi_ok ||
      napi_create_int32(env, define_status, &status_value) != napi_ok ||
      napi_set_named_property(env, exports, "defineStatus", status_value) != napi_ok ||
      napi_create_int32(env, freeze_status, &status_value) != napi_ok ||
      napi_set_named_property(env, exports, "functionFreezeStatus", status_value) != napi_ok ||
      napi_create_function(env, "ProbeConstructor", NAPI_AUTO_LENGTH,
                           constructor_callback, NULL, &constructor) != napi_ok ||
      napi_set_named_property(env, exports, "ProbeConstructor", constructor) != napi_ok ||
      napi_create_function(env, "constructAndCheck", NAPI_AUTO_LENGTH,
                           construct_and_check, NULL, &construct_check) != napi_ok ||
      napi_set_named_property(env, exports, "constructAndCheck", construct_check) != napi_ok)
    return NULL;
  return exports;
}
"#
        .replace("__MARKER_PATH__", &marker.to_string_lossy());
        fs::write(&source, c_source).unwrap();
        let built = Command::new("cc")
            .args([
                "-std=c11",
                "-O2",
                "-fPIC",
                "-shared",
                "-pthread",
                "-DNAPI_VERSION=8",
                "-I",
            ])
            .arg(&include)
            .arg(&source)
            .arg("-o")
            .arg(&addon)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "Node-API v8 fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        fs::write(
            root.join("main.cjs"),
            r#"
const addon = require('./fixture.node');
const constructed = new addon.ProbeConstructor();
const nativeFunction = addon.probe;
const nativeFunctionMarker = Object.getOwnPropertyDescriptor(nativeFunction, 'nativeMarker') || {};
const nativeFunctionDefinedMarker = Object.getOwnPropertyDescriptor(nativeFunction, 'definedMarker') || {};
const nativeFunctionName = Object.getOwnPropertyDescriptor(nativeFunction, 'name') || {};
const nativeFunctionLength = Object.getOwnPropertyDescriptor(nativeFunction, 'length') || {};
const nativeFunctionPrototype = Object.getOwnPropertyDescriptor(nativeFunction, 'prototype') || {};
nativeFunction.nativeMarker = 99;
const target = {value: 41};
const sealedTarget = {value: 9};
const frozenArray = [13];
let napiAccessorValue = 5;
const napiAccessorArray = [];
Object.defineProperty(napiAccessorArray, '0', {
  get() { return napiAccessorValue; },
  set(value) { napiAccessorValue = value + 1; },
  configurable: true,
});
function FrozenFunction() {}
const native = addon.probe(target, sealedTarget, FrozenFunction, frozenArray, napiAccessorArray);
frozenArray[0] = 88;
frozenArray[1] = 99;
delete frozenArray[0];
frozenArray.length = 0;
let frozenArrayPushThrows = false;
let frozenArraySpliceThrows = false;
try { frozenArray.push(101); } catch (error) { frozenArrayPushThrows = error.name === 'TypeError'; }
try { frozenArray.splice(1, 0); } catch (error) { frozenArraySpliceThrows = error.name === 'TypeError'; }
const accessorArray = [];
let accessorValue = 0;
Object.defineProperty(accessorArray, '0', {
  get() { return accessorValue; },
  set(value) { accessorValue = value + 1; },
  enumerable: true,
  configurable: true,
});
accessorArray[0] = 40;
const indexedAccessorValue = accessorArray[0];
const indexedAccessorDescriptor = Object.getOwnPropertyDescriptor(accessorArray, '0');
Object.defineProperty(accessorArray, 'named', {
  get() { return accessorValue; },
  set(value) { accessorValue = value + 2; },
  enumerable: false,
  configurable: true,
});
accessorArray.named = 50;
const namedAccessorValue = accessorArray.named;
const sealedArray = [17];
Object.seal(sealedArray);
sealedArray[0] = 19;
sealedArray[1] = 21;
delete sealedArray[0];
sealedArray.length = 0;
const guestDefined = [5];
Object.defineProperty(guestDefined, '2', {
  value: 7, writable: false, enumerable: true, configurable: false,
});
Object.defineProperty(guestDefined, 'label', {
  value: 'guest', writable: true, enumerable: false, configurable: true,
});
Object.defineProperty(guestDefined, 'length', {writable: false});
let guestIndexRedefinitionThrows = false;
let guestLengthRedefinitionThrows = false;
try { Object.defineProperty(guestDefined, '2', {value: 8}); }
catch (error) { guestIndexRedefinitionThrows = error.name === 'TypeError'; }
try { Object.defineProperty(guestDefined, 'length', {value: 4}); }
catch (error) { guestLengthRedefinitionThrows = error.name === 'TypeError'; }
const frozenArrayIndexDescriptor = Object.getOwnPropertyDescriptor(frozenArray, '0');
const frozenArraySecondDescriptor = Object.getOwnPropertyDescriptor(frozenArray, '1');
const frozenArrayTagDescriptor = Object.getOwnPropertyDescriptor(frozenArray, 'tag');
const frozenArrayLengthDescriptor = Object.getOwnPropertyDescriptor(frozenArray, 'length');
const sealedArrayIndexDescriptor = Object.getOwnPropertyDescriptor(sealedArray, '0');
const guestDefinedIndexDescriptor = Object.getOwnPropertyDescriptor(guestDefined, '2');
module.exports = {
  ...native,
  hostMarkerStatus: addon.markerStatus,
  hostDefineStatus: addon.defineStatus,
  hostFunctionFreezeStatus: addon.functionFreezeStatus,
  constructorResult: constructed.constructed,
  constructorInstanceof: constructed instanceof addon.ProbeConstructor,
  constructorPrototypeMatches: Object.getPrototypeOf(constructed) === addon.ProbeConstructor.prototype,
  constructorBackReferenceMatches: constructed.constructor === addon.ProbeConstructor,
  napiNewInstanceInstanceof: addon.constructAndCheck(addon.ProbeConstructor),
  nativeFunctionFrozen: Object.isFrozen(nativeFunction),
  nativeFunctionName: nativeFunction.name,
  nativeFunctionLength: nativeFunction.length,
  nativeFunctionMarker: nativeFunction.nativeMarker,
  nativeFunctionDefinedMarker: nativeFunction.definedMarker,
  nativeFunctionPrototype: typeof nativeFunction.prototype,
  nativeFunctionPrototypeConstructorMatches: nativeFunction.prototype.constructor === nativeFunction,
  nativeFunctionEnumerableKeys: Object.keys(nativeFunction),
  nativeFunctionOwnProperties: Object.getOwnPropertyNames(nativeFunction)
    .filter((name) => ['definedMarker', 'length', 'name', 'nativeMarker', 'prototype'].includes(name)).sort(),
  nativeFunctionMarkerWritable: nativeFunctionMarker.writable,
  nativeFunctionMarkerEnumerable: nativeFunctionMarker.enumerable,
  nativeFunctionMarkerConfigurable: nativeFunctionMarker.configurable,
  nativeFunctionDefinedMarkerWritable: nativeFunctionDefinedMarker.writable,
  nativeFunctionDefinedMarkerEnumerable: nativeFunctionDefinedMarker.enumerable,
  nativeFunctionDefinedMarkerConfigurable: nativeFunctionDefinedMarker.configurable,
  nativeFunctionNameWritable: nativeFunctionName.writable,
  nativeFunctionLengthValue: nativeFunctionLength.value,
  nativeFunctionPrototypeWritable: nativeFunctionPrototype.writable,
  nativeFunctionPrototypeEnumerable: nativeFunctionPrototype.enumerable,
  nativeFunctionPrototypeConfigurable: nativeFunctionPrototype.configurable,
  frozen: Object.isFrozen(target),
  sealed: Object.isSealed(sealedTarget),
  functionFrozen: Object.isFrozen(FrozenFunction),
  arrayFrozen: Object.isFrozen(frozenArray),
  frozenArrayLength: frozenArray.length,
  frozenArrayValue: frozenArray[0],
  frozenArraySecondValue: frozenArray[1],
  frozenArrayTag: frozenArray.tag,
  frozenArrayNativeAccessor: frozenArray.nativeAccessor,
  frozenArrayPushThrows,
  frozenArraySpliceThrows,
  frozenIndexWritable: frozenArrayIndexDescriptor.writable,
  frozenIndexConfigurable: frozenArrayIndexDescriptor.configurable,
  frozenSecondWritable: frozenArraySecondDescriptor.writable,
  frozenSecondEnumerable: frozenArraySecondDescriptor.enumerable,
  frozenSecondConfigurable: frozenArraySecondDescriptor.configurable,
  frozenTagWritable: frozenArrayTagDescriptor.writable,
  indexedAccessorValue,
  indexedAccessorDescriptorKeys: Object.keys(indexedAccessorDescriptor),
  namedAccessorValue,
  frozenLengthWritable: frozenArrayLengthDescriptor.writable,
  frozenLengthConfigurable: frozenArrayLengthDescriptor.configurable,
  sealedArraySealed: Object.isSealed(sealedArray),
  sealedArrayFrozen: Object.isFrozen(sealedArray),
  sealedArrayLength: sealedArray.length,
  sealedArrayValue: sealedArray[0],
  sealedIndexWritable: sealedArrayIndexDescriptor.writable,
  sealedIndexConfigurable: sealedArrayIndexDescriptor.configurable,
  guestDefinedLength: guestDefined.length,
  guestDefinedIndex: guestDefined[2],
  guestDefinedLabel: guestDefined.label,
  guestDefinedKeys: Object.keys(guestDefined),
  guestDefinedIndexWritable: guestDefinedIndexDescriptor.writable,
  guestDefinedIndexEnumerable: guestDefinedIndexDescriptor.enumerable,
  guestDefinedIndexConfigurable: guestDefinedIndexDescriptor.configurable,
  guestIndexRedefinitionThrows,
  guestLengthRedefinitionThrows,
  targetValue: target.value,
  sealedValue: sealedTarget.value,
};
"#,
        )
        .unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest)
                    .entry(root.join("main.cjs")),
            )
            .unwrap();
        let observer = unsafe { Library::open(Some(addon.as_os_str()), RTLD_NOW) }.unwrap();
        let vm_report = interpreter
            .eval_source("JSON.stringify(require('./main.cjs'));")
            .unwrap();
        let Value::String(ref vm_report) = vm_report else {
            panic!("Node-API v8 VM fixture did not return JSON");
        };
        let vm_report: serde_json::Value = serde_json::from_str(vm_report).unwrap();
        drop(interpreter);
        assert_eq!(
            fs::read_to_string(&marker).unwrap(),
            "async-start|sync|async-done|",
            "napi-vm cleanup hook ordering differed"
        );
        let runner = "process.stdout.write(JSON.stringify(require('./main.cjs')))";
        let mut reference_reports = Vec::new();
        for runtime in ["node", "bun"] {
            if !Command::new(runtime)
                .arg("--version")
                .output()
                .is_ok_and(|out| out.status.success())
            {
                continue;
            }
            let _ = fs::remove_file(&marker);
            let reference = Command::new(runtime)
                .current_dir(&root)
                .args(["-e", runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "{runtime} Node-API v8 fixture failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let report: serde_json::Value = serde_json::from_slice(&reference.stdout)
                .unwrap_or_else(|_| {
                    panic!(
                        "{runtime} v8 result was not JSON: {}",
                        String::from_utf8_lossy(&reference.stdout)
                    )
                });
            // Bun 1.4.0 returns the same v8 API results but exits without
            // awaiting async cleanup hooks, so the teardown-order assertion
            // is Node-specific while the API result remains differential.
            if runtime == "node" {
                let cleanup_result = fs::read_to_string(&marker)
                    .unwrap_or_else(|error| panic!("Node async cleanup marker missing: {error}"));
                assert_eq!(
                    cleanup_result, "async-start|sync|async-done|",
                    "Node cleanup order changed"
                );
            }
            reference_reports.push((runtime, report));
        }
        assert!(
            !reference_reports.is_empty(),
            "Node or Bun is required for the N-API differential fixture"
        );
        for (runtime, report) in reference_reports {
            assert_eq!(
                vm_report, report,
                "Node-API v8 result differs from {runtime}"
            );
        }
        assert_eq!(vm_report["tagMatches"], true);
        assert_eq!(vm_report["wrongTagMatches"], false);
        assert_eq!(vm_report["duplicateTagStatus"], NAPI_INVALID_ARG);
        assert_eq!(vm_report["freezeStatus"], NAPI_OK);
        assert_eq!(vm_report["sealStatus"], NAPI_OK);
        assert_eq!(vm_report["functionFreezeStatus"], NAPI_OK);
        assert_eq!(vm_report["hostMarkerStatus"], NAPI_OK);
        assert_eq!(vm_report["hostDefineStatus"], NAPI_OK);
        assert_eq!(vm_report["hostFunctionFreezeStatus"], NAPI_OK);
        assert_eq!(vm_report["constructorResult"], 73);
        assert_eq!(vm_report["constructorInstanceof"], true);
        assert_eq!(vm_report["constructorPrototypeMatches"], true);
        assert_eq!(vm_report["constructorBackReferenceMatches"], true);
        assert_eq!(vm_report["napiNewInstanceInstanceof"], true);
        assert_eq!(vm_report["nativeFunctionFrozen"], true);
        assert_eq!(vm_report["nativeFunctionName"], "probe");
        assert_eq!(vm_report["nativeFunctionLength"], 0);
        assert_eq!(vm_report["nativeFunctionLengthValue"], 0);
        assert_eq!(vm_report["nativeFunctionMarker"], 64);
        assert_eq!(vm_report["nativeFunctionDefinedMarker"], 64);
        assert_eq!(vm_report["nativeFunctionPrototype"], "object");
        assert_eq!(vm_report["nativeFunctionPrototypeConstructorMatches"], true);
        assert_eq!(
            vm_report["nativeFunctionEnumerableKeys"],
            serde_json::json!(["nativeMarker", "definedMarker"])
        );
        assert_eq!(
            vm_report["nativeFunctionOwnProperties"],
            serde_json::json!([
                "definedMarker",
                "length",
                "name",
                "nativeMarker",
                "prototype"
            ])
        );
        assert_eq!(vm_report["nativeFunctionMarkerWritable"], false);
        assert_eq!(vm_report["nativeFunctionMarkerEnumerable"], true);
        assert_eq!(vm_report["nativeFunctionMarkerConfigurable"], false);
        assert_eq!(vm_report["nativeFunctionDefinedMarkerWritable"], false);
        assert_eq!(vm_report["nativeFunctionDefinedMarkerEnumerable"], true);
        assert_eq!(vm_report["nativeFunctionDefinedMarkerConfigurable"], false);
        assert_eq!(vm_report["nativeFunctionNameWritable"], false);
        assert_eq!(vm_report["nativeFunctionPrototypeWritable"], false);
        assert_eq!(vm_report["nativeFunctionPrototypeEnumerable"], false);
        assert_eq!(vm_report["nativeFunctionPrototypeConfigurable"], false);
        assert_eq!(vm_report["arrayDefineStatus"], NAPI_OK);
        assert_eq!(vm_report["arrayFreezeStatus"], NAPI_OK);
        assert_eq!(vm_report["frozenArraySetStatus"], NAPI_OK);
        assert_eq!(vm_report["arrayAccessorReadStatus"], NAPI_OK);
        assert_eq!(vm_report["arrayAccessorWriteStatus"], NAPI_OK);
        assert_eq!(vm_report["arrayAccessorBefore"], 5);
        assert_eq!(vm_report["arrayAccessorAfter"], 42);
        assert_eq!(vm_report["frozen"], true);
        assert_eq!(vm_report["sealed"], true);
        assert_eq!(vm_report["functionFrozen"], true);
        assert_eq!(vm_report["arrayFrozen"], true);
        assert_eq!(vm_report["frozenArrayLength"], 2);
        assert_eq!(vm_report["frozenArrayValue"], 13);
        assert_eq!(vm_report["frozenArraySecondValue"], 23);
        assert_eq!(vm_report["frozenArrayTag"], "native-tag");
        assert_eq!(vm_report["frozenArrayNativeAccessor"], "native-accessor");
        assert_eq!(vm_report["frozenArrayPushThrows"], true);
        assert_eq!(vm_report["frozenArraySpliceThrows"], true);
        assert_eq!(vm_report["frozenIndexWritable"], false);
        assert_eq!(vm_report["frozenIndexConfigurable"], false);
        assert_eq!(vm_report["frozenSecondWritable"], false);
        assert_eq!(vm_report["frozenSecondEnumerable"], false);
        assert_eq!(vm_report["frozenSecondConfigurable"], false);
        assert_eq!(vm_report["frozenTagWritable"], false);
        assert_eq!(vm_report["indexedAccessorValue"], 41);
        assert_eq!(
            vm_report["indexedAccessorDescriptorKeys"],
            serde_json::json!(["get", "set", "enumerable", "configurable"])
        );
        assert_eq!(vm_report["namedAccessorValue"], 52);
        assert_eq!(vm_report["frozenLengthWritable"], false);
        assert_eq!(vm_report["frozenLengthConfigurable"], false);
        assert_eq!(vm_report["sealedArraySealed"], true);
        assert_eq!(vm_report["sealedArrayFrozen"], false);
        assert_eq!(vm_report["sealedArrayLength"], 1);
        assert_eq!(vm_report["sealedArrayValue"], 19);
        assert_eq!(vm_report["sealedIndexWritable"], true);
        assert_eq!(vm_report["sealedIndexConfigurable"], false);
        assert_eq!(vm_report["guestDefinedLength"], 3);
        assert_eq!(vm_report["guestDefinedIndex"], 7);
        assert_eq!(vm_report["guestDefinedLabel"], "guest");
        assert_eq!(vm_report["guestDefinedKeys"], serde_json::json!(["0", "2"]));
        assert_eq!(vm_report["guestDefinedIndexWritable"], false);
        assert_eq!(vm_report["guestDefinedIndexEnumerable"], true);
        assert_eq!(vm_report["guestDefinedIndexConfigurable"], false);
        assert_eq!(vm_report["guestIndexRedefinitionThrows"], true);
        assert_eq!(vm_report["guestLengthRedefinitionThrows"], true);
        assert_eq!(vm_report["targetValue"], 41);
        assert_eq!(vm_report["sealedValue"], 9);
        drop(observer);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_and_calls_a_real_napi_v7_addon_without_a_node_sidecar() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let compiler = Command::new("cc").arg("--version").output();
        let mut include_dirs = Vec::new();
        if let Some(include) = std::env::var_os("NODE_INCLUDE_DIR") {
            include_dirs.push(PathBuf::from(include));
        }
        include_dirs.push(PathBuf::from("/usr/include/node"));
        include_dirs.push(PathBuf::from("/usr/local/include/node"));
        let include = include_dirs
            .into_iter()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(compiler), Some(include)) = (compiler, include) else {
            eprintln!("skipping Rust Node-API host fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");

        let source = root.join("fixture.c");
        let addon = root.join("fixture.node");
        fs::write(
            &source,
            include_str!("fixtures/real_napi_v7.c"),
        )
        .unwrap();
        let mut build = Command::new("cc");
        build.args(["-std=c11", "-O2", "-fPIC"]);
        #[cfg(target_os = "linux")]
        build.arg("-shared");
        #[cfg(target_os = "macos")]
        build.args(["-dynamiclib", "-undefined", "dynamic_lookup"]);
        let built = build
            .args(["-pthread", "-DNAPI_VERSION=7", "-I"])
            .arg(include)
            .arg(&source)
            .arg("-o")
            .arg(&addon)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "Node-API fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        fs::write(
            root.join("main.cjs"),
            include_str!("fixtures/real_napi_v7_main.cjs"),
        )
        .unwrap();
        fs::write(
            root.join("instanceof.cjs"),
            include_str!("fixtures/real_napi_v7_instanceof.cjs"),
        )
        .unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest)
                    .entry(root.join("main.cjs")),
            )
            .unwrap();
        let observer = unsafe { Library::open(Some(addon.as_os_str()), RTLD_NOW) }.unwrap();
        let wrapped_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_wrapped_finalizer_calls\0")
                .unwrap()
        };
        let added_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_added_finalizer_calls\0")
                .unwrap()
        };
        let instance_data_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_instance_data_finalizer_calls\0")
                .unwrap()
        };
        let replaced_instance_data_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_replaced_instance_data_finalizer_calls\0")
                .unwrap()
        };
        let instance_data_visible_in_finalizer: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_instance_data_visible_in_finalizer\0")
                .unwrap()
        };
        let removed_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_removed_finalizer_calls\0")
                .unwrap()
        };
        let external_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_external_finalizer_calls\0")
                .unwrap()
        };
        let external_arraybuffer_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_external_arraybuffer_finalizer_calls\0")
                .unwrap()
        };
        let external_buffer_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_external_buffer_finalizer_calls\0")
                .unwrap()
        };
        let finalizer_create_function_status: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_finalizer_create_function_status\0")
                .unwrap()
        };
        let cleanup_hook_count: unsafe extern "C" fn() -> i32 =
            unsafe { *observer.get(b"napi_vm_test_cleanup_hook_count\0").unwrap() };
        let cleanup_hook_value: unsafe extern "C" fn(i32) -> i32 =
            unsafe { *observer.get(b"napi_vm_test_cleanup_hook_value\0").unwrap() };
        let cleanup_before_wrap_finalizer: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_cleanup_before_wrap_finalizer\0")
                .unwrap()
        };
        let threadsafe_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_threadsafe_finalizer_calls\0")
                .unwrap()
        };
        let threadsafe_worker_context_ok: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_threadsafe_worker_context_ok\0")
                .unwrap()
        };
        let threadsafe_worker_call_status: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_threadsafe_worker_call_status\0")
                .unwrap()
        };
        let threadsafe_worker_blocking_status: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_threadsafe_worker_blocking_status\0")
                .unwrap()
        };
        assert_eq!(unsafe { wrapped_finalizer_calls() }, 0);
        assert_eq!(unsafe { added_finalizer_calls() }, 0);
        assert_eq!(unsafe { removed_finalizer_calls() }, 0);
        assert_eq!(unsafe { external_finalizer_calls() }, 0);
        assert_eq!(unsafe { external_arraybuffer_finalizer_calls() }, 0);
        assert_eq!(unsafe { external_buffer_finalizer_calls() }, 0);
        assert_eq!(unsafe { cleanup_hook_count() }, 0);
        let result = interpreter.eval_source("require('./main.cjs');").unwrap();
        let invalid_utf16 = interpreter
            .eval_source("require('./fixture.node').invalidUtf16Status();")
            .unwrap();
        assert!(matches!(
            invalid_utf16.get_prop("status"),
            Some(Value::Number(status)) if status == NAPI_GENERIC_FAILURE as f64
        ));
        assert!(matches!(
            invalid_utf16.get_prop("messageMatches"),
            Some(Value::Bool(true))
        ));
        let initialized_promise_value = interpreter
            .eval_source("await require('./fixture.node').initializedPromise;")
            .unwrap();
        assert!(
            matches!(&initialized_promise_value, Value::String(value) if value == "resolved-during-init"),
            "unexpected initialized promise result: {initialized_promise_value:?}"
        );
        assert!(matches!(
            interpreter
                .eval_source("await require('./fixture.node').resolvedPromise();")
                .unwrap(),
            Value::String(ref value) if value == "resolved-from-addon"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "try { await require('./fixture.node').rejectedPromise(); } catch (reason) { reason; }"
                )
                .unwrap(),
            Value::String(ref value) if value == "rejected-from-addon"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "globalThis.napiHostPendingPromise = require('./fixture.node').pendingPromise(); globalThis.napiHostPendingResult = 'waiting'; globalThis.napiHostPendingPromise.then(value => { globalThis.napiHostPendingResult = value; }); 'created';"
                )
                .unwrap(),
            Value::String(ref value) if value == "created"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "require('./fixture.node').resolvePendingPromise({ then: resolve => resolve('settled-after-callback') });"
                )
                .unwrap(),
            Value::Bool(true)
        ));
        assert!(matches!(
            interpreter
                .eval_source("await globalThis.napiHostPendingPromise;")
                .unwrap(),
            Value::String(ref value) if value == "settled-after-callback"
        ));
        assert!(matches!(
            interpreter
                .eval_source("globalThis.napiHostPendingResult;")
                .unwrap(),
            Value::String(ref value) if value == "settled-after-callback"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "globalThis.napiHostAdoptedPromise = require('./fixture.node').pendingPromise(); 'created';"
                )
                .unwrap(),
            Value::String(ref value) if value == "created"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "require('./fixture.node').resolvePendingPromise(require('./fixture.node').resolvedPromise());"
                )
                .unwrap(),
            Value::Bool(true)
        ));
        assert!(matches!(
            interpreter
                .eval_source("await globalThis.napiHostAdoptedPromise;")
                .unwrap(),
            Value::String(ref value) if value == "resolved-from-addon"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "globalThis.napiHostRejectedThenable = require('./fixture.node').pendingPromise(); 'created';"
                )
                .unwrap(),
            Value::String(ref value) if value == "created"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "require('./fixture.node').resolvePendingPromise({ then() { throw new TypeError('thenable failed'); } });"
                )
                .unwrap(),
            Value::Bool(true)
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "try { await globalThis.napiHostRejectedThenable; } catch (error) { error.message; }"
                )
                .unwrap(),
            Value::String(ref value) if value == "thenable failed"
        ));
        assert_eq!(unsafe { threadsafe_finalizer_calls() }, 0);
        let threadsafe_statuses = interpreter
            .eval_source(
                "globalThis.threadsafeValues = []; const addon = require('./fixture.node'); addon.runThreadsafe(value => { threadsafeValues.push(value); if (value === 'threadsafe-value') queueMicrotask(() => threadsafeValues.push('worker-microtask')); }); const queueFullStatus = addon.probeThreadsafeQueue(value => threadsafeValues.push(value)); const abortStatus = addon.probeThreadsafeAbort(); globalThis.threadsafeStatuses = {queueFullStatus, abortStatus}; threadsafeStatuses;",
            )
            .unwrap();
        assert!(matches!(
            threadsafe_statuses.get_prop("queueFullStatus"),
            Some(Value::Number(15.0))
        ));
        assert!(matches!(
            threadsafe_statuses.get_prop("abortStatus"),
            Some(Value::Number(16.0))
        ));
        assert_eq!(unsafe { threadsafe_worker_context_ok() }, 1);
        assert_eq!(unsafe { threadsafe_worker_call_status() }, 0);
        let has_valid_threadsafe_order = |events: &Value| {
            matches!(events, Value::String(events)
                if events == "threadsafe-value,worker-microtask,queue-first,threadsafe-second"
                    || events == "threadsafe-value,worker-microtask,threadsafe-second,queue-first"
                    || events == "queue-first,threadsafe-value,worker-microtask,threadsafe-second")
        };
        for _ in 0..10 {
            let _ = interpreter
                .run_event_loop_once(Duration::from_millis(250))
                .unwrap();
            let received = interpreter
                .eval_source("threadsafeValues.join(',');")
                .unwrap();
            if has_valid_threadsafe_order(&received) && unsafe { threadsafe_finalizer_calls() } == 2
            {
                break;
            }
        }
        let received = interpreter
            .eval_source("threadsafeValues.join(',');")
            .unwrap();
        assert!(
            has_valid_threadsafe_order(&received),
            "unexpected thread-safe callback events: {received:?}; finalizers={}; worker_status={}; blocking_status={}",
            unsafe { threadsafe_finalizer_calls() },
            unsafe { threadsafe_worker_call_status() },
            unsafe { threadsafe_worker_blocking_status() }
        );
        assert_eq!(unsafe { threadsafe_finalizer_calls() }, 2);
        assert_eq!(unsafe { threadsafe_worker_blocking_status() }, 0);
        let vm_threadsafe_json = interpreter
            .eval_source(
                "JSON.stringify({events: threadsafeValues.slice().sort(), queueStatus: threadsafeStatuses.queueFullStatus, abortStatus: threadsafeStatuses.abortStatus});",
            )
            .unwrap();
        let Value::String(ref vm_threadsafe_json) = vm_threadsafe_json else {
            panic!("thread-safe Node-API fixture did not return JSON");
        };
        let vm_threadsafe_result: serde_json::Value =
            serde_json::from_str(vm_threadsafe_json).expect("VM thread-safe result is valid JSON");
        let threadsafe_runner = "(async function() { const addon = require('./fixture.node'); const events = []; let finish; const done = new Promise(resolve => { finish = resolve; }); const callback = value => { events.push(value); if (value === 'threadsafe-value') queueMicrotask(() => events.push('worker-microtask')); if (events.includes('threadsafe-value') && events.includes('threadsafe-second') && events.includes('queue-first')) finish(); }; addon.runThreadsafe(callback); const queueStatus = addon.probeThreadsafeQueue(callback); const abortStatus = addon.probeThreadsafeAbort(); const timeout = setTimeout(() => { console.error('thread-safe function timed out'); process.exitCode = 1; }, 3000); timeout.unref?.(); await done; clearTimeout(timeout); process.stdout.write(JSON.stringify({events: events.sort(), queueStatus, abortStatus})); })().catch(error => { console.error(error); process.exitCode = 1; });";

        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let reference = Command::new("node")
                .current_dir(&root)
                .args(["-e", threadsafe_runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Node thread-safe reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let node_result: serde_json::Value = serde_json::from_slice(&reference.stdout)
                .expect("Node thread-safe result is valid JSON");
            assert_eq!(vm_threadsafe_result, node_result);
        }

        if let Ok(bun_version) = Command::new("bun").arg("--version").output()
            && bun_version.status.success()
        {
            let reference = Command::new("bun")
                .current_dir(&root)
                .args(["-e", threadsafe_runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Bun thread-safe reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let bun_result: serde_json::Value = serde_json::from_slice(&reference.stdout)
                .expect("Bun thread-safe result is valid JSON");
            assert_eq!(vm_threadsafe_result, bun_result);
        }
        assert!(matches!(result.get_prop("same"), Some(Value::Bool(true))));
        assert!(matches!(result.get_prop("global"), Some(Value::Bool(true))));
        assert!(matches!(
            result.get_prop("globalHasObject"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("definedMethod"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("definedValueBefore"),
            Some(Value::Number(5.0))
        ));
        assert!(matches!(
            result.get_prop("definedValueAfter"),
            Some(Value::Number(23.0))
        ));
        let target_call_counts = result.get_prop("targetCallCounts").unwrap();
        assert!(matches!(
            target_call_counts.get_prop("calls"),
            Some(Value::Number(1.0))
        ));
        assert!(matches!(
            target_call_counts.get_prop("constructs"),
            Some(Value::Number(0.0))
        ));
        let target_construct_counts = result.get_prop("targetConstructCounts").unwrap();
        assert!(matches!(
            target_construct_counts.get_prop("calls"),
            Some(Value::Number(1.0))
        ));
        assert!(matches!(
            target_construct_counts.get_prop("constructs"),
            Some(Value::Number(1.0))
        ));
        let strict_equal = result.get_prop("strictEqual").unwrap();
        assert!(matches!(
            strict_equal.get_prop("sameObject"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            strict_equal.get_prop("distinctObjects"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            strict_equal.get_prop("equalNumbers"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            strict_equal.get_prop("nan"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            strict_equal.get_prop("sameError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            strict_equal.get_prop("distinctErrors"),
            Some(Value::Bool(false))
        ));
        let counter_new_target_info = result.get_prop("counterNewTargetInfo").unwrap();
        assert!(matches!(
            counter_new_target_info.get_prop("seen"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            counter_new_target_info.get_prop("child"),
            Some(Value::Bool(false))
        ));
        let child_new_target_info = result.get_prop("childNewTargetInfo").unwrap();
        assert!(matches!(
            child_new_target_info.get_prop("seen"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            child_new_target_info.get_prop("child"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("childCounterValue"),
            Some(Value::Number(6.0))
        ));
        let instance_checks = result.get_prop("instanceChecks").unwrap();
        for (name, expected) in [
            ("counterIsCounter", true),
            ("counterIsChild", false),
            ("childIsCounter", true),
            ("childIsChild", true),
            ("numberIsCounter", false),
            ("typeErrorIsError", true),
            ("typeErrorIsTypeError", true),
        ] {
            assert!(matches!(
                instance_checks.get_prop(name),
                Some(Value::Bool(value)) if value == expected
            ));
        }
        let custom_instance_result = interpreter
            .eval_source("JSON.stringify(require('./instanceof.cjs'));")
            .unwrap();
        let Value::String(ref custom_instance_json) = custom_instance_result else {
            panic!("custom instanceof fixture did not return JSON");
        };
        let vm_result: serde_json::Value = serde_json::from_str(custom_instance_json).unwrap();
        let mut expected_vm_result = vm_result.clone();
        let has_instance_result_keys = [
            "functionPrototypeHasInstanceIsFunction",
            "functionPrototypeHasInstanceDescriptor",
            "functionPrototypeHasInstanceOrdinary",
            "functionPrototypeHasInstanceBound",
            "functionPrototypeHasInstanceRejectsNonFunction",
        ];
        let has_instance_results = has_instance_result_keys
            .iter()
            .map(|key| ((*key).to_string(), vm_result[*key].clone()))
            .collect::<serde_json::Map<_, _>>();
        for key in has_instance_result_keys {
            expected_vm_result.as_object_mut().unwrap().remove(key);
        }
        assert_eq!(
            serde_json::Value::Object(has_instance_results),
            serde_json::json!({
                "functionPrototypeHasInstanceIsFunction": true,
                "functionPrototypeHasInstanceDescriptor": true,
                "functionPrototypeHasInstanceOrdinary": true,
                "functionPrototypeHasInstanceBound": true,
                "functionPrototypeHasInstanceRejectsNonFunction": true
            })
        );
        let bound_result_keys = [
            "boundFunctionMatched",
            "boundGuestFunctionMatched",
            "boundFunctionReceiverWasTarget",
            "boundOwnSymbolHasInstance",
            "boundOwnGuestSymbolHasInstance",
            "boundOwnSymbolReceiverWasBound",
            "boundOrdinaryGuestInstanceof",
            "boundOrdinaryTargetInstanceof",
            "boundOrdinaryNapiInstanceof",
            "boundOrdinaryValue",
            "boundOrdinaryHasNoOwnPrototype",
            "boundClassGuestInstanceof",
            "boundClassTargetInstanceof",
            "boundClassNapiInstanceof",
            "boundClassValue",
            "boundCallResult",
            "reboundCallResult",
            "boundName",
            "boundLength",
            "reboundName",
            "reboundLength",
            "boundArrowConstructThrows",
        ];
        let bound_results = bound_result_keys
            .iter()
            .map(|key| ((*key).to_string(), vm_result[*key].clone()))
            .collect::<serde_json::Map<_, _>>();
        for key in bound_result_keys {
            expected_vm_result.as_object_mut().unwrap().remove(key);
        }
        assert_eq!(
            serde_json::Value::Object(bound_results),
            serde_json::json!({
                "boundFunctionMatched": true,
                "boundGuestFunctionMatched": true,
                "boundFunctionReceiverWasTarget": true,
                "boundOwnSymbolHasInstance": true,
                "boundOwnGuestSymbolHasInstance": true,
                "boundOwnSymbolReceiverWasBound": true,
                "boundOrdinaryGuestInstanceof": true,
                "boundOrdinaryTargetInstanceof": true,
                "boundOrdinaryNapiInstanceof": true,
                "boundOrdinaryValue": 23,
                "boundOrdinaryHasNoOwnPrototype": true,
                "boundClassGuestInstanceof": true,
                "boundClassTargetInstanceof": true,
                "boundClassNapiInstanceof": true,
                "boundClassValue": 31,
                "boundCallResult": 16,
                "reboundCallResult": 19,
                "boundName": "bound BoundAdd",
                "boundLength": 1,
                "reboundName": "bound bound BoundAdd",
                "reboundLength": 0,
                "boundArrowConstructThrows": true
            })
        );
        let expected_proxy_trap_calls = serde_json::json!({
            "objectGetPrototype": 3,
            "napiGetPrototype": 0,
            "constructorHasInstance": 1,
            "constructorMissingHasInstance": 1
        });
        let expected_proxy_results = serde_json::json!({
            "proxyObjectIsInstance": true,
            "proxyObjectPrototypeMatches": true,
            "napiProxyObjectPrototypeMatches": false,
            "napiProxyObjectPrototypeMatchesTrapResult": false,
            "napiProxyObjectPrototypeIsNull": true,
            "napiTransparentProxyPrototypeIsNull": true,
            "proxyObjectIsPrototypeOf": true,
            "proxyConstructorIsInstance": true,
            "proxyConstructorWithoutHasInstanceIsInstance": true,
            "invalidGetPrototypeTrapThrows": true,
            "proxyTrapCalls": expected_proxy_trap_calls
        });
        let mut expected_compatibility_result = serde_json::json!({
            "matched": true,
            "rejected": false,
            "inherited": true,
            "guestMatched": true,
            "guestInherited": true,
            "receiverWasConstructor": true,
            "functionMatched": true,
            "functionRejected": false,
            "functionInherited": true,
            "guestFunctionMatched": true,
            "guestFunctionInherited": true,
            "functionReceiverWasConstructor": true,
            "ordinaryIsInstance": true,
            "ordinaryGuestInstanceof": true,
            "ordinaryIsObject": true,
            "ordinaryIsFunction": true,
            "ordinaryPrototypeIsObject": true,
            "ordinaryPrototypeShared": true,
            "ordinaryConstructorShared": true,
            "ordinaryDefaultFunctionPrototype": true,
            "functionConstructorPrototype": true,
            "functionPrototypeIsCallable": true,
            "functionPrototypeObjectPrototype": true,
            "functionPrototypeConstructorShared": true,
            "functionCallIsInherited": true,
            "functionApplyIsInherited": true,
            "functionApplyUsesReceiverAndArguments": true,
            "functionApplyReadsArrayLike": true,
            "functionPrototypeNapiIdentity": true,
            "functionPrototypeObjectNapiIdentity": true,
            "functionConstructorNapiIdentity": true,
            "ordinaryName": "Ordinary",
            "ordinaryLength": 1,
            "ordinaryStandardProperties": true,
            "ordinaryDescriptors": true,
            "nameDeleted": true,
            "ordinaryValue": 17
        });
        expected_compatibility_result
            .as_object_mut()
            .unwrap()
            .extend(expected_proxy_results.as_object().unwrap().clone());
        assert_eq!(expected_vm_result, expected_compatibility_result);
        let custom_instance_runner =
            "process.stdout.write(JSON.stringify(require('./instanceof.cjs')));";
        for runtime in ["node", "bun"] {
            if Command::new(runtime)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
            {
                let reference = Command::new(runtime)
                    .current_dir(&root)
                    .args(["-e", custom_instance_runner])
                    .output()
                    .unwrap();
                assert!(
                    reference.status.success(),
                    "{runtime} custom Symbol.hasInstance reference failed: {}",
                    String::from_utf8_lossy(&reference.stderr)
                );
                let reference_result: serde_json::Value =
                    serde_json::from_slice(&reference.stdout).unwrap();
                if runtime == "bun" {
                    // Bun's napi_get_prototype currently invokes a Proxy
                    // getPrototypeOf trap; Node returns null without invoking
                    // it. Assert and document that difference, then compare
                    // the shared observables.
                    assert_eq!(reference_result["napiProxyObjectPrototypeMatches"], false);
                    assert_eq!(
                        reference_result["napiProxyObjectPrototypeMatchesTrapResult"],
                        true
                    );
                    assert_eq!(reference_result["napiProxyObjectPrototypeIsNull"], false);
                    assert_eq!(
                        reference_result["napiTransparentProxyPrototypeIsNull"],
                        false
                    );
                    assert_eq!(reference_result["proxyTrapCalls"]["napiGetPrototype"], 1);
                    let mut vm_shared = vm_result.clone();
                    let mut bun_shared = reference_result;
                    for result in [&mut vm_shared, &mut bun_shared] {
                        let object = result.as_object_mut().unwrap();
                        object.remove("napiProxyObjectPrototypeMatches");
                        object.remove("napiProxyObjectPrototypeMatchesTrapResult");
                        object.remove("napiProxyObjectPrototypeIsNull");
                        object.remove("napiTransparentProxyPrototypeIsNull");
                        object
                            .get_mut("proxyTrapCalls")
                            .unwrap()
                            .as_object_mut()
                            .unwrap()
                            .remove("napiGetPrototype");
                    }
                    assert_eq!(vm_shared, bun_shared, "{runtime} shared results differ");
                } else {
                    assert_eq!(vm_result, reference_result, "{runtime} results differ");
                }
            }
        }
        assert!(matches!(
            result.get_prop("supportsNapiV7"),
            Some(Value::Bool(true))
        ));
        let detachment = result.get_prop("arrayBufferDetachment").unwrap();
        for (name, expected) in [
            ("ownedDetachStatus", NAPI_OK),
            ("detachStatus", NAPI_OK),
            ("secondDetachStatus", NAPI_OK),
            ("nonArrayBufferStatus", NAPI_OK),
            ("arraybufferLength", 0),
            ("viewLength", 0),
            ("byteOffset", 0),
            ("dataViewLength", 0),
            ("dataViewOffset", 0),
            ("guestArrayBufferLength", 0),
            ("guestViewLength", 0),
            ("guestViewByteLength", 0),
            ("guestViewByteOffset", 0),
        ] {
            assert!(
                matches!(
                    detachment.get_prop(name),
                    Some(Value::Number(value)) if value == expected as f64
                ),
                "unexpected Node-API v7 detachment field {name}: {:?}",
                detachment.get_prop(name)
            );
        }
        assert!(matches!(
            detachment.get_prop("detachedBefore"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            detachment.get_prop("detachedAfter"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            detachment.get_prop("detachedNonArrayBuffer"),
            Some(Value::Bool(false))
        ));
        for name in [
            "guestDataViewByteLength",
            "guestDataViewByteOffset",
            "guestDataViewRead",
            "typedArrayConstruction",
            "dataViewConstruction",
            "arrayBufferSlice",
        ] {
            assert!(
                matches!(
                    detachment.get_prop(name),
                    Some(Value::String(ref value)) if value == "TypeError"
                ),
                "expected detached-buffer TypeError for {name}: {:?}",
                detachment.get_prop(name)
            );
        }
        let bigint_api = result.get_prop("bigintApi").unwrap();
        for (name, expected) in [
            ("signed", "-9223372036854775808"),
            ("unsigned", "18446744073709551615"),
            ("signedRoundtrip", "-9223372036854775808"),
            ("unsignedRoundtrip", "18446744073709551615"),
            ("wrappedSigned", "-1"),
            ("wrappedUnsigned", "9223372036854775808"),
        ] {
            assert!(matches!(
                bigint_api.get_prop(name),
                Some(Value::String(ref value)) if value == expected
            ));
        }
        assert!(matches!(
            bigint_api.get_prop("signedLossless"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            bigint_api.get_prop("unsignedLossless"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            bigint_api.get_prop("wrappedSignedLossless"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            bigint_api.get_prop("wrappedUnsignedLossless"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            bigint_api.get_prop("invalidTypeStatus"),
            Some(Value::Number(status)) if status == NAPI_BIGINT_EXPECTED as f64
        ));
        assert!(matches!(
            result.get_prop("instanceDataMatches"),
            Some(Value::Bool(true))
        ));
        let property_names = result.get_prop("propertyNames").unwrap();
        let get_names = |name: &str| -> Vec<String> {
            let Value::Array(values) = &property_names.get_prop(name).unwrap() else {
                panic!("Node-API v6 property name result {name} is not an array");
            };
            values
                .borrow()
                .iter()
                .map(|value| match value {
                    Value::String(value) => value.clone(),
                    _ => panic!("Node-API v6 property name result has non-string label"),
                })
                .collect()
        };
        assert_eq!(
            get_names("allOwn"),
            [
                "string:3",
                "string:visible",
                "string:hidden",
                "string:01",
                "Symbol(own)"
            ]
        );
        assert_eq!(
            get_names("enumerable"),
            ["string:3", "string:visible", "string:01", "Symbol(own)"]
        );
        assert_eq!(get_names("skipStrings"), ["Symbol(own)"]);
        assert_eq!(
            get_names("withPrototype"),
            [
                "string:3",
                "string:visible",
                "string:01",
                "Symbol(own)",
                "string:inheritedName"
            ]
        );
        assert_eq!(
            get_names("keepNumbers"),
            ["number:3", "string:visible", "string:01", "Symbol(own)"]
        );
        assert_eq!(
            get_names("writable"),
            [
                "string:3",
                "string:visible",
                "string:hidden",
                "string:01",
                "Symbol(own)"
            ]
        );
        assert_eq!(
            get_names("configurable"),
            ["string:3", "string:visible", "string:01", "Symbol(own)"]
        );
        assert_eq!(
            get_names("class"),
            [
                "string:baseValue",
                "string:constant",
                "string:length",
                "string:name",
                "string:offset",
                "string:prototype",
                "string:readOnly"
            ]
        );
        assert_eq!(
            get_names("function"),
            [
                "string:length",
                "string:name",
                "string:prototype",
                "string:nativeProperty",
                "string:definedByNapi"
            ]
        );
        assert_eq!(get_names("array"), ["string:0", "string:length"]);
        for property in ["promiseHasThen", "promiseHasCatch", "promiseHasFinally"] {
            assert!(
                matches!(property_names.get_prop(property), Some(Value::Bool(true))),
                "Node-API prototype key collection missed Promise.prototype.{property}"
            );
        }
        let proxy_property_names = result.get_prop("proxyPropertyNames").unwrap();
        let get_proxy_names = |name: &str| -> Vec<String> {
            let Value::Array(values) = &proxy_property_names.get_prop(name).unwrap() else {
                panic!("Node-API proxy property name result {name} is not an array");
            };
            values
                .borrow()
                .iter()
                .map(|value| match value {
                    Value::String(value) => value.clone(),
                    _ => panic!("Node-API proxy property name result has non-string label"),
                })
                .collect()
        };
        assert_eq!(
            get_proxy_names("allOwn"),
            [
                "string:visible",
                "string:fixed",
                "Symbol(proxy-own)",
                "string:virtual"
            ]
        );
        assert_eq!(
            get_proxy_names("enumerable"),
            ["string:visible", "Symbol(proxy-own)"]
        );
        assert_eq!(get_proxy_names("skipStrings"), ["Symbol(proxy-own)"]);
        assert_eq!(
            get_proxy_names("withPrototype"),
            [
                "string:visible",
                "Symbol(proxy-own)",
                "string:proxyInherited"
            ]
        );
        assert_eq!(
            get_proxy_names("writable"),
            [
                "string:visible",
                "string:fixed",
                "Symbol(proxy-own)",
                "string:virtual"
            ]
        );
        assert_eq!(
            get_proxy_names("configurable"),
            [
                "string:visible",
                "string:fixed",
                "Symbol(proxy-own)",
                "string:virtual"
            ]
        );
        assert!(matches!(
            proxy_property_names.get_prop("ownKeysCalls"),
            Some(Value::Number(value)) if value == 6.0
        ));
        let global_property_names = result.get_prop("globalPropertyNames").unwrap();
        for name in ["hasObject", "hasGlobalThis"] {
            assert!(
                matches!(
                    global_property_names.get_prop(name),
                    Some(Value::Bool(true))
                ),
                "Node-API global property enumeration missed {name}"
            );
        }
        assert!(matches!(
            global_property_names.get_prop("prototypeSupported"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            global_property_names.get_prop("hasObjectPrototypeNames"),
            Some(Value::Bool(true))
        ));
        let class_name_value = interpreter
            .eval_source(
                "require('./fixture.node').propertyNamesProbe({}, require('./fixture.node').Counter, function Reflected(argument) {}, Promise.resolve(1)).classNames;",
            )
            .unwrap();
        let Value::Array(class_names) = &class_name_value else {
            panic!("Node-API v6 class own keys are not an array");
        };
        let class_names = class_names
            .borrow()
            .iter()
            .map(|value| match value {
                Value::String(value) => value.clone(),
                _ => panic!("Node-API v6 class key is not a string"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            class_names,
            [
                "length",
                "name",
                "prototype",
                "constant",
                "baseValue",
                "offset",
                "readOnly"
            ]
        );
        let date_api = result.get_prop("dateApi").unwrap();
        assert!(matches!(
            date_api.get_prop("value"),
            Some(Value::Number(1_700_000_000_123.0))
        ));
        assert!(matches!(
            date_api.get_prop("guestValue"),
            Some(Value::Number(1_700_000_000_123.0))
        ));
        assert!(matches!(
            date_api.get_prop("isDate"),
            Some(Value::Bool(true))
        ));
        for property in [
            "guestPrototypeMatches",
            "nativeApiPrototypeMatches",
            "methodIsShared",
            "prototypeParentMatches",
            "constructorMatches",
            "methodsAreHidden",
        ] {
            assert!(
                matches!(date_api.get_prop(property), Some(Value::Bool(true))),
                "Date prototype result {property} did not match Node/Bun"
            );
        }
        assert!(matches!(
            date_api.get_prop("guestInstance"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            date_api.get_prop("guestConstructedInstance"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            date_api.get_prop("aliasedDateInstance"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            date_api.get_prop("referenceMatches"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            date_api.get_prop("napiInstance"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            date_api.get_prop("numberIsDate"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            date_api.get_prop("invalidDateStatus"),
            Some(Value::Number(18.0))
        ));
        assert!(matches!(
            result.get_prop("externalProbe"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("externalArrayBufferAlias"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("externalBufferAlias"),
            Some(Value::Bool(true))
        ));
        let async_context_result = result.get_prop("asyncContextResult").unwrap();
        assert!(matches!(
            async_context_result.get_prop("callbackResult"),
            Some(Value::String(ref value)) if value == "native-resource:callback-value"
        ));
        assert!(matches!(
            async_context_result.get_prop("nestedCloseStatus"),
            Some(Value::Number(value)) if value == NAPI_OK as f64
        ));
        assert!(matches!(
            async_context_result.get_prop("closeStatus"),
            Some(Value::Number(value)) if value == NAPI_OK as f64
        ));
        assert!(matches!(
            async_context_result.get_prop("destroyStatus"),
            Some(Value::Number(value)) if value == NAPI_OK as f64
        ));
        assert!(matches!(
            async_context_result.get_prop("microtaskRanBeforeReturn"),
            Some(Value::Bool(false))
        ));
        let async_context_events = result
            .get_prop("asyncContextEventsAtReturn")
            .expect("async-context event snapshot exists");
        assert!(matches!(
            async_context_events,
            Value::Array(ref events) if matches!(
                events.borrow().as_slice(),
                [Value::String(callback)] if callback == "callback"
            )
        ));
        assert!(matches!(
            result.get_prop("externalType"),
            Some(Value::String(ref value)) if value == "object"
        ));
        assert!(matches!(
            result.get_prop("externalKeys"),
            Some(Value::Array(ref values)) if values.borrow().is_empty()
        ));
        assert!(matches!(
            result.get_prop("externalJson"),
            Some(Value::String(ref value)) if value == "{}"
        ));
        assert!(matches!(
            result.get_prop("definedConstant"),
            Some(Value::Number(7.0))
        ));
        assert!(matches!(
            result.get_prop("definedSymbolValue"),
            Some(Value::Number(17.0))
        ));
        assert!(matches!(
            result.get_prop("definedMethodEnumerable"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            result.get_prop("definedValueEnumerable"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("definedConstantWritable"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterValue"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("counterIncremented"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("counterInstance"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterConstructor"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterStaticMethod"),
            Some(Value::Number(105.0))
        ));
        assert!(matches!(
            result.get_prop("counterStaticBaseValue"),
            Some(Value::Number(6.0))
        ));
        assert!(matches!(
            result.get_prop("counterStaticBefore"),
            Some(Value::Number(8.0))
        ));
        assert!(matches!(
            result.get_prop("counterStaticAfter"),
            Some(Value::Number(18.0))
        ));
        assert!(matches!(
            result.get_prop("counterReadOnlyBefore"),
            Some(Value::Number(21.0))
        ));
        assert!(matches!(
            result.get_prop("counterReadOnlyAfter"),
            Some(Value::Number(21.0))
        ));
        assert!(matches!(
            result.get_prop("counterStaticDeleteRejected"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            result.get_prop("counterInheritedStatic"),
            Some(Value::String(ref value)) if value == "inherited"
        ));
        assert!(matches!(
            result.get_prop("counterHasInheritedStatic"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterInheritedBaseValue"),
            Some(Value::Number(6.0))
        ));
        assert!(matches!(
            result.get_prop("counterInheritedStaticMethod"),
            Some(Value::Number(105.0))
        ));
        assert!(matches!(
            result.get_prop("counterStaticEnumerable"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterStaticMethodEnumerable"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            result.get_prop("counterStaticBaseWritable"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterStaticKeys"),
            Some(Value::String(ref value)) if value == "baseValue,offset,readOnly"
        ));
        let symbols = result.get_prop("symbols").unwrap();
        assert!(matches!(
            symbols.get_prop("value"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            symbols.get_prop("hasKey"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            symbols.get_prop("hasOtherKey"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            symbols.get_prop("noDescriptionIsSymbol"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            symbols.get_prop("stringKeyCount"),
            Some(Value::Number(0.0))
        ));
        assert!(matches!(
            result.get_prop("sum"),
            Some(Value::Number(value)) if value == 42.0
        ));
        assert!(matches!(
            result.get_prop("version"),
            Some(Value::Number(value)) if value == 1.0
        ));
        assert!(matches!(result.get_prop("truth"), Some(Value::Bool(true))));
        assert!(matches!(
            result.get_prop("nothing"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("missing"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("greeting"),
            Some(Value::String(ref value)) if value == "Node-API ✓"
        ));
        let string_encodings = result.get_prop("stringEncodings").unwrap();
        assert!(matches!(
            string_encodings.get_prop("created"),
            Some(Value::String(ref value)) if value == "A\0éÿ"
        ));
        assert!(matches!(
            string_encodings.get_prop("required"),
            Some(Value::Number(5.0))
        ));
        assert!(matches!(
            string_encodings.get_prop("copied"),
            Some(Value::Number(5.0))
        ));
        assert!(matches!(
            string_encodings.get_prop("truncatedText"),
            Some(Value::String(ref value)) if value == "Aé¬"
        ));
        assert!(matches!(
            string_encodings.get_prop("truncatedCopied"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            string_encodings.get_prop("wrongTypeStatus"),
            Some(Value::Number(value)) if value == NAPI_STRING_EXPECTED as f64
        ));
        let latin1_byte_values = string_encodings.get_prop("bytes").unwrap();
        let Value::Array(string_bytes) = &latin1_byte_values else {
            panic!("Latin-1 extraction did not return an array");
        };
        let extracted_bytes = string_bytes
            .borrow()
            .iter()
            .map(|value| match value {
                Value::Number(value) => *value as u8,
                _ => panic!("Latin-1 byte array contains a non-number"),
            })
            .collect::<Vec<_>>();
        assert_eq!(extracted_bytes, [b'A', 0xE9, 0xAC, 0x3D, 0x00]);
        let utf16 = result.get_prop("utf16").unwrap();
        assert!(matches!(
            utf16.get_prop("roundTrip"),
            Some(Value::String(ref value)) if value == "Aé😀\0Z"
        ));
        assert!(matches!(
            utf16.get_prop("autoLength"),
            Some(Value::String(ref value)) if value == "TERM"
        ));
        assert!(matches!(utf16.get_prop("length"), Some(Value::Number(6.0))));
        assert!(matches!(utf16.get_prop("copied"), Some(Value::Number(6.0))));
        assert!(matches!(
            utf16.get_prop("truncatedCopied"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            utf16.get_prop("wrongTypeStatus"),
            Some(Value::Number(value)) if value == NAPI_STRING_EXPECTED as f64
        ));
        assert_eq!(
            number_array(utf16.get_prop("units").unwrap()),
            [65, 233, 0xD83D, 0xDE00, 0, 90]
        );
        assert_eq!(
            number_array(utf16.get_prop("truncatedUnits").unwrap()),
            [65, 233, 0xD83D]
        );
        let element_delete = result.get_prop("elementDelete").unwrap();
        assert!(matches!(
            element_delete.get_prop("deleted"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            element_delete.get_prop("present"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            element_delete.get_prop("length"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            result.get_prop("elementDeleteLength"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            result.get_prop("elementDeleteHole"),
            Some(Value::Bool(true))
        ));
        assert_eq!(
            number_array(result.get_prop("elementDeleteRemaining").unwrap()),
            [10, 30]
        );
        let escapable_scope = result.get_prop("escapableScope").unwrap();
        assert!(matches!(
            escapable_scope.get_prop("escaped"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            escapable_scope.get_prop("secondEscapeStatus"),
            Some(Value::Number(value)) if value == NAPI_ESCAPE_CALLED_TWICE as f64
        ));
        assert!(matches!(
            result.get_prop("runScriptResult"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("runScriptSideEffect"),
            Some(Value::Number(1.0))
        ));
        assert!(matches!(
            result.get_prop("runScriptMicrotaskRanDuringCall"),
            Some(Value::Bool(false))
        ));
        let boolean_coercions = result.get_prop("booleanCoercions").unwrap();
        let Value::Array(boolean_coercions) = &boolean_coercions else {
            panic!("Node-API boolean coercion fixture did not return an array");
        };
        let boolean_coercion_values = boolean_coercions
            .borrow()
            .iter()
            .map(|value| matches!(value, Value::Bool(true)))
            .collect::<Vec<_>>();
        assert_eq!(
            boolean_coercion_values,
            [
                false, false, false, false, false, false, false, false, true, true
            ]
        );
        let number_coercions = result.get_prop("numberCoercions").unwrap();
        let Value::Array(number_coercions) = &number_coercions else {
            panic!("Node-API number coercion fixture did not return an array");
        };
        let number_coercion_values = number_coercions.borrow();
        let number_coercion_values = number_coercion_values
            .iter()
            .map(|value| match value {
                Value::Number(value) => *value,
                _ => panic!("Node-API number coercion returned a non-number"),
            })
            .collect::<Vec<_>>();
        assert!(number_coercion_values[0].is_nan());
        assert_eq!(
            &number_coercion_values[1..11],
            &[0.0, 0.0, 1.0, 0.0, 0.0, 16.0, 3.0, 8.0, 1.5, f64::INFINITY]
        );
        assert!(number_coercion_values[11].is_nan());
        let string_coercions = result.get_prop("stringCoercions").unwrap();
        let Value::Array(string_coercions) = &string_coercions else {
            panic!("Node-API string coercion fixture did not return an array");
        };
        let string_coercion_values = {
            let values = string_coercions.borrow();
            values
                .iter()
                .map(|value| match value {
                    Value::String(value) => value.clone(),
                    _ => panic!("Node-API string coercion returned a non-string"),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            string_coercion_values,
            [
                "0",
                "0",
                "null",
                "undefined",
                "true",
                "12",
                "1,2",
                "[object Object]"
            ]
        );
        assert!(matches!(
            result.get_prop("guestNumberCoercion"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("guestStringCoercion"),
            Some(Value::String(ref value)) if value == "23"
        ));
        assert!(matches!(
            result.get_prop("exoticNumberCoercion"),
            Some(Value::Number(44.0))
        ));
        assert!(matches!(
            result.get_prop("exoticStringCoercion"),
            Some(Value::String(ref value)) if value == "exotic"
        ));
        let coercion_errors = result.get_prop("coercionErrors").unwrap();
        for name in ["symbolNumber", "symbolString", "bigintNumber"] {
            let error = coercion_errors.get_prop(name).unwrap();
            assert!(matches!(
                error.get_prop("name"),
                Some(Value::String(ref value)) if value == "TypeError"
            ));
            assert!(matches!(
                error.get_prop("isTypeError"),
                Some(Value::Bool(true))
            ));
        }
        let object_coercions = result.get_prop("objectCoercions").unwrap();
        let Value::Array(object_coercions) = &object_coercions else {
            panic!("Node-API object coercion fixture did not return an array");
        };
        let object_coercions = object_coercions.borrow();
        let object_strings = object_coercions
            .iter()
            .map(|value| match value.get_prop("string") {
                Some(Value::String(ref value)) => value.clone(),
                _ => panic!("boxed primitive string conversion did not return a string"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            object_strings,
            ["false", "12", "abc", "Symbol(value)", "13"]
        );
        let primitive_types = object_coercions
            .iter()
            .map(|value| match value.get_prop("primitiveType") {
                Some(Value::String(ref value)) => value.clone(),
                _ => panic!("boxed primitive valueOf returned an unexpected type"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            primitive_types,
            ["boolean", "number", "string", "symbol", "bigint"]
        );
        for (index, value) in object_coercions.iter().enumerate() {
            assert!(matches!(value.get_prop("type"), Some(Value::String(ref t)) if t == "object"));
            assert!(matches!(value.get_prop("same"), Some(Value::Bool(false))));
            assert!(
                matches!(value.get_prop("guestPrototype"), Some(Value::Bool(true))),
                "boxed primitive {index} has a different guest prototype: {value:?}"
            );
            assert!(
                matches!(value.get_prop("napiPrototype"), Some(Value::Bool(true))),
                "boxed primitive {index} has a different Node-API prototype: {value:?}"
            );
        }
        assert!(matches!(
            object_coercions[2].get_prop("length"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            result.get_prop("objectCoercionPreservesIdentity"),
            Some(Value::Bool(true))
        ));
        let object_coercion_errors = result.get_prop("objectCoercionErrors").unwrap();
        let Value::Array(object_coercion_errors) = &object_coercion_errors else {
            panic!("Node-API nullish object coercion fixture did not return an array");
        };
        assert!(object_coercion_errors.borrow().iter().all(|error| {
            matches!(error.get_prop("name"), Some(Value::String(ref name)) if name == "TypeError")
                && matches!(error.get_prop("isTypeError"), Some(Value::Bool(true)))
        }));
        let coercion_events = result.get_prop("coercionEvents").unwrap();
        let Value::Array(coercion_events) = &coercion_events else {
            panic!("Node-API coercion event fixture did not return an array");
        };
        assert!(matches!(
            coercion_events.borrow().as_slice(),
            [
                Value::String(number),
                Value::String(string),
                Value::String(number_hint),
                Value::String(string_hint)
            ] if number == "number.valueOf"
                && string == "string.toString"
                && number_hint == "symbol:number"
                && string_hint == "symbol:string"
        ));
        assert!(matches!(
            result.get_prop("fraction"),
            Some(Value::Number(value)) if value == 1.25
        ));
        assert!(matches!(
            result.get_prop("maxUint32"),
            Some(Value::Number(value)) if value == u32::MAX as f64
        ));
        assert!(matches!(
            result.get_prop("int64"),
            Some(Value::Number(value)) if value == 2_147_483_648.0
        ));
        let int64_conversions = result.get_prop("int64Conversions").unwrap();
        let Value::Array(int64_conversions) = &int64_conversions else {
            panic!("Node-API int64 conversion fixture did not return an array");
        };
        let int64_conversion_values = int64_conversions
            .borrow()
            .iter()
            .map(|value| match value {
                Value::Number(value) => *value,
                _ => panic!("Node-API int64 conversion fixture returned a non-number"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            int64_conversion_values,
            [
                0.0,
                0.0,
                0.0,
                0.0,
                3.0,
                -3.0,
                i64::MAX as f64,
                i64::MIN as f64
            ]
        );
        let prototypes = result.get_prop("prototypes").unwrap();
        assert!(matches!(
            prototypes.get_prop("defaultMatches"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            prototypes.get_prop("nativeFunctionMatches"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            prototypes.get_prop("customMatches"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            prototypes.get_prop("nullMatches"),
            Some(Value::Bool(true))
        ));
        let array_prototypes = result.get_prop("arrayPrototypes").unwrap();
        for property in [
            "defaultMatches",
            "prototypeParentMatches",
            "prototypeIsArray",
            "constructorMatches",
            "mapIsShared",
            "iteratorIsValues",
            "mapDescriptor",
            "keysAreEmpty",
            "inheritsObjectMethod",
            "nativeApiDefaultMatches",
            "nativeApiParentMatches",
            "objectCreateInherits",
            "customPrototypeMutation",
        ] {
            assert!(
                matches!(array_prototypes.get_prop(property), Some(Value::Bool(true))),
                "Array prototype result {property} did not match Node/Bun"
            );
        }
        let promise_prototypes = result.get_prop("promisePrototypes").unwrap();
        for property in [
            "defaultMatches",
            "nativeApiMatches",
            "instanceofMatches",
            "parentMatches",
            "constructorMatches",
            "methodIsShared",
            "methodsAreHidden",
        ] {
            assert!(
                matches!(
                    promise_prototypes.get_prop(property),
                    Some(Value::Bool(true))
                ),
                "Promise prototype result {property} did not match Node/Bun"
            );
        }
        let binary_prototypes = result.get_prop("binaryPrototypes").unwrap();
        for property in [
            "typedArrayDefault",
            "typedArrayNapi",
            "typedArrayParentsShared",
            "typedArrayConstructor",
            "typedArrayIteratorIsValues",
            "typedArrayMethodShared",
            "typedArrayMethodCall",
            "typedArrayMethodsHidden",
            "arrayBufferDefault",
            "arrayBufferNapi",
            "arrayBufferMethodShared",
            "arrayBufferSlice",
            "sharedArrayBufferDefault",
            "sharedArrayBufferNapi",
            "sharedArrayBufferMethodShared",
            "dataViewDefault",
            "dataViewNapi",
            "dataViewMethodShared",
            "dataViewMethodCall",
            "dataViewMethodsHidden",
            "napiBufferIsBuffer",
            "napiBufferDefault",
            "napiBufferPrototype",
            "bufferPrototypeParent",
            "bufferConstructorParent",
            "napiBufferText",
            "napiBufferJson",
        ] {
            assert!(
                matches!(
                    binary_prototypes.get_prop(property),
                    Some(Value::Bool(true))
                ),
                "binary prototype result {property} did not match Node/Bun"
            );
        }
        let round_trip = result.get_prop("roundTrip").unwrap();
        assert!(matches!(
            round_trip.get_prop("flag"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            round_trip.get_prop("number"),
            Some(Value::Number(value)) if value == 4.75
        ));
        assert!(matches!(
            round_trip.get_prop("text"),
            Some(Value::String(ref value)) if value == "native ✓"
        ));
        assert!(matches!(
            round_trip.get_prop("uint32"),
            Some(Value::Number(value)) if value == u32::MAX as f64
        ));
        assert!(matches!(
            round_trip.get_prop("int64"),
            Some(Value::Number(value)) if value == -2.0
        ));
        assert!(matches!(
            round_trip.get_prop("boolType"),
            Some(Value::Number(2.0))
        ));
        assert!(matches!(
            round_trip.get_prop("numberType"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            round_trip.get_prop("stringType"),
            Some(Value::Number(4.0))
        ));
        let array = result.get_prop("array").unwrap();
        assert!(matches!(array.get_prop("isArray"), Some(Value::Bool(true))));
        assert!(matches!(array.get_prop("proxyArray"), Some(Value::Bool(false))));
        assert!(matches!(
            array.get_prop("nestedProxyArray"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            array.get_prop("proxyObjectIsArray"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(array.get_prop("jsIsArray"), Some(Value::Bool(true))));
        assert!(matches!(array.get_prop("length"), Some(Value::Number(5.0))));
        assert!(matches!(
            array.get_prop("emptyLength"),
            Some(Value::Number(0.0))
        ));
        assert!(matches!(
            array.get_prop("firstPresent"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            array.get_prop("secondPresent"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            array.get_prop("holePresent"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(array.get_prop("value"), Some(Value::Bool(true))));
        let vm_array_json = interpreter
            .eval_source("JSON.stringify(require('./main.cjs').array);")
            .unwrap();
        let Value::String(ref vm_array_json) = vm_array_json else {
            panic!("Node-API array probe did not return JSON");
        };
        let vm_array: serde_json::Value = serde_json::from_str(vm_array_json).unwrap();
        let array_probe_runner = r#"
            const addon = require('./fixture.node');
            const proxyArray = new Proxy([], {});
            const probe = addon.arrayProbe(
                proxyArray,
                new Proxy(new Proxy([], {}), {}),
                new Proxy({}, {}),
            );
            probe.jsIsArray = Array.isArray(proxyArray);
            process.stdout.write(JSON.stringify(probe));
        "#;
        for runtime in ["node", "bun"] {
            if Command::new(runtime)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
            {
                let reference = Command::new(runtime)
                    .current_dir(&root)
                    .args(["-e", array_probe_runner])
                    .output()
                    .unwrap();
                assert!(
                    reference.status.success(),
                    "{runtime} Node-API array reference failed: {}",
                    String::from_utf8_lossy(&reference.stderr)
                );
                let reference_array: serde_json::Value =
                    serde_json::from_slice(&reference.stdout).unwrap();
                assert_eq!(vm_array, reference_array, "{runtime} array probe differs");
            }
        }
        assert!(
            matches!(result.get_prop("wrapped"), Some(Value::String(ref value)) if value == "wrapped-native-data")
        );
        assert!(
            matches!(result.get_prop("removedWrap"), Some(Value::String(ref value)) if value == "removed-native-data")
        );
        assert!(
            matches!(result.get_prop("duplicateWrapStatus"), Some(Value::Number(value)) if value == NAPI_INVALID_ARG as f64)
        );
        assert!(matches!(
            result.get_prop("distinctFunctionIdentity"),
            Some(Value::Bool(true))
        ));
        let buffers = result.get_prop("buffers").unwrap();
        let copied = buffers.get_prop("copy").unwrap();
        let Value::Array(copied) = &copied else {
            panic!("copied buffer values are not an array");
        };
        let copied = copied.borrow();
        assert!(matches!(copied.first(), Some(Value::Number(65.0))));
        assert!(matches!(copied.get(1), Some(Value::Number(120.0))));
        assert!(matches!(copied.get(2), Some(Value::Number(67.0))));
        assert!(matches!(copied.get(3), Some(Value::Number(68.0))));
        let allocated = buffers.get_prop("allocated").unwrap();
        let Value::Array(allocated) = &allocated else {
            panic!("allocated buffer values are not an array");
        };
        let allocated = allocated.borrow();
        assert!(matches!(allocated.first(), Some(Value::Number(7.0))));
        assert!(matches!(allocated.get(1), Some(Value::Number(8.0))));
        assert!(matches!(allocated.get(2), Some(Value::Number(9.0))));
        assert!(matches!(
            buffers.get_prop("copyIsBuffer"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            buffers.get_prop("allocatedIsBuffer"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            buffers.get_prop("arrayIsBuffer"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            buffers.get_prop("copyLength"),
            Some(Value::Number(4.0))
        ));
        assert!(matches!(
            buffers.get_prop("allocatedLength"),
            Some(Value::Number(3.0))
        ));
        let typed_arrays = result.get_prop("typedArrays").unwrap();
        let bytes = typed_arrays.get_prop("bytes").unwrap();
        let Value::Array(bytes) = &bytes else {
            panic!("backing bytes are not an array");
        };
        let bytes = bytes.borrow();
        let expected = [10.0, 11.0, 12.0, 55.0, 77.0, 15.0, 16.0, 17.0];
        for (byte, expected) in bytes.iter().zip(expected) {
            assert!(matches!(byte, Value::Number(value) if *value == expected));
        }
        assert!(matches!(
            typed_arrays.get_prop("isArrayBuffer"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            typed_arrays.get_prop("isTypedArray"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            typed_arrays.get_prop("isDataView"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            typed_arrays.get_prop("kind"),
            Some(Value::Number(4.0))
        ));
        assert!(matches!(
            typed_arrays.get_prop("typedLength"),
            Some(Value::Number(2.0))
        ));
        assert!(matches!(
            typed_arrays.get_prop("typedOffset"),
            Some(Value::Number(2.0))
        ));
        assert!(matches!(
            typed_arrays.get_prop("viewLength"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            typed_arrays.get_prop("viewOffset"),
            Some(Value::Number(4.0))
        ));
        assert_error_fields(
            &result.get_prop("typedArrayError").unwrap(),
            "RangeError",
            "start offset of Uint16Array should be a multiple of 2",
            Some("ERR_NAPI_INVALID_TYPEDARRAY_ALIGNMENT"),
        );
        assert!(matches!(
            result
                .get_prop("typedArrayError")
                .unwrap()
                .get_prop("isRangeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result
                .get_prop("typedArrayError")
                .unwrap()
                .get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("callbackResult"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("callbackReceiverBase"),
            Some(Value::Number(42.0))
        ));
        assert_error_fields(
            &result.get_prop("callbackError").unwrap(),
            "RangeError",
            "guest callback failure",
            None,
        );
        assert!(matches!(
            result
                .get_prop("callbackError")
                .unwrap()
                .get_prop("isRangeError"),
            Some(Value::Bool(true))
        ));
        assert!(
            matches!(
                result
                    .get_prop("callbackError")
                    .unwrap()
                    .get_prop("isError"),
                Some(Value::Bool(true))
            ),
            "callback error: {:?}",
            result.get_prop("callbackError")
        );
        assert!(matches!(
            result.get_prop("constructedValue"),
            Some(Value::String(ref value)) if value == "constructed"
        ));
        assert!(matches!(
            result.get_prop("callbackThrown"),
            Some(Value::String(ref value)) if value == "guest primitive failure"
        ));
        let properties = result.get_prop("properties").unwrap();
        assert!(matches!(
            properties.get_prop("computed"),
            Some(Value::Number(23.0))
        ));
        assert!(matches!(
            properties.get_prop("hasInherited"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            properties.get_prop("hasNamedInherited"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            properties.get_prop("hasOwnInherited"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            properties.get_prop("deleted"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            properties.get_prop("getterCount"),
            Some(Value::Number(1.0))
        ));
        assert!(matches!(
            properties.get_prop("setterValue"),
            Some(Value::String(ref value)) if value == "set through addon"
        ));
        assert!(matches!(
            properties.get_prop("removedFromGuest"),
            Some(Value::Bool(true))
        ));
        let names = properties.get_prop("names").unwrap();
        let Value::Array(names) = &names else {
            panic!("property names are not an array");
        };
        let names = names.borrow();
        assert!(
            matches!(names.as_slice(), [Value::String(a), Value::String(b), Value::String(c)] if a == "computed" && b == "assigned" && c == "inherited")
        );
        assert!(matches!(
            result.get_prop("undefinedResult"),
            Some(Value::Bool(true))
        ));
        let reference = result.get_prop("reference").unwrap();
        assert!(matches!(
            reference.get_prop("sameValue"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            reference.get_prop("countAfterUnref"),
            Some(Value::Number(0.0))
        ));
        assert!(matches!(
            reference.get_prop("countAfterRef"),
            Some(Value::Number(1.0))
        ));
        assert!(matches!(
            reference.get_prop("released"),
            Some(Value::Bool(true))
        ));
        let errors = result.get_prop("errors").unwrap();
        assert_error_fields(
            &errors.get_prop("error").unwrap(),
            "Error",
            "created",
            Some("E_CREATED"),
        );
        assert!(matches!(
            errors.get_prop("error").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &errors.get_prop("typeError").unwrap(),
            "TypeError",
            "created",
            Some("E_CREATED"),
        );
        assert!(matches!(
            errors
                .get_prop("typeError")
                .unwrap()
                .get_prop("isTypeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            errors.get_prop("typeError").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &errors.get_prop("rangeError").unwrap(),
            "RangeError",
            "created",
            Some("E_CREATED"),
        );
        assert!(matches!(
            errors
                .get_prop("rangeError")
                .unwrap()
                .get_prop("isRangeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            errors.get_prop("rangeError").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &result.get_prop("typeError").unwrap(),
            "TypeError",
            "type failure",
            Some("E_TYPE"),
        );
        assert!(matches!(
            result
                .get_prop("typeError")
                .unwrap()
                .get_prop("isTypeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("typeError").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &result.get_prop("rangeError").unwrap(),
            "RangeError",
            "range failure",
            None,
        );
        assert!(matches!(
            result
                .get_prop("rangeError")
                .unwrap()
                .get_prop("isRangeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("rangeError").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &result.get_prop("createdThrow").unwrap(),
            "TypeError",
            "thrown",
            None,
        );
        assert!(matches!(
            result
                .get_prop("createdThrow")
                .unwrap()
                .get_prop("isTypeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("createdThrow").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &result.get_prop("cleared").unwrap(),
            "Error",
            "cleared failure",
            Some("E_CLEARED"),
        );

        assert!(matches!(
            interpreter
                .eval_source("require('./fixture.node').externalProbe();")
                .unwrap(),
            Value::Bool(true)
        ));
        assert!(matches!(
            interpreter
                .eval_source("require('./fixture.node').externalPropertyProbe();")
                .unwrap(),
            Value::Bool(true)
        ));
        assert!(matches!(
            interpreter
                .eval_source("require('./fixture.node').externalMemoryProbe();")
                .unwrap(),
            Value::Number(value) if value == -4096.0
        ));
        assert!(matches!(
            interpreter
                .eval_source("(() => { const e = require('./fixture.node').external; return Object.getPrototypeOf(e) === null && !Object.isExtensible(e) && e.missing === undefined && Object.keys(e).length === 0; })();")
                .unwrap(),
            Value::Bool(true)
        ));

        // Bun 1.4.0 currently returns zero for this accounting API, so keep
        // the Node-semantic check separate from the shared Node/Bun fixture.
        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let reference = Command::new("node")
                .current_dir(&root)
                .args([
                    "-e",
                    "process.stdout.write(String(require('./fixture.node').externalMemoryProbe()))",
                ])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Node external-memory reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let node_delta = String::from_utf8_lossy(&reference.stdout)
                .trim()
                .parse::<i64>()
                .expect("Node external-memory delta is an integer");
            assert_eq!(node_delta, -4096);
        }

        let guest_json = interpreter
            .eval_source("JSON.stringify(require('./main.cjs'));")
            .unwrap();
        let Value::String(ref guest_json) = guest_json else {
            panic!("JSON.stringify did not return a string");
        };
        let guest_result: serde_json::Value =
            serde_json::from_str(guest_json).expect("guest result is valid JSON");

        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let reference = Command::new("node")
                .args([
                    "-e",
                    "process.stdout.write(JSON.stringify(require(process.argv[1])))",
                ])
                .arg(root.join("main.cjs"))
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Node reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let node_result: serde_json::Value =
                serde_json::from_slice(&reference.stdout).expect("Node result is valid JSON");
            assert_eq!(
                node_result.get("arrayBufferDetachment"),
                guest_result.get("arrayBufferDetachment"),
                "Node-API v7 detachment mismatch"
            );
            assert_eq!(
                node_result.get("regexpPrototypes"),
                guest_result.get("regexpPrototypes"),
                "Node-API RegExp prototype mismatch"
            );
            assert_eq!(node_result, guest_result);
        }

        if let Ok(bun_version) = Command::new("bun").arg("--version").output()
            && bun_version.status.success()
        {
            let reference = Command::new("bun")
                .args([
                    "-e",
                    "process.stdout.write(JSON.stringify(require(process.argv[1])))",
                ])
                .arg(root.join("main.cjs"))
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Bun reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let mut bun_result: serde_json::Value =
                serde_json::from_slice(&reference.stdout).expect("Bun result is valid JSON");
            let mut normalized_guest_result = guest_result.clone();
            let bun_non_arraybuffer_status = bun_result
                .pointer("/arrayBufferDetachment/nonArrayBufferStatus")
                .and_then(serde_json::Value::as_i64);
            let vm_non_arraybuffer_status = normalized_guest_result
                .pointer("/arrayBufferDetachment/nonArrayBufferStatus")
                .and_then(serde_json::Value::as_i64);
            assert_eq!(vm_non_arraybuffer_status, Some(NAPI_OK as i64));
            assert_eq!(
                bun_non_arraybuffer_status,
                Some(NAPI_ARRAYBUFFER_EXPECTED as i64),
                "Bun's napi_is_detached_arraybuffer type check changed; review the known Node/Bun semantic difference"
            );
            for output in [&mut bun_result, &mut normalized_guest_result] {
                output["arrayBufferDetachment"]
                    .as_object_mut()
                    .unwrap()
                    .remove("nonArrayBufferStatus");
            }
            for output in [&mut bun_result, &mut normalized_guest_result] {
                if let Some(error) = output
                    .get_mut("typedArrayError")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    // Node and Bun both throw a RangeError for this invalid
                    // typed-array view, but the message and Node error code
                    // are runtime-specific details.
                    error.remove("message");
                    error.remove("code");
                }
            }
            let node_proxy_filter_result = serde_json::json!([
                "string:visible",
                "string:fixed",
                "Symbol(proxy-own)",
                "string:virtual"
            ]);
            let bun_proxy_filter_result =
                serde_json::json!(["string:visible", "Symbol(proxy-own)"]);
            for filter in ["writable", "configurable"] {
                assert_eq!(
                    normalized_guest_result.pointer(&format!("/proxyPropertyNames/{filter}")),
                    Some(&node_proxy_filter_result),
                    "napi-vm should follow Node's Proxy {filter} filter result"
                );
                assert_eq!(
                    bun_result.pointer(&format!("/proxyPropertyNames/{filter}")),
                    Some(&bun_proxy_filter_result),
                    "Bun's Proxy {filter} filter behavior changed; review this runtime difference"
                );
            }
            for output in [&mut bun_result, &mut normalized_guest_result] {
                let proxy_names = output
                    .get_mut("proxyPropertyNames")
                    .and_then(serde_json::Value::as_object_mut)
                    .unwrap();
                proxy_names.remove("writable");
                proxy_names.remove("configurable");
            }
            assert_eq!(
                bun_result.get("regexpPrototypes"),
                normalized_guest_result.get("regexpPrototypes"),
                "Bun/N-API RegExp prototype mismatch"
            );
            assert_eq!(bun_result, normalized_guest_result);
        }

        let async_runner = "(async function() { const addon = require('./fixture.node'); process.stdout.write(JSON.stringify({result: await addon.runAsync()})); })().catch(error => { console.error(error); process.exitCode = 1; });";
        let vm_async_result = interpreter
            .eval_source("let asyncResult = await require('./fixture.node').runAsync(); JSON.stringify({result: asyncResult});")
            .unwrap();
        let Value::String(ref vm_async_json) = vm_async_result else {
            panic!("async Node-API fixture did not return JSON: {vm_async_result:?}");
        };
        let vm_async_result: serde_json::Value =
            serde_json::from_str(vm_async_json).expect("VM async result is valid JSON");

        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let reference = Command::new("node")
                .current_dir(&root)
                .args(["-e", async_runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Node async reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let node_async_result: serde_json::Value =
                serde_json::from_slice(&reference.stdout).expect("Node async result is valid JSON");
            assert_eq!(vm_async_result, node_async_result);
        }

        if let Ok(bun_version) = Command::new("bun").arg("--version").output()
            && bun_version.status.success()
        {
            let reference = Command::new("bun")
                .current_dir(&root)
                .args(["-e", async_runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Bun async reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let bun_async_result: serde_json::Value =
                serde_json::from_slice(&reference.stdout).expect("Bun async result is valid JSON");
            assert_eq!(vm_async_result, bun_async_result);
        }

        let invalid_env = interpreter
            .eval_source("require('./fixture.node').invalidEnvironment();")
            .unwrap();
        assert!(matches!(
            invalid_env,
            Value::Number(value) if value == NAPI_INVALID_ARG as f64
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "JSON.stringify(require('./fixture.node').cleanupMisuseStatus());"
                )
                .unwrap(),
            Value::String(ref value) if value == "{\"duplicate\":1,\"unmatched\":1}"
        ));
        assert!(matches!(
            interpreter
                .eval_source("JSON.stringify(require('./fixture.node').errorInfoProbe());")
                .unwrap(),
            Value::String(ref value)
                if value == "{\"lastStatus\":6,\"messageMatches\":true}"
        ));

        drop(result);
        drop(invalid_env);
        drop(interpreter);
        assert_eq!(unsafe { wrapped_finalizer_calls() }, 1);
        assert_eq!(unsafe { added_finalizer_calls() }, 1);
        assert_eq!(unsafe { instance_data_finalizer_calls() }, 1);
        assert_eq!(unsafe { replaced_instance_data_finalizer_calls() }, 0);
        assert_eq!(unsafe { instance_data_visible_in_finalizer() }, 1);
        assert_eq!(unsafe { removed_finalizer_calls() }, 0);
        assert_eq!(unsafe { external_finalizer_calls() }, 1);
        assert_eq!(unsafe { external_arraybuffer_finalizer_calls() }, 1);
        assert_eq!(unsafe { external_buffer_finalizer_calls() }, 1);
        assert_eq!(unsafe { finalizer_create_function_status() }, NAPI_OK);
        assert_eq!(unsafe { cleanup_hook_count() }, 2);
        assert_eq!(unsafe { cleanup_hook_value(0) }, 4);
        assert_eq!(unsafe { cleanup_hook_value(1) }, 3);
        assert_eq!(unsafe { cleanup_before_wrap_finalizer() }, 1);
        drop(observer);
        fs::remove_dir_all(root).unwrap();
    }
