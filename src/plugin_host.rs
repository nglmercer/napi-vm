//! Rust-native host orchestration for isolated JavaScript plugins.
//!
//! Plugin source remains guest JavaScript. This module owns `plugin.json`,
//! capability grants, filesystem policy, module registration and lifecycle;
//! it never evaluates plugin JavaScript with the host's `require()`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
use std::time::Duration;

use serde_json::{Map, Value as JsonValue};

use crate::host::HostBridge;
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
use crate::host::{HostCallback, HostEvent};
use crate::interpreter::FileCommonJsLoader;
use crate::interpreter::Interpreter;
use crate::parser::Statement;
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
use crate::value::PromiseInner;
use crate::value::Value;
use crate::{Lexer, Parser, VmErr};

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
use crate::{NativeAddonRuntime, RustNodeApiOptions};

pub const PLUGIN_MANIFEST_FILENAME: &str = "plugin.json";
pub const DEFAULT_MAX_PLUGIN_FILE_BYTES: u64 = 8 * 1024 * 1024;
const PLUGIN_FUNCTION_TAG: usize = 1usize << (usize::BITS - 1);

/// An error while preparing or managing a plugin.
#[derive(Debug)]
pub enum PluginHostError {
    Manifest(String),
    Load(String),
    Vm(VmErr),
    Io(String),
}

impl std::fmt::Display for PluginHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manifest(message) => write!(f, "PluginManifestError: {message}"),
            Self::Load(message) => write!(f, "PluginLoadError: {message}"),
            Self::Vm(error) => write!(f, "{error}"),
            Self::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for PluginHostError {}

impl From<VmErr> for PluginHostError {
    fn from(value: VmErr) -> Self {
        Self::Vm(value)
    }
}

/// A validated plugin manifest. Permissions remain JSON data so applications
/// can inspect the same values that the host compiled and enforced.
#[derive(Clone, Debug)]
pub struct RustPluginManifest {
    pub name: String,
    pub version: String,
    pub api_version: u32,
    pub entry: String,
    pub permissions: JsonValue,
}

/// Host grants for plugin capabilities. Filesystem patterns and the path
/// facade are separate grants; custom capability modules require both a
/// plugin request and a non-false entry here. Defaults deny all host access.
#[derive(Clone, Debug, Default)]
pub struct RustPluginPolicy {
    /// Read patterns granted by the desktop host, relative to each plugin
    /// root unless the pattern is absolute. These intersect manifest grants.
    pub fs_read: Vec<String>,
    /// Write patterns granted by the desktop host, relative to each plugin
    /// root unless the pattern is absolute. These intersect manifest grants.
    pub fs_write: Vec<String>,
    /// Whether the host permits installing `node:path` when requested.
    pub path: bool,
    pub capabilities: BTreeMap<String, JsonValue>,
}

impl RustPluginPolicy {
    pub fn grant_fs_read(mut self, pattern: impl Into<String>) -> Self {
        self.fs_read.push(pattern.into());
        self
    }

    pub fn grant_fs_write(mut self, pattern: impl Into<String>) -> Self {
        self.fs_write.push(pattern.into());
        self
    }

    pub fn grant_path(mut self) -> Self {
        self.path = true;
        self
    }

    pub fn grant(mut self, name: impl Into<String>, grant: JsonValue) -> Self {
        self.capabilities.insert(name.into(), grant);
        self
    }

    pub fn deny(mut self, name: impl Into<String>) -> Self {
        self.capabilities
            .insert(name.into(), JsonValue::Bool(false));
        self
    }
}

#[derive(Clone, Debug)]
pub struct RustPluginHostOptions {
    pub policy: RustPluginPolicy,
    pub max_file_bytes: u64,
}

impl Default for RustPluginHostOptions {
    fn default() -> Self {
        Self {
            policy: RustPluginPolicy::default(),
            max_file_bytes: DEFAULT_MAX_PLUGIN_FILE_BYTES,
        }
    }
}

/// A trusted Rust host callback exposed through one capability module.
pub type RustPluginFunction = Rc<dyn Fn(Vec<Value>) -> Result<Value, VmErr>>;

/// A host-owned guest capability module. Plugins can import it only when they
/// request its name in `permissions.capabilities` and the host grants it.
/// Exports execute in Rust and are trusted application code.
#[derive(Clone)]
pub struct RustPluginCapability {
    name: String,
    exports: Vec<(String, RustPluginFunction)>,
}

impl RustPluginCapability {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            exports: Vec::new(),
        }
    }

    pub fn export(
        mut self,
        name: impl Into<String>,
        function: impl Fn(Vec<Value>) -> Result<Value, VmErr> + 'static,
    ) -> Self {
        self.exports.push((name.into(), Rc::new(function)));
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RustPluginStatus {
    Loaded,
    Error(String),
}

/// One isolated plugin interpreter and its host-side lifecycle metadata.
pub struct RustLoadedPlugin {
    pub manifest: RustPluginManifest,
    /// Canonical root, retained on the host and never passed into guest code.
    pub root: PathBuf,
    pub status: RustPluginStatus,
    pub load_result: Option<JsonValue>,
    pub capabilities: Vec<String>,
    interpreter: Interpreter,
    plugin_bridge: Rc<PluginHostBridge>,
    module_ids: Vec<String>,
    bridge_globals: Vec<String>,
    #[cfg(all(
        feature = "node-api-host",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    native_runtime: Option<NativeAddonRuntime>,
}

impl RustLoadedPlugin {
    pub fn interpreter(&self) -> &Interpreter {
        &self.interpreter
    }

    /// The embedding application is trusted and may drive the plugin's VM
    /// between lifecycle hooks. Guest code still receives only installed APIs.
    pub fn interpreter_mut(&mut self) -> &mut Interpreter {
        &mut self.interpreter
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PermissionRule {
    absolute: bool,
    pattern: String,
}

#[derive(Clone, Debug, Default)]
struct FsPermissions {
    read: Vec<PermissionRule>,
    write: Vec<PermissionRule>,
}

type GuestModuleSources = Vec<(String, String)>;
type GuestModuleAliases = Vec<(String, String, String)>;

struct PreparedPlugin {
    manifest: RustPluginManifest,
    root: PathBuf,
    entry_path: PathBuf,
    entry_id: String,
    sources: GuestModuleSources,
    module_aliases: GuestModuleAliases,
    fs_permissions: FsPermissions,
    path_enabled: bool,
    capability_requests: BTreeMap<String, JsonValue>,
}

/// A Rust-native equivalent of the TypeScript `PluginHost` for the guest
/// module/lifecycle contract. Standard filesystem and path facades are
/// installed as guest modules; trusted Rust functions are opt-in capabilities.
pub struct RustPluginHost {
    options: RustPluginHostOptions,
    capabilities: BTreeMap<String, RustPluginCapability>,
    disabled: HashSet<String>,
    plugins: BTreeMap<String, RustLoadedPlugin>,
    #[cfg(all(
        feature = "node-api-host",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    napi_addons: HashMap<String, RustPluginNapiOptions>,
}

impl RustPluginHost {
    pub fn new(options: RustPluginHostOptions) -> Self {
        Self {
            options,
            capabilities: BTreeMap::new(),
            disabled: HashSet::new(),
            plugins: BTreeMap::new(),
            #[cfg(all(
                feature = "node-api-host",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            napi_addons: HashMap::new(),
        }
    }

    pub fn define_capability(
        &mut self,
        capability: RustPluginCapability,
    ) -> Result<(), PluginHostError> {
        validate_capability_name(&capability.name)?;
        if capability.exports.is_empty() {
            return Err(PluginHostError::Load(format!(
                "capability \"{}\" must export at least one function",
                capability.name
            )));
        }
        let mut names = HashSet::new();
        for (name, _) in &capability.exports {
            if !is_js_identifier(name) || !names.insert(name) {
                return Err(PluginHostError::Load(format!(
                    "capability \"{}\" has an invalid or duplicate export \"{name}\"",
                    capability.name
                )));
            }
        }
        if self.capabilities.contains_key(&capability.name) {
            return Err(PluginHostError::Load(format!(
                "capability \"{}\" is already defined",
                capability.name
            )));
        }
        self.capabilities
            .insert(capability.name.clone(), capability);
        Ok(())
    }

    pub fn set_capability_enabled(
        &mut self,
        name: &str,
        enabled: bool,
    ) -> Result<(), PluginHostError> {
        if !self.capabilities.contains_key(name) {
            return Err(PluginHostError::Load(format!(
                "unknown capability \"{name}\""
            )));
        }
        if enabled {
            self.disabled.remove(name);
        } else {
            self.disabled.insert(name.to_owned());
        }
        Ok(())
    }

    pub fn is_capability_enabled(&self, name: &str) -> bool {
        self.capabilities.contains_key(name) && !self.disabled.contains(name)
    }

    /// Configure the in-process Node-API backend for one plugin. The plugin
    /// root is used as the CommonJS source root; every native library must be
    /// separately allowlisted with a digest from trusted host metadata.
    #[cfg(all(
        feature = "node-api-host",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    pub fn configure_napi_addons(
        &mut self,
        plugin_name: impl Into<String>,
        options: RustPluginNapiOptions,
    ) -> Result<(), PluginHostError> {
        let plugin_name = plugin_name.into();
        if !valid_plugin_name(&plugin_name) {
            return Err(PluginHostError::Load(
                "invalid plugin name for Node-API configuration".into(),
            ));
        }
        if options.max_napi_version == 0 {
            return Err(PluginHostError::Load(
                "max Node-API version must be at least 1".into(),
            ));
        }
        if options.allowed_addons.is_empty()
            && options.native_prebuild_aliases.is_empty()
            && options.native_package_prebuilds.is_empty()
        {
            return Err(PluginHostError::Load(
                "Node-API configuration requires at least one trusted addon".into(),
            ));
        }
        self.napi_addons.insert(plugin_name, options);
        Ok(())
    }

    /// Load a plugin directory and run its `onLoad` hook.
    ///
    /// Lifecycle hooks may return a Promise; the host drives this plugin's VM
    /// event loop until it settles before returning to Rust.
    pub fn load(
        &mut self,
        plugin_directory: impl AsRef<Path>,
    ) -> Result<&mut RustLoadedPlugin, PluginHostError> {
        let prepared = prepare_plugin(plugin_directory.as_ref(), self.options.max_file_bytes)?;
        let name = prepared.manifest.name.clone();
        if self
            .plugins
            .get(&name)
            .is_some_and(|plugin| plugin.status == RustPluginStatus::Loaded)
        {
            return Err(PluginHostError::Load(format!(
                "plugin \"{name}\" is already loaded"
            )));
        }

        let mut plugin = self.instantiate(prepared)?;
        match invoke_json(
            &mut plugin.interpreter,
            &format!("__plugin_onLoad({})", context_json(&plugin.manifest, None)),
        ) {
            Ok(result) => plugin.load_result = result,
            Err(error) => {
                self.dispose(&mut plugin);
                plugin.status = RustPluginStatus::Error(error.to_string());
                self.plugins.insert(name.clone(), plugin);
                return Err(PluginHostError::Load(format!(
                    "plugin \"{name}\" failed in onLoad: {error}"
                )));
            }
        }
        self.plugins.insert(name.clone(), plugin);
        Ok(self.plugins.get_mut(&name).expect("plugin inserted above"))
    }

    /// Tear down the current instance, create a fresh interpreter from disk,
    /// and pass JSON-serializable unload state to `onReload`. Promise-returning
    /// hooks are driven to settlement on the VM's existing event loop.
    pub fn reload(&mut self, name: &str) -> Result<&mut RustLoadedPlugin, PluginHostError> {
        let Some(mut current) = self.plugins.remove(name) else {
            return Err(PluginHostError::Load(format!(
                "plugin \"{name}\" is not loaded"
            )));
        };
        let previous_state = if current.status == RustPluginStatus::Loaded {
            match invoke_json(
                &mut current.interpreter,
                &format!(
                    "__plugin_onUnload({})",
                    context_json(&current.manifest, Some("reload"))
                ),
            ) {
                Ok(state) => state,
                Err(error) => {
                    self.dispose(&mut current);
                    current.status = RustPluginStatus::Error(error.to_string());
                    self.plugins.insert(name.to_owned(), current);
                    return Err(PluginHostError::Load(format!(
                        "plugin \"{name}\" failed in onUnload: {error}"
                    )));
                }
            }
        } else {
            None
        };
        let root = current.root.clone();
        self.dispose(&mut current);

        let prepared = prepare_plugin(&root, self.options.max_file_bytes)?;
        if prepared.manifest.name != name {
            return Err(PluginHostError::Load(format!(
                "plugin directory now declares \"{}\", expected \"{name}\"",
                prepared.manifest.name
            )));
        }
        let mut plugin = self.instantiate(prepared)?;
        let state = previous_state.unwrap_or(JsonValue::Null);
        let state_source = serde_json::to_string(&state)
            .map_err(|error| PluginHostError::Load(error.to_string()))?;
        let source = format!(
            "__plugin_onReload({}, {})",
            context_json(&plugin.manifest, None),
            state_source
        );
        match invoke_json(&mut plugin.interpreter, &source) {
            Ok(result) => plugin.load_result = result,
            Err(error) => {
                self.dispose(&mut plugin);
                plugin.status = RustPluginStatus::Error(error.to_string());
                self.plugins.insert(name.to_owned(), plugin);
                return Err(PluginHostError::Load(format!(
                    "plugin \"{name}\" failed in onReload: {error}"
                )));
            }
        }
        self.plugins.insert(name.to_owned(), plugin);
        Ok(self.plugins.get_mut(name).expect("plugin inserted above"))
    }

    /// Run `onUnload`, revoke the plugin runtime, and forget it. If the hook
    /// returns a Promise, drive the VM event loop until it settles.
    pub fn unload(&mut self, name: &str) -> Result<Option<JsonValue>, PluginHostError> {
        let Some(mut plugin) = self.plugins.remove(name) else {
            return Err(PluginHostError::Load(format!(
                "plugin \"{name}\" is not loaded"
            )));
        };
        let state = if plugin.status == RustPluginStatus::Loaded {
            invoke_json(
                &mut plugin.interpreter,
                &format!(
                    "__plugin_onUnload({})",
                    context_json(&plugin.manifest, Some("unload"))
                ),
            )
        } else {
            Ok(None)
        };
        self.dispose(&mut plugin);
        state.map_err(|error| {
            PluginHostError::Load(format!("plugin \"{name}\" failed in onUnload: {error}"))
        })
    }

    pub fn get(&self, name: &str) -> Option<&RustLoadedPlugin> {
        self.plugins.get(name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut RustLoadedPlugin> {
        self.plugins.get_mut(name)
    }

    pub fn list(&self) -> impl Iterator<Item = &RustLoadedPlugin> {
        self.plugins.values()
    }

    /// Unload every plugin. One failing hook does not prevent cleanup of the
    /// others; failures are returned after all entries have been removed.
    pub fn unload_all(&mut self) -> Vec<PluginHostError> {
        let names: Vec<_> = self.plugins.keys().cloned().collect();
        let mut errors = Vec::new();
        for name in names {
            if let Err(error) = self.unload(&name) {
                errors.push(error);
            }
        }
        errors
    }

    fn instantiate(&self, prepared: PreparedPlugin) -> Result<RustLoadedPlugin, PluginHostError> {
        let bridge = Rc::new(PluginHostBridge::default());
        let mut interpreter = Interpreter::with_builtins();
        interpreter.set_host_bridge(bridge.clone());
        let host_fs_permissions = compile_host_fs_permissions(&self.options.policy)?;
        let fs = Rc::new(PluginFileSystem::new(
            prepared.root.clone(),
            prepared.fs_permissions.clone(),
            host_fs_permissions,
            self.options.max_file_bytes,
        ));
        let mut module_ids = Vec::new();
        let mut bridge_globals = Vec::new();
        let mut active_capabilities = Vec::new();

        install_fs_module(
            &mut interpreter,
            &bridge,
            fs,
            &mut module_ids,
            &mut bridge_globals,
        );
        if prepared.path_enabled && self.options.policy.path {
            install_path_module(
                &mut interpreter,
                &bridge,
                &mut module_ids,
                &mut bridge_globals,
            );
        }

        for (name, request) in &prepared.capability_requests {
            if request == &JsonValue::Bool(false) || self.disabled.contains(name) {
                continue;
            }
            let Some(capability) = self.capabilities.get(name) else {
                return Err(PluginHostError::Load(format!(
                    "unknown capability \"{name}\""
                )));
            };
            let Some(grant) = self.options.policy.capabilities.get(name) else {
                continue;
            };
            if grant == &JsonValue::Bool(false) || grant.is_null() {
                continue;
            }
            if let JsonValue::Object(options) = request
                && !options.is_empty()
            {
                return Err(PluginHostError::Load(format!(
                    "capability \"{name}\" takes no options"
                )));
            }
            if !matches!(request, JsonValue::Bool(true) | JsonValue::Object(_)) {
                return Err(PluginHostError::Load(format!(
                    "capability \"{name}\" request must be true or an options object"
                )));
            }
            install_custom_capability(
                &mut interpreter,
                &bridge,
                capability,
                &mut module_ids,
                &mut bridge_globals,
            )?;
            active_capabilities.push(name.clone());
        }

        for (name, source) in &prepared.sources {
            interpreter.define_module(name, source.clone());
            module_ids.push(name.clone());
        }
        for (importer, specifier, target) in &prepared.module_aliases {
            interpreter.define_module_alias(importer, specifier, target);
        }

        // Force facades to capture their host functions, then remove bootstrap
        // globals before any plugin source executes.
        for module in [
            Some("node:fs"),
            prepared.path_enabled.then_some("node:path"),
        ]
        .into_iter()
        .flatten()
        .chain(active_capabilities.iter().map(String::as_str))
        {
            if !interpreter
                .ensure_module(module)
                .map_err(PluginHostError::Vm)?
            {
                return Err(PluginHostError::Load(format!(
                    "failed to install capability module \"{module}\""
                )));
            }
        }
        for name in &bridge_globals {
            interpreter.global.borrow_mut().remove(name);
        }

        // Keep JavaScript package loading inside the VM even when this plugin
        // has no native addons. If the plugin opts into napi, the backend
        // below replaces this resolver with the same root plus its trusted
        // native-addon provider.
        let commonjs_loader =
            FileCommonJsLoader::new([prepared.root.clone()]).map_err(PluginHostError::Vm)?;
        interpreter.set_commonjs_entry(prepared.entry_path.to_string_lossy().into_owned());
        interpreter
            .set_commonjs_loader(Rc::new(commonjs_loader))
            .map_err(PluginHostError::Vm)?;

        #[cfg(all(
            feature = "node-api-host",
            any(target_os = "linux", target_os = "macos", target_os = "windows")
        ))]
        let native_runtime = if let Some(config) = self.napi_addons.get(&prepared.manifest.name) {
            let mut options = RustNodeApiOptions::new([prepared.root.clone()])
                .max_napi_version(config.max_napi_version)
                .entry(prepared.entry_path.clone());
            if let Some(enabled) = config.node_gyp_build_prebuilds_only {
                options = options.node_gyp_build_prebuilds_only(enabled);
            }
            if let Some(exec_path) = &config.node_gyp_build_exec_path {
                options = options.node_gyp_build_exec_path(exec_path.clone());
            }

            let mut prebuild_resolver =
                FileCommonJsLoader::new([prepared.root.clone()]).map_err(PluginHostError::Vm)?;
            if let Some(enabled) = config.node_gyp_build_prebuilds_only {
                prebuild_resolver = prebuild_resolver.with_node_gyp_build_prebuilds_only(enabled);
            }
            if let Some(exec_path) = &config.node_gyp_build_exec_path {
                prebuild_resolver =
                    prebuild_resolver.with_node_gyp_build_exec_path(exec_path.clone());
            }

            let mut canonical_addons = Vec::with_capacity(config.allowed_addons.len());
            for (addon, digest) in &config.allowed_addons {
                let candidate = if addon.is_absolute() {
                    addon.clone()
                } else {
                    prepared.root.join(addon)
                };
                let canonical = fs::canonicalize(&candidate).map_err(|error| {
                    PluginHostError::Load(format!(
                        "cannot resolve configured native addon: {error}"
                    ))
                })?;
                if !canonical.starts_with(&prepared.root) {
                    return Err(PluginHostError::Load(
                        "native addon path is outside plugin root".into(),
                    ));
                }
                options = options.allow_native_addon_with_sha256(&canonical, *digest);
                canonical_addons.push(canonical);
            }
            for alias in &config.native_prebuild_aliases {
                let package_root = plugin_napi_package_root(&prepared.root, &alias.package_root)?;
                let selected = prebuild_resolver
                    .resolve_node_api_prebuild(&package_root)
                    .map_err(PluginHostError::Vm)?;
                options = options.allow_native_prebuild_with_sha256(
                    &alias.request,
                    &package_root,
                    alias.expected_sha256,
                );
                canonical_addons.push(PathBuf::from(selected.filename));
            }
            for package in &config.native_package_prebuilds {
                let package_root = plugin_napi_package_root(&prepared.root, &package.package_root)?;
                let selected = prebuild_resolver
                    .resolve_node_api_prebuild(&package_root)
                    .map_err(PluginHostError::Vm)?;
                options = options.allow_native_package_prebuild_with_sha256(
                    &package_root,
                    package.expected_sha256,
                );
                canonical_addons.push(PathBuf::from(selected.filename));
            }
            let runtime = interpreter
                .enable_native_addons(options)
                .map_err(PluginHostError::Vm)?;
            for addon in &canonical_addons {
                if let Err(error) = runtime.preflight_addon(addon) {
                    let _ = runtime.shutdown();
                    return Err(PluginHostError::Vm(error));
                }
            }
            let native_bridge = native_host_bridge(&runtime);
            interpreter.set_host_bridge(Rc::new(CompositeHostBridge {
                plugin: bridge.clone(),
                native: native_bridge,
            }));
            Some(runtime)
        } else {
            None
        };

        // The lifecycle wrappers and guest default export share one realm.
        let bootstrap = lifecycle_bootstrap(&prepared.entry_id);
        if let Err(error) = interpreter.eval_source(&bootstrap) {
            #[cfg(all(
                feature = "node-api-host",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            if let Some(runtime) = &native_runtime {
                let _ = runtime.shutdown();
            }
            return Err(PluginHostError::Vm(error));
        }
        let shape = match interpreter.eval_source("__plugin_describe()") {
            Ok(shape) => shape,
            Err(error) => {
                #[cfg(all(
                    feature = "node-api-host",
                    any(target_os = "linux", target_os = "macos", target_os = "windows")
                ))]
                if let Some(runtime) = &native_runtime {
                    let _ = runtime.shutdown();
                }
                return Err(PluginHostError::Vm(error));
            }
        };
        if !matches!(shape.get_prop("hasInstance"), Some(Value::Bool(true))) {
            #[cfg(all(
                feature = "node-api-host",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            if let Some(runtime) = &native_runtime {
                let _ = runtime.shutdown();
            }
            return Err(PluginHostError::Load(format!(
                "plugin \"{}\" must default-export an object or a class",
                prepared.manifest.name
            )));
        }
        bridge_globals.extend([
            "__plugin_onLoad".into(),
            "__plugin_onUnload".into(),
            "__plugin_onReload".into(),
            "__plugin_describe".into(),
        ]);

        Ok(RustLoadedPlugin {
            manifest: prepared.manifest,
            root: prepared.root,
            status: RustPluginStatus::Loaded,
            load_result: None,
            capabilities: active_capabilities,
            interpreter,
            plugin_bridge: bridge,
            module_ids,
            bridge_globals,
            #[cfg(all(
                feature = "node-api-host",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            native_runtime,
        })
    }

    fn dispose(&self, plugin: &mut RustLoadedPlugin) {
        for module in plugin.module_ids.drain(..) {
            plugin.interpreter.remove_module(&module);
        }
        for global in plugin.bridge_globals.drain(..) {
            plugin.interpreter.global.borrow_mut().remove(&global);
        }
        plugin.interpreter.global.borrow_mut().remove("require");
        plugin.interpreter.clear_commonjs_cache();
        plugin.plugin_bridge.functions.borrow_mut().clear();
        #[cfg(all(
            feature = "node-api-host",
            any(target_os = "linux", target_os = "macos", target_os = "windows")
        ))]
        if let Some(runtime) = plugin.native_runtime.take() {
            let _ = runtime.shutdown();
        }
    }
}

/// Per-plugin explicit Node-API addon allowlist. Each digest must come from
/// trusted desktop application metadata; native code is not sandboxed.
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
#[derive(Clone, Debug)]
pub struct RustPluginNapiOptions {
    max_napi_version: u32,
    allowed_addons: Vec<(PathBuf, [u8; 32])>,
    native_prebuild_aliases: Vec<RustPluginNapiPrebuildAlias>,
    native_package_prebuilds: Vec<RustPluginNapiPackagePrebuild>,
    node_gyp_build_prebuilds_only: Option<bool>,
    node_gyp_build_exec_path: Option<PathBuf>,
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
#[derive(Clone, Debug)]
struct RustPluginNapiPrebuildAlias {
    request: String,
    package_root: PathBuf,
    expected_sha256: [u8; 32],
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
#[derive(Clone, Debug)]
struct RustPluginNapiPackagePrebuild {
    package_root: PathBuf,
    expected_sha256: [u8; 32],
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
impl Default for RustPluginNapiOptions {
    fn default() -> Self {
        Self {
            max_napi_version: 10,
            allowed_addons: Vec::new(),
            native_prebuild_aliases: Vec::new(),
            native_package_prebuilds: Vec::new(),
            node_gyp_build_prebuilds_only: None,
            node_gyp_build_exec_path: None,
        }
    }
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
impl RustPluginNapiOptions {
    pub fn allow_addon_with_sha256(mut self, path: impl Into<PathBuf>, digest: [u8; 32]) -> Self {
        self.allowed_addons.push((path.into(), digest));
        self
    }

    /// Resolve an N-API prebuild for this platform and expose it through a
    /// bare `require()` request. The digest must come from trusted host
    /// metadata and is checked against the selected binary before loading.
    pub fn allow_native_prebuild_with_sha256(
        mut self,
        request: impl Into<String>,
        package_root: impl Into<PathBuf>,
        expected_sha256: [u8; 32],
    ) -> Self {
        self.native_prebuild_aliases
            .push(RustPluginNapiPrebuildAlias {
                request: request.into(),
                package_root: package_root.into(),
                expected_sha256,
            });
        self
    }

    /// Allow a package wrapper that calls
    /// `require("node-gyp-build")(__dirname)` to load its selected N-API
    /// prebuild. The selected binary must match the trusted digest.
    pub fn allow_native_package_prebuild_with_sha256(
        mut self,
        package_root: impl Into<PathBuf>,
        expected_sha256: [u8; 32],
    ) -> Self {
        self.native_package_prebuilds
            .push(RustPluginNapiPackagePrebuild {
                package_root: package_root.into(),
                expected_sha256,
            });
        self
    }

    /// Restrict package prebuild lookup to `prebuilds/<platform>-<arch>`.
    pub fn node_gyp_build_prebuilds_only(mut self, enabled: bool) -> Self {
        self.node_gyp_build_prebuilds_only = Some(enabled);
        self
    }

    /// Set the executable path used by the nearby-prebuild fallback.
    pub fn node_gyp_build_exec_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.node_gyp_build_exec_path = Some(path.into());
        self
    }

    pub fn max_napi_version(mut self, version: u32) -> Self {
        self.max_napi_version = version;
        self
    }
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
fn plugin_napi_package_root(
    plugin_root: &Path,
    package_root: &Path,
) -> Result<PathBuf, PluginHostError> {
    let candidate = if package_root.is_absolute() {
        package_root.to_path_buf()
    } else {
        plugin_root.join(package_root)
    };
    let canonical = fs::canonicalize(&candidate).map_err(|error| {
        PluginHostError::Load(format!("cannot resolve configured native package: {error}"))
    })?;
    if !canonical.is_dir() || !canonical.starts_with(plugin_root) {
        return Err(PluginHostError::Load(
            "configured native package root is outside the plugin directory".into(),
        ));
    }
    Ok(canonical)
}

fn prepare_plugin(
    directory: &Path,
    max_file_bytes: u64,
) -> Result<PreparedPlugin, PluginHostError> {
    let root = fs::canonicalize(directory).map_err(|error| {
        PluginHostError::Io(format!("cannot resolve plugin directory: {error}"))
    })?;
    if !root.is_dir() {
        return Err(PluginHostError::Load(
            "plugin directory is not a directory".into(),
        ));
    }
    let manifest_path = root.join(PLUGIN_MANIFEST_FILENAME);
    let manifest_path = fs::canonicalize(&manifest_path).map_err(|_| {
        PluginHostError::Load(format!(
            "missing {PLUGIN_MANIFEST_FILENAME} in plugin directory"
        ))
    })?;
    if !manifest_path.starts_with(&root) || !manifest_path.is_file() {
        return Err(PluginHostError::Load(
            "plugin manifest must be a file inside its directory".into(),
        ));
    }
    let manifest_bytes = read_limited(&manifest_path, max_file_bytes)?;
    let manifest_source = std::str::from_utf8(&manifest_bytes)
        .map_err(|_| PluginHostError::Manifest("plugin.json must be UTF-8".into()))?;
    let manifest = parse_manifest(manifest_source)?;
    let (fs_permissions, path_enabled, capability_requests) =
        parse_permissions(&manifest.permissions)?;
    let entry_relative = validate_entry_path(&manifest.entry)?;
    let entry_path = root.join(&entry_relative);
    let entry_path = fs::canonicalize(&entry_path)
        .map_err(|_| PluginHostError::Load(format!("entry file not found: {}", manifest.entry)))?;
    if !entry_path.starts_with(&root) || !entry_path.is_file() {
        return Err(PluginHostError::Manifest(
            "entry must be a file inside the plugin directory".into(),
        ));
    }
    let (mut sources, module_aliases) =
        collect_guest_module_graph(&root, &entry_path, &manifest.name, max_file_bytes)?;
    let relative_entry_id = module_id(&root, &entry_path, &manifest.name)?;
    let entry_id = relative_entry_id
        .strip_prefix("./")
        .expect("module ids have a relative alias")
        .to_owned();
    // The host bootstrap has no importing module, so it uses a bare virtual
    // entry ID. A re-export wrapper keeps the actual entry in its canonical
    // path module, where package aliases remain scoped to that source file.
    let entry_filename = entry_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| PluginHostError::Load("entry filename must be UTF-8".into()))?;
    sources.push((
        entry_id.clone(),
        format!(
            "export {{ default }} from {:?};",
            format!("./{entry_filename}")
        ),
    ));
    Ok(PreparedPlugin {
        manifest,
        root,
        entry_path,
        entry_id,
        sources,
        module_aliases,
        fs_permissions,
        path_enabled,
        capability_requests,
    })
}

fn parse_manifest(source: &str) -> Result<RustPluginManifest, PluginHostError> {
    let raw: JsonValue = serde_json::from_str(source).map_err(|error| {
        PluginHostError::Manifest(format!("plugin.json is not valid JSON: {error}"))
    })?;
    let object = raw
        .as_object()
        .ok_or_else(|| PluginHostError::Manifest("manifest must be a JSON object".into()))?;
    let name = required_nonempty_string(object, "name")?;
    if !valid_plugin_name(&name) {
        return Err(PluginHostError::Manifest(
            "name must match /^[A-Za-z0-9][A-Za-z0-9._-]*$/".into(),
        ));
    }
    let version = required_nonempty_string(object, "version")?;
    let api_version = object
        .get("apiVersion")
        .and_then(JsonValue::as_f64)
        .filter(|value| value.is_finite() && value.fract() == 0.0 && *value >= 0.0)
        .and_then(|value| u32::try_from(value as u64).ok())
        .ok_or_else(|| PluginHostError::Manifest("apiVersion must be an integer".into()))?;
    if api_version != 1 {
        return Err(PluginHostError::Manifest(format!(
            "apiVersion {api_version} is not supported (expected 1)"
        )));
    }
    let entry = required_nonempty_string(object, "entry")?;
    let permissions = match object.get("permissions") {
        None => JsonValue::Object(Map::new()),
        Some(value @ JsonValue::Object(_)) => value.clone(),
        Some(_) => {
            return Err(PluginHostError::Manifest(
                "permissions must be an object".into(),
            ));
        }
    };
    Ok(RustPluginManifest {
        name,
        version,
        api_version,
        entry,
        permissions,
    })
}

fn required_nonempty_string(
    object: &Map<String, JsonValue>,
    field: &str,
) -> Result<String, PluginHostError> {
    let value = object
        .get(field)
        .and_then(JsonValue::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| PluginHostError::Manifest(format!("{field} must be a non-empty string")))?;
    Ok(value.to_owned())
}

fn parse_permissions(
    permissions: &JsonValue,
) -> Result<(FsPermissions, bool, BTreeMap<String, JsonValue>), PluginHostError> {
    let object = permissions.as_object().expect("validated as object");
    for key in object.keys() {
        if !matches!(key.as_str(), "fs" | "path" | "capabilities") {
            return Err(PluginHostError::Manifest(format!(
                "unknown permission \"{key}\""
            )));
        }
    }
    let mut fs_permissions = FsPermissions::default();
    if let Some(fs_value) = object.get("fs") {
        let fs_object = fs_value
            .as_object()
            .ok_or_else(|| PluginHostError::Manifest("permissions.fs must be an object".into()))?;
        for key in fs_object.keys() {
            if !matches!(key.as_str(), "read" | "write") {
                return Err(PluginHostError::Manifest(format!(
                    "permissions.fs.{key} is not a permission"
                )));
            }
        }
        if let Some(read) = fs_object.get("read") {
            fs_permissions.read = compile_permission_rules(read, "permissions.fs.read")?;
        }
        if let Some(write) = fs_object.get("write") {
            fs_permissions.write = compile_permission_rules(write, "permissions.fs.write")?;
        }
    }
    let path_enabled = match object.get("path") {
        None => false,
        Some(JsonValue::Bool(value)) => *value,
        Some(_) => {
            return Err(PluginHostError::Manifest(
                "permissions.path must be a boolean".into(),
            ));
        }
    };
    let mut capability_requests = BTreeMap::new();
    if let Some(requests) = object.get("capabilities") {
        let requests = requests.as_object().ok_or_else(|| {
            PluginHostError::Manifest("permissions.capabilities must be an object".into())
        })?;
        for (name, request) in requests {
            validate_capability_name(name)?;
            if !(request.is_boolean() || request.as_object().is_some_and(|_| true)) {
                return Err(PluginHostError::Manifest(format!(
                    "permissions.capabilities[\"{name}\"] must be a boolean or an options object"
                )));
            }
            capability_requests.insert(name.clone(), request.clone());
        }
    }
    Ok((fs_permissions, path_enabled, capability_requests))
}

fn compile_permission_rules(
    value: &JsonValue,
    field: &str,
) -> Result<Vec<PermissionRule>, PluginHostError> {
    match value {
        JsonValue::Bool(false) => Ok(Vec::new()),
        JsonValue::Bool(true) => Ok(vec![PermissionRule {
            absolute: false,
            pattern: "**".into(),
        }]),
        JsonValue::String(pattern) => compile_one_pattern(pattern, field).map(|rule| vec![rule]),
        JsonValue::Array(patterns) => patterns
            .iter()
            .map(|pattern| {
                let pattern = pattern.as_str().ok_or_else(|| {
                    PluginHostError::Manifest(format!("{field} array entries must be strings"))
                })?;
                compile_one_pattern(pattern, field)
            })
            .collect(),
        _ => Err(PluginHostError::Manifest(format!(
            "{field} must be boolean, string, or string[]"
        ))),
    }
}

fn compile_host_fs_permissions(
    policy: &RustPluginPolicy,
) -> Result<FsPermissions, PluginHostError> {
    let compile = |patterns: &[String], field: &str| {
        patterns
            .iter()
            .map(|pattern| compile_one_pattern(pattern, field))
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(FsPermissions {
        read: compile(&policy.fs_read, "host policy fs.read")?,
        write: compile(&policy.fs_write, "host policy fs.write")?,
    })
}

fn compile_one_pattern(pattern: &str, field: &str) -> Result<PermissionRule, PluginHostError> {
    if pattern.contains('\0') {
        return Err(PluginHostError::Manifest(format!(
            "{field} contains a NUL byte"
        )));
    }
    let pattern = pattern.trim().replace('\\', "/");
    if pattern.is_empty() {
        return Err(PluginHostError::Manifest(format!(
            "{field} contains an empty pattern"
        )));
    }
    if matches!(pattern.as_str(), "*" | "**") {
        return Ok(PermissionRule {
            absolute: false,
            pattern: "**".into(),
        });
    }
    let absolute = guest_path_is_absolute(&pattern);
    let normalized = normalize_guest_path(&pattern, absolute);
    if !absolute && (normalized == ".." || normalized.starts_with("../")) {
        return Err(PluginHostError::Manifest(format!(
            "{field} pattern \"{pattern}\" escapes the plugin root"
        )));
    }
    if normalized.is_empty() || normalized == "." {
        return Err(PluginHostError::Manifest(format!(
            "{field} pattern \"{pattern}\" resolves to an empty path"
        )));
    }
    Ok(PermissionRule {
        absolute,
        pattern: normalized,
    })
}

fn validate_entry_path(entry: &str) -> Result<PathBuf, PluginHostError> {
    if entry.contains('\0') {
        return Err(PluginHostError::Manifest(
            "entry contains a NUL byte".into(),
        ));
    }
    let normalized = entry.replace('\\', "/");
    if guest_path_is_absolute(&normalized) {
        return Err(PluginHostError::Manifest(
            "entry must be a path inside the plugin directory".into(),
        ));
    }
    let mut parts = Vec::new();
    for component in normalized.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(PluginHostError::Manifest(
                        "entry must be a path inside the plugin directory".into(),
                    ));
                }
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(PluginHostError::Manifest(
            "entry must be a path inside the plugin directory".into(),
        ));
    }
    Ok(parts.iter().collect())
}

fn collect_guest_module_graph(
    root: &Path,
    entry: &Path,
    name: &str,
    max_file_bytes: u64,
) -> Result<(GuestModuleSources, GuestModuleAliases), PluginHostError> {
    let mut pending = vec![(entry.to_path_buf(), None)];
    let mut seen = HashSet::new();
    let mut sources = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    while let Some((path, imported_by)) = pending.pop() {
        let canonical = fs::canonicalize(&path).map_err(|error| {
            PluginHostError::Load(format!("cannot resolve guest module: {error}"))
        })?;
        if !canonical.starts_with(root) {
            return Err(PluginHostError::Load(
                "relative module import escapes the plugin directory".into(),
            ));
        }
        let id = module_id(root, &canonical, name)?;
        if let Some((importer, specifier)) = imported_by {
            aliases.insert((importer, specifier), id.clone());
        }
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let extension = canonical
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        if !matches!(extension, "js" | "mjs" | "json") {
            return Err(PluginHostError::Load(format!(
                "unsupported guest module extension .{extension}"
            )));
        }
        let bytes = read_limited(&canonical, max_file_bytes)?;
        let source = std::str::from_utf8(&bytes)
            .map_err(|_| PluginHostError::Load("guest modules must be UTF-8".into()))?;
        let guest_source = if extension == "json" {
            let json: JsonValue = serde_json::from_str(source).map_err(|error| {
                PluginHostError::Load(format!("guest JSON module is invalid: {error}"))
            })?;
            format!("export default {};", serde_json::to_string(&json).unwrap())
        } else {
            source.to_owned()
        };
        sources.insert(id.clone(), guest_source);
        if extension == "json" {
            continue;
        }
        for specifier in static_module_specifiers(source)? {
            if specifier.starts_with("node:") {
                // These names are resolved from host-installed capability
                // modules such as node:fs and node:path.
                continue;
            }
            let target = if specifier.starts_with('.') {
                Some(resolve_relative_module(root, &canonical, &specifier)?)
            } else {
                resolve_plugin_package(root, &canonical, &specifier, max_file_bytes)?
            };
            if let Some(target) = target {
                pending.push((target, Some((id.clone(), specifier))));
            }
        }
    }
    Ok((
        sources.into_iter().collect(),
        aliases
            .into_iter()
            .map(|((importer, specifier), target)| (importer, specifier, target))
            .collect(),
    ))
}

fn resolve_plugin_package(
    root: &Path,
    importer: &Path,
    request: &str,
    max_file_bytes: u64,
) -> Result<Option<PathBuf>, PluginHostError> {
    let (package_name, subpath) = split_plugin_package_request(request)?;
    let mut directory = importer.parent().unwrap_or(root);
    loop {
        if directory.starts_with(root)
            && directory
                .file_name()
                .is_none_or(|name| name != "node_modules")
        {
            let candidate = directory.join("node_modules").join(&package_name);
            if candidate.is_dir() {
                let package_root = fs::canonicalize(&candidate).map_err(|error| {
                    PluginHostError::Load(format!("cannot resolve package {package_name}: {error}"))
                })?;
                if !package_root.starts_with(root) {
                    return Err(PluginHostError::Load(format!(
                        "package {package_name} escapes the plugin directory"
                    )));
                }
                return resolve_plugin_package_entry(
                    root,
                    &package_root,
                    &package_name,
                    &subpath,
                    max_file_bytes,
                )
                .map(Some);
            }
        }
        if directory == root {
            break;
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        if !parent.starts_with(root) {
            break;
        }
        directory = parent;
    }
    Ok(None)
}

fn split_plugin_package_request(request: &str) -> Result<(String, String), PluginHostError> {
    if request.is_empty()
        || request.starts_with('.')
        || request.starts_with('/')
        || request.starts_with('#')
        || request.contains('\\')
        || request.contains(':')
        || request.contains('\0')
        || request.chars().any(char::is_whitespace)
    {
        return Err(PluginHostError::Load(format!(
            "unsupported guest package import '{request}'"
        )));
    }
    let parts: Vec<_> = request.split('/').collect();
    let package_end = if request.starts_with('@') { 2 } else { 1 };
    if parts.len() < package_end || parts[..package_end].iter().any(|part| part.is_empty()) {
        return Err(PluginHostError::Load(format!(
            "invalid guest package import '{request}'"
        )));
    }
    let package_name = parts[..package_end].join("/");
    let subpath = parts[package_end..].join("/");
    if package_name == "@" || parts.iter().any(|part| matches!(*part, "." | "..")) {
        return Err(PluginHostError::Load(format!(
            "invalid guest package import '{request}'"
        )));
    }
    Ok((package_name, subpath))
}

fn resolve_plugin_package_entry(
    root: &Path,
    package_root: &Path,
    package_name: &str,
    subpath: &str,
    max_file_bytes: u64,
) -> Result<PathBuf, PluginHostError> {
    let manifest_path = package_root.join("package.json");
    let manifest = if manifest_path.is_file() {
        let bytes = read_limited(&manifest_path, max_file_bytes)?;
        serde_json::from_slice::<JsonValue>(&bytes).map_err(|error| {
            PluginHostError::Load(format!(
                "package {package_name} has invalid package.json: {error}"
            ))
        })?
    } else {
        JsonValue::Null
    };
    let target = if let Some(exports) = manifest.get("exports") {
        let export_key = if subpath.is_empty() {
            ".".to_string()
        } else {
            format!("./{subpath}")
        };
        let target = plugin_exports_target(exports, &export_key)?.ok_or_else(|| {
            PluginHostError::Load(format!(
                "package {package_name} does not export subpath {export_key} for ESM import"
            ))
        })?;
        package_root.join(target)
    } else if subpath.is_empty() {
        let entry = manifest
            .get("module")
            .and_then(JsonValue::as_str)
            .or_else(|| manifest.get("main").and_then(JsonValue::as_str))
            .unwrap_or("index");
        package_root.join(entry)
    } else {
        package_root.join(subpath)
    };
    resolve_plugin_package_file(root, package_root, &target).map_err(|error| {
        PluginHostError::Load(format!(
            "cannot resolve ESM entry for package {package_name}: {error}"
        ))
    })
}

fn plugin_exports_target(
    exports: &JsonValue,
    key: &str,
) -> Result<Option<String>, PluginHostError> {
    let selected = match exports {
        JsonValue::String(_) | JsonValue::Array(_) | JsonValue::Null if key == "." => {
            select_plugin_export_condition(exports)?
        }
        JsonValue::Object(entries) if entries.keys().any(|entry| entry.starts_with('.')) => {
            if let Some(target) = entries.get(key) {
                select_plugin_export_condition(target)?
            } else {
                let mut best: Option<(usize, usize, String, &JsonValue)> = None;
                for (pattern, target) in entries {
                    let Some(capture) = match_plugin_export_pattern(pattern, key) else {
                        continue;
                    };
                    let Some(star) = pattern.find('*') else {
                        continue;
                    };
                    let specificity = (star, pattern.len());
                    if best
                        .as_ref()
                        .is_none_or(|(prefix, length, _, _)| specificity > (*prefix, *length))
                    {
                        best = Some((star, pattern.len(), capture, target));
                    }
                }
                if let Some((_, _, capture, target)) = best {
                    select_plugin_export_condition(target)?
                        .map(|target| target.replace('*', &capture))
                } else {
                    None
                }
            }
        }
        JsonValue::Object(_) if key == "." => select_plugin_export_condition(exports)?,
        _ => None,
    };
    if let Some(target) = selected {
        let Some(relative) = target.strip_prefix("./") else {
            return Err(PluginHostError::Load(format!(
                "unsupported package exports target '{target}'"
            )));
        };
        if relative.is_empty()
            || relative.contains('\\')
            || relative.split('/').any(|part| matches!(part, "." | ".."))
        {
            return Err(PluginHostError::Load(format!(
                "unsupported package exports target '{target}'"
            )));
        }
        Ok(Some(target))
    } else {
        Ok(None)
    }
}

fn select_plugin_export_condition(value: &JsonValue) -> Result<Option<String>, PluginHostError> {
    match value {
        JsonValue::String(target) => Ok(Some(target.clone())),
        JsonValue::Array(targets) => {
            for target in targets {
                if let Some(selected) = select_plugin_export_condition(target)? {
                    return Ok(Some(selected));
                }
            }
            Ok(None)
        }
        JsonValue::Object(conditions) => {
            for (condition, value) in conditions {
                if matches!(condition.as_str(), "import" | "node" | "default")
                    && let Some(target) = select_plugin_export_condition(value)?
                {
                    return Ok(Some(target));
                }
            }
            Ok(None)
        }
        JsonValue::Null => Ok(None),
        _ => Err(PluginHostError::Load(
            "package exports entry must be a string, array, condition object, or null".into(),
        )),
    }
}

fn match_plugin_export_pattern(pattern: &str, key: &str) -> Option<String> {
    let star = pattern.find('*')?;
    let prefix = &pattern[..star];
    let suffix = &pattern[star + 1..];
    let capture = key.strip_prefix(prefix)?.strip_suffix(suffix)?;
    (!capture.is_empty()).then(|| capture.to_string())
}

fn resolve_plugin_package_file(
    root: &Path,
    package_root: &Path,
    target: &Path,
) -> Result<PathBuf, String> {
    let candidates = if target.extension().is_some() {
        vec![target.to_path_buf()]
    } else {
        vec![
            target.with_extension("mjs"),
            target.with_extension("js"),
            target.with_extension("json"),
            target.join("index.mjs"),
            target.join("index.js"),
            target.join("index.json"),
        ]
    };
    for candidate in candidates {
        let Ok(canonical) = fs::canonicalize(&candidate) else {
            continue;
        };
        if !canonical.starts_with(root) || !canonical.starts_with(package_root) {
            return Err("package target escapes its plugin or package root".into());
        }
        if !canonical.is_file() {
            continue;
        }
        let extension = canonical
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        if !matches!(extension, "js" | "mjs" | "json") {
            return Err(format!("unsupported package module extension .{extension}"));
        }
        return Ok(canonical);
    }
    Err(format!(
        "package target does not exist: {}",
        target.display()
    ))
}

fn static_module_specifiers(source: &str) -> Result<Vec<String>, PluginHostError> {
    let tokens = Lexer::new(source).tokenize_with_spans();
    let mut parser = Parser::new_with_spans(tokens);
    let statements = parser
        .parse_program()
        .map_err(|error| PluginHostError::Load(format!("guest module parse failed: {error}")))?;
    let mut modules = Vec::new();
    for statement in statements {
        match statement {
            Statement::Import { module, .. } | Statement::ExportAll { source: module, .. } => {
                modules.push(module);
            }
            Statement::ExportNamed {
                source: Some(module),
                ..
            } => modules.push(module),
            _ => {}
        }
    }
    Ok(modules)
}

fn resolve_relative_module(
    root: &Path,
    parent: &Path,
    request: &str,
) -> Result<PathBuf, PluginHostError> {
    let request = request.replace('\\', "/");
    let mut base = parent.parent().unwrap_or(root).to_path_buf();
    for component in request.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if !base.pop() || !base.starts_with(root) {
                    return Err(PluginHostError::Load(
                        "relative module import escapes the plugin directory".into(),
                    ));
                }
            }
            part => base.push(part),
        }
    }
    let candidates = if base.extension().is_some() {
        vec![base.clone()]
    } else {
        vec![
            base.with_extension("js"),
            base.with_extension("mjs"),
            base.with_extension("json"),
            base.join("index.js"),
            base.join("index.mjs"),
        ]
    };
    for candidate in candidates {
        if let Ok(canonical) = fs::canonicalize(&candidate) {
            if canonical.starts_with(root) && canonical.is_file() {
                return Ok(canonical);
            }
            return Err(PluginHostError::Load(
                "relative module import escapes the plugin directory".into(),
            ));
        }
    }
    Err(PluginHostError::Load(format!(
        "cannot resolve relative module \"{request}\""
    )))
}

fn module_id(root: &Path, path: &Path, name: &str) -> Result<String, PluginHostError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| PluginHostError::Load("guest module is outside plugin directory".into()))?;
    let relative = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    Ok(format!("./plugin:{name}/{relative}"))
}

fn read_limited(path: &Path, max_file_bytes: u64) -> Result<Vec<u8>, PluginHostError> {
    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let file = options
        .open(path)
        .map_err(|error| PluginHostError::Io(format!("cannot open plugin file: {error}")))?;
    let metadata = file
        .metadata()
        .map_err(|error| PluginHostError::Io(format!("cannot inspect plugin file: {error}")))?;
    if !metadata.is_file() {
        return Err(PluginHostError::Load(
            "plugin source is not a regular file".into(),
        ));
    }
    if metadata.len() > max_file_bytes {
        return Err(PluginHostError::Load(format!(
            "plugin file exceeds the {max_file_bytes} byte read limit"
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_file_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| PluginHostError::Io(format!("cannot read plugin file: {error}")))?;
    if bytes.len() as u64 > max_file_bytes {
        return Err(PluginHostError::Load(format!(
            "plugin file exceeds the {max_file_bytes} byte read limit"
        )));
    }
    Ok(bytes)
}

fn valid_plugin_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

fn validate_capability_name(name: &str) -> Result<(), PluginHostError> {
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(PluginHostError::Manifest(format!(
            "invalid capability name \"{name}\""
        )));
    }
    Ok(())
}

fn is_js_identifier(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "new",
        "null",
        "return",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        "yield",
        "let",
        "enum",
        "implements",
        "interface",
        "package",
        "private",
        "protected",
        "public",
        "static",
    ];
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
        && !RESERVED.contains(&name)
}

fn guest_path_is_absolute(path: &str) -> bool {
    path.starts_with('/') || path.starts_with("//") || is_drive_path(path)
}

fn is_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

fn normalize_guest_path(path: &str, absolute: bool) -> String {
    let mut segments = Vec::<&str>::new();
    let mut prefix = "";
    let mut body = path;
    if is_drive_path(path) {
        prefix = &path[..2];
        body = &path[2..];
    }
    for segment in body.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|part| *part != "..") {
                    segments.pop();
                } else if !absolute {
                    segments.push("..");
                }
            }
            value => segments.push(value),
        }
    }
    let joined = segments.join("/");
    if absolute {
        if prefix.is_empty() {
            format!("/{joined}")
        } else {
            format!("{prefix}/{joined}")
        }
    } else if joined.is_empty() {
        ".".into()
    } else {
        joined
    }
}

fn path_pattern_matches(pattern: &str, candidate: &str) -> bool {
    let pattern_segments: Vec<_> = pattern.trim_matches('/').split('/').collect();
    let candidate_segments: Vec<_> = candidate.trim_matches('/').split('/').collect();
    fn match_segments(pattern: &[&str], candidate: &[&str]) -> bool {
        if pattern.is_empty() {
            return candidate.is_empty();
        }
        if pattern[0] == "**" {
            return match_segments(&pattern[1..], candidate)
                || (!candidate.is_empty() && match_segments(pattern, &candidate[1..]));
        }
        !candidate.is_empty()
            && wildcard_match(pattern[0], candidate[0])
            && match_segments(&pattern[1..], &candidate[1..])
    }
    match_segments(&pattern_segments, &candidate_segments)
}

fn wildcard_match(pattern: &str, candidate: &str) -> bool {
    let parts: Vec<_> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == candidate;
    }
    let mut offset = 0;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            if !candidate[offset..].starts_with(part) {
                return false;
            }
            offset += part.len();
        } else if index + 1 == parts.len() {
            return candidate[offset..].ends_with(part);
        } else if let Some(found) = candidate[offset..].find(part) {
            offset += found + part.len();
        } else {
            return false;
        }
    }
    true
}

#[derive(Clone, Copy)]
enum FsOperation {
    Read,
    Write,
}

#[derive(Debug)]
struct GuestCallError {
    name: &'static str,
    message: String,
}

struct PluginFileSystem {
    root: PathBuf,
    requested_permissions: FsPermissions,
    host_permissions: FsPermissions,
    max_file_bytes: u64,
}

impl PluginFileSystem {
    fn new(
        root: PathBuf,
        requested_permissions: FsPermissions,
        host_permissions: FsPermissions,
        max_file_bytes: u64,
    ) -> Self {
        Self {
            root,
            requested_permissions,
            host_permissions,
            max_file_bytes,
        }
    }

    fn resolve(&self, requested: &str, operation: FsOperation) -> Result<PathBuf, GuestCallError> {
        if requested.is_empty() {
            return Err(permission_error(
                "filesystem path must be a non-empty string",
            ));
        }
        if requested.contains('\0') {
            return Err(permission_error("path contains a NUL byte"));
        }
        let folded = requested.replace('\\', "/");
        let absolute = guest_path_is_absolute(&folded);
        let normalized = normalize_guest_path(&folded, absolute);
        if !absolute && (normalized == ".." || normalized.starts_with("../")) {
            return Err(permission_error("path escapes plugin root"));
        }
        let candidate = if absolute {
            let candidate = PathBuf::from(native_path_string(&normalized));
            if !candidate.is_absolute() {
                return Err(permission_error("absolute path is not valid on this host"));
            }
            candidate
        } else if normalized == "." {
            self.root.clone()
        } else {
            self.root.join(native_path_string(&normalized))
        };
        let canonical = canonicalize_missing_tail(&candidate)
            .map_err(|_| permission_error("path escapes plugin root"))?;
        if !canonical.starts_with(&self.root) {
            return Err(permission_error("path escapes plugin root"));
        }
        let relative = canonical
            .strip_prefix(&self.root)
            .map(slash_path)
            .unwrap_or_default();
        let absolute = slash_path(&canonical);
        let (requested_rules, host_rules) = match operation {
            FsOperation::Read => (
                &self.requested_permissions.read,
                &self.host_permissions.read,
            ),
            FsOperation::Write => (
                &self.requested_permissions.write,
                &self.host_permissions.write,
            ),
        };
        let matches = |rules: &[PermissionRule]| {
            rules.iter().any(|rule| {
                if rule.absolute {
                    path_pattern_matches(&rule.pattern, &absolute)
                } else {
                    path_pattern_matches(&rule.pattern, &relative)
                }
            })
        };
        let allowed = matches(requested_rules) && matches(host_rules);
        if !allowed {
            let operation = match operation {
                FsOperation::Read => "read",
                FsOperation::Write => "write",
            };
            return Err(permission_error(&format!(
                "fs.{operation} is not permitted for {requested:?}"
            )));
        }
        Ok(canonical)
    }

    fn read_text(&self, requested: &str) -> Result<String, GuestCallError> {
        let path = self.resolve(requested, FsOperation::Read)?;
        let mut options = OpenOptions::new();
        options.read(true);
        set_no_follow(&mut options);
        let file = options
            .open(&path)
            .map_err(|_| io_guest_error("cannot read requested plugin file"))?;
        let metadata = file
            .metadata()
            .map_err(|_| io_guest_error("cannot inspect requested plugin file"))?;
        if !metadata.is_file() {
            return Err(resource_error("path is not a regular file"));
        }
        if metadata.len() > self.max_file_bytes {
            return Err(resource_error(&format!(
                "file is larger than the {} byte read limit",
                self.max_file_bytes
            )));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(self.max_file_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| io_guest_error("cannot read requested plugin file"))?;
        if bytes.len() as u64 > self.max_file_bytes {
            return Err(resource_error(&format!(
                "file is larger than the {} byte read limit",
                self.max_file_bytes
            )));
        }
        String::from_utf8(bytes).map_err(|_| GuestCallError {
            name: "TypeError",
            message: "the sandboxed node:fs facade supports UTF-8 text only".into(),
        })
    }

    fn write_text(&self, requested: &str, contents: &str) -> Result<(), GuestCallError> {
        if contents.len() as u64 > self.max_file_bytes {
            return Err(resource_error(&format!(
                "contents are larger than the {} byte write limit",
                self.max_file_bytes
            )));
        }
        let path = self.resolve(requested, FsOperation::Write)?;
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        set_no_follow(&mut options);
        let mut file = options
            .open(&path)
            .map_err(|_| io_guest_error("cannot write requested plugin file"))?;
        file.write_all(contents.as_bytes())
            .map_err(|_| io_guest_error("cannot write requested plugin file"))
    }

    fn exists(&self, requested: &str) -> Result<bool, GuestCallError> {
        let path = self.resolve(requested, FsOperation::Read)?;
        Ok(path.exists())
    }
}

#[cfg(unix)]
fn set_no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW);
}

#[cfg(not(unix))]
fn set_no_follow(_options: &mut OpenOptions) {}

fn native_path_string(path: &str) -> String {
    if std::path::MAIN_SEPARATOR == '/' {
        path.to_owned()
    } else {
        path.replace('/', std::path::MAIN_SEPARATOR_STR)
    }
}

fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn canonicalize_missing_tail(path: &Path) -> std::io::Result<PathBuf> {
    let mut unresolved = Vec::new();
    let mut current = path;
    loop {
        match fs::canonicalize(current) {
            Ok(mut resolved) => {
                for part in unresolved.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = current
                    .file_name()
                    .ok_or_else(|| std::io::Error::new(error.kind(), error.to_string()))?;
                unresolved.push(name.to_os_string());
                current = current
                    .parent()
                    .ok_or_else(|| std::io::Error::new(error.kind(), error.to_string()))?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn permission_error(message: &str) -> GuestCallError {
    GuestCallError {
        name: "PermissionDenied",
        message: message.to_owned(),
    }
}

fn resource_error(message: &str) -> GuestCallError {
    GuestCallError {
        name: "ResourceLimit",
        message: message.to_owned(),
    }
}

fn io_guest_error(message: &str) -> GuestCallError {
    GuestCallError {
        name: "Error",
        message: message.to_owned(),
    }
}

fn guest_result(result: Result<Value, GuestCallError>) -> Value {
    match result {
        Ok(value) => Value::object(vec![
            ("ok".into(), Value::Bool(true)),
            ("value".into(), value),
        ]),
        Err(error) => Value::object(vec![
            ("ok".into(), Value::Bool(false)),
            ("name".into(), Value::String(error.name.into())),
            ("message".into(), Value::String(error.message)),
        ]),
    }
}

type HostFunction = Rc<dyn Fn(Vec<Value>) -> Result<Value, VmErr>>;

#[derive(Default)]
struct PluginHostBridge {
    next_id: Cell<usize>,
    functions: RefCell<HashMap<usize, HostFunction>>,
}

impl PluginHostBridge {
    fn register(&self, function: HostFunction) -> Result<usize, PluginHostError> {
        let next = self
            .next_id
            .get()
            .checked_add(1)
            .ok_or_else(|| PluginHostError::Load("too many plugin host functions".into()))?;
        if next >= PLUGIN_FUNCTION_TAG {
            return Err(PluginHostError::Load(
                "too many plugin host functions".into(),
            ));
        }
        let id = PLUGIN_FUNCTION_TAG | next;
        self.next_id.set(next);
        self.functions.borrow_mut().insert(id, function);
        Ok(id)
    }

    #[cfg(all(
        feature = "node-api-host",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    fn has_tag(id: usize) -> bool {
        id & PLUGIN_FUNCTION_TAG != 0
    }
}

impl HostBridge for PluginHostBridge {
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        let function = self
            .functions
            .borrow()
            .get(&id)
            .cloned()
            .ok_or_else(|| VmErr::Msg(format!("unknown plugin host function id {id}")))?;
        function(args)
    }
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
struct CompositeHostBridge {
    plugin: Rc<PluginHostBridge>,
    native: Rc<dyn HostBridge>,
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
impl HostBridge for CompositeHostBridge {
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.call_host(id, args)
        } else {
            self.native.call_host(id, args)
        }
    }

    fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        self.native.poll_host_events(timeout)
    }

    fn has_pending_host_work(&self, promise: &Rc<RefCell<PromiseInner>>) -> bool {
        self.native.has_pending_host_work(promise)
    }

    fn call_host_with_this(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.call_host_with_this(id, this_value, args)
        } else {
            self.native.call_host_with_this(id, this_value, args)
        }
    }

    fn call_host_with_callback_handler(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin
                .call_host_with_callback_handler(id, this_value, args, callback_handler)
        } else {
            self.native
                .call_host_with_callback_handler(id, this_value, args, callback_handler)
        }
    }

    fn construct_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.construct_host(id, args)
        } else {
            self.native.construct_host(id, args)
        }
    }

    fn construct_host_with_callback_handler(
        &self,
        id: usize,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin
                .construct_host_with_callback_handler(id, args, callback_handler)
        } else {
            self.native
                .construct_host_with_callback_handler(id, args, callback_handler)
        }
    }

    fn construct_host_with_callback_handler_and_target(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        new_target: Value,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.construct_host_with_callback_handler_and_target(
                id,
                this_value,
                args,
                new_target,
                callback_handler,
            )
        } else {
            self.native.construct_host_with_callback_handler_and_target(
                id,
                this_value,
                args,
                new_target,
                callback_handler,
            )
        }
    }

    fn call_host_constructor_with_callback_handler_and_target(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        new_target: Value,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin
                .call_host_constructor_with_callback_handler_and_target(
                    id,
                    this_value,
                    args,
                    new_target,
                    callback_handler,
                )
        } else {
            self.native
                .call_host_constructor_with_callback_handler_and_target(
                    id,
                    this_value,
                    args,
                    new_target,
                    callback_handler,
                )
        }
    }

    fn is_async_fn(&self, id: usize) -> bool {
        !PluginHostBridge::has_tag(id) && self.native.is_async_fn(id)
    }

    fn call_host_async(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.call_host_async(id, args)
        } else {
            self.native.call_host_async(id, args)
        }
    }

    fn call_host_async_with_this(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.call_host_async_with_this(id, this_value, args)
        } else {
            self.native.call_host_async_with_this(id, this_value, args)
        }
    }

    fn await_host(&self, pending_id: usize) -> Result<Value, VmErr> {
        self.native.await_host(pending_id)
    }
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
fn native_host_bridge(runtime: &NativeAddonRuntime) -> Rc<dyn HostBridge> {
    match runtime {
        NativeAddonRuntime::RustNodeApi(host) => host.clone(),
        NativeAddonRuntime::NodeSidecar(host) => host.clone(),
    }
}

fn expose_plugin_function(
    interpreter: &mut Interpreter,
    bridge: &PluginHostBridge,
    name: &str,
    function: impl Fn(Vec<Value>) -> Result<Value, VmErr> + 'static,
) -> Result<(), PluginHostError> {
    let id = bridge.register(Rc::new(function))?;
    interpreter
        .global
        .borrow_mut()
        .set(name, Value::host_function(name, id));
    Ok(())
}

fn install_fs_module(
    interpreter: &mut Interpreter,
    bridge: &PluginHostBridge,
    filesystem: Rc<PluginFileSystem>,
    module_ids: &mut Vec<String>,
    bridge_globals: &mut Vec<String>,
) {
    let read_fs = filesystem.clone();
    let read_name = "__napi_vm_plugin_fs_read";
    expose_plugin_function(interpreter, bridge, read_name, move |args| {
        let Some(Value::String(path)) = args.first() else {
            return Ok(guest_result(Err(permission_error(
                "fs.read requires a non-empty path string",
            ))));
        };
        Ok(guest_result(read_fs.read_text(path).map(Value::String)))
    })
    .expect("bootstrap callback registration is bounded");
    let write_fs = filesystem.clone();
    let write_name = "__napi_vm_plugin_fs_write";
    expose_plugin_function(interpreter, bridge, write_name, move |args| {
        let (Some(Value::String(path)), Some(Value::String(contents))) =
            (args.first(), args.get(1))
        else {
            return Ok(guest_result(Err(permission_error(
                "writeFileSync(path, contents): path and contents must be strings",
            ))));
        };
        Ok(guest_result(
            write_fs
                .write_text(path, contents)
                .map(|()| Value::Undefined),
        ))
    })
    .expect("bootstrap callback registration is bounded");
    let exists_fs = filesystem;
    let exists_name = "__napi_vm_plugin_fs_exists";
    expose_plugin_function(interpreter, bridge, exists_name, move |args| {
        let Some(Value::String(path)) = args.first() else {
            return Ok(guest_result(Err(permission_error(
                "fs.exists requires a non-empty path string",
            ))));
        };
        Ok(guest_result(exists_fs.exists(path).map(Value::Bool)))
    })
    .expect("bootstrap callback registration is bounded");
    bridge_globals.extend([read_name.into(), write_name.into(), exists_name.into()]);
    interpreter.define_module(
        "node:fs",
        r#"
const hostRead = __napi_vm_plugin_fs_read;
const hostWrite = __napi_vm_plugin_fs_write;
const hostExists = __napi_vm_plugin_fs_exists;
function unwrap(result) {
  if (result.ok) return result.value;
  const error = new Error(result.message);
  error.name = result.name;
  throw error;
}
export function readFileSync(path, encoding) {
  if (encoding !== "utf8" && encoding !== "utf-8") {
    throw new TypeError("the sandboxed node:fs facade supports UTF-8 text reads only");
  }
  return unwrap(hostRead(path));
}
export function writeFileSync(path, contents) {
  if (typeof contents !== "string") {
    throw new TypeError("writeFileSync(path, contents): contents must be a string");
  }
  unwrap(hostWrite(path, contents));
}
export function existsSync(path) {
  return unwrap(hostExists(path));
}
"#
        .into(),
    );
    module_ids.push("node:fs".into());
}

fn install_path_module(
    interpreter: &mut Interpreter,
    bridge: &PluginHostBridge,
    module_ids: &mut Vec<String>,
    bridge_globals: &mut Vec<String>,
) {
    let mut exports = Vec::new();
    for (operation, export_name) in [
        (0, "join"),
        (1, "normalize"),
        (2, "dirname"),
        (3, "basename"),
        (4, "extname"),
        (5, "resolve"),
        (6, "relative"),
        (7, "isAbsolute"),
    ] {
        let global_name = format!("__napi_vm_plugin_path_{operation}");
        expose_plugin_function(interpreter, bridge, &global_name, move |args| {
            let strings = args.iter().map(value_to_guest_string).collect::<Vec<_>>();
            Ok(path_operation(operation, &strings))
        })
        .expect("bootstrap callback registration is bounded");
        bridge_globals.push(global_name.clone());
        exports.push((export_name, global_name));
    }
    let mut source = String::new();
    for (export_name, global_name) in exports {
        source.push_str(&format!(
            "const __host_{export_name} = {global_name};\nexport function {export_name}(...parts) {{ return __host_{export_name}(...parts.map(String)); }}\n"
        ));
    }
    source.push_str(&format!(
        "export const sep = {};\n",
        serde_json::to_string(std::path::MAIN_SEPARATOR_STR).unwrap()
    ));
    interpreter.define_module("node:path", source);
    module_ids.push("node:path".into());
}

fn install_custom_capability(
    interpreter: &mut Interpreter,
    bridge: &PluginHostBridge,
    capability: &RustPluginCapability,
    module_ids: &mut Vec<String>,
    bridge_globals: &mut Vec<String>,
) -> Result<(), PluginHostError> {
    let mut source = String::new();
    for (index, (export_name, callback)) in capability.exports.iter().enumerate() {
        let global_name = format!(
            "__napi_vm_cap_{}_{}",
            sanitize_global(&capability.name),
            index
        );
        expose_plugin_function(interpreter, bridge, &global_name, {
            let callback = callback.clone();
            move |args| callback(args)
        })?;
        bridge_globals.push(global_name.clone());
        source.push_str(&format!(
            "const __host_{index} = {global_name};\nexport function {export_name}(...args) {{ return __host_{index}(...args); }}\n"
        ));
    }
    interpreter.define_module(&capability.name, source);
    module_ids.push(capability.name.clone());
    Ok(())
}

fn sanitize_global(value: &str) -> String {
    value.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn value_to_guest_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => crate::format::number_string(*value),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".into(),
        Value::Undefined => "undefined".into(),
        other => format!("{other:?}"),
    }
}

fn lifecycle_bootstrap(entry_id: &str) -> String {
    let entry = serde_json::to_string(entry_id).expect("module id is serializable");
    format!(
        r#"
import __Plugin from {entry};
const __pluginInstance = typeof __Plugin === "function" ? new __Plugin() : __Plugin;
function __plugin_describe() {{
  return {{ hasInstance: __pluginInstance !== undefined && __pluginInstance !== null }};
}}
function __plugin_onLoad(context) {{
  if (__pluginInstance && typeof __pluginInstance.onLoad === "function") return __pluginInstance.onLoad(context);
}}
function __plugin_onUnload(context) {{
  if (__pluginInstance && typeof __pluginInstance.onUnload === "function") return __pluginInstance.onUnload(context);
}}
function __plugin_onReload(context, previousState) {{
  if (__pluginInstance && typeof __pluginInstance.onReload === "function") return __pluginInstance.onReload(context, previousState);
  return __plugin_onLoad(context);
}}
undefined;
"#
    )
}

fn context_json(manifest: &RustPluginManifest, reason: Option<&str>) -> String {
    let mut context = serde_json::Map::new();
    context.insert("name".into(), JsonValue::String(manifest.name.clone()));
    context.insert(
        "version".into(),
        JsonValue::String(manifest.version.clone()),
    );
    if let Some(reason) = reason {
        context.insert("reason".into(), JsonValue::String(reason.into()));
    }
    serde_json::to_string(&JsonValue::Object(context)).expect("context is serializable")
}

fn invoke_json(
    interpreter: &mut Interpreter,
    expression: &str,
) -> Result<Option<JsonValue>, PluginHostError> {
    let source = format!(
        r#"await (async () => {{
  const value = await ({expression});
  if (value === undefined) return JSON.stringify({{ defined: false }});
  const serialized = JSON.stringify(value);
  if (typeof serialized !== "string") throw new TypeError("plugin lifecycle state must be JSON serializable");
  return JSON.stringify({{ defined: true, serialized }});
}})()"#
    );
    let result = interpreter.eval_source(&source)?;
    let Value::String(envelope) = &result else {
        return Err(PluginHostError::Load(
            "plugin lifecycle hook returned an invalid host result".into(),
        ));
    };
    let envelope: JsonValue = serde_json::from_str(envelope)
        .map_err(|error| PluginHostError::Load(format!("invalid lifecycle result: {error}")))?;
    if envelope.get("defined") == Some(&JsonValue::Bool(false)) {
        return Ok(None);
    }
    let serialized = envelope
        .get("serialized")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| PluginHostError::Load("lifecycle state is not serializable".into()))?;
    serde_json::from_str(serialized)
        .map(Some)
        .map_err(|error| PluginHostError::Load(format!("invalid lifecycle JSON: {error}")))
}

fn path_operation(operation: usize, parts: &[String]) -> Value {
    let sep = std::path::MAIN_SEPARATOR;
    let first = parts.first().map(String::as_str).unwrap_or("");
    match operation {
        0 => {
            let joined = parts
                .iter()
                .filter(|part| !part.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(&sep.to_string());
            Value::String(normalize_host_path(&joined))
        }
        1 => Value::String(normalize_host_path(first)),
        2 => Value::String(host_dirname(first)),
        3 => Value::String(host_basename(first, parts.get(1).map(String::as_str))),
        4 => Value::String(host_extname(first)),
        5 => Value::String(host_resolve(parts)),
        6 => Value::String(host_relative(
            first,
            parts.get(1).map(String::as_str).unwrap_or(""),
        )),
        7 => Value::Bool(Path::new(first).is_absolute()),
        _ => Value::Undefined,
    }
}

#[cfg(not(windows))]
fn normalize_host_path(path: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    let converted = path.to_owned();
    let absolute = converted.starts_with(sep);
    let trailing = converted.ends_with(sep);
    let mut segments = Vec::<String>::new();
    for part in converted.split(sep) {
        match part {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|value| value != "..") {
                    segments.pop();
                } else if !absolute {
                    segments.push("..".into());
                }
            }
            value => segments.push(value.into()),
        }
    }
    let mut result = segments.join(&sep.to_string());
    if absolute {
        result.insert(0, sep);
    }
    if result.is_empty() {
        result = if absolute {
            sep.to_string()
        } else {
            ".".into()
        };
    }
    if trailing && result != sep.to_string() && !result.ends_with(sep) {
        result.push(sep);
    }
    result
}

#[cfg(windows)]
fn normalize_host_path(path: &str) -> String {
    let sep = std::path::MAIN_SEPARATOR;
    let converted = path.replace('/', "\\");
    let trailing = converted.ends_with(sep);
    let mut prefix = String::new();
    let body_storage;
    let mut body = converted.as_str();
    let absolute;
    if let Some(rest) = body.strip_prefix("\\\\") {
        let mut pieces = rest.split('\\');
        let server = pieces.next().unwrap_or_default();
        let share = pieces.next().unwrap_or_default();
        if !server.is_empty() && !share.is_empty() {
            prefix = format!("\\\\{server}\\{share}");
            body_storage = pieces.collect::<Vec<_>>().join("\\");
            body = &body_storage;
        } else {
            body = rest;
        }
        absolute = true;
    } else if body.len() >= 2
        && body.as_bytes()[0].is_ascii_alphabetic()
        && body.as_bytes()[1] == b':'
    {
        prefix = body[..2].to_owned();
        body = &body[2..];
        absolute = body.starts_with(sep);
        body = body.trim_start_matches(sep);
    } else {
        absolute = body.starts_with(sep);
        body = body.trim_start_matches(sep);
    }
    let mut segments = Vec::<String>::new();
    for part in body.split(sep) {
        match part {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|value| value != "..") {
                    segments.pop();
                } else if !absolute {
                    segments.push("..".into());
                }
            }
            value => segments.push(value.into()),
        }
    }
    let joined = segments.join(&sep.to_string());
    let mut result = if absolute {
        if prefix.is_empty() {
            format!("{sep}{joined}")
        } else if joined.is_empty() {
            format!("{prefix}{sep}")
        } else {
            format!("{prefix}{sep}{joined}")
        }
    } else {
        format!("{prefix}{joined}")
    };
    if result.is_empty() {
        result = if absolute {
            sep.to_string()
        } else {
            ".".into()
        };
    }
    if trailing && result != sep.to_string() && !result.ends_with(sep) {
        result.push(sep);
    }
    result
}

fn host_dirname(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    #[cfg(windows)]
    {
        let normalized = path.replace('/', "\\");
        let trimmed = normalized.trim_end_matches('\\');
        if trimmed.is_empty() {
            return "\\".into();
        }
        if trimmed.len() == 2
            && trimmed.as_bytes()[0].is_ascii_alphabetic()
            && trimmed.as_bytes()[1] == b':'
        {
            return format!("{trimmed}\\");
        }
        return Path::new(trimmed)
            .parent()
            .map(|parent| {
                let value = parent.to_string_lossy().into_owned();
                if value.is_empty() { ".".into() } else { value }
            })
            .unwrap_or_else(|| "\\".into());
    }
    #[cfg(not(windows))]
    {
        if path.chars().all(|ch| ch == std::path::MAIN_SEPARATOR) {
            return std::path::MAIN_SEPARATOR.to_string();
        }
        let normalized = path.trim_end_matches(std::path::MAIN_SEPARATOR);
        let Some(index) = normalized.rfind(std::path::MAIN_SEPARATOR) else {
            return ".".into();
        };
        if index == 0 {
            std::path::MAIN_SEPARATOR.to_string()
        } else {
            normalized[..index].to_owned()
        }
    }
}

fn host_basename(path: &str, suffix: Option<&str>) -> String {
    #[cfg(windows)]
    let path = path.replace('/', "\\");
    #[cfg(windows)]
    let path = path.as_str();
    let trimmed = path.trim_end_matches(std::path::MAIN_SEPARATOR);
    let mut base = trimmed
        .rsplit(std::path::MAIN_SEPARATOR)
        .next()
        .unwrap_or("");
    if let Some(suffix) = suffix.filter(|suffix| !suffix.is_empty())
        && base.len() > suffix.len()
        && base.ends_with(suffix)
    {
        base = &base[..base.len() - suffix.len()];
    }
    base.to_owned()
}

fn host_extname(path: &str) -> String {
    let base = host_basename(path, None);
    if matches!(base.as_str(), "." | "..") {
        return String::new();
    }
    let Some(index) = base.rfind('.') else {
        return String::new();
    };
    if index == 0 {
        String::new()
    } else {
        base[index..].to_owned()
    }
}

fn host_resolve(parts: &[String]) -> String {
    let mut resolved = PathBuf::new();
    let mut found_absolute = false;
    for part in parts.iter().rev() {
        if part.is_empty() {
            continue;
        }
        let path = Path::new(part);
        if path.is_absolute() {
            resolved = path.to_path_buf();
            found_absolute = true;
            break;
        }
        resolved = path.join(resolved);
    }
    if !found_absolute {
        resolved = std::env::current_dir().unwrap_or_default().join(resolved);
    }
    normalize_host_path(&resolved.to_string_lossy())
}

fn host_relative(from: &str, to: &str) -> String {
    let from = host_resolve(&[from.to_owned()]);
    let to = host_resolve(&[to.to_owned()]);
    let from_parts: Vec<_> = Path::new(&from).components().collect();
    let to_parts: Vec<_> = Path::new(&to).components().collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = vec!["..".to_owned(); from_parts.len() - common];
    relative.extend(
        to_parts[common..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
    );
    relative.join(std::path::MAIN_SEPARATOR_STR)
}

#[cfg(test)]
mod tests {
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

    fn string_value(args: Vec<Value>) -> Result<Value, VmErr> {
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
const aliasedAddon = require("fixture-native");
const addon = require("fixture.node");
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
}
