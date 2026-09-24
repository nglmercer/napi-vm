use super::*;
use std::collections::HashMap;

#[derive(Default)]
struct MemoryLoader(HashMap<String, ResolvedCommonJsModule>);

impl MemoryLoader {
    fn module(
        id: &str,
        format: CommonJsModuleFormat,
        source: Option<&str>,
    ) -> ResolvedCommonJsModule {
        ResolvedCommonJsModule {
            id: id.to_string(),
            filename: id.to_string(),
            format,
            source: source.map(str::to_string),
        }
    }
}

fn normalize(path: &Path) -> String {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized.to_string_lossy().into_owned()
}

impl CommonJsModuleLoader for MemoryLoader {
    fn resolve(
        &self,
        request: &str,
        parent: Option<&str>,
    ) -> Result<ResolvedCommonJsModule, VmErr> {
        let requested = if request.starts_with("./") || request.starts_with("../") {
            normalize(
                &parent
                    .and_then(|parent| Path::new(parent).parent())
                    .unwrap_or(Path::new("/virtual"))
                    .join(request),
            )
        } else {
            request.to_string()
        };
        let resolved = if self.0.contains_key(&requested) {
            Some(requested)
        } else if Path::new(&requested).extension().is_none() {
            ["js", "cjs", "json", "node"]
                .into_iter()
                .map(|extension| format!("{requested}.{extension}"))
                .find(|candidate| self.0.contains_key(candidate))
        } else {
            None
        };
        resolved
            .and_then(|id| self.0.get(&id).cloned())
            .ok_or_else(|| VmErr::Msg(format!("Cannot find module '{request}'")))
    }
}

fn interpreter(loader: MemoryLoader) -> crate::interpreter::Interpreter {
    let mut interp = crate::interpreter::Interpreter::with_builtins();
    interp.set_commonjs_loader(Rc::new(loader)).unwrap();
    interp.set_commonjs_entry("/virtual/main.cjs");
    interp
}

#[test]
fn require_executes_and_caches_commonjs_source() {
    let mut loader = MemoryLoader::default();
    loader.0.insert(
            "/virtual/value.cjs".into(),
            MemoryLoader::module(
                "/virtual/value.cjs",
                CommonJsModuleFormat::JavaScript,
                Some("globalThis.requireLoads = (globalThis.requireLoads || 0) + 1; module.exports = {value: globalThis.requireLoads};"),
            ),
        );
    let result = interpreter(loader)
            .eval_source("const first = require('./value.cjs'); const second = require('./value.cjs'); ({same: first === second, value: first.value, requireType: typeof require});")
            .unwrap();
    assert!(matches!(result.get_prop("same"), Some(Value::Bool(true))));
    assert!(matches!(result.get_prop("value"), Some(Value::Number(1.0))));
    assert!(matches!(
        result.get_prop("requireType"),
        Some(Value::String(ref kind)) if kind == "function"
    ));
}

#[test]
fn require_resolve_uses_the_configured_loader_without_evaluating_modules() {
    let mut loader = MemoryLoader::default();
    loader.0.insert(
            "/virtual/side-effect.cjs".into(),
            MemoryLoader::module(
                "/virtual/side-effect.cjs",
                CommonJsModuleFormat::JavaScript,
                Some("globalThis.resolveLoads = (globalThis.resolveLoads || 0) + 1; module.exports = 'loaded';"),
            ),
        );
    loader.0.insert(
        "/virtual/native-addon.node".into(),
        MemoryLoader::module(
            "/virtual/native-addon.node",
            CommonJsModuleFormat::NativeAddon,
            None,
        ),
    );

    let result = interpreter(loader)
            .eval_source(
                "const sourcePath = require.resolve('./side-effect'); const addonPath = require.resolve('./native-addon'); const before = globalThis.resolveLoads || 0; const loaded = require('./side-effect'); ({sourcePath, addonPath, before, after: globalThis.resolveLoads, loaded});",
            )
            .unwrap();

    assert!(matches!(
        result.get_prop("sourcePath"),
        Some(Value::String(ref path)) if path == "/virtual/side-effect.cjs"
    ));
    assert!(matches!(
        result.get_prop("addonPath"),
        Some(Value::String(ref path)) if path == "/virtual/native-addon.node"
    ));
    assert!(matches!(
        result.get_prop("before"),
        Some(Value::Number(0.0))
    ));
    assert!(matches!(result.get_prop("after"), Some(Value::Number(1.0))));
    assert!(matches!(
        result.get_prop("loaded"),
        Some(Value::String(ref value)) if value == "loaded"
    ));
}

#[test]
fn require_resolve_native_path_matches_node_and_bun() {
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-require-resolve-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("main.cjs"), "").unwrap();
    fs::write(
        root.join("side-effect.js"),
        "globalThis.resolveSideEffect = true; module.exports = true;",
    )
    .unwrap();
    fs::write(root.join("fixture.node"), "not loaded by resolve").unwrap();
    fs::write(
            root.join("probe.cjs"),
            "module.exports = { source: require.resolve('./side-effect'), addon: require.resolve('./fixture'), sideEffect: typeof globalThis.resolveSideEffect };",
        )
        .unwrap();

    let mut interpreter = crate::interpreter::Interpreter::with_builtins();
    interpreter
        .set_commonjs_loader(Rc::new(FileCommonJsLoader::new([&root]).unwrap()))
        .unwrap();
    interpreter.set_commonjs_entry(root.join("main.cjs").to_string_lossy());
    let vm_value = interpreter
        .eval_source("JSON.stringify(require('./probe.cjs'));")
        .unwrap();
    let Value::String(vm_json) = &vm_value else {
        panic!("require.resolve fixture did not return JSON: {vm_value:?}");
    };
    let vm_result: serde_json::Value = serde_json::from_str(vm_json).unwrap();

    let runner = "process.stdout.write(JSON.stringify(require('./probe.cjs')))";
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
            "{runtime} require.resolve reference failed: {}",
            String::from_utf8_lossy(&reference.stderr)
        );
        let reference: serde_json::Value = serde_json::from_slice(&reference.stdout).unwrap();
        assert_eq!(vm_result, reference, "{runtime} and napi-vm differ");
    }

    drop(interpreter);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn require_executes_functions_and_handles_circular_partial_exports() {
    let mut loader = MemoryLoader::default();
    loader.0.insert(
        "/virtual/lib/increment.cjs".into(),
        MemoryLoader::module(
            "/virtual/lib/increment.cjs",
            CommonJsModuleFormat::JavaScript,
            Some("module.exports = function(value) { return value + 1; };"),
        ),
    );
    loader.0.insert(
            "/virtual/a.cjs".into(),
            MemoryLoader::module(
                "/virtual/a.cjs",
                CommonJsModuleFormat::JavaScript,
                Some("exports.name = 'a'; const b = require('./b.cjs'); module.exports = {seen: b.seen};"),
            ),
        );
    loader.0.insert(
        "/virtual/b.cjs".into(),
        MemoryLoader::module(
            "/virtual/b.cjs",
            CommonJsModuleFormat::JavaScript,
            Some("const a = require('./a.cjs'); module.exports = {seen: a.name};"),
        ),
    );
    loader.0.insert(
            "/virtual/c.cjs".into(),
            MemoryLoader::module(
                "/virtual/c.cjs",
                CommonJsModuleFormat::JavaScript,
                Some("module.exports = {name: 'assigned'}; const d = require('./d.cjs'); module.exports.seen = d.seen;"),
            ),
        );
    loader.0.insert(
        "/virtual/d.cjs".into(),
        MemoryLoader::module(
            "/virtual/d.cjs",
            CommonJsModuleFormat::JavaScript,
            Some("const c = require('./c.cjs'); module.exports = {seen: c.name};"),
        ),
    );
    let mut interp = interpreter(loader);
    let value = interp
            .eval_source("const increment = require('./lib/increment.cjs'); const a = require('./a.cjs'); const c = require('./c.cjs'); ({answer: increment(41), seen: a.seen, assigned: c.seen});")
            .unwrap();
    assert!(matches!(
        value.get_prop("answer"),
        Some(Value::Number(42.0))
    ));
    assert!(matches!(
        value.get_prop("seen"),
        Some(Value::String(ref name)) if name == "a"
    ));
    assert!(matches!(
        value.get_prop("assigned"),
        Some(Value::String(ref name)) if name == "assigned"
    ));
}

#[test]
fn require_loader_and_cache_are_shared_with_async_function_realms() {
    let mut loader = MemoryLoader::default();
    loader.0.insert(
        "/virtual/async.cjs".into(),
        MemoryLoader::module(
            "/virtual/async.cjs",
            CommonJsModuleFormat::JavaScript,
            Some(
                "module.exports = async function() { return require('./dependency.cjs').answer; };",
            ),
        ),
    );
    loader.0.insert(
        "/virtual/dependency.cjs".into(),
        MemoryLoader::module(
            "/virtual/dependency.cjs",
            CommonJsModuleFormat::JavaScript,
            Some("module.exports = {answer: 29};"),
        ),
    );
    let mut interp = interpreter(loader);
    interp
            .eval_source("const loadLater = require('./async.cjs'); loadLater().then(value => { globalThis.asyncRequireAnswer = value; });")
            .unwrap();
    assert!(matches!(
        interp.global_value("asyncRequireAnswer"),
        Some(Value::Number(29.0))
    ));
}

#[test]
fn require_parses_json_as_guest_values() {
    let mut loader = MemoryLoader::default();
    loader.0.insert(
        "/virtual/data.json".into(),
        MemoryLoader::module(
            "/virtual/data.json",
            CommonJsModuleFormat::Json,
            Some(r#"{"ok":true,"count":3}"#),
        ),
    );
    let result = interpreter(loader)
        .eval_source("require('./data.json');")
        .unwrap();
    assert!(matches!(result.get_prop("ok"), Some(Value::Bool(true))));
    assert!(matches!(result.get_prop("count"), Some(Value::Number(3.0))));
}

#[test]
fn require_is_disabled_until_the_host_configures_a_loader() {
    let mut interp = crate::interpreter::Interpreter::with_builtins();
    assert!(matches!(
        interp.eval_source("typeof require;"),
        Ok(Value::String(ref kind)) if kind == "object"
    ));
    let error = interp.require_commonjs("./missing.cjs", None).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("configure a host CommonJS module loader")
    );
}

struct FakeNativeAddon;

impl NativeAddonLoader for FakeNativeAddon {
    fn load(&self, _filename: &Path) -> Result<Value, VmErr> {
        Ok(Value::Number(17.0))
    }
}

struct InitialExportsNativeAddon {
    attempts: std::cell::Cell<usize>,
    fail_first_attempt: bool,
}

impl NativeAddonLoader for InitialExportsNativeAddon {
    fn load(&self, _filename: &Path) -> Result<Value, VmErr> {
        Ok(Value::Number(17.0))
    }

    fn load_with_exports(&self, _filename: &Path, exports: Value) -> Result<Value, VmErr> {
        let attempt = self.attempts.get() + 1;
        self.attempts.set(attempt);
        if self.fail_first_attempt && attempt == 1 {
            return Err(VmErr::Msg("fixture initializer failed".into()));
        }
        exports.set_prop("initialized".into(), Value::Bool(true))?;
        Ok(exports)
    }
}

#[test]
fn native_addon_publishes_initial_exports_and_retries_after_initialization_failure() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-native-addon-cache-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    let addon = root.join("fixture.node");
    fs::write(&addon, b"trusted test fixture").unwrap();
    let provider = Rc::new(InitialExportsNativeAddon {
        attempts: std::cell::Cell::new(0),
        fail_first_attempt: true,
    });
    let loader = FileCommonJsLoader::new([&root])
        .unwrap()
        .allow_native_addon(&addon)
        .unwrap()
        .with_native_addon_loader(provider.clone());
    let mut interpreter = crate::interpreter::Interpreter::with_builtins();
    interpreter.set_commonjs_loader(Rc::new(loader)).unwrap();

    let first = interpreter.require_commonjs("./fixture.node", None);
    assert!(matches!(first, Err(VmErr::Msg(message)) if message == "fixture initializer failed"));
    assert!(matches!(
        interpreter.require_commonjs("./fixture.node", None),
        Ok(Value::Object { .. })
    ));
    let result = interpreter
            .eval_source(
                "const first = require('./fixture.node'); ({initialized: first.initialized, cached: first === require('./fixture.node')});",
            )
            .unwrap();
    assert!(matches!(
        result.get_prop("initialized"),
        Some(Value::Bool(true))
    ));
    assert!(matches!(result.get_prop("cached"), Some(Value::Bool(true))));
    assert_eq!(provider.attempts.get(), 2);

    drop(interpreter);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn node_api_prebuild_resolution_filters_incompatible_tags_and_aliases_package() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-node-api-prebuild-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let package_root = root.join("node_modules/fixture");
    let target = NodeApiPrebuildTarget::current();
    let tuple_name = format!("{}-{}", target.platform, target.architecture);
    let prebuild_dir = package_root.join("prebuilds").join(tuple_name);
    fs::create_dir_all(&prebuild_dir).unwrap();
    for filename in [
        "node.abi999.node",
        "electron.napi.node",
        "node.napi.uv1.node",
        "node.napi.node",
    ] {
        fs::write(prebuild_dir.join(filename), filename).unwrap();
    }
    if let Some(libc) = &target.libc {
        let filename = format!("node.napi.{libc}.node");
        fs::write(prebuild_dir.join(filename), b"libc specific napi").unwrap();
    }
    if let Some(armv) = &target.armv {
        let filename = format!("node.napi.armv{armv}.node");
        fs::write(prebuild_dir.join(filename), b"arm specific napi").unwrap();
    }
    if let (Some(libc), Some(armv)) = (&target.libc, &target.armv) {
        let filename = format!("node.napi.{libc}.armv{armv}.node");
        fs::write(prebuild_dir.join(filename), b"libc and arm specific napi").unwrap();
    }
    fs::create_dir_all(&root).unwrap();

    let loader = FileCommonJsLoader::new([&root]).unwrap();
    let selected = loader.resolve_node_api_prebuild(&package_root).unwrap();
    let selected_path = PathBuf::from(&selected.filename);
    assert_eq!(selected.format, CommonJsModuleFormat::NativeAddon);
    let selected_filename = selected_path.file_name().unwrap().to_str().unwrap();
    if let (Some(libc), Some(armv)) = (&target.libc, &target.armv) {
        assert_eq!(
            selected_filename,
            format!("node.napi.{libc}.armv{armv}.node")
        );
    } else if let Some(libc) = target.libc {
        assert_eq!(selected_filename, format!("node.napi.{libc}.node"));
    } else if let Some(armv) = target.armv {
        assert_eq!(selected_filename, format!("node.napi.armv{armv}.node"));
    } else {
        assert_eq!(selected_filename, "node.napi.node");
    }

    let loader = loader
        .allow_native_addon(&selected_path)
        .unwrap()
        .with_native_addon_alias("fixture", &selected_path)
        .unwrap()
        .with_native_addon_loader(Rc::new(FakeNativeAddon));
    let mut interpreter = crate::interpreter::Interpreter::with_builtins();
    interpreter.set_commonjs_loader(Rc::new(loader)).unwrap();
    let result = interpreter
            .eval_source(
                "const first = require('fixture'); ({path: require.resolve('fixture'), value: first, cached: first === require('fixture')});",
            )
            .unwrap();
    assert!(matches!(
        result.get_prop("path"),
        Some(Value::String(ref path)) if path == &selected.filename
    ));
    assert!(matches!(
        result.get_prop("value"),
        Some(Value::Number(17.0))
    ));
    assert!(matches!(result.get_prop("cached"), Some(Value::Bool(true))));

    drop(interpreter);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn node_api_prebuild_lookup_honors_prebuilds_only() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-node-api-prebuilds-only-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let package_root = root.join("node_modules/fixture");
    let target = NodeApiPrebuildTarget::current();
    let prebuild_dir = package_root
        .join("prebuilds")
        .join(format!("{}-{}", target.platform, target.architecture));
    let release_dir = package_root.join("build/Release");
    fs::create_dir_all(&prebuild_dir).unwrap();
    fs::create_dir_all(&release_dir).unwrap();
    let release_addon = release_dir.join("fixture.node");
    let prebuild_addon = prebuild_dir.join("node.napi.node");
    fs::write(&release_addon, b"release addon").unwrap();
    fs::write(&prebuild_addon, b"prebuild addon").unwrap();

    let loader = FileCommonJsLoader::new([&root]).unwrap();
    assert_eq!(
        PathBuf::from(
            loader
                .resolve_node_api_prebuild(&package_root)
                .unwrap()
                .filename
        ),
        release_addon.canonicalize().unwrap()
    );
    let prebuilds_only = loader.with_node_gyp_build_prebuilds_only(true);
    assert_eq!(
        PathBuf::from(
            prebuilds_only
                .resolve_node_api_prebuild(&package_root)
                .unwrap()
                .filename
        ),
        prebuild_addon.canonicalize().unwrap()
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn node_api_prebuild_selection_uses_node_gyp_tag_precedence() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-node-api-prebuild-tag-precedence-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let package_root = root.join("node_modules/fixture");
    let target = NodeApiPrebuildTarget::current();
    let prebuild_dir = package_root
        .join("prebuilds")
        .join(format!("{}-{}", target.platform, target.architecture));
    fs::create_dir_all(&prebuild_dir).unwrap();
    fs::write(
        prebuild_dir.join("node.napi.node.napi.node"),
        b"more specific generic N-API build",
    )
    .unwrap();
    fs::write(
        prebuild_dir.join("node.abi999.napi.node"),
        b"ABI-tagged N-API build",
    )
    .unwrap();

    let loader = FileCommonJsLoader::new([&root]).unwrap();
    let selected = loader.resolve_node_api_prebuild(&package_root).unwrap();
    assert_eq!(
        Path::new(&selected.filename).file_name().unwrap(),
        "node.abi999.napi.node"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn node_api_prebuild_lookup_uses_exec_path_neighbor_as_fallback() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-node-api-prebuild-exec-path-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let package_root = root.join("node_modules/fixture");
    let executable_directory = root.join("application");
    let target = NodeApiPrebuildTarget::current();
    let prebuild_dir = executable_directory
        .join("prebuilds")
        .join(format!("{}-{}", target.platform, target.architecture));
    fs::create_dir_all(&package_root).unwrap();
    fs::create_dir_all(&prebuild_dir).unwrap();
    let addon = prebuild_dir.join("node.napi.node");
    fs::write(&addon, b"nearby prebuild").unwrap();

    let loader = FileCommonJsLoader::new([&root])
        .unwrap()
        .with_node_gyp_build_exec_path(executable_directory.join("desktop-app"));
    assert_eq!(
        PathBuf::from(
            loader
                .resolve_node_api_prebuild(&package_root)
                .unwrap()
                .filename
        ),
        addon.canonicalize().unwrap()
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn node_gyp_build_package_prebuild_override_is_canonical_and_root_checked() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-node-gyp-build-prebuild-override-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let package_root = root.join("node_modules/sample-addon");
    let override_root = root.join("app/prebuilt-addon");
    fs::create_dir_all(&package_root).unwrap();
    fs::create_dir_all(&override_root).unwrap();
    fs::write(
        package_root.join("package.json"),
        r#"{"name":"sample-addon"}"#,
    )
    .unwrap();
    let loader = FileCommonJsLoader::new([&root]).unwrap();
    assert_eq!(
        node_gyp_build_prebuild_override_variable("sample-addon"),
        "SAMPLE_ADDON_PREBUILD"
    );
    assert_eq!(
        loader
            .node_gyp_build_package_root_with_override(
                &package_root,
                Some("sample-addon"),
                Some(override_root.clone()),
            )
            .unwrap(),
        override_root.canonicalize().unwrap()
    );
    let outside = std::env::temp_dir().join(format!(
        "napi-vm-outside-node-gyp-prebuild-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&outside).unwrap();
    let error = loader
        .node_gyp_build_package_root_with_override(
            &package_root,
            Some("sample-addon"),
            Some(outside.clone()),
        )
        .unwrap_err();
    assert!(error.to_string().contains("inside configured roots"));

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn node_gyp_build_runtime_builtin_exposes_tag_and_tuple_helpers() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-node-gyp-build-helpers-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("main.cjs"), "").unwrap();
    let loader = FileCommonJsLoader::new([&root])
        .unwrap()
        .with_node_gyp_build_compat();
    let mut interpreter = crate::interpreter::Interpreter::with_builtins();
    interpreter.set_commonjs_loader(Rc::new(loader)).unwrap();
    interpreter.set_commonjs_entry(root.join("main.cjs").to_string_lossy().into_owned());
    let value = interpreter
        .eval_source(
            r#"
const helper = require('node-gyp-build');
const napi = helper.parseTags('node.abi115.napi.node');
const abiOnly = helper.parseTags('node.abi115.node');
const uv = helper.parseTags('node.napi.uv1.node');
const tuple = helper.parseTuple('darwin-x64+arm64');
JSON.stringify({
  file: napi.file,
  runtime: napi.runtime,
  abi: napi.abi,
  napi: napi.napi,
  specificity: napi.specificity,
  napiMatches: helper.matchTags('node', '115')(napi),
  abiOnlyMatches: helper.matchTags('node', '115')(abiOnly),
  uvMatches: helper.matchTags('node', '115')(uv),
  tupleName: tuple.name,
  tuplePlatform: tuple.platform,
  tupleArchitectures: tuple.architectures,
  tupleMatches: helper.matchTuple('darwin', 'arm64')(tuple),
  tupleComparison: helper.compareTuples(tuple, helper.parseTuple('darwin-x64')),
  tagComparison: helper.compareTags('node')(helper.parseTags('node.napi.node'), abiOnly),
  invalidTuple: helper.parseTuple('linux-x64-debug') === undefined,
  pathAlias: helper.path === helper.resolve
});
"#,
        )
        .unwrap();
    let Value::String(ref json) = value else {
        panic!("node-gyp-build helper fixture did not return JSON: {value:?}");
    };
    let result: JsonValue = serde_json::from_str(json).unwrap();
    assert_eq!(result["file"], "node.abi115.napi.node");
    assert_eq!(result["runtime"], "node");
    assert_eq!(result["abi"], "115");
    assert_eq!(result["napi"], true);
    assert_eq!(result["specificity"], 3);
    assert_eq!(result["napiMatches"], true);
    assert_eq!(result["abiOnlyMatches"], false);
    assert_eq!(result["uvMatches"], false);
    assert_eq!(result["tupleName"], "darwin-x64+arm64");
    assert_eq!(result["tuplePlatform"], "darwin");
    assert_eq!(
        result["tupleArchitectures"],
        serde_json::json!(["x64", "arm64"])
    );
    assert_eq!(result["tupleMatches"], true);
    assert_eq!(result["tupleComparison"], 1);
    assert_eq!(result["tagComparison"], 1);
    assert_eq!(result["invalidTuple"], true);
    assert_eq!(result["pathAlias"], true);

    drop(interpreter);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn filesystem_loader_resolves_exports_and_requires_native_addon_allowlisting() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-commonjs-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let package_dir = root.join("node_modules").join("fixture");
    fs::create_dir_all(package_dir.join("dist")).unwrap();
    fs::write(root.join("main.cjs"), "").unwrap();
    fs::write(
        package_dir.join("package.json"),
        r#"{"exports":{".":{"require":"./dist/main.cjs","default":"./index.js"}}}"#,
    )
    .unwrap();
    fs::write(
        package_dir.join("dist/main.cjs"),
        "module.exports = {source: 'package'};",
    )
    .unwrap();
    fs::write(root.join("addon.node"), "test fixture").unwrap();

    let parent = root.join("main.cjs").to_string_lossy().into_owned();
    let loader = FileCommonJsLoader::new([&root]).unwrap();
    let package = loader.resolve("fixture", Some(&parent)).unwrap();
    assert_eq!(package.format, CommonJsModuleFormat::JavaScript);
    assert!(package.filename.ends_with("dist/main.cjs"));

    let addon = loader.resolve("./addon.node", Some(&parent)).unwrap();
    let denied = loader.load_native_addon(&addon).unwrap_err();
    assert!(denied.to_string().contains("not allowlisted"));

    let bad_digest = FileCommonJsLoader::new([&root])
        .unwrap()
        .allow_native_addon_with_sha256(root.join("addon.node"), [0; 32])
        .unwrap_err();
    assert!(
        bad_digest
            .to_string()
            .contains("integrity check failed while configuring")
    );

    const TEST_FIXTURE_SHA256: [u8; 32] = [
        0x68, 0xe8, 0x9f, 0x8b, 0x20, 0x74, 0xe2, 0x62, 0x7d, 0x62, 0xbe, 0xe3, 0xa2, 0xb6, 0x94,
        0xe2, 0x81, 0x43, 0x2e, 0xf9, 0x09, 0xeb, 0x7a, 0x55, 0x05, 0xf0, 0x7b, 0xbf, 0xfd, 0x91,
        0x7c, 0xbf,
    ];
    let loader = Rc::new(
        FileCommonJsLoader::new([&root])
            .unwrap()
            .allow_native_addon_with_sha256(root.join("addon.node"), TEST_FIXTURE_SHA256)
            .unwrap()
            .with_native_addon_loader(Rc::new(FakeNativeAddon)),
    );
    let mut interpreter = crate::interpreter::Interpreter::with_builtins();
    interpreter.set_commonjs_entry(parent);
    interpreter.set_commonjs_loader(loader.clone()).unwrap();
    assert!(matches!(
        interpreter.eval_source("require('./addon.node');"),
        Ok(Value::Number(17.0))
    ));
    fs::write(root.join("addon.node"), "tampered fixture").unwrap();
    let integrity_error = loader.load_native_addon(&addon).unwrap_err();
    assert!(
        integrity_error
            .to_string()
            .contains("integrity check failed")
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn filesystem_loader_resolves_wildcard_exports_with_node_pattern_precedence() {
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-commonjs-exports-pattern-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let package = root.join("node_modules/fixture");
    let release = package.join("build/Release");
    let fallback = package.join("fallback");
    let generic = package.join("dist/generic");
    let modern = package.join("dist/modern");
    let extension = package.join("dist/extensions");
    for directory in [&release, &fallback, &generic, &modern, &extension] {
        fs::create_dir_all(directory).unwrap();
    }
    let entry = root.join("main.cjs");
    fs::write(&entry, "").unwrap();
    fs::write(
            package.join("package.json"),
            r#"{"exports":{".":{"node":"./dist/node.cjs","require":"./dist/require.cjs","default":"./dist/default.cjs"},"./native/*":{"node-addons":"./build/Release/*.node","default":"./fallback/*.js"},"./features/*":"./dist/generic/*.js","./features/modern-*":"./dist/modern/*.js","./features/*.js":"./dist/extensions/*.js"}}"#,
        )
        .unwrap();
    fs::write(package.join("dist/node.cjs"), "").unwrap();
    fs::write(package.join("dist/require.cjs"), "").unwrap();
    fs::write(package.join("dist/default.cjs"), "").unwrap();
    fs::write(release.join("fixture.node"), "").unwrap();
    fs::write(fallback.join("fixture.js"), "").unwrap();
    fs::write(generic.join("modern-item.js"), "").unwrap();
    fs::write(modern.join("item.js"), "").unwrap();
    fs::write(generic.join("read.js.js"), "").unwrap();
    fs::write(extension.join("read.js"), "").unwrap();

    let entry_name = entry.to_string_lossy().into_owned();
    let loader = FileCommonJsLoader::new([&root])
        .unwrap()
        .with_native_addon_loader(Rc::new(FakeNativeAddon));
    let resolved = [
        ("fixture", "dist/node.cjs"),
        ("fixture/native/fixture", "build/Release/fixture.node"),
        ("fixture/features/modern-item", "dist/modern/item.js"),
        ("fixture/features/read.js", "dist/extensions/read.js"),
    ];
    for (specifier, expected_suffix) in resolved {
        let module = loader.resolve(specifier, Some(&entry_name)).unwrap();
        assert!(
            module.filename.ends_with(expected_suffix),
            "{specifier} resolved to {}",
            module.filename
        );
        if module.format == CommonJsModuleFormat::NativeAddon {
            assert!(module.source.is_none());
        }

        if Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            let output = Command::new("node")
                    .arg("-e")
                    .arg("const {createRequire}=require('node:module');process.stdout.write(createRequire(process.argv[1]).resolve(process.argv[2]))")
                    .arg(&entry)
                    .arg(specifier)
                    .output()
                    .unwrap();
            assert!(
                output.status.success(),
                "Node could not resolve {specifier}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                module.filename,
                String::from_utf8_lossy(&output.stdout).trim(),
                "Node and napi-vm resolved {specifier} differently"
            );
        }
    }

    let addon_free_loader = FileCommonJsLoader::new([&root]).unwrap();
    let fallback_module = addon_free_loader
        .resolve("fixture/native/fixture", Some(&entry_name))
        .unwrap();
    assert!(fallback_module.filename.ends_with("fallback/fixture.js"));
    assert_eq!(fallback_module.format, CommonJsModuleFormat::JavaScript);
    if Command::new("node")
        .arg("--no-addons")
        .arg("-e")
        .arg("process.stdout.write(require.resolve(process.argv[1], { paths: [process.argv[2]] }))")
        .arg("fixture/native/fixture")
        .arg(&root)
        .output()
        .is_ok_and(|output| output.status.success())
    {
        let output = Command::new("node")
                .arg("--no-addons")
                .arg("-e")
                .arg("process.stdout.write(require.resolve(process.argv[1], { paths: [process.argv[2]] }))")
                .arg("fixture/native/fixture")
                .arg(&root)
                .output()
                .unwrap();
        assert_eq!(
            fallback_module.filename,
            String::from_utf8_lossy(&output.stdout),
            "Node --no-addons and addon-free napi-vm resolved different exports"
        );
    }

    assert!(
        loader
            .resolve("fixture/private/missing", Some(&entry_name))
            .is_err()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn filesystem_loader_resolves_package_self_references_only_when_exported() {
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-commonjs-self-reference-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let package = root.join("workspace/fixture");
    let source_dir = package.join("src");
    let dist_dir = package.join("dist");
    fs::create_dir_all(&source_dir).unwrap();
    fs::create_dir_all(&dist_dir).unwrap();
    let parent = source_dir.join("main.cjs");
    fs::write(&parent, "").unwrap();
    fs::write(
        package.join("package.json"),
        r#"{"name":"fixture","exports":{".":"./dist/index.cjs","./feature":"./dist/feature.cjs"}}"#,
    )
    .unwrap();
    fs::write(dist_dir.join("index.cjs"), "").unwrap();
    fs::write(dist_dir.join("feature.cjs"), "").unwrap();
    fs::write(dist_dir.join("private.cjs"), "").unwrap();

    let legacy_package = root.join("workspace/legacy");
    fs::create_dir_all(&legacy_package).unwrap();
    let legacy_parent = legacy_package.join("main.cjs");
    fs::write(&legacy_parent, "").unwrap();
    fs::write(
        legacy_package.join("package.json"),
        r#"{"name":"legacy","main":"./index.cjs"}"#,
    )
    .unwrap();
    fs::write(legacy_package.join("index.cjs"), "").unwrap();

    let loader = FileCommonJsLoader::new([&root]).unwrap();
    let parent_name = parent.to_string_lossy().into_owned();
    let root_module = loader.resolve("fixture", Some(&parent_name)).unwrap();
    assert!(root_module.filename.ends_with("dist/index.cjs"));
    let subpath = loader
        .resolve("fixture/feature", Some(&parent_name))
        .unwrap();
    assert!(subpath.filename.ends_with("dist/feature.cjs"));
    assert!(
        loader
            .resolve("fixture/private", Some(&parent_name))
            .is_err()
    );

    let legacy_parent_name = legacy_parent.to_string_lossy().into_owned();
    assert!(loader.resolve("legacy", Some(&legacy_parent_name)).is_err());

    if Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
    {
        for (specifier, resolved) in [
            ("fixture", Some(root_module.filename.as_str())),
            ("fixture/feature", Some(subpath.filename.as_str())),
            ("fixture/private", None),
            ("legacy", None),
        ] {
            let output = Command::new("node")
                    .arg("-e")
                    .arg("const {createRequire}=require('node:module');process.stdout.write(createRequire(process.argv[1]).resolve(process.argv[2]))")
                    .arg(if specifier == "legacy" {
                        legacy_parent.as_path()
                    } else {
                        parent.as_path()
                    })
                    .arg(specifier)
                    .output()
                    .unwrap();
            assert_eq!(
                output.status.success(),
                resolved.is_some(),
                "Node self-reference result differed for {specifier}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if let Some(resolved) = resolved {
                assert_eq!(String::from_utf8_lossy(&output.stdout), resolved);
            }
        }
    }

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn filesystem_loader_resolves_package_import_maps() {
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-commonjs-import-map-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let package = root.join("workspace/fixture");
    let source_dir = package.join("src");
    let features_dir = source_dir.join("features");
    let release_dir = package.join("build/Release");
    let fallback_dir = package.join("fallback");
    let dependency = package.join("node_modules/fixture-dep");
    for directory in [
        &source_dir,
        &features_dir,
        &release_dir,
        &fallback_dir,
        &dependency,
    ] {
        fs::create_dir_all(directory).unwrap();
    }
    let parent = source_dir.join("main.cjs");
    fs::write(&parent, "").unwrap();
    fs::write(
            package.join("package.json"),
            r##"{"name":"fixture","imports":{"#internal":"./src/internal.cjs","#features/*":"./src/features/*.cjs","#external":"fixture-dep","#condition":{"node":"./src/node-condition.cjs","require":"./src/require-condition.cjs","default":"./src/default.cjs"},"#native/*":{"node-addons":"./build/Release/*.node","default":"./fallback/*.js"}}}"##,
        )
        .unwrap();
    fs::write(source_dir.join("internal.cjs"), "").unwrap();
    fs::write(features_dir.join("alpha.cjs"), "").unwrap();
    fs::write(source_dir.join("node-condition.cjs"), "").unwrap();
    fs::write(source_dir.join("require-condition.cjs"), "").unwrap();
    fs::write(source_dir.join("default.cjs"), "").unwrap();
    fs::write(release_dir.join("fixture.node"), "").unwrap();
    fs::write(fallback_dir.join("fixture.js"), "").unwrap();
    fs::write(dependency.join("package.json"), r#"{"main":"./index.cjs"}"#).unwrap();
    fs::write(dependency.join("index.cjs"), "").unwrap();

    let parent_name = parent.to_string_lossy().into_owned();
    let loader = FileCommonJsLoader::new([&root])
        .unwrap()
        .with_native_addon_loader(Rc::new(FakeNativeAddon));
    let resolved = [
        ("#internal", "src/internal.cjs"),
        ("#features/alpha", "src/features/alpha.cjs"),
        ("#external", "node_modules/fixture-dep/index.cjs"),
        ("#condition", "src/node-condition.cjs"),
        ("#native/fixture", "build/Release/fixture.node"),
    ];
    for (specifier, expected_suffix) in resolved {
        let module = loader.resolve(specifier, Some(&parent_name)).unwrap();
        assert!(
            module.filename.ends_with(expected_suffix),
            "{specifier} resolved to {}",
            module.filename
        );
        if specifier.starts_with("#native/") {
            assert_eq!(module.format, CommonJsModuleFormat::NativeAddon);
            assert!(module.source.is_none());
        }

        if Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            let output = Command::new("node")
                    .arg("-e")
                    .arg("const {createRequire}=require('node:module');process.stdout.write(createRequire(process.argv[1]).resolve(process.argv[2]))")
                    .arg(&parent)
                    .arg(specifier)
                    .output()
                    .unwrap();
            assert!(
                output.status.success(),
                "Node could not resolve {specifier}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                module.filename,
                String::from_utf8_lossy(&output.stdout),
                "Node and napi-vm resolved {specifier} differently"
            );
        }
    }

    let addon_free_loader = FileCommonJsLoader::new([&root]).unwrap();
    let fallback_module = addon_free_loader
        .resolve("#native/fixture", Some(&parent_name))
        .unwrap();
    assert!(fallback_module.filename.ends_with("fallback/fixture.js"));
    assert_eq!(fallback_module.format, CommonJsModuleFormat::JavaScript);
    let node_without_addons = Command::new("node")
            .arg("--no-addons")
            .arg("-e")
            .arg("const {createRequire}=require('node:module');process.stdout.write(createRequire(process.argv[1]).resolve(process.argv[2]))")
            .arg(&parent)
            .arg("#native/fixture")
            .output();
    if let Ok(output) = node_without_addons
        && output.status.success()
    {
        assert_eq!(
            fallback_module.filename,
            String::from_utf8_lossy(&output.stdout),
            "Node --no-addons and addon-free napi-vm resolved different imports"
        );
    }

    assert!(loader.resolve("#unmapped", Some(&parent_name)).is_err());
    fs::write(root.join("outside.cjs"), "").unwrap();
    fs::write(
        package.join("package.json"),
        r##"{"name":"fixture","imports":{"#escape":"./../outside.cjs"}}"##,
    )
    .unwrap();
    assert!(loader.resolve("#escape", Some(&parent_name)).is_err());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn filesystem_loader_uses_export_array_fallbacks_only_for_invalid_targets() {
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "napi-vm-commonjs-exports-array-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let entry = root.join("main.cjs");
    fs::create_dir_all(root.join("node_modules")).unwrap();
    fs::write(&entry, "").unwrap();
    let cases = [
        ("invalid", r#"["not:valid","./fallback.cjs"]"#, true),
        ("null", r#"[null,"./fallback.cjs"]"#, true),
        (
            "no-condition",
            r#"[{"browser":"./browser.cjs"},"./fallback.cjs"]"#,
            true,
        ),
        ("bad-target", r#"["../outside.cjs","./fallback.cjs"]"#, true),
        ("encoded-path", r#""./fallback%2ecjs""#, true),
        (
            "encoded-dotdot",
            r#"["./%2e%2e/outside.cjs","./fallback.cjs"]"#,
            true,
        ),
        (
            "encoded-node-modules",
            r#"["./%6eode_modules/no.cjs","./fallback.cjs"]"#,
            true,
        ),
        ("missing", r#"["./missing.cjs","./fallback.cjs"]"#, false),
        (
            "conditional-missing",
            r#"{"node":"./missing.cjs","require":"./fallback.cjs"}"#,
            false,
        ),
    ];
    for (name, exports, _) in cases {
        let package = root.join("node_modules").join(name);
        fs::create_dir_all(&package).unwrap();
        fs::write(
            package.join("package.json"),
            format!(r#"{{"exports":{exports}}}"#),
        )
        .unwrap();
        fs::write(package.join("fallback.cjs"), "").unwrap();
    }

    let parent = entry.to_string_lossy().into_owned();
    let loader = FileCommonJsLoader::new([&root]).unwrap();
    let node_available = Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    for (name, _, should_fallback) in cases {
        let resolved = loader.resolve(name, Some(&parent));
        if should_fallback {
            let module = resolved.unwrap();
            assert!(module.filename.ends_with("fallback.cjs"));
        } else {
            assert!(
                resolved.is_err(),
                "{name} must not fall through on missing files"
            );
        }

        if node_available {
            let output = Command::new("node")
                    .arg("-e")
                    .arg("const {createRequire}=require('node:module');process.stdout.write(createRequire(process.argv[1]).resolve(process.argv[2]))")
                    .arg(&entry)
                    .arg(name)
                    .output()
                    .unwrap();
            assert_eq!(
                output.status.success(),
                should_fallback,
                "Node resolution status differed for {name}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if should_fallback {
                let module = loader.resolve(name, Some(&parent)).unwrap();
                assert_eq!(
                    module.filename,
                    String::from_utf8_lossy(&output.stdout).trim(),
                    "Node and napi-vm chose different fallback targets for {name}"
                );
            }
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn filesystem_loader_refuses_paths_outside_configured_roots() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let base = std::env::temp_dir().join(format!(
        "napi-vm-commonjs-boundary-{}-{}",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let root = base.join("allowed");
    fs::create_dir_all(&root).unwrap();
    let parent = root.join("main.cjs");
    fs::write(&parent, "").unwrap();
    let outside = base.join("outside.cjs");
    fs::write(&outside, "module.exports = true;").unwrap();

    let loader = FileCommonJsLoader::new([&root]).unwrap();
    let error = loader
        .resolve("../outside.cjs", Some(&parent.to_string_lossy()))
        .unwrap_err();
    assert!(error.to_string().contains("escapes configured roots"));

    fs::remove_dir_all(base).unwrap();
}
