    #[test]
    fn node_api_v10_header_symbols_are_exported_by_the_shim() {
        use std::io::Write;
        use std::process::Stdio;

        let include_dirs = [
            std::env::var_os("NODE_INCLUDE_DIR").map(PathBuf::from),
            Some(PathBuf::from("/usr/include/node")),
            Some(PathBuf::from("/usr/local/include/node")),
        ];
        let Some(include) = include_dirs
            .into_iter()
            .flatten()
            .find(|path| path.join("node_api.h").is_file())
        else {
            eprintln!("skipping Node-API symbol audit: Node headers are unavailable");
            return;
        };
        if !Command::new("cc")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            eprintln!("skipping Node-API symbol audit: C compiler is unavailable");
            return;
        }

        let mut preprocessor = Command::new("cc")
            .args([
                "-E",
                "-x",
                "c",
                "-DNAPI_VERSION=10",
                "-DNAPI_EXPERIMENTAL",
                "-I",
            ])
            .arg(&include)
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("C compiler was available above");
        preprocessor
            .stdin
            .take()
            .expect("preprocessor stdin is piped")
            .write_all(b"#include <node_api.h>\n#include <js_native_api.h>\n")
            .unwrap();
        let headers = preprocessor.wait_with_output().unwrap();
        assert!(
            headers.status.success(),
            "could not preprocess Node-API headers: {}",
            String::from_utf8_lossy(&headers.stderr)
        );

        fn tokens(source: &str) -> Vec<&str> {
            let mut tokens = Vec::new();
            let mut token_start = None;
            for (index, character) in source.char_indices() {
                if character.is_ascii_alphanumeric() || character == '_' {
                    token_start.get_or_insert(index);
                } else {
                    if let Some(start) = token_start.take() {
                        tokens.push(&source[start..index]);
                    }
                    if character == '(' {
                        tokens.push("(");
                    }
                }
            }
            if let Some(start) = token_start {
                tokens.push(&source[start..]);
            }
            tokens
        }
        let is_node_api_symbol = |name: &str| {
            name.starts_with("napi_") || name.starts_with("node_api_")
        };
        let preprocessed_headers = String::from_utf8_lossy(&headers.stdout);
        let header_tokens = tokens(&preprocessed_headers);
        let declared_symbols = header_tokens
            .windows(3)
            .filter(|window| {
                matches!(window[0], "napi_status" | "void")
                    && is_node_api_symbol(window[1])
                    && window[2] == "("
            })
            .map(|window| window[1].to_owned())
            .collect::<HashSet<_>>();
        let shim_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("native")
            .join("node_api_shim.c");
        let shim = fs::read_to_string(shim_path).unwrap();
        let shim_tokens = tokens(&shim);
        let exported_symbols = shim_tokens
            .windows(4)
            .filter(|window| {
                window[0] == "NAPI_VM_EXPORT"
                    && matches!(window[1], "napi_status" | "void")
                    && is_node_api_symbol(window[2])
                    && window[3] == "("
            })
            .map(|window| window[2].to_owned())
            .chain(
                shim_tokens
                    .windows(5)
                    .filter(|window| {
                        window[0] == "NAPI_VM_EXPORT"
                            && window[1] == "NAPI_VM_NO_RETURN"
                            && window[2] == "void"
                            && is_node_api_symbol(window[3])
                            && window[4] == "("
                    })
                    .map(|window| window[3].to_owned()),
            )
            .collect::<HashSet<_>>();
        let missing = declared_symbols
            .difference(&exported_symbols)
            .cloned()
            .collect::<Vec<_>>();

        assert!(!declared_symbols.is_empty(), "no Node-API symbols found");
        assert!(
            declared_symbols.len() > 100,
            "the Node-API symbol audit found only {} declarations",
            declared_symbols.len()
        );
        assert!(
            missing.is_empty(),
            "Node-API v10 header symbols are missing from the Rust host shim: {missing:?}"
        );
    }

    #[test]
    fn configured_node_api_ceiling_is_reported_and_enforced() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-version-limit-{}-{}",
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
            eprintln!("skipping Node-API version fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");

        let source = root.join("fixture.c");
        let addon = root.join("fixture.node");
        fs::write(
            &source,
            r#"
#define NAPI_VERSION 7
#include <node_api.h>

NAPI_MODULE_INIT() {
  uint32_t supported_version = 0;
  napi_value version;
  if (napi_get_version(env, &supported_version) != napi_ok ||
      napi_create_uint32(env, supported_version, &version) != napi_ok ||
      napi_set_named_property(env, exports, "supportedVersion", version) != napi_ok)
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
            "Node-API version fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut compatible = Interpreter::with_builtins();
        compatible
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest)
                    .max_napi_version(7),
            )
            .unwrap();
        assert!(matches!(
            compatible
                .eval_source("require('./fixture.node').supportedVersion;")
                .unwrap(),
            Value::Number(7.0)
        ));
        drop(compatible);

        let mut incompatible = Interpreter::with_builtins();
        incompatible
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest)
                    .max_napi_version(6),
            )
            .unwrap();
        let error = incompatible
            .eval_source("require('./fixture.node');")
            .unwrap_err();
        assert!(error.to_string().contains("requests version 7"));
        assert!(
            error
                .to_string()
                .contains("configured for Node-API versions 1 through 6")
        );
        drop(incompatible);

        let mut invalid = Interpreter::with_builtins();
        let error = match invalid.enable_rust_node_api_addons(
            RustNodeApiOptions::new(std::iter::empty::<PathBuf>()).max_napi_version(0),
        ) {
            Err(error) => error,
            Ok(_) => panic!("invalid Node-API ceiling was accepted"),
        };
        assert!(
            error
                .to_string()
                .contains("outside the supported range 1 through 10")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn experimental_sharedarraybuffer_node_api_preserves_shared_identity_and_views() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-shared-arraybuffer-{}-{}",
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
            eprintln!("skipping SharedArrayBuffer fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");
        let experimental_headers = fs::read_to_string(include.join("js_native_api.h")).unwrap();
        if !experimental_headers.contains("node_api_create_sharedarraybuffer")
            || !experimental_headers.contains("node_api_create_external_sharedarraybuffer")
            || !experimental_headers.contains("node_api_is_sharedarraybuffer")
        {
            eprintln!("skipping SharedArrayBuffer fixture: experimental APIs are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        }

        let source = root.join("fixture.c");
        let addon = root.join("fixture.node");
        fs::write(
            &source,
            r#"
#define NAPI_EXPERIMENTAL 1
#define NAPI_VERSION 10
#include <node_api.h>
#include <stdint.h>

static _Alignas(8) uint32_t external_shared[2];
static int external_finalizer_count;

int fixture_external_finalizer_count(void) {
  return external_finalizer_count;
}

static void finalize_shared(void* data, void* hint) {
  (void)data; (void)hint;
  external_finalizer_count++;
}

static napi_value make_shared(napi_env env, napi_callback_info info) {
  void* data = NULL;
  napi_value result;
  (void)info;
  if (node_api_create_sharedarraybuffer(env, 8, &data, &result) != napi_ok)
    return NULL;
  ((uint8_t*)data)[0] = 17;
  return result;
}

static napi_value make_external_shared(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  external_shared[0] = UINT32_C(0x12345678);
  external_shared[1] = UINT32_C(0xabcdef01);
  if (node_api_create_external_sharedarraybuffer(
          env, external_shared, sizeof(external_shared), finalize_shared,
          NULL, &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value is_shared(napi_env env, napi_callback_info info) {
  napi_value args[1], result;
  size_t argc = 1;
  bool shared = false;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok ||
      argc != 1 || node_api_is_sharedarraybuffer(env, args[0], &shared) != napi_ok ||
      napi_get_boolean(env, shared, &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value is_arraybuffer(napi_env env, napi_callback_info info) {
  napi_value args[1], result;
  size_t argc = 1;
  bool arraybuffer = false;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok ||
      argc != 1 || napi_is_arraybuffer(env, args[0], &arraybuffer) != napi_ok ||
      napi_get_boolean(env, arraybuffer, &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value external_finalizers(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_create_int32(env, external_finalizer_count, &result) != napi_ok)
    return NULL;
  return result;
}

NAPI_MODULE_INIT() {
  napi_property_descriptor properties[] = {
      { .utf8name = "makeShared", .method = make_shared },
      { .utf8name = "makeExternalShared", .method = make_external_shared },
      { .utf8name = "isShared", .method = is_shared },
      { .utf8name = "isArrayBuffer", .method = is_arraybuffer },
      { .utf8name = "externalFinalizers", .method = external_finalizers },
  };
  if (napi_define_properties(env, exports,
                             sizeof(properties) / sizeof(properties[0]),
                             properties) != napi_ok)
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
            "SharedArrayBuffer Node-API fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );

        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()]).allow_native_addon(&addon),
            )
            .unwrap();
        let guest = interpreter
            .eval_source(
                r#"
const addon = require('./fixture.node');
const shared = addon.makeShared();
const view = new Uint8Array(shared);
const dataView = new DataView(shared);
view[1] = 29;
dataView.setUint8(2, 41);
const words = new Int32Array(shared);
const stored = Atomics.store(words, 1, 20);
const prior = Atomics.add(words, 1, 7);
const compared = Atomics.compareExchange(words, 1, 27, 40);
const exchanged = Atomics.exchange(words, 1, 41);
const atomicLoaded = Atomics.load(words, 1);
const bigWords = new BigInt64Array(shared);
const bigPrior = Atomics.add(bigWords, 0, 2n);
const clone = structuredClone(shared);
new Uint8Array(clone)[3] = 53;
const graphClone = structuredClone({ shared, view });
const slice = shared.slice(1, 3);
new Uint8Array(slice)[0] = 67;
const external = addon.makeExternalShared();
const externalView = new Uint32Array(external);
externalView[1] = 0x76543210;
JSON.stringify({
  byteLength: shared.byteLength,
  isShared: addon.isShared(shared),
  isArrayBuffer: addon.isArrayBuffer(shared),
  view: Array.from(view),
  atomics: [stored, prior, compared, exchanged, atomicLoaded],
  atomicsBigInt: [String(bigPrior), String(Atomics.load(bigWords, 0))],
  isLockFree: [Atomics.isLockFree(1), Atomics.isLockFree(4), Atomics.isLockFree(8)],
  dataViewByte: dataView.getUint8(2),
  cloneIsDistinct: clone !== shared,
  cloneWriteVisible: view[3],
  cloneKeepsBufferAlias: graphClone.shared === graphClone.view.buffer,
  slice: Array.from(new Uint8Array(slice)),
  externalIsShared: addon.isShared(external),
  externalValues: Array.from(externalView),
  finalizersBeforeShutdown: addon.externalFinalizers()
});
"#,
            )
            .unwrap();
        let Value::String(ref guest_json) = guest else {
            panic!("SharedArrayBuffer fixture returned {guest:?}");
        };
        let guest_result: serde_json::Value = serde_json::from_str(guest_json).unwrap();
        let finalizer_library = unsafe {
            Library::open(Some(&addon), RTLD_NOW | RTLD_GLOBAL)
                .expect("retain SharedArrayBuffer fixture for finalizer verification")
        };

        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let runner = r#"
const addon = require('./fixture.node');
const shared = addon.makeShared();
const view = new Uint8Array(shared);
const dataView = new DataView(shared);
view[1] = 29;
dataView.setUint8(2, 41);
const words = new Int32Array(shared);
const stored = Atomics.store(words, 1, 20);
const prior = Atomics.add(words, 1, 7);
const compared = Atomics.compareExchange(words, 1, 27, 40);
const exchanged = Atomics.exchange(words, 1, 41);
const atomicLoaded = Atomics.load(words, 1);
const bigWords = new BigInt64Array(shared);
const bigPrior = Atomics.add(bigWords, 0, 2n);
const clone = structuredClone(shared);
new Uint8Array(clone)[3] = 53;
const graphClone = structuredClone({ shared, view });
const slice = shared.slice(1, 3);
new Uint8Array(slice)[0] = 67;
const external = addon.makeExternalShared();
const externalView = new Uint32Array(external);
externalView[1] = 0x76543210;
process.stdout.write(JSON.stringify({
  byteLength: shared.byteLength,
  isShared: addon.isShared(shared),
  isArrayBuffer: addon.isArrayBuffer(shared),
  view: Array.from(view),
  atomics: [stored, prior, compared, exchanged, atomicLoaded],
  atomicsBigInt: [String(bigPrior), String(Atomics.load(bigWords, 0))],
  isLockFree: [Atomics.isLockFree(1), Atomics.isLockFree(4), Atomics.isLockFree(8)],
  dataViewByte: dataView.getUint8(2),
  cloneIsDistinct: clone !== shared,
  cloneWriteVisible: view[3],
  cloneKeepsBufferAlias: graphClone.shared === graphClone.view.buffer,
  slice: Array.from(new Uint8Array(slice)),
  externalIsShared: addon.isShared(external),
  externalValues: Array.from(externalView),
  finalizersBeforeShutdown: addon.externalFinalizers()
}));
"#;
            let reference = Command::new("node")
                .current_dir(&root)
                .args(["-e", runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Node SharedArrayBuffer reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let node_result: serde_json::Value = serde_json::from_slice(&reference.stdout).unwrap();
            assert_eq!(guest_result, node_result);
        }

        drop(interpreter);
        let finalizer_count = unsafe {
            finalizer_library
                .get::<unsafe extern "C" fn() -> i32>(b"fixture_external_finalizer_count\0")
                .expect("SharedArrayBuffer finalizer counter is exported")()
        };
        assert_eq!(
            finalizer_count, 1,
            "external shared buffer finalizer runs once"
        );
        drop(finalizer_library);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rust_node_api_host_enforces_allowlist_and_digest_when_called_directly() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-addon-policy-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let pinned_addon = root.join("pinned.node");
        let untrusted_addon = root.join("untrusted.node");
        let original = b"configured addon bytes";
        fs::write(&pinned_addon, original).unwrap();
        fs::write(&untrusted_addon, b"untrusted addon bytes").unwrap();
        let digest: [u8; 32] = Sha256::digest(original).into();

        let mut interpreter = Interpreter::with_builtins();
        let host = interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&pinned_addon, digest),
            )
            .unwrap();

        let untrusted_error = NativeAddonLoader::load(host.as_ref(), &untrusted_addon).unwrap_err();
        assert!(untrusted_error.to_string().contains("not allowlisted"));

        fs::write(&pinned_addon, b"changed after host configuration").unwrap();
        let integrity_error = NativeAddonLoader::load(host.as_ref(), &pinned_addon).unwrap_err();
        assert!(
            integrity_error
                .to_string()
                .contains("integrity check failed")
        );

        drop(host);
        drop(interpreter);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rust_node_api_require_classifies_a_non_library_file_before_dlopen() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-invalid-binary-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let addon = root.join("fixture.node");
        fs::write(&addon, b"not a native library").unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest),
            )
            .unwrap();
        let error = interpreter
            .eval_source("require('./fixture.node');")
            .unwrap_err();
        assert!(error.to_string().contains("incompatible native addon"));
        assert!(match std::env::consts::OS {
            "linux" => error.to_string().contains("not an ELF shared library"),
            "macos" => error.to_string().contains("not a Mach-O shared library"),
            _ => false,
        });

        drop(interpreter);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn napi_reference_primitive_lifetime_matches_requested_api_version() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-reference-version-{}-{}",
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
            eprintln!("skipping Node-API reference fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");

        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/node-api/reference-v10.c");
        let runner = "process.stdout.write(JSON.stringify(require('./main.cjs')))";
        let lifetime_runner = "process.stdout.write(JSON.stringify(require('./lifetime.cjs')))";
        let mut node_reports = Vec::new();
        for api_version in [9, 10] {
            let version_root = root.join(format!("v{api_version}"));
            fs::create_dir_all(&version_root).unwrap();
            let addon = version_root.join("fixture.node");
            let built = Command::new("cc")
                .args([
                    "-std=c11",
                    "-O2",
                    "-fPIC",
                    "-shared",
                    &format!("-DNAPI_VERSION={api_version}"),
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
                "Node-API v{api_version} reference fixture compilation failed: {}",
                String::from_utf8_lossy(&built.stderr)
            );
            fs::write(
                version_root.join("main.cjs"),
                "module.exports = require('./fixture.node').referenceProbe();\n",
            )
            .unwrap();
            fs::write(
                version_root.join("lifetime.cjs"),
                r#"
const addon = require('./fixture.node');
function makeWeakTargetUnreachable() {
  const target = {};
  addon.createWeakReference(target);
}
makeWeakTargetUnreachable();
if (typeof globalThis.gc === 'function') {
  for (let i = 0; i < 8; i++) {
    globalThis.gc();
    const pressure = new Array(100000).fill({});
  }
}
const collected = addon.weakReferenceIsNull();
const refStatus = addon.weakReferenceRefStatus();
const remainsCollected = addon.weakReferenceIsNull();
addon.deleteWeakReference();
module.exports = {collected, refStatus, remainsCollected};
"#,
            )
            .unwrap();

            let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();
            let mut interpreter = Interpreter::with_builtins();
            interpreter
                .enable_rust_node_api_addons(
                    RustNodeApiOptions::new([version_root.clone()])
                        .allow_native_addon_with_sha256(&addon, digest)
                        .entry(version_root.join("main.cjs")),
                )
                .unwrap();
            let vm_value = interpreter
                .eval_source("JSON.stringify(require('./main.cjs'));")
                .unwrap();
            let Value::String(ref vm_json) = vm_value else {
                panic!("Node-API v{api_version} fixture did not return JSON text");
            };
            let vm_report: serde_json::Value = serde_json::from_str(vm_json).unwrap();
            let vm_lifetime_value = interpreter
                .eval_source("JSON.stringify(require('./lifetime.cjs'));")
                .unwrap();
            let Value::String(ref vm_lifetime_json) = vm_lifetime_value else {
                panic!("Node-API v{api_version} lifetime fixture did not return JSON text");
            };
            let vm_lifetime: serde_json::Value = serde_json::from_str(vm_lifetime_json).unwrap();

            let node = Command::new("node")
                .current_dir(&version_root)
                .args(["-e", runner])
                .output()
                .unwrap_or_else(|error| {
                    panic!("Node is required for N-API differential tests: {error}")
                });
            assert!(
                node.status.success(),
                "Node-API v{api_version} Node reference failed: {}",
                String::from_utf8_lossy(&node.stderr)
            );
            let node_report: serde_json::Value = serde_json::from_slice(&node.stdout).unwrap();
            assert_eq!(
                vm_report, node_report,
                "Node-API v{api_version} primitive reference behavior differs from Node"
            );
            let node_lifetime = Command::new("node")
                .current_dir(&version_root)
                .args(["--expose-gc", "-e", lifetime_runner])
                .output()
                .unwrap_or_else(|error| {
                    panic!("Node with --expose-gc is required for weak refs: {error}")
                });
            assert!(
                node_lifetime.status.success(),
                "Node-API v{api_version} weak-reference Node fixture failed: {}",
                String::from_utf8_lossy(&node_lifetime.stderr)
            );
            let node_lifetime: serde_json::Value =
                serde_json::from_slice(&node_lifetime.stdout).unwrap();
            assert_eq!(
                vm_lifetime, node_lifetime,
                "Node-API v{api_version} weak object references differ from Node after GC"
            );
            assert_eq!(vm_lifetime["collected"], true);
            assert_eq!(vm_lifetime["refStatus"], NAPI_OK);
            assert_eq!(vm_lifetime["remainsCollected"], true);
            if api_version == 9 {
                assert_eq!(vm_report["createStrongStatus"], NAPI_INVALID_ARG);
                assert_eq!(vm_report["createZeroStatus"], NAPI_INVALID_ARG);
            } else {
                assert_eq!(vm_report["createStrongStatus"], NAPI_OK);
                assert_eq!(vm_report["initialValuePresent"], true);
                assert_eq!(vm_report["unrefStatus"], NAPI_OK);
                assert_eq!(vm_report["countAfterUnref"], 0);
                assert_eq!(vm_report["releasedValueStatus"], NAPI_OK);
                assert_eq!(vm_report["releasedValueIsNull"], true);
                assert_eq!(vm_report["refAfterReleaseStatus"], NAPI_OK);
                assert_eq!(vm_report["valueAfterReleaseRefStatus"], NAPI_OK);
                assert_eq!(vm_report["valueAfterReleaseRefIsNull"], true);
                assert_eq!(vm_report["createZeroStatus"], NAPI_OK);
                assert_eq!(vm_report["zeroValueStatus"], NAPI_OK);
                assert_eq!(vm_report["zeroValueIsNull"], true);
            }
            node_reports.push((api_version, node_report));
            drop(interpreter);
        }

        if Command::new("bun")
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
        {
            for (api_version, node_report) in &node_reports {
                let version_root = root.join(format!("v{api_version}"));
                let bun = Command::new("bun")
                    .current_dir(&version_root)
                    .args(["-e", runner])
                    .output()
                    .unwrap();
                assert!(
                    bun.status.success(),
                    "Node-API v{api_version} Bun reference failed: {}",
                    String::from_utf8_lossy(&bun.stderr)
                );
                let bun_report: serde_json::Value = serde_json::from_slice(&bun.stdout).unwrap();
                if bun_report != *node_report {
                    assert_eq!(*api_version, 10);
                    assert_eq!(
                        bun_report,
                        serde_json::json!({
                            "createStrongStatus": NAPI_INVALID_ARG,
                            "initialValueStatus": NAPI_GENERIC_FAILURE,
                            "unrefStatus": NAPI_GENERIC_FAILURE,
                            "countAfterUnref": 0,
                            "releasedValueStatus": NAPI_GENERIC_FAILURE,
                            "refAfterReleaseStatus": NAPI_GENERIC_FAILURE,
                            "valueAfterReleaseRefStatus": NAPI_GENERIC_FAILURE,
                            "createZeroStatus": NAPI_INVALID_ARG,
                            "zeroValueStatus": NAPI_GENERIC_FAILURE,
                            "initialValuePresent": false,
                            "releasedValueIsNull": false,
                            "valueAfterReleaseRefIsNull": false,
                            "zeroValueIsNull": false,
                        }),
                        "unexpected Bun / Node difference in primitive references"
                    );
                    eprintln!(
                        "Bun currently rejects v10 primitive napi_ref creation; Node is the v10 conformance reference"
                    );
                }
            }
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn loads_node_addon_api_cpp_fixture_with_shared_runtime_source() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-node-addon-api-cpp-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let compiler = Command::new("c++").arg("--version").output();
        let addon_api_dirs = [
            std::env::var_os("NODE_ADDON_API_DIR").map(PathBuf::from),
            std::env::current_dir()
                .ok()
                .map(|path| path.join("node_modules/node-addon-api")),
            Some(PathBuf::from("/usr/include/node-addon-api")),
            Some(PathBuf::from("/usr/local/include/node-addon-api")),
        ];
        let addon_api_include = addon_api_dirs
            .into_iter()
            .flatten()
            .find(|path| path.join("napi.h").is_file());
        let node_include_dirs = [
            std::env::var_os("NODE_INCLUDE_DIR").map(PathBuf::from),
            Some(PathBuf::from("/usr/include/node")),
            Some(PathBuf::from("/usr/local/include/node")),
        ];
        let node_include = node_include_dirs
            .into_iter()
            .flatten()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(compiler), Some(addon_api_include), Some(node_include)) =
            (compiler, addon_api_include, node_include)
        else {
            eprintln!(
                "skipping node-addon-api fixture: c++, Node headers, or node-addon-api headers are unavailable"
            );
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "c++ --version failed");

        let source = root.join("fixture.cc");
        let addon = root.join("fixture.node");
        fs::write(
            &source,
            r#"
#define NAPI_VERSION 8
#include <napi.h>
#include <string>
#include <utility>

class Counter : public Napi::ObjectWrap<Counter> {
 public:
  static Napi::Function Init(Napi::Env env, Napi::Object exports) {
    Napi::Function constructor = DefineClass(
        env, "Counter",
        {InstanceMethod("increment", &Counter::Increment),
         InstanceAccessor("value", &Counter::GetValue, &Counter::SetValue)});
    exports.Set("Counter", constructor);
    return constructor;
  }

  explicit Counter(const Napi::CallbackInfo& info)
      : Napi::ObjectWrap<Counter>(info),
        value_(info.Length() > 0 ? info[0].As<Napi::Number>().Int32Value() : 0) {}

 private:
  Napi::Value Increment(const Napi::CallbackInfo& info) {
    ++value_;
    return Napi::Number::New(info.Env(), value_);
  }

  Napi::Value GetValue(const Napi::CallbackInfo& info) {
    return Napi::Number::New(info.Env(), value_);
  }

  void SetValue(const Napi::CallbackInfo& info, const Napi::Value& value) {
    value_ = value.As<Napi::Number>().Int32Value();
  }

  int32_t value_;
};

class EchoWorker : public Napi::AsyncWorker {
 public:
  EchoWorker(Napi::Function callback, std::string value)
      : Napi::AsyncWorker(callback), value_(std::move(value)) {}

  void Execute() override { result_ = value_; }

  void OnOK() override {
    Callback().Call({Env().Undefined(), Napi::String::New(Env(), result_)});
  }

 private:
  std::string value_;
  std::string result_;
};

Napi::Value AsyncEcho(const Napi::CallbackInfo& info) {
  auto worker = new EchoWorker(info[0].As<Napi::Function>(), "async-cpp");
  worker->Queue();
  return info.Env().Undefined();
}

Napi::Object Init(Napi::Env env, Napi::Object exports) {
  Counter::Init(env, exports);
  exports.Set("asyncEcho", Napi::Function::New(env, AsyncEcho));
  return exports;
}

NODE_API_MODULE(napi_vm_node_addon_api_fixture, Init)
"#,
        )
        .unwrap();
        let built = Command::new("c++")
            .args([
                "-std=c++17",
                "-O2",
                "-fPIC",
                "-shared",
                "-DNAPI_VERSION=8",
                "-I",
            ])
            .arg(&node_include)
            .arg("-I")
            .arg(&addon_api_include)
            .arg(&source)
            .arg("-o")
            .arg(&addon)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "node-addon-api fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );

        let main = root.join("main.cjs");
        fs::write(
            &main,
            "const { Counter, asyncEcho } = require('./fixture.node');\nconst counter = new Counter(4);\nconst before = counter.value;\ncounter.value = 10;\nglobalThis.asyncEchoCallbackCalled = false;\nmodule.exports = new Promise((resolve, reject) => {\n  asyncEcho((error, echoed) => {\n    globalThis.asyncEchoCallbackCalled = true;\n    if (error) return reject(error);\n    resolve({ before, incremented: counter.increment(), value: counter.value, echoed, asyncEchoName: asyncEcho.name });\n  });\n});\n",
        )
        .unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest)
                    .entry(&main),
            )
            .unwrap();
        let result = interpreter
            .eval_source("let cxxResult = await require('./main.cjs'); JSON.stringify({result: cxxResult, callbackCalled: globalThis.asyncEchoCallbackCalled});")
            .unwrap();
        let Value::String(vm_json) = &result else {
            panic!("C++ addon fixture did not return JSON text: {result:?}");
        };
        let vm_result: serde_json::Value = serde_json::from_str(vm_json).unwrap();
        assert_eq!(
            vm_result,
            serde_json::json!({"result": {"before": 4, "incremented": 11, "value": 11, "echoed": "async-cpp", "asyncEchoName": ""}, "callbackCalled": true})
        );

        let runner = "(async () => process.stdout.write(JSON.stringify({result: await require('./main.cjs'), callbackCalled: globalThis.asyncEchoCallbackCalled})))().catch(error => { console.error(error); process.exitCode = 1; })";
        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let reference = Command::new("node")
                .current_dir(&root)
                .args(["-e", runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Node C++ addon reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let node_result: serde_json::Value = serde_json::from_slice(&reference.stdout).unwrap();
            assert_eq!(
                vm_result, node_result,
                "Node and napi-vm C++ results differ"
            );
        }
        if let Ok(bun_version) = Command::new("bun").arg("--version").output()
            && bun_version.status.success()
        {
            let reference = Command::new("bun")
                .current_dir(&root)
                .args(["-e", runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Bun C++ addon reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let bun_result: serde_json::Value = serde_json::from_slice(&reference.stdout).unwrap();
            assert_eq!(vm_result, bun_result, "Bun and napi-vm C++ results differ");
        }
        drop(interpreter);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[test]
    fn loads_napi_rs_addon_with_the_same_commonjs_entry_on_node_bun_and_vm() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-napi-rs-fixture-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/node-api/napi-rs/Cargo.toml");
        let target_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/node-api-fixtures/napi-rs");
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
            "napi-rs fixture build failed: {}",
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
        assert!(
            compiled_addon.is_file(),
            "napi-rs fixture was not produced at {}",
            compiled_addon.display()
        );
        let addon = root.join("fixture.node");
        fs::copy(&compiled_addon, &addon).unwrap();
        let guest_entry = include_str!("../../../../tests/fixtures/node-api/napi-rs-main.cjs");
        fs::write(root.join("main.cjs"), guest_entry).unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest)
                    .entry(root.join("main.cjs")),
            )
            .unwrap();
        let result = interpreter
            .eval_source("JSON.stringify(await require('./main.cjs'));")
            .unwrap();
        let Value::String(vm_json) = &result else {
            panic!("napi-rs fixture did not return JSON text: {result:?}");
        };
        let vm_result: serde_json::Value = serde_json::from_str(vm_json).unwrap();
        assert_eq!(
            vm_result,
            serde_json::json!({
                "sum": 42,
                "text": "rust-napi",
                "counter": { "initial": 40, "incremented": 41, "value": 41 },
                "bytes": [4, 3, 2, 1],
                "profile": { "value": { "name": "Ada", "scores": [30, 37, 1], "active": true } },
                "sumValues": { "value": 6 },
                "optionalValues": [
                    { "value": "label:ready" },
                    { "value": null },
                    { "value": null }
                ],
                "callback": { "value": "HELLO" },
                "json": { "value": { "nested": [1, "two", null], "enabled": true } },
                "enumValues": [
                    { "value": "Fast" },
                    { "value": "Safe" },
                    { "error": "value `\"Unknown\"` does not match any variant of enum `PluginMode`", "name": "Error", "code": "InvalidArg" }
                ],
                "bigints": [
                    { "value": "123456789012345678901234567890" },
                    { "value": "-98765432109876543210987654321" }
                ],
                "typedArray": { "value": [255, 2, 1] },
                "threadsafe": { "status": 0, "value": "from-napi-rs-tsfn" },
                "failure": { "name": "Error", "message": "fixture failure" },
                "asyncSum": 42
            }),
            "napi-rs conversion fixture returned an unexpected result"
        );

        let runner = "(async () => { process.stdout.write(JSON.stringify(await require('./main.cjs'))); })().catch(error => { console.error(error); process.exitCode = 1; });";
        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let reference = Command::new("node")
                .current_dir(&root)
                .args(["-e", runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Node napi-rs fixture failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let node_result: serde_json::Value =
                serde_json::from_slice(&reference.stdout).unwrap();
            assert_eq!(vm_result, node_result, "Node and napi-vm differ for napi-rs");
        } else {
            eprintln!("skipping Node napi-rs differential: Node.js is unavailable");
        }

        if let Ok(bun_version) = Command::new("bun").arg("--version").output()
            && bun_version.status.success()
        {
            let reference = Command::new("bun")
                .current_dir(&root)
                .args(["-e", runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Bun napi-rs fixture failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let bun_result: serde_json::Value =
                serde_json::from_slice(&reference.stdout).unwrap();
            assert_eq!(vm_result, bun_result, "Bun and napi-vm differ for napi-rs");
        }

        drop(interpreter);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn experimental_node_api_object_and_finalizer_apis_match_reference_runtimes() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-node-api-set-prototype-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let compiler = Command::new("cc").arg("--version").output();
        let node_include_dirs = [
            std::env::var_os("NODE_INCLUDE_DIR").map(PathBuf::from),
            Some(PathBuf::from("/usr/include/node")),
            Some(PathBuf::from("/usr/local/include/node")),
        ];
        let node_include = node_include_dirs
            .into_iter()
            .flatten()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(compiler), Some(node_include)) = (compiler, node_include) else {
            eprintln!("skipping experimental Node-API fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");
        let experimental_headers = ["node_api.h", "js_native_api.h"]
            .iter()
            .filter_map(|name| fs::read_to_string(node_include.join(name)).ok())
            .collect::<String>();
        if !experimental_headers.contains("node_api_set_prototype")
            || !experimental_headers.contains("node_api_create_object_with_properties")
            || !experimental_headers.contains("node_api_post_finalizer")
        {
            eprintln!(
                "skipping experimental Node-API fixture: installed Node headers lack required experimental APIs"
            );
            let _ = fs::remove_dir_all(&root);
            return;
        }

        let source = root.join("fixture.c");
        let addon = root.join("fixture.node");
        fs::write(
            &source,
            r#"
#define NAPI_EXPERIMENTAL
#define NAPI_VERSION 10
#include <node_api.h>

static napi_value set_prototype_probe(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value args[2], result;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc != 2)
    return NULL;
  napi_status status = node_api_set_prototype(env, args[0], args[1]);
  if (napi_create_int32(env, status, &result) != napi_ok) return NULL;
  return result;
}

static napi_value get_prototype_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value object, prototype;
  if (napi_get_cb_info(env, info, &argc, &object, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_prototype(env, object, &prototype) != napi_ok)
    return NULL;
  return prototype;
}

static napi_value default_cycle_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value args[1], prototype, result;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_prototype(env, args[0], &prototype) != napi_ok)
    return NULL;
  napi_status status = node_api_set_prototype(env, prototype, args[0]);
  if (napi_create_int32(env, status, &result) != napi_ok) return NULL;
  return result;
}

static int posted_finalizer_calls;
static int posted_finalizer_api_status = -1;
static int posted_finalizer_post_count;
static int posted_finalizer_values[16];
static int posted_finalizer_value;

static void posted_finalizer(napi_env env, void* data, void* hint) {
  (void)hint;
  napi_value global, value;
  int finalized_value = *(int*)data;
  posted_finalizer_calls++;
  posted_finalizer_value = finalized_value;
  if (finalized_value == 42) return;
  posted_finalizer_api_status = napi_get_global(env, &global);
  if (posted_finalizer_api_status == napi_ok) {
    posted_finalizer_api_status = napi_create_int32(env, finalized_value, &value);
  }
  if (posted_finalizer_api_status == napi_ok) {
    posted_finalizer_api_status = napi_set_named_property(
        env, global, "postedFinalizerValue", value);
  }
}

static napi_value post_finalizer_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value args[1];
  napi_value result;
  int32_t data;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_value_int32(env, args[0], &data) != napi_ok ||
      posted_finalizer_post_count >= 16)
    return NULL;
  int* finalizer_data = &posted_finalizer_values[posted_finalizer_post_count++];
  *finalizer_data = data;
  napi_status status = node_api_post_finalizer(
      env, posted_finalizer, finalizer_data, NULL);
  if (napi_create_int32(env, status, &result) != napi_ok) return NULL;
  return result;
}

static napi_value posted_finalizer_calls_probe(napi_env env, napi_callback_info info) {
  napi_value result;
  if (napi_create_int32(env, posted_finalizer_calls, &result) != napi_ok) return NULL;
  return result;
}

static napi_value posted_finalizer_status_probe(napi_env env, napi_callback_info info) {
  napi_value result;
  if (napi_create_int32(env, posted_finalizer_api_status, &result) != napi_ok) return NULL;
  return result;
}

int napi_vm_test_posted_finalizer_calls(void) {
  return posted_finalizer_calls;
}

int napi_vm_test_posted_finalizer_value(void) {
  return posted_finalizer_value;
}

static napi_value create_object_with_properties_probe(
    napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value args[1], names[3], values[3], object, result;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc > 1 ||
      napi_create_string_utf8(env, "label", NAPI_AUTO_LENGTH, &names[0]) != napi_ok ||
      napi_create_string_utf8(env, "value", NAPI_AUTO_LENGTH, &names[1]) != napi_ok ||
      napi_create_symbol(env, NULL, &names[2]) != napi_ok ||
      napi_create_string_utf8(env, "native", NAPI_AUTO_LENGTH, &values[0]) != napi_ok ||
      napi_create_int32(env, 17, &values[1]) != napi_ok ||
      napi_create_string_utf8(env, "symbol-value", NAPI_AUTO_LENGTH, &values[2]) != napi_ok ||
      node_api_create_object_with_properties(
          env, argc == 0 ? NULL : args[0], names, values, 3, &object) != napi_ok ||
      napi_create_array(env, &result) != napi_ok ||
      napi_set_element(env, result, 0, object) != napi_ok ||
      napi_set_element(env, result, 1, names[2]) != napi_ok)
    return NULL;
  return result;
}

NAPI_MODULE_INIT() {
  napi_value function, get_function, cycle_function, create_function, post_function;
  napi_value finalizer_calls_function, finalizer_status_function;
  if (napi_create_function(env, "setPrototype", NAPI_AUTO_LENGTH,
                           set_prototype_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "setPrototype", function) != napi_ok ||
      napi_create_function(env, "getPrototype", NAPI_AUTO_LENGTH,
                           get_prototype_probe, NULL, &get_function) != napi_ok ||
      napi_set_named_property(env, exports, "getPrototype", get_function) != napi_ok ||
      napi_create_function(env, "defaultCycle", NAPI_AUTO_LENGTH,
                           default_cycle_probe, NULL, &cycle_function) != napi_ok ||
      napi_set_named_property(env, exports, "defaultCycle", cycle_function) != napi_ok ||
      napi_create_function(env, "createObject", NAPI_AUTO_LENGTH,
                           create_object_with_properties_probe, NULL,
                           &create_function) != napi_ok ||
      napi_set_named_property(env, exports, "createObject", create_function) != napi_ok ||
      napi_create_function(env, "postFinalizer", NAPI_AUTO_LENGTH,
                           post_finalizer_probe, NULL, &post_function) != napi_ok ||
      napi_set_named_property(env, exports, "postFinalizer", post_function) != napi_ok ||
      napi_create_function(env, "postedFinalizerCalls", NAPI_AUTO_LENGTH,
                           posted_finalizer_calls_probe, NULL,
                           &finalizer_calls_function) != napi_ok ||
      napi_set_named_property(env, exports, "postedFinalizerCalls",
                              finalizer_calls_function) != napi_ok ||
      napi_create_function(env, "postedFinalizerStatus", NAPI_AUTO_LENGTH,
                           posted_finalizer_status_probe, NULL,
                           &finalizer_status_function) != napi_ok ||
      napi_set_named_property(env, exports, "postedFinalizerStatus",
                              finalizer_status_function) != napi_ok)
    return NULL;
  return exports;
}
"#,
        )
        .unwrap();
        let built = Command::new("cc")
            .args([
                "-std=c11",
                "-O2",
                "-fPIC",
                "-shared",
                "-DNAPI_EXPERIMENTAL",
                "-DNAPI_VERSION=10",
                "-I",
            ])
            .arg(&node_include)
            .arg(&source)
            .arg("-o")
            .arg(&addon)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "experimental Node-API fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );

        let main = root.join("main.cjs");
        fs::write(
            &main,
            r#"const addon = require('./fixture.node');
const prototype = { marker: 'inherited', twice() { return this.value * 2; } };
const target = { value: 21 };
const status = addon.setPrototype(target, prototype);
const arrayTarget = [7];
const arrayStatus = addon.setPrototype(arrayTarget, prototype);
function FunctionTarget() {}
function FunctionParent() {}
FunctionParent.marker = 'function-inherited';
const functionStatus = addon.setPrototype(FunctionTarget, FunctionParent);
const cycleStatus = addon.setPrototype(target, target);
const defaultCycleStatus = addon.defaultCycle({});
const nullTarget = {};
const nullStatus = addon.setPrototype(nullTarget, null);
Object.freeze(target);
const frozenSameStatus = addon.setPrototype(target, prototype);
const frozenChangeStatus = addon.setPrototype(target, null);
const [created, symbolKey] = addon.createObject(prototype);
const [functionCreated] = addon.createObject(FunctionParent);
const [nullCreated] = addon.createObject(null);
const [implicitlyNullCreated] = addon.createObject();
const finalizerStatus = addon.postFinalizer(23);
const postedFinalizerCallsImmediately = addon.postedFinalizerCalls();
module.exports = {
  run: () => JSON.stringify({
  status,
  arrayStatus,
  arrayPrototypeSame: addon.getPrototype(arrayTarget) === prototype,
  arrayInherited: arrayTarget.marker,
  arrayElementPreserved: arrayTarget[0] === 7,
  arrayMapRemoved: arrayTarget.map === undefined,
  functionStatus,
  functionPrototypeSame: Object.getPrototypeOf(FunctionTarget) === FunctionParent,
  functionNapiPrototypeSame: addon.getPrototype(FunctionTarget) === FunctionParent,
  functionInherited: FunctionTarget.marker,
  cycleStatus,
  defaultCycleStatus,
  nullStatus,
  nullPrototype: Object.getPrototypeOf(nullTarget) === null,
  frozenSameStatus,
  frozenChangeStatus,
  samePrototype: Object.getPrototypeOf(target) === prototype,
  marker: target.marker,
  twice: target.twice(),
  createdPrototype: Object.getPrototypeOf(created) === prototype,
  createdLabel: created.label,
  createdValue: created.value,
  createdSymbol: created[symbolKey],
  inherited: created.marker,
  functionCreatedPrototype: Object.getPrototypeOf(functionCreated) === FunctionParent,
  functionCreatedInherited: functionCreated.marker,
  nullCreatedPrototype: Object.getPrototypeOf(nullCreated) === null,
  implicitlyNullCreatedPrototype: Object.getPrototypeOf(implicitlyNullCreated) === null,
  finalizerStatus,
  postedFinalizerCallsImmediately,
  postedFinalizerCalls: addon.postedFinalizerCalls(),
  postedFinalizerApiStatus: addon.postedFinalizerStatus(),
  postedFinalizerValue: globalThis.postedFinalizerValue
  }),
  postAgain: () => addon.postFinalizer(42)
};
"#,
        )
        .unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut interpreter = Interpreter::with_builtins();
        let host = interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest)
                    .entry(&main),
            )
            .unwrap();
        let host_weak = Rc::downgrade(&host);
        drop(host);
        let loaded_main = interpreter
            .run_script_source("require('./main.cjs');")
            .unwrap();
        assert!(matches!(loaded_main, Value::Object { .. }));
        let observer = unsafe { Library::open(Some(addon.as_os_str()), RTLD_NOW) }.unwrap();
        let posted_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_posted_finalizer_calls\0")
                .unwrap()
        };
        let posted_finalizer_value: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_posted_finalizer_value\0")
                .unwrap()
        };
        let pre_event = interpreter
            .run_script_source("require('./main.cjs').run();")
            .unwrap();
        let Value::String(pre_event_json) = &pre_event else {
            panic!(
                "experimental Node-API pre-event fixture did not return JSON text: {pre_event:?}"
            );
        };
        let pre_event_result: serde_json::Value = serde_json::from_str(pre_event_json).unwrap();
        assert_eq!(pre_event_result["postedFinalizerCallsImmediately"], 0);
        assert!(interpreter.run_event_loop_once(Duration::ZERO).unwrap());
        let result = interpreter
            .run_script_source("require('./main.cjs').run();")
            .unwrap();
        let Value::String(vm_json) = &result else {
            panic!("experimental Node-API fixture did not return JSON text: {result:?}");
        };
        let vm_result: serde_json::Value = serde_json::from_str(vm_json).unwrap();
        assert_eq!(
            vm_result,
            serde_json::json!({
                "status": 0,
                "arrayStatus": 0,
                "arrayPrototypeSame": true,
                "arrayInherited": "inherited",
                "arrayElementPreserved": true,
                "arrayMapRemoved": true,
                "functionStatus": 0,
                "functionPrototypeSame": true,
                "functionNapiPrototypeSame": true,
                "functionInherited": "function-inherited",
                "cycleStatus": 9,
                "defaultCycleStatus": 9,
                "nullStatus": 0,
                "nullPrototype": true,
                "frozenSameStatus": 0,
                "frozenChangeStatus": 9,
                "samePrototype": true,
                "marker": "inherited",
                "twice": 42,
                "createdPrototype": true,
                "createdLabel": "native",
                "createdValue": 17,
                "createdSymbol": "symbol-value",
                "inherited": "inherited",
                "functionCreatedPrototype": true,
                "functionCreatedInherited": "function-inherited",
                "nullCreatedPrototype": true,
                "implicitlyNullCreatedPrototype": true,
                "finalizerStatus": 0,
                "postedFinalizerCallsImmediately": 0,
                "postedFinalizerCalls": 1,
                "postedFinalizerApiStatus": 0,
                "postedFinalizerValue": 23
            })
        );

        let runner = "const main = require('./main.cjs'); const deadline = Date.now() + 1000; function poll() { const result = JSON.parse(main.run()); if (result.postedFinalizerCalls === 1) { process.stdout.write(JSON.stringify(result)); return; } if (Date.now() >= deadline) { process.stderr.write('posted finalizer did not run\\n'); process.exitCode = 1; return; } setTimeout(poll, 1); } poll();";
        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let reference = Command::new("node")
                .current_dir(&root)
                .args(["-e", runner])
                .output()
                .unwrap();
            if reference.status.success() {
                let node_result: serde_json::Value =
                    serde_json::from_slice(&reference.stdout).unwrap();
                assert_eq!(vm_result, node_result, "Node and napi-vm results differ");
            } else {
                let stderr = String::from_utf8_lossy(&reference.stderr);
                assert!(
                    stderr.contains("node_api_set_prototype")
                        || stderr.contains("node_api_create_object_with_properties")
                        || stderr.contains("node_api_post_finalizer"),
                    "Node experimental Node-API fixture failed for an unexpected reason: {stderr}"
                );
                eprintln!(
                    "Node runtime lacks the experimental APIs; skipped this reference comparison"
                );
            }
        }
        if let Ok(bun_version) = Command::new("bun").arg("--version").output()
            && bun_version.status.success()
        {
            let reference = Command::new("bun")
                .current_dir(&root)
                .args(["-e", runner])
                .output()
                .unwrap();
            if reference.status.success() {
                let bun_result: serde_json::Value =
                    serde_json::from_slice(&reference.stdout).unwrap();
                assert_eq!(vm_result, bun_result, "Bun and napi-vm results differ");
            } else {
                let stderr = String::from_utf8_lossy(&reference.stderr);
                assert!(
                    stderr.contains("node_api_set_prototype")
                        || stderr.contains("node_api_create_object_with_properties")
                        || stderr.contains("node_api_post_finalizer"),
                    "Bun experimental Node-API fixture failed for an unexpected reason: {stderr}"
                );
                eprintln!(
                    "Bun does not export these experimental APIs; skipped this reference comparison"
                );
            }
        }

        let Value::Number(post_status) = interpreter
            .run_script_source("require('./main.cjs').postAgain();")
            .unwrap()
        else {
            panic!("shutdown finalizer scheduling did not return a status");
        };
        assert_eq!(post_status, 0.0);
        drop(interpreter.commonjs_loader.take());
        drop(interpreter.host.take());
        assert!(host_weak.upgrade().is_none(), "native host was not dropped");
        let Value::Number(shutdown_finalizer_value) = interpreter
            .run_script_source("globalThis.postedFinalizerValue")
            .unwrap()
        else {
            panic!("shutdown finalizer did not update the guest global");
        };
        assert_eq!(shutdown_finalizer_value, 23.0);
        assert_eq!(unsafe { posted_finalizer_calls() }, 2);
        assert_eq!(unsafe { posted_finalizer_value() }, 42);
        drop(observer);
        drop(interpreter);
        fs::remove_dir_all(root).unwrap();
    }
