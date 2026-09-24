//! A deliberately host-configured CommonJS loader.
//!
//! Guest `require()` always goes through this interface. JavaScript and JSON
//! source are parsed/executed by napi-vm; native addon files are handed to a
//! host supplied provider because a `.node` library needs a Node-API runtime
//! and cannot be made executable by filesystem resolution alone.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

use crate::error::VmErr;
use crate::value::Value;

mod exports;
mod node_gyp_build;
mod prebuild;
#[cfg(test)]
mod tests;

use exports::*;
use node_gyp_build::*;
use prebuild::*;
/// Maximum guest source file size accepted by [`FileCommonJsLoader`].
pub const MAX_COMMONJS_SOURCE_BYTES: usize = 16 * 1024 * 1024;

/// The source format returned by a CommonJS resolver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommonJsModuleFormat {
    JavaScript,
    Json,
    NativeAddon,
    /// A runtime-provided helper or installed guest-module namespace without
    /// a filesystem source file.
    RuntimeBuiltin,
}

/// One resolved CommonJS module. `id` is the stable cache key; `filename` is
/// passed to guest `__filename`/`__dirname` and to the native addon provider.
#[derive(Clone, Debug)]
pub struct ResolvedCommonJsModule {
    pub id: String,
    pub filename: String,
    pub format: CommonJsModuleFormat,
    /// The exact source text for JavaScript and JSON. Native addons and
    /// runtime-provided modules have no source text and are represented by
    /// `None`.
    pub source: Option<String>,
}

/// Host policy and source-resolution boundary for guest `require()`.
///
/// Implementations must not execute JavaScript with a host `require()` or
/// `import()`. JavaScript source returned here is always evaluated by the VM.
pub trait CommonJsModuleLoader {
    /// Resolve one request relative to the requiring module, or to the
    /// configured application entry when `parent` is `None`.
    fn resolve(&self, request: &str, parent: Option<&str>)
    -> Result<ResolvedCommonJsModule, VmErr>;

    /// Load a resolved `.node` addon. The default fails closed. An
    /// implementation must supply a real Node-API host/provider and should
    /// treat addon code as trusted host code, outside the guest sandbox.
    fn load_native_addon(&self, module: &ResolvedCommonJsModule) -> Result<Value, VmErr> {
        Err(VmErr::Msg(format!(
            "native addon loading is not configured for {}",
            module.filename
        )))
    }

    /// Load a native addon with the provisional CommonJS `exports` object that
    /// was published before initialization. Providers that cannot expose this
    /// object to their initializer keep their existing behavior through the
    /// default implementation.
    fn load_native_addon_with_exports(
        &self,
        module: &ResolvedCommonJsModule,
        _exports: Value,
    ) -> Result<Value, VmErr> {
        self.load_native_addon(module)
    }

    /// Load a native addon while allowing the host to request guest callbacks
    /// through the interpreter's paused-call checkpoint. Providers without
    /// synchronous callback support keep their existing behavior.
    fn load_native_addon_with_callback_handler(
        &self,
        module: &ResolvedCommonJsModule,
        exports: Value,
        _callback_handler: &mut dyn FnMut(crate::host::HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.load_native_addon_with_exports(module, exports)
    }

    /// Resolve a trusted Node-API prebuild for a guest `node-gyp-build(dir)`
    /// call. The default loader has no platform prebuild policy.
    fn resolve_node_api_prebuild_for_package(
        &self,
        _package_root: &Path,
    ) -> Result<ResolvedCommonJsModule, VmErr> {
        Err(VmErr::Msg(
            "Node-API prebuild resolution is not configured for this CommonJS loader".into(),
        ))
    }
}

/// Allowlisted native addon provider hook.
///
/// Implementors bridge an addon through an actual Node-API implementation or
/// another explicitly chosen host runtime. Merely opening the shared library
/// is insufficient: its initializer expects a valid `napi_env`.
pub trait NativeAddonLoader {
    /// Check whether this provider can load `filename` without initializing
    /// the addon. Providers with no separate preflight operation fail clearly.
    fn preflight_addon(&self, filename: &Path) -> Result<(), VmErr> {
        Err(VmErr::Msg(format!(
            "native addon preflight is not available for {}",
            filename.display()
        )))
    }

    /// Initialize the addon at `filename` and return its `module.exports`.
    fn load(&self, filename: &Path) -> Result<Value, VmErr>;

    /// Initialize the addon with the provisional `exports` object that
    /// CommonJS placed in its cache before initialization. Existing providers
    /// can continue implementing only `load`; Node-API hosts should override
    /// this method to preserve Node's initialization and cycle semantics.
    fn load_with_exports(&self, filename: &Path, _exports: Value) -> Result<Value, VmErr> {
        self.load(filename)
    }

    /// Initialize an addon with access to the active interpreter callback
    /// checkpoint. The default path does not request guest callbacks.
    fn load_with_callback_handler(
        &self,
        filename: &Path,
        exports: Value,
        _callback_handler: &mut dyn FnMut(crate::host::HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.load_with_exports(filename, exports)
    }
}

/// Filesystem resolver for JavaScript, JSON, and `.node` CommonJS modules.
///
/// Resolution is limited to configured roots. Symlinks are canonicalized and
/// rejected when their targets leave those roots. Built-in modules are not
/// loaded implicitly. Package `exports` supports exact and wildcard subpaths
/// and the `require`, `node`, `node-addons`, and `default` conditions. The
/// `node-addons` condition is active only when a native addon provider is
/// configured, matching Node's `--no-addons` behavior for runtimes that omit
/// native addon support.
pub struct FileCommonJsLoader {
    roots: Vec<PathBuf>,
    native_addons: Option<Rc<dyn NativeAddonLoader>>,
    allowed_native_addons: HashMap<PathBuf, [u8; 32]>,
    native_addon_aliases: HashMap<String, PathBuf>,
    node_gyp_build_compat: bool,
    node_gyp_build_prebuilds_only: Option<bool>,
    node_gyp_build_exec_path: Option<PathBuf>,
}

impl std::fmt::Debug for FileCommonJsLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileCommonJsLoader")
            .field("roots", &self.roots)
            .field("native_addons", &self.native_addons.is_some())
            .field("allowed_native_addons", &self.allowed_native_addons)
            .field("native_addon_aliases", &self.native_addon_aliases)
            .field("node_gyp_build_compat", &self.node_gyp_build_compat)
            .field(
                "node_gyp_build_prebuilds_only",
                &self.node_gyp_build_prebuilds_only,
            )
            .field("node_gyp_build_exec_path", &self.node_gyp_build_exec_path)
            .finish()
    }
}

impl FileCommonJsLoader {
    /// Create a resolver restricted to `roots`. Roots must exist and are
    /// canonicalized at construction time.
    pub fn new<I, P>(roots: I) -> Result<Self, VmErr>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut canonical_roots = Vec::new();
        for root in roots {
            let path = fs::canonicalize(root.as_ref()).map_err(|error| {
                VmErr::Msg(format!(
                    "cannot use CommonJS root {}: {error}",
                    root.as_ref().display()
                ))
            })?;
            if !path.is_dir() {
                return Err(VmErr::Msg(format!(
                    "CommonJS root is not a directory: {}",
                    path.display()
                )));
            }
            if !canonical_roots.iter().any(|existing| existing == &path) {
                canonical_roots.push(path);
            }
        }
        if canonical_roots.is_empty() {
            return Err(VmErr::Msg(
                "at least one CommonJS module root is required".to_string(),
            ));
        }
        Ok(Self {
            roots: canonical_roots,
            native_addons: None,
            allowed_native_addons: HashMap::new(),
            native_addon_aliases: HashMap::new(),
            node_gyp_build_compat: false,
            node_gyp_build_prebuilds_only: None,
            node_gyp_build_exec_path: None,
        })
    }

    /// Attach an explicitly trusted native addon provider. Without this, a
    /// resolved `.node` file fails with a clear configuration error.
    pub fn with_native_addon_loader(mut self, loader: Rc<dyn NativeAddonLoader>) -> Self {
        self.native_addons = Some(loader);
        self
    }

    /// Provide the Node-API subset of `node-gyp-build` used by package entry
    /// points: callable loading plus `.path()` and `.resolve()` selection.
    /// Binary loading still passes through the normal root and digest policy.
    pub fn with_node_gyp_build_compat(mut self) -> Self {
        self.node_gyp_build_compat = true;
        self
    }

    /// Match `PREBUILDS_ONLY` when choosing a package prebuild. When unset,
    /// the loader follows the host process environment variable.
    pub fn with_node_gyp_build_prebuilds_only(mut self, enabled: bool) -> Self {
        self.node_gyp_build_prebuilds_only = Some(enabled);
        self
    }

    /// Set the executable path used for `node-gyp-build`'s nearby-prebuild
    /// fallback. By default, the embedding process's current executable is
    /// used, matching `process.execPath` in a runtime embedded in that app.
    pub fn with_node_gyp_build_exec_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.node_gyp_build_exec_path = Some(path.into());
        self
    }

    /// Allow one specific `.node` binary and pin its SHA-256 digest. Native
    /// addons are never enabled for every package under a root by default;
    /// each binary must be opted in and remain byte-for-byte unchanged.
    pub fn allow_native_addon(mut self, path: impl AsRef<Path>) -> Result<Self, VmErr> {
        self.register_native_addon(path, None)?;
        Ok(self)
    }

    /// Allow one `.node` binary only when its contents match a SHA-256 digest
    /// supplied by trusted host metadata, such as a signed build manifest.
    /// The digest is checked now and again immediately before loading.
    pub fn allow_native_addon_with_sha256(
        mut self,
        path: impl AsRef<Path>,
        expected_sha256: [u8; 32],
    ) -> Result<Self, VmErr> {
        self.register_native_addon(path, Some(expected_sha256))?;
        Ok(self)
    }

    fn register_native_addon(
        &mut self,
        path: impl AsRef<Path>,
        expected_sha256: Option<[u8; 32]>,
    ) -> Result<(), VmErr> {
        let canonical = fs::canonicalize(path.as_ref()).map_err(|error| {
            VmErr::Msg(format!(
                "cannot allow native addon {}: {error}",
                path.as_ref().display()
            ))
        })?;
        if !self.in_roots(&canonical) {
            return Err(VmErr::Msg(format!(
                "native addon escapes configured roots: {}",
                canonical.display()
            )));
        }
        if canonical.extension().and_then(|ext| ext.to_str()) != Some("node") {
            return Err(VmErr::Msg(format!(
                "native addon allowlist entries must use the .node extension: {}",
                canonical.display()
            )));
        }
        let actual_sha256 = sha256_file(&canonical).map_err(|error| {
            VmErr::Msg(format!(
                "cannot pin native addon {}: {error}",
                canonical.display()
            ))
        })?;
        let digest = expected_sha256.unwrap_or(actual_sha256);
        if actual_sha256 != digest {
            return Err(VmErr::Msg(format!(
                "native addon integrity check failed while configuring: {}",
                canonical.display()
            )));
        }
        self.allowed_native_addons.insert(canonical, digest);
        Ok(())
    }

    /// The canonical roots this loader may read from.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Resolve a Node-API addon in a package's `build/Release`, `build/Debug`,
    /// or `prebuilds/<platform>-<arch>` directory. Prebuild selection accepts
    /// N-API-tagged binaries only; Node ABI and libuv-specific builds are not
    /// compatible with this host's declared ABI boundary.
    pub fn resolve_node_api_prebuild(
        &self,
        package_root: impl AsRef<Path>,
    ) -> Result<ResolvedCommonJsModule, VmErr> {
        let package_root = fs::canonicalize(package_root.as_ref()).map_err(|error| {
            VmErr::Msg(format!(
                "cannot resolve native package {}: {error}",
                package_root.as_ref().display()
            ))
        })?;
        if !package_root.is_dir() || !self.in_roots(&package_root) {
            return Err(VmErr::Msg(format!(
                "native package root is not a directory inside configured roots: {}",
                package_root.display()
            )));
        }
        let package_root = self.node_gyp_build_package_root(&package_root)?;
        let target = NodeApiPrebuildTarget::current();
        let prebuilds_only = self.node_gyp_build_prebuilds_only.unwrap_or_else(|| {
            std::env::var("PREBUILDS_ONLY").is_ok_and(|value| !value.is_empty())
        });
        let nearby_root = self
            .node_gyp_build_exec_path
            .clone()
            .or_else(|| std::env::current_exe().ok())
            .and_then(|path| path.parent().map(Path::to_path_buf));
        let candidate = select_node_api_prebuild(&package_root, &target, prebuilds_only)
            .or_else(|| {
                nearby_root
                    .as_deref()
                    .filter(|root| *root != package_root.as_path())
                    .and_then(|root| select_node_api_prebuild(root, &target, prebuilds_only))
            })
            .ok_or_else(|| {
                VmErr::Msg(format!(
                    "no compatible Node-API prebuild found for {}-{} in {}{}",
                    target.platform,
                    target.architecture,
                    package_root.display(),
                    nearby_root
                        .as_deref()
                        .filter(|root| *root != package_root.as_path())
                        .map(|root| format!(" or {}", root.display()))
                        .unwrap_or_default()
                ))
            })?;
        let candidate = self.canonical_file(&candidate)?.ok_or_else(|| {
            VmErr::Msg(format!(
                "selected Node-API prebuild is missing: {}",
                candidate.display()
            ))
        })?;
        self.resolved_file(candidate)
    }

    fn node_gyp_build_package_root(&self, package_root: &Path) -> Result<PathBuf, VmErr> {
        let manifest = fs::read(package_root.join("package.json"));
        let package_name = manifest
            .ok()
            .and_then(|source| serde_json::from_slice::<JsonValue>(&source).ok())
            .and_then(|manifest| manifest.get("name")?.as_str().map(str::to_owned));
        let override_path = package_name
            .as_deref()
            .map(node_gyp_build_prebuild_override_variable)
            .and_then(std::env::var_os)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        self.node_gyp_build_package_root_with_override(
            package_root,
            package_name.as_deref(),
            override_path,
        )
    }

    fn node_gyp_build_package_root_with_override(
        &self,
        package_root: &Path,
        package_name: Option<&str>,
        override_path: Option<PathBuf>,
    ) -> Result<PathBuf, VmErr> {
        let Some(override_path) = override_path else {
            return Ok(package_root.to_path_buf());
        };
        let override_path = fs::canonicalize(&override_path).map_err(|error| {
            VmErr::Msg(format!(
                "cannot resolve node-gyp-build prebuild override for {}: {error}",
                package_name.unwrap_or("unknown package")
            ))
        })?;
        if !override_path.is_dir() || !self.in_roots(&override_path) {
            return Err(VmErr::Msg(format!(
                "node-gyp-build prebuild override for {} is not a directory inside configured roots: {}",
                package_name.unwrap_or("unknown package"),
                override_path.display()
            )));
        }
        Ok(override_path)
    }

    /// Map a bare package request to a prebuild already selected and added to
    /// this loader's native-addon allowlist. This lets a desktop host expose a
    /// package's native entry through ordinary `require('package')`.
    pub fn with_native_addon_alias(
        mut self,
        request: impl Into<String>,
        addon_path: impl AsRef<Path>,
    ) -> Result<Self, VmErr> {
        let request = request.into();
        let (package_name, subpath) = split_package_request(&request)?;
        if package_name != request || !subpath.is_empty() {
            return Err(VmErr::Msg(format!(
                "native addon alias must be a bare package request: {request}"
            )));
        }
        let addon_path = fs::canonicalize(addon_path.as_ref()).map_err(|error| {
            VmErr::Msg(format!(
                "cannot resolve aliased native addon {}: {error}",
                addon_path.as_ref().display()
            ))
        })?;
        if addon_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("node")
        {
            return Err(VmErr::Msg(format!(
                "native addon alias target must use the .node extension: {}",
                addon_path.display()
            )));
        }
        if !self.allowed_native_addons.contains_key(&addon_path) {
            return Err(VmErr::Msg(format!(
                "native addon alias target is not allowlisted: {}",
                addon_path.display()
            )));
        }
        if !self.in_roots(&addon_path) {
            return Err(VmErr::Msg(format!(
                "native addon alias target escapes configured roots: {}",
                addon_path.display()
            )));
        }
        if self
            .native_addon_aliases
            .insert(request.clone(), addon_path)
            .is_some()
        {
            return Err(VmErr::Msg(format!(
                "native addon alias is already configured: {request}"
            )));
        }
        Ok(self)
    }

    pub(super) fn allowed_native_addon_digests(&self) -> &HashMap<PathBuf, [u8; 32]> {
        &self.allowed_native_addons
    }

    fn in_roots(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }

    fn canonical_file(&self, path: &Path) -> Result<Option<PathBuf>, VmErr> {
        if !path.is_file() {
            return Ok(None);
        }
        let canonical = fs::canonicalize(path)
            .map_err(|error| VmErr::Msg(format!("cannot resolve {}: {error}", path.display())))?;
        if !self.in_roots(&canonical) {
            return Err(VmErr::Msg(format!(
                "CommonJS module escapes configured roots: {}",
                path.display()
            )));
        }
        Ok(Some(canonical))
    }

    fn resolve_path(&self, path: &Path, depth: usize) -> Result<Option<PathBuf>, VmErr> {
        if depth > 8 {
            return Err(VmErr::Msg(
                "CommonJS package entry resolution exceeded its depth limit".to_string(),
            ));
        }
        if let Some(file) = self.canonical_file(path)? {
            return Ok(Some(file));
        }

        if path.is_dir() {
            let package_json = path.join("package.json");
            if package_json.is_file() {
                let package = read_package_json(&package_json)?;
                let entry = if let Some(exports) = package.get("exports") {
                    exports_target(exports, ".", self.native_addons.is_some())?.ok_or_else(
                        || {
                            VmErr::Msg(format!(
                                "package does not export its root entry: {}",
                                path.display()
                            ))
                        },
                    )?
                } else {
                    package
                        .get("main")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("index")
                        .to_string()
                };
                let entry_path = path.join(entry);
                if let Some(found) = self.resolve_path(&entry_path, depth + 1)? {
                    return Ok(Some(found));
                }
            }
            for name in ["index.js", "index.cjs", "index.json", "index.node"] {
                if let Some(found) = self.canonical_file(&path.join(name))? {
                    return Ok(Some(found));
                }
            }
            return Ok(None);
        }

        if path.extension().is_none() {
            for ext in ["js", "cjs", "json", "node"] {
                let candidate = path.with_extension(ext);
                if let Some(found) = self.canonical_file(&candidate)? {
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    }

    fn package_scope(&self, parent: &str) -> Result<Option<(PathBuf, JsonValue)>, VmErr> {
        let parent_path = Path::new(parent);
        if !parent_path.is_absolute() {
            return Ok(None);
        }
        let canonical_parent = fs::canonicalize(parent_path)
            .map_err(|error| VmErr::Msg(format!("invalid requiring module {parent}: {error}")))?;
        let mut directory = canonical_parent.parent();
        while let Some(current) = directory {
            if !self.in_roots(current) {
                break;
            }
            let package_json = current.join("package.json");
            if package_json.is_file() {
                return Ok(Some((
                    current.to_path_buf(),
                    read_package_json(&package_json)?,
                )));
            }
            directory = current.parent();
        }
        Ok(None)
    }

    fn resolve_package_export(
        &self,
        package_root: &Path,
        package_name: &str,
        subpath: &str,
        exports: &JsonValue,
    ) -> Result<PathBuf, VmErr> {
        let export_key = if subpath.is_empty() {
            ".".to_string()
        } else {
            format!("./{subpath}")
        };
        let target = exports_target(exports, &export_key, self.native_addons.is_some())?
            .ok_or_else(|| {
                VmErr::Msg(format!(
                    "package {package_name} does not export subpath {export_key}"
                ))
            })?;
        let target_path = package_root.join(target);
        let found = self
            .resolve_path(&target_path, 0)?
            .ok_or_else(|| VmErr::Msg(format!("Cannot find module '{package_name}'")))?;
        if !found.starts_with(package_root) {
            return Err(VmErr::Msg(format!(
                "package exports target escapes package root: {}",
                found.display()
            )));
        }
        Ok(found)
    }

    fn resolve_package_import(
        &self,
        request: &str,
        parent: Option<&str>,
    ) -> Result<PathBuf, VmErr> {
        if request == "#" || request.starts_with("#/") {
            return Err(VmErr::Msg(format!(
                "invalid package import specifier '{request}'"
            )));
        }
        let parent = parent.ok_or_else(|| {
            VmErr::Msg(format!(
                "package import '{request}' requires a CommonJS parent module"
            ))
        })?;
        let (package_root, package) = self
            .package_scope(parent)?
            .ok_or_else(|| VmErr::Msg(format!("Cannot find package import '{request}'")))?;
        let imports = package
            .get("imports")
            .ok_or_else(|| VmErr::Msg(format!("package does not define import '{request}'")))?;
        match imports_target(imports, request, self.native_addons.is_some())? {
            ImportTarget::Relative(target) => {
                let found = self
                    .resolve_path(&package_root.join(target), 0)?
                    .ok_or_else(|| VmErr::Msg(format!("Cannot find package import '{request}'")))?;
                if !found.starts_with(&package_root) {
                    return Err(VmErr::Msg(format!(
                        "package import target escapes package root: {}",
                        found.display()
                    )));
                }
                Ok(found)
            }
            ImportTarget::External(specifier) => self.resolve_request(&specifier, Some(parent)),
        }
    }

    fn package_request(&self, request: &str, parent: Option<&str>) -> Result<PathBuf, VmErr> {
        let (package_name, subpath) = split_package_request(request)?;
        if let Some(parent) = parent
            && let Some((package_root, package)) = self.package_scope(parent)?
            && package.get("name").and_then(JsonValue::as_str) == Some(package_name.as_str())
            && let Some(exports) = package.get("exports")
        {
            return self.resolve_package_export(&package_root, &package_name, &subpath, exports);
        }

        let mut search_dirs = Vec::new();
        if let Some(parent) = parent {
            let parent_path = Path::new(parent);
            if parent_path.is_absolute() {
                let canonical_parent = fs::canonicalize(parent_path).map_err(|error| {
                    VmErr::Msg(format!("invalid requiring module {parent}: {error}"))
                })?;
                let mut dir = canonical_parent.parent();
                while let Some(current) = dir {
                    if !self.in_roots(current) {
                        break;
                    }
                    search_dirs.push(current.to_path_buf());
                    dir = current.parent();
                }
            }
        }
        for root in &self.roots {
            if !search_dirs.contains(root) {
                search_dirs.push(root.clone());
            }
        }

        for directory in search_dirs {
            let package_root = directory.join("node_modules").join(&package_name);
            if !package_root.is_dir() {
                continue;
            }
            let package_root = fs::canonicalize(&package_root).map_err(|error| {
                VmErr::Msg(format!("cannot resolve package {package_name}: {error}"))
            })?;
            if !self.in_roots(&package_root) {
                return Err(VmErr::Msg(format!(
                    "package {package_name} escapes configured roots"
                )));
            }
            let package_json_path = package_root.join("package.json");
            let package = if package_json_path.is_file() {
                read_package_json(&package_json_path)?
            } else {
                JsonValue::Null
            };

            let target = if let Some(exports) = package.get("exports") {
                return self.resolve_package_export(
                    &package_root,
                    &package_name,
                    &subpath,
                    exports,
                );
            } else if subpath.is_empty() {
                package_root.join(
                    package
                        .get("main")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("index"),
                )
            } else {
                package_root.join(&subpath)
            };
            if let Some(found) = self.resolve_path(&target, 0)? {
                return Ok(found);
            }
        }
        Err(VmErr::Msg(format!("Cannot find module '{request}'")))
    }

    fn resolve_request(&self, request: &str, parent: Option<&str>) -> Result<PathBuf, VmErr> {
        if request.is_empty() || request.starts_with("node:") {
            return Err(VmErr::Msg(format!(
                "Cannot find module '{request}' (host built-ins are not enabled)"
            )));
        }
        if let Some(addon) = self.native_addon_aliases.get(request) {
            return Ok(addon.clone());
        }
        if request.starts_with('#') {
            return self.resolve_package_import(request, parent);
        }
        let request_path = Path::new(request);
        let candidate = if request_path.is_absolute() {
            request_path.to_path_buf()
        } else if request.starts_with("./")
            || request.starts_with("../")
            || request == "."
            || request == ".."
        {
            if let Some(parent) = parent {
                Path::new(parent)
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(request_path)
            } else {
                self.roots[0].join(request_path)
            }
        } else {
            return self.package_request(request, parent);
        };
        if let Some(found) = self.resolve_path(&candidate, 0)? {
            return Ok(found);
        }
        Err(VmErr::Msg(format!("Cannot find module '{request}'")))
    }

    fn resolved_file(&self, path: PathBuf) -> Result<ResolvedCommonJsModule, VmErr> {
        if !self.in_roots(&path) {
            return Err(VmErr::Msg(format!(
                "CommonJS module escapes configured roots: {}",
                path.display()
            )));
        }
        let filename = path.to_string_lossy().into_owned();
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let format = match extension.as_str() {
            "js" | "cjs" => CommonJsModuleFormat::JavaScript,
            "json" => CommonJsModuleFormat::Json,
            "node" => CommonJsModuleFormat::NativeAddon,
            "mjs" => {
                return Err(VmErr::Msg(format!(
                    "ERR_REQUIRE_ESM: CommonJS require cannot load {}",
                    path.display()
                )));
            }
            _ => {
                return Err(VmErr::Msg(format!(
                    "unsupported CommonJS module format: {}",
                    path.display()
                )));
            }
        };
        let source = if format == CommonJsModuleFormat::NativeAddon {
            None
        } else {
            let metadata = fs::metadata(&path)
                .map_err(|error| VmErr::Msg(format!("cannot stat {}: {error}", path.display())))?;
            if metadata.len() > MAX_COMMONJS_SOURCE_BYTES as u64 {
                return Err(VmErr::Msg(format!(
                    "CommonJS source exceeds the {} byte limit: {}",
                    MAX_COMMONJS_SOURCE_BYTES,
                    path.display()
                )));
            }
            Some(
                fs::read_to_string(&path).map_err(|error| {
                    VmErr::Msg(format!("cannot read {}: {error}", path.display()))
                })?,
            )
        };
        Ok(ResolvedCommonJsModule {
            id: filename.clone(),
            filename,
            format,
            source,
        })
    }
}

impl CommonJsModuleLoader for FileCommonJsLoader {
    fn resolve(
        &self,
        request: &str,
        parent: Option<&str>,
    ) -> Result<ResolvedCommonJsModule, VmErr> {
        if self.node_gyp_build_compat && request == "node-gyp-build" {
            let (id, filename) = match self.resolve_request(request, parent) {
                Ok(path) => {
                    let filename = path.to_string_lossy().into_owned();
                    (filename.clone(), filename)
                }
                Err(error) if error.to_string() == "Cannot find module 'node-gyp-build'" => {
                    let builtin = "napi-vm:node-gyp-build".to_string();
                    (builtin.clone(), builtin)
                }
                Err(error) => return Err(error),
            };
            return Ok(ResolvedCommonJsModule {
                id,
                filename,
                format: CommonJsModuleFormat::RuntimeBuiltin,
                source: None,
            });
        }
        self.resolved_file(self.resolve_request(request, parent)?)
    }

    fn resolve_node_api_prebuild_for_package(
        &self,
        package_root: &Path,
    ) -> Result<ResolvedCommonJsModule, VmErr> {
        self.resolve_node_api_prebuild(package_root)
    }

    fn load_native_addon(&self, module: &ResolvedCommonJsModule) -> Result<Value, VmErr> {
        self.load_native_addon_with_exports(module, Value::object(Vec::new()))
    }

    fn load_native_addon_with_exports(
        &self,
        module: &ResolvedCommonJsModule,
        exports: Value,
    ) -> Result<Value, VmErr> {
        self.load_native_addon_impl(module, exports, None)
    }

    fn load_native_addon_with_callback_handler(
        &self,
        module: &ResolvedCommonJsModule,
        exports: Value,
        callback_handler: &mut dyn FnMut(crate::host::HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.load_native_addon_impl(module, exports, Some(callback_handler))
    }
}

impl FileCommonJsLoader {
    fn load_native_addon_impl(
        &self,
        module: &ResolvedCommonJsModule,
        exports: Value,
        callback_handler: Option<&mut dyn FnMut(crate::host::HostCallback) -> Result<Value, VmErr>>,
    ) -> Result<Value, VmErr> {
        let path = fs::canonicalize(&module.filename).map_err(|error| {
            VmErr::Msg(format!(
                "cannot verify native addon {}: {error}",
                module.filename
            ))
        })?;
        let expected_digest = self.allowed_native_addons.get(&path).ok_or_else(|| {
            VmErr::Msg(format!(
                "native addon is not allowlisted: {}",
                path.display()
            ))
        })?;
        let actual_digest = sha256_file(&path).map_err(|error| {
            VmErr::Msg(format!(
                "cannot verify native addon {}: {error}",
                path.display()
            ))
        })?;
        if &actual_digest != expected_digest {
            return Err(VmErr::Msg(format!(
                "native addon integrity check failed: {}",
                path.display()
            )));
        }
        let loader = self.native_addons.as_ref().ok_or_else(|| {
            VmErr::Msg(format!(
                "native addon loading is not configured for {}",
                module.filename
            ))
        })?;
        match callback_handler {
            Some(callback_handler) => {
                loader.load_with_callback_handler(&path, exports, callback_handler)
            }
            None => loader.load_with_exports(&path, exports),
        }
    }
}

pub(super) fn sha256_file(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(digest.finalize().into())
}

pub(super) struct CommonJsCacheEntry {
    pub(super) exports: Value,
    pub(super) module: Option<Value>,
}

pub(super) fn require_module(
    interp: &mut crate::interpreter::Interpreter,
    request: &str,
    parent: Option<&str>,
) -> Result<Value, VmErr> {
    let loader = interp.commonjs_loader.clone().ok_or_else(|| {
        VmErr::Msg("require is disabled: configure a host CommonJS module loader first".to_string())
    })?;
    let module = resolve_guest_module_builtin(interp, request)
        .map(Ok)
        .unwrap_or_else(|| loader.resolve(request, parent))?;
    let cached = interp
        .commonjs_cache
        .borrow()
        .get(&module.id)
        .map(|cached| {
            cached
                .module
                .as_ref()
                .and_then(|module| module.get_prop("exports"))
                .unwrap_or_else(|| cached.exports.clone())
        });
    if let Some(cached) = cached {
        return Ok(cached);
    }

    match module.format {
        CommonJsModuleFormat::NativeAddon => {
            // Node inserts the CommonJS module in its cache before running a
            // native initializer. Preserve that partial-export visibility for
            // addons that re-enter module loading during initialization.
            let initial_exports = Value::object(Vec::new());
            interp.commonjs_cache.borrow_mut().insert(
                module.id.clone(),
                CommonJsCacheEntry {
                    exports: initial_exports.clone(),
                    module: None,
                },
            );
            let loaded = loader.load_native_addon_with_callback_handler(
                &module,
                initial_exports,
                &mut |callback| interp.run_host_callback(callback),
            );
            match loaded {
                Ok(exports) => {
                    if let Some(entry) = interp.commonjs_cache.borrow_mut().get_mut(&module.id) {
                        entry.exports = exports.clone();
                    }
                    Ok(exports)
                }
                Err(error) => {
                    // CommonJS removes a module whose initializer throws so a
                    // later require can retry it.
                    interp.commonjs_cache.borrow_mut().remove(&module.id);
                    Err(error)
                }
            }
        }
        CommonJsModuleFormat::Json => {
            let source = module.source.as_deref().ok_or_else(|| {
                VmErr::Msg(format!("JSON module has no source: {}", module.filename))
            })?;
            let parsed: JsonValue = serde_json::from_str(source).map_err(|error| {
                VmErr::Msg(format!("invalid JSON in {}: {error}", module.filename))
            })?;
            let exports = json_to_guest(parsed)?;
            interp.commonjs_cache.borrow_mut().insert(
                module.id,
                CommonJsCacheEntry {
                    exports: exports.clone(),
                    module: None,
                },
            );
            Ok(exports)
        }
        CommonJsModuleFormat::JavaScript => evaluate_commonjs_source(interp, module),
        CommonJsModuleFormat::RuntimeBuiltin => {
            let exports = if let Some(module_name) = module.id.strip_prefix("napi-vm:guest-module:")
            {
                if !interp.ensure_module(module_name)? {
                    return Err(VmErr::Msg(format!(
                        "CommonJS builtin module is not installed: {}",
                        module.filename
                    )));
                }
                let module_record = interp.module(module_name).ok_or_else(|| {
                    VmErr::Msg(format!(
                        "CommonJS builtin module has no exports: {}",
                        module.filename
                    ))
                })?;
                crate::interpreter::Interpreter::namespace_object(&module_record)?
            } else {
                make_node_gyp_build(interp)?
            };
            interp.commonjs_cache.borrow_mut().insert(
                module.id,
                CommonJsCacheEntry {
                    exports: exports.clone(),
                    module: None,
                },
            );
            Ok(exports)
        }
    }
}

fn evaluate_commonjs_source(
    interp: &mut crate::interpreter::Interpreter,
    module: ResolvedCommonJsModule,
) -> Result<Value, VmErr> {
    let source = module.source.as_deref().ok_or_else(|| {
        VmErr::Msg(format!(
            "JavaScript module has no source: {}",
            module.filename
        ))
    })?;
    let exports = Value::object(vec![]);
    let require = make_require(interp, Some(&module.filename))?;
    let filename = Value::String(module.filename.clone());
    let dirname = Value::String(
        Path::new(&module.filename)
            .parent()
            .unwrap_or(Path::new("."))
            .to_string_lossy()
            .into_owned(),
    );
    let module_object = Value::object(vec![
        ("id".to_string(), filename.clone()),
        ("filename".to_string(), filename.clone()),
        ("loaded".to_string(), Value::Bool(false)),
        ("exports".to_string(), exports.clone()),
        ("require".to_string(), require.clone()),
    ]);

    // Publish the initial exports before running the wrapper. A module in a
    // cycle then receives the same partial export object that Node exposes.
    interp.commonjs_cache.borrow_mut().insert(
        module.id.clone(),
        CommonJsCacheEntry {
            exports: exports.clone(),
            module: Some(module_object.clone()),
        },
    );

    let wrapped =
        format!("(function(exports, require, module, __filename, __dirname) {{\n{source}\n}})");
    let old_source_lines = std::mem::take(&mut interp.source_lines);
    interp.set_source(&wrapped);
    let outcome = (|| {
        let tokens = crate::lexer::Lexer::new(&wrapped).tokenize_with_spans();
        let mut parser = crate::parser::Parser::new_with_spans(tokens);
        let statements = match parser.parse_program() {
            Ok(statements) => statements,
            Err(_) if parser.depth_exceeded => {
                return Err(VmErr::Msg(
                    "RangeError: Maximum parse depth exceeded".to_string(),
                ));
            }
            Err(error) => return Err(VmErr::Msg(error.to_string())),
        };
        let factory = interp.run_program_body(&statements)?;
        interp.call_this(
            &factory,
            exports.clone(),
            vec![
                exports.clone(),
                require,
                module_object.clone(),
                filename,
                dirname,
            ],
        )?;
        let exports = module_object
            .get_prop("exports")
            .unwrap_or(Value::Undefined);
        module_object.set_prop("loaded".to_string(), Value::Bool(true))?;
        Ok(exports)
    })();
    interp.source_lines = old_source_lines;

    match outcome {
        Ok(exports) => {
            if let Some(entry) = interp.commonjs_cache.borrow_mut().get_mut(&module.id) {
                entry.exports = exports.clone();
                entry.module = Some(module_object.clone());
            }
            Ok(exports)
        }
        Err(error) => {
            interp.commonjs_cache.borrow_mut().remove(&module.id);
            Err(error)
        }
    }
}

/// Create a genuine guest function that closes over its parent path. This
/// keeps `typeof require === "function"` and preserves resolution when the
/// function escapes its module or runs later from an async function.
pub(super) fn make_require(
    interp: &mut crate::interpreter::Interpreter,
    parent: Option<&str>,
) -> Result<Value, VmErr> {
    const SOURCE: &str = "(function require(specifier) { return __napi_vm_require_with_parent(__napi_vm_require_parent, specifier); })";
    const RESOLVE_SOURCE: &str = "(function resolve(specifier) { return __napi_vm_resolve_with_parent(__napi_vm_require_parent, specifier); })";
    let outer = interp.push_scope();
    let old_source_lines = std::mem::take(&mut interp.source_lines);
    let result = (|| {
        interp.set_binding(
            "__napi_vm_require_parent",
            parent
                .map(|parent| Value::String(parent.to_string()))
                .unwrap_or(Value::Undefined),
        )?;
        interp.set_binding(
            "__napi_vm_require_with_parent",
            Value::NativeFunction {
                name: "require".into(),
                callable: |interp, _this, args| require_with_parent_builtin(interp, args),
            },
        )?;
        interp.set_binding(
            "__napi_vm_resolve_with_parent",
            Value::NativeFunction {
                name: "resolve".into(),
                callable: |interp, _this, args| resolve_with_parent_builtin(interp, args),
            },
        )?;
        interp.set_source(SOURCE);
        let tokens = crate::lexer::Lexer::new(SOURCE).tokenize_with_spans();
        let mut parser = crate::parser::Parser::new_with_spans(tokens);
        let statements = parser
            .parse_program()
            .map_err(|error| VmErr::Msg(error.to_string()))?;
        let require = interp.run_program_body(&statements)?;

        interp.set_source(RESOLVE_SOURCE);
        let tokens = crate::lexer::Lexer::new(RESOLVE_SOURCE).tokenize_with_spans();
        let mut parser = crate::parser::Parser::new_with_spans(tokens);
        let statements = parser
            .parse_program()
            .map_err(|error| VmErr::Msg(error.to_string()))?;
        let resolve = interp.run_program_body(&statements)?;
        require.set_prop("resolve".to_string(), resolve)?;
        Ok(require)
    })();
    interp.pop_scope(outer);
    interp.source_lines = old_source_lines;
    result
}

pub(super) fn create_require_builtin(
    interp: &mut crate::interpreter::Interpreter,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let value = args.first().ok_or_else(|| {
        VmErr::Msg("TypeError: createRequire requires an absolute filename or file URL".into())
    })?;
    let filename = match value {
        Value::String(value) => value.clone(),
        other => match other.get_prop("href") {
            Some(Value::String(ref value)) => value.clone(),
            _ => {
                return Err(VmErr::Msg(
                    "TypeError: createRequire requires an absolute filename or file URL".into(),
                ));
            }
        },
    };
    #[cfg(not(target_arch = "wasm32"))]
    let filename = if filename.starts_with("file:") {
        url::Url::parse(&filename)
            .ok()
            .filter(|url| url.scheme() == "file")
            .and_then(|url| url.to_file_path().ok())
            .ok_or_else(|| VmErr::Msg("TypeError: createRequire requires a valid file URL".into()))?
            .to_string_lossy()
            .into_owned()
    } else {
        filename
    };
    if !Path::new(&filename).is_absolute() {
        return Err(VmErr::Msg(
            "TypeError: createRequire requires an absolute filename or file URL".into(),
        ));
    }
    make_require(interp, Some(&filename))
}

pub(super) fn is_builtin_builtin(
    _interp: &mut crate::interpreter::Interpreter,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let Some(Value::String(name)) = args.first() else {
        return Err(VmErr::Msg("TypeError: isBuiltin requires a string".into()));
    };
    Ok(Value::Bool(matches!(
        name.as_str(),
        "module" | "node:module" | "fs" | "node:fs" | "path" | "node:path"
    )))
}
