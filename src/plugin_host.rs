//! Rust-native host orchestration for isolated JavaScript plugins.
//!
//! Plugin source remains guest JavaScript. This module owns `plugin.json`,
//! capability grants, filesystem policy, module registration and lifecycle;
//! it never evaluates plugin JavaScript with the host's `require()`.

mod bridge;
mod filesystem;
mod lifecycle;
mod manifest;
mod modules;
mod path_facade;
mod resolve;
#[cfg(test)]
mod tests;
mod util;

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

use bridge::*;
use filesystem::*;
use lifecycle::*;
use manifest::*;
use modules::*;
use path_facade::*;
use resolve::*;
use util::*;

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
            && options.native_addon_aliases.is_empty()
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
            let prefix = format!("./plugin:{}/", prepared.manifest.name);
            if let Some(relative) = name.strip_prefix(&prefix) {
                let path = prepared.root.join(relative);
                if let Ok(url) = url::Url::from_file_path(path) {
                    interpreter.define_module_file_url(name, url.into());
                }
            }
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
                options = match digest {
                    Some(digest) => options.allow_native_addon_with_sha256(&canonical, *digest),
                    None => options.allow_native_addon(&canonical),
                };
                canonical_addons.push(canonical);
            }
            for alias in &config.native_addon_aliases {
                let candidate = if alias.addon.is_absolute() {
                    alias.addon.clone()
                } else {
                    prepared.root.join(&alias.addon)
                };
                let canonical = fs::canonicalize(&candidate).map_err(|error| {
                    PluginHostError::Load(format!("cannot resolve aliased native addon: {error}"))
                })?;
                if !canonical.starts_with(&prepared.root) {
                    return Err(PluginHostError::Load(
                        "native addon alias target is outside plugin root".into(),
                    ));
                }
                options = options.allow_native_addon_alias(&alias.request, &canonical);
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

/// Per-plugin explicit Node-API addon allowlist. Digests are optional:
/// without one the selected binary is pinned to its contents at setup,
/// with one it must additionally match trusted host metadata. Either way
/// native code is not sandboxed.
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
#[derive(Clone, Debug)]
pub struct RustPluginNapiOptions {
    max_napi_version: u32,
    allowed_addons: Vec<(PathBuf, Option<[u8; 32]>)>,
    native_addon_aliases: Vec<RustPluginNapiAddonAlias>,
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
struct RustPluginNapiAddonAlias {
    request: String,
    addon: PathBuf,
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
            native_addon_aliases: Vec::new(),
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
    /// Allowlist one `.node` binary, pinning its contents at setup. The
    /// path may be absolute or relative to the plugin root; it is
    /// canonicalized and contained before anything loads.
    pub fn allow_addon(mut self, path: impl Into<PathBuf>) -> Self {
        self.allowed_addons.push((path.into(), None));
        self
    }

    /// Allowlist one `.node` binary only when it matches a digest from
    /// trusted host metadata, checked at setup and again before loading.
    pub fn allow_addon_with_sha256(mut self, path: impl Into<PathBuf>, digest: [u8; 32]) -> Self {
        self.allowed_addons.push((path.into(), Some(digest)));
        self
    }

    /// Expose an allowlisted `.node` binary through a bare guest
    /// `require()` request. The target must have been added with
    /// [`Self::allow_addon`] or [`Self::allow_addon_with_sha256`]; the
    /// alias replaces any JavaScript entry for that request, so guests
    /// load the addon without executing package loader code. Only bare
    /// package names are accepted.
    pub fn allow_addon_alias(
        mut self,
        request: impl Into<String>,
        addon: impl Into<PathBuf>,
    ) -> Self {
        self.native_addon_aliases.push(RustPluginNapiAddonAlias {
            request: request.into(),
            addon: addon.into(),
        });
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
