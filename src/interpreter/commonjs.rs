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

/// Maximum guest source file size accepted by [`FileCommonJsLoader`].
pub const MAX_COMMONJS_SOURCE_BYTES: usize = 16 * 1024 * 1024;

/// The source format returned by a CommonJS resolver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommonJsModuleFormat {
    JavaScript,
    Json,
    NativeAddon,
    /// A runtime-provided CommonJS helper with no filesystem source file.
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

#[derive(Clone, Debug)]
struct NodeApiPrebuildTarget {
    platform: String,
    architecture: String,
    libc: Option<String>,
    armv: Option<String>,
}

impl NodeApiPrebuildTarget {
    fn current() -> Self {
        let platform = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        }
        .to_string();
        let architecture = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "x86" => "ia32",
            "aarch64" => "arm64",
            "powerpc" => "ppc",
            "powerpc64" | "powerpc64le" => "ppc64",
            "loongarch64" => "loong64",
            other => other,
        }
        .to_string();
        let libc = if platform == "linux" {
            Some(
                std::env::var("LIBC")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        if cfg!(target_env = "musl") || Path::new("/etc/alpine-release").is_file() {
                            "musl".into()
                        } else {
                            "glibc".into()
                        }
                    }),
            )
        } else {
            None
        };
        let armv = std::env::var("ARM_VERSION")
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| (architecture == "arm64").then(|| "8".into()));
        Self {
            platform,
            architecture,
            libc,
            armv,
        }
    }
}

#[derive(Debug)]
struct PrebuildTuple {
    name: String,
    platform: String,
    architectures: Vec<String>,
}

fn node_gyp_build_prebuild_override_variable(package_name: &str) -> String {
    format!(
        "{}_PREBUILD",
        package_name.to_ascii_uppercase().replace('-', "_")
    )
}

fn parse_prebuild_tuple(name: &str) -> Option<PrebuildTuple> {
    if name.split('-').count() != 2 {
        return None;
    }
    let (platform, architectures) = name.split_once('-')?;
    let architectures = architectures
        .split('+')
        .map(str::to_string)
        .collect::<Vec<_>>();
    if platform.is_empty() || architectures.is_empty() || architectures.iter().any(String::is_empty)
    {
        return None;
    }
    Some(PrebuildTuple {
        name: name.to_string(),
        platform: platform.to_string(),
        architectures,
    })
}

#[derive(Default)]
struct PrebuildTags {
    file: String,
    field_order: Vec<String>,
    runtime: Option<String>,
    napi: bool,
    abi: Option<String>,
    uv: Option<String>,
    libc: Option<String>,
    armv: Option<String>,
    specificity: usize,
}

fn parse_prebuild_tags(filename: &str) -> Option<PrebuildTags> {
    let stem = filename.strip_suffix(".node")?;
    let mut tags = PrebuildTags {
        file: filename.to_string(),
        ..PrebuildTags::default()
    };
    for tag in stem.split('.') {
        let field = match tag {
            "node" | "electron" | "node-webkit" => {
                tags.runtime = Some(tag.to_string());
                "runtime"
            }
            "napi" => {
                tags.napi = true;
                "napi"
            }
            "glibc" | "musl" => {
                tags.libc = Some(tag.to_string());
                "libc"
            }
            _ if tag.starts_with("abi") => {
                tags.abi = Some(tag[3..].to_string());
                "abi"
            }
            _ if tag.starts_with("uv") => {
                tags.uv = Some(tag[2..].to_string());
                "uv"
            }
            _ if tag.starts_with("armv") => {
                tags.armv = Some(tag[4..].to_string());
                "armv"
            }
            _ => continue,
        };
        if !tags.field_order.iter().any(|existing| existing == field) {
            tags.field_order.push(field.to_string());
        }
        tags.specificity += 1;
    }
    Some(tags)
}

fn select_node_api_prebuild(
    package_root: &Path,
    target: &NodeApiPrebuildTarget,
    prebuilds_only: bool,
) -> Option<PathBuf> {
    if !prebuilds_only {
        for build_dir in ["build/Release", "build/Debug"] {
            let mut candidates = read_node_addon_files(&package_root.join(build_dir));
            if let Some(candidate) = candidates.drain(..).next() {
                return Some(candidate);
            }
        }
    }

    let prebuilds_root = package_root.join("prebuilds");
    let mut tuples = fs::read_dir(&prebuilds_root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_dir() {
                return None;
            }
            parse_prebuild_tuple(&entry.file_name().to_string_lossy())
        })
        .filter(|tuple| {
            tuple.platform == target.platform && tuple.architectures.contains(&target.architecture)
        })
        .collect::<Vec<_>>();
    tuples.sort_by(|left, right| {
        left.architectures
            .len()
            .cmp(&right.architectures.len())
            .then_with(|| left.name.cmp(&right.name))
    });
    let tuple = tuples.into_iter().next()?;
    let directory = prebuilds_root.join(tuple.name);

    let candidates = read_node_addon_files(&directory)
        .into_iter()
        .filter_map(|path| {
            let filename = path.file_name()?.to_str()?;
            let tags = parse_prebuild_tags(filename)?;
            if !tags.napi
                || tags.uv.as_deref().is_some_and(|uv| !uv.is_empty())
                || tags
                    .runtime
                    .as_deref()
                    .is_some_and(|runtime| runtime != "node")
                || tags
                    .libc
                    .as_deref()
                    .is_some_and(|libc| target.libc.as_deref() != Some(libc))
                || tags
                    .armv
                    .as_deref()
                    .is_some_and(|armv| target.armv.as_deref() != Some(armv))
            {
                return None;
            }
            Some((path, tags))
        })
        .collect::<Vec<_>>();
    let mut candidates = candidates;
    candidates.sort_by(|(left_path, left_tags), (right_path, right_tags)| {
        let left_runtime = usize::from(left_tags.runtime.as_deref() == Some("node"));
        let right_runtime = usize::from(right_tags.runtime.as_deref() == Some("node"));
        let left_abi = usize::from(left_tags.abi.as_deref().is_some_and(|abi| !abi.is_empty()));
        let right_abi = usize::from(right_tags.abi.as_deref().is_some_and(|abi| !abi.is_empty()));
        right_runtime
            .cmp(&left_runtime)
            .then_with(|| right_abi.cmp(&left_abi))
            .then_with(|| right_tags.specificity.cmp(&left_tags.specificity))
            .then_with(|| left_path.cmp(right_path))
    });
    candidates.into_iter().next().map(|(path, _)| path)
}

fn read_node_addon_files(directory: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(directory)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("node")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
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

fn read_package_json(path: &Path) -> Result<JsonValue, VmErr> {
    let source = fs::read_to_string(path)
        .map_err(|error| VmErr::Msg(format!("cannot read {}: {error}", path.display())))?;
    serde_json::from_str(&source)
        .map_err(|error| VmErr::Msg(format!("invalid {}: {error}", path.display())))
}

fn split_package_request(request: &str) -> Result<(String, String), VmErr> {
    let parts: Vec<&str> = request.split('/').collect();
    if parts.is_empty() || parts[0].is_empty() {
        return Err(VmErr::Msg(format!("invalid package specifier '{request}'")));
    }
    let (package_end, subpath_start) = if parts[0].starts_with('@') {
        if parts.len() < 2 || parts[1].is_empty() {
            return Err(VmErr::Msg(format!("invalid package specifier '{request}'")));
        }
        (2, 2)
    } else {
        (1, 1)
    };
    let name = parts[..package_end].join("/");
    let subpath = parts[subpath_start..].join("/");
    Ok((name, subpath))
}

fn exports_target(
    exports: &JsonValue,
    key: &str,
    node_addons: bool,
) -> Result<Option<String>, VmErr> {
    let selection = match exports {
        JsonValue::String(_) | JsonValue::Array(_) | JsonValue::Null if key == "." => {
            select_export_target(exports, None, node_addons)
        }
        JsonValue::Object(entries) if entries.keys().any(|entry| entry.starts_with('.')) => {
            if let Some(target) = entries.get(key) {
                return finish_export_selection(select_export_target(target, None, node_addons));
            }

            let mut best: Option<(usize, usize, String, &JsonValue)> = None;
            for (pattern, target) in entries {
                let Some(capture) = match_export_pattern(pattern, key) else {
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
            best.map_or(
                ExportTargetSelection::NoTarget,
                |(_, _, capture, target)| select_export_target(target, Some(&capture), node_addons),
            )
        }
        JsonValue::Object(_) if key == "." => select_export_target(exports, None, node_addons),
        _ => ExportTargetSelection::NoTarget,
    };
    finish_export_selection(selection)
}

enum ImportTarget {
    Relative(String),
    External(String),
}

enum ImportTargetSelection {
    Target(ImportTarget),
    NoTarget,
    Invalid(String),
}

fn imports_target(
    imports: &JsonValue,
    key: &str,
    node_addons: bool,
) -> Result<ImportTarget, VmErr> {
    let entries = imports
        .as_object()
        .ok_or_else(|| VmErr::Msg("package imports must be an object of specifiers".to_string()))?;
    let selection = if let Some(target) = entries.get(key) {
        select_import_target(target, None, node_addons)
    } else {
        let mut best: Option<(usize, usize, String, &JsonValue)> = None;
        for (pattern, target) in entries {
            let Some(capture) = match_export_pattern(pattern, key) else {
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
        best.map_or(
            ImportTargetSelection::NoTarget,
            |(_, _, capture, target)| select_import_target(target, Some(&capture), node_addons),
        )
    };
    match selection {
        ImportTargetSelection::Target(target) => Ok(target),
        ImportTargetSelection::NoTarget => Err(VmErr::Msg(format!(
            "package does not define import '{key}'"
        ))),
        ImportTargetSelection::Invalid(message) => Err(VmErr::Msg(message)),
    }
}

fn select_import_target(
    value: &JsonValue,
    capture: Option<&str>,
    node_addons: bool,
) -> ImportTargetSelection {
    match value {
        JsonValue::String(target) => {
            let target = match (target.contains('*'), capture) {
                (true, Some(capture)) => target.replace('*', capture),
                (true, None) => {
                    return ImportTargetSelection::Invalid(format!(
                        "unsupported package import target '{target}'"
                    ));
                }
                (false, _) => target.clone(),
            };
            if target.starts_with("./") {
                return normalize_exports_target(&target).map_or_else(
                    || {
                        ImportTargetSelection::Invalid(format!(
                            "unsupported package import target '{target}'"
                        ))
                    },
                    |path| ImportTargetSelection::Target(ImportTarget::Relative(path)),
                );
            }
            if target.starts_with("node:") || is_bare_import_target(&target) {
                ImportTargetSelection::Target(ImportTarget::External(target))
            } else {
                ImportTargetSelection::Invalid(format!(
                    "unsupported package import target '{target}'"
                ))
            }
        }
        JsonValue::Array(entries) => {
            // Like exports arrays, imports arrays skip invalid or unmatched
            // entries, but do not skip a valid target whose file is missing.
            let mut last_invalid = None;
            for entry in entries {
                match select_import_target(entry, capture, node_addons) {
                    selection @ ImportTargetSelection::Target(_) => return selection,
                    ImportTargetSelection::NoTarget => {}
                    ImportTargetSelection::Invalid(message) => last_invalid = Some(message),
                }
            }
            last_invalid.map_or(
                ImportTargetSelection::NoTarget,
                ImportTargetSelection::Invalid,
            )
        }
        JsonValue::Object(entries) => {
            for (condition, value) in entries {
                if (condition == "node-addons" && node_addons)
                    || matches!(condition.as_str(), "node" | "require" | "default")
                {
                    return select_import_target(value, capture, node_addons);
                }
            }
            ImportTargetSelection::NoTarget
        }
        JsonValue::Null => ImportTargetSelection::NoTarget,
        _ => ImportTargetSelection::Invalid(
            "invalid package imports entry; expected a path, package specifier, conditions, array, or null"
                .into(),
        ),
    }
}

fn is_bare_import_target(target: &str) -> bool {
    if target.is_empty()
        || target.starts_with('.')
        || target.starts_with('/')
        || target.starts_with('#')
        || target.contains('\\')
        || target.contains(':')
        || target.chars().any(char::is_whitespace)
        || split_package_request(target).is_err()
    {
        return false;
    }
    target
        .split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn match_export_pattern(pattern: &str, key: &str) -> Option<String> {
    let star = pattern.find('*')?;
    let prefix = &pattern[..star];
    let suffix = &pattern[star + 1..];
    if suffix.contains('*')
        || !key.starts_with(prefix)
        || !key.ends_with(suffix)
        || key.len() < prefix.len() + suffix.len()
    {
        return None;
    }
    let capture_end = key.len() - suffix.len();
    Some(key[prefix.len()..capture_end].to_string())
}

enum ExportTargetSelection {
    Target(String),
    NoTarget,
    Invalid(String),
}

fn finish_export_selection(selection: ExportTargetSelection) -> Result<Option<String>, VmErr> {
    match selection {
        ExportTargetSelection::Target(target) => Ok(Some(target)),
        ExportTargetSelection::NoTarget => Ok(None),
        ExportTargetSelection::Invalid(message) => Err(VmErr::Msg(message)),
    }
}

fn select_export_target(
    value: &JsonValue,
    capture: Option<&str>,
    node_addons: bool,
) -> ExportTargetSelection {
    match value {
        JsonValue::String(path) => {
            let target = match (path.contains('*'), capture) {
                (true, Some(capture)) => path.replace('*', capture),
                (true, None) => {
                    return ExportTargetSelection::Invalid(format!(
                        "unsupported package exports target '{path}'"
                    ));
                }
                (false, _) => path.clone(),
            };
            let Some(target_path) = normalize_exports_target(&target) else {
                return ExportTargetSelection::Invalid(format!(
                    "unsupported package exports target '{target}'"
                ));
            };
            ExportTargetSelection::Target(target_path)
        }
        JsonValue::Array(entries) => {
            // Select the first syntactically usable target. The filesystem is
            // checked later; a missing file does not make an otherwise valid
            // export target fall through to the next array entry.
            let mut last_invalid = None;
            for entry in entries {
                match select_export_target(entry, capture, node_addons) {
                    selection @ ExportTargetSelection::Target(_) => return selection,
                    ExportTargetSelection::NoTarget => {}
                    ExportTargetSelection::Invalid(message) => last_invalid = Some(message),
                }
            }
            last_invalid.map_or(
                ExportTargetSelection::NoTarget,
                ExportTargetSelection::Invalid,
            )
        }
        JsonValue::Object(entries) => {
            for (condition, value) in entries {
                if (condition == "node-addons" && node_addons)
                    || matches!(condition.as_str(), "node" | "require" | "default")
                {
                    return select_export_target(value, capture, node_addons);
                }
            }
            ExportTargetSelection::NoTarget
        }
        JsonValue::Null => ExportTargetSelection::NoTarget,
        _ => ExportTargetSelection::Invalid(
            "invalid package exports entry; expected a path, conditions, array, or null".into(),
        ),
    }
}

fn normalize_exports_target(target: &str) -> Option<String> {
    let path = target.strip_prefix("./")?;
    let path = percent_decode_path(path)?;
    if path.is_empty() || path.contains('\\') {
        return None;
    }
    if path.split('/').any(|segment| {
        segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.eq_ignore_ascii_case("node_modules")
    }) {
        return None;
    }
    Some(path)
}

fn percent_decode_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push((hex_digit(high)? << 4) | hex_digit(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) struct CommonJsCacheEntry {
    pub(super) exports: Value,
    pub(super) module: Option<Value>,
}

/// Internal call target held in each guest require function's closure.
pub(crate) fn require_with_parent_builtin(
    interp: &mut crate::interpreter::Interpreter,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let parent = match args.first() {
        Some(Value::String(parent)) => Some(parent.clone()),
        Some(Value::Undefined) | None => interp.commonjs_entry.clone(),
        Some(_) => {
            return Err(VmErr::Msg(
                "TypeError: invalid internal require parent".into(),
            ));
        }
    };
    let request = match args.get(1) {
        Some(Value::String(request)) => request.clone(),
        Some(_) => {
            return Err(VmErr::Msg(
                "TypeError: require module specifier must be a string".into(),
            ));
        }
        None => {
            return Err(VmErr::Msg(
                "TypeError: require expects a module specifier".into(),
            ));
        }
    };
    interp.require_commonjs(&request, parent.as_deref())
}

pub(crate) fn resolve_with_parent_builtin(
    interp: &mut crate::interpreter::Interpreter,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let parent = match args.first() {
        Some(Value::String(parent)) => Some(parent.as_str()),
        Some(Value::Undefined) | None => interp.commonjs_entry.as_deref(),
        Some(_) => {
            return Err(VmErr::Msg(
                "TypeError: invalid internal require parent".into(),
            ));
        }
    };
    let request = match args.get(1) {
        Some(Value::String(request)) => request,
        Some(_) => {
            return Err(VmErr::Msg(
                "TypeError: require.resolve module specifier must be a string".into(),
            ));
        }
        None => {
            return Err(VmErr::Msg(
                "TypeError: require.resolve expects a module specifier".into(),
            ));
        }
    };
    let loader = interp.commonjs_loader.clone().ok_or_else(|| {
        VmErr::Msg("require is disabled: configure a host CommonJS module loader first".to_string())
    })?;
    let module = loader.resolve(request, parent)?;
    Ok(Value::String(module.filename))
}

pub(super) fn require_module(
    interp: &mut crate::interpreter::Interpreter,
    request: &str,
    parent: Option<&str>,
) -> Result<Value, VmErr> {
    let loader = interp.commonjs_loader.clone().ok_or_else(|| {
        VmErr::Msg("require is disabled: configure a host CommonJS module loader first".to_string())
    })?;
    let module = loader.resolve(request, parent)?;
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
            let exports = make_node_gyp_build(interp)?;
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

fn make_node_gyp_build(interp: &mut crate::interpreter::Interpreter) -> Result<Value, VmErr> {
    const LOAD_SOURCE: &str =
        "(function nodeGypBuild(directory) { return __napi_vm_node_gyp_build_load(directory); })";
    const RESOLVE_SOURCE: &str = "(function nodeGypBuildResolve(directory) { return __napi_vm_node_gyp_build_resolve(directory); })";
    const PARSE_TAGS_SOURCE: &str =
        "(function parseTags(file) { return __napi_vm_node_gyp_build_parse_tags(file); })";
    const MATCH_TAGS_SOURCE: &str = "(function matchTags(runtime, abi) { return function match(tags) { return __napi_vm_node_gyp_build_match_tags(runtime, abi, tags); }; })";
    const COMPARE_TAGS_SOURCE: &str = "(function compareTags(runtime) { return function compare(a, b) { return __napi_vm_node_gyp_build_compare_tags(runtime, a, b); }; })";
    const PARSE_TUPLE_SOURCE: &str =
        "(function parseTuple(name) { return __napi_vm_node_gyp_build_parse_tuple(name); })";
    const MATCH_TUPLE_SOURCE: &str = "(function matchTuple(platform, architecture) { return function match(tuple) { return __napi_vm_node_gyp_build_match_tuple(platform, architecture, tuple); }; })";
    const COMPARE_TUPLES_SOURCE: &str =
        "(function compareTuples(a, b) { return __napi_vm_node_gyp_build_compare_tuples(a, b); })";

    let outer = interp.push_scope();
    let old_source_lines = std::mem::take(&mut interp.source_lines);
    let result = (|| {
        interp.set_binding(
            "__napi_vm_node_gyp_build_load",
            Value::NativeFunction {
                name: "node-gyp-build".into(),
                callable: node_gyp_build_load,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_resolve",
            Value::NativeFunction {
                name: "node-gyp-build.resolve".into(),
                callable: node_gyp_build_resolve,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_parse_tags",
            Value::NativeFunction {
                name: "node-gyp-build.parseTags".into(),
                callable: node_gyp_build_parse_tags,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_match_tags",
            Value::NativeFunction {
                name: "node-gyp-build.matchTags".into(),
                callable: node_gyp_build_match_tags,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_compare_tags",
            Value::NativeFunction {
                name: "node-gyp-build.compareTags".into(),
                callable: node_gyp_build_compare_tags,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_parse_tuple",
            Value::NativeFunction {
                name: "node-gyp-build.parseTuple".into(),
                callable: node_gyp_build_parse_tuple,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_match_tuple",
            Value::NativeFunction {
                name: "node-gyp-build.matchTuple".into(),
                callable: node_gyp_build_match_tuple,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_compare_tuples",
            Value::NativeFunction {
                name: "node-gyp-build.compareTuples".into(),
                callable: node_gyp_build_compare_tuples,
            },
        )?;
        let load = compile_guest_function(interp, LOAD_SOURCE)?;
        let resolve = compile_guest_function(interp, RESOLVE_SOURCE)?;
        let parse_tags = compile_guest_function(interp, PARSE_TAGS_SOURCE)?;
        let match_tags = compile_guest_function(interp, MATCH_TAGS_SOURCE)?;
        let compare_tags = compile_guest_function(interp, COMPARE_TAGS_SOURCE)?;
        let parse_tuple = compile_guest_function(interp, PARSE_TUPLE_SOURCE)?;
        let match_tuple = compile_guest_function(interp, MATCH_TUPLE_SOURCE)?;
        let compare_tuples = compile_guest_function(interp, COMPARE_TUPLES_SOURCE)?;
        load.set_prop("path".into(), resolve.clone())?;
        load.set_prop("resolve".into(), resolve)?;
        load.set_prop("parseTags".into(), parse_tags)?;
        load.set_prop("matchTags".into(), match_tags)?;
        load.set_prop("compareTags".into(), compare_tags)?;
        load.set_prop("parseTuple".into(), parse_tuple)?;
        load.set_prop("matchTuple".into(), match_tuple)?;
        load.set_prop("compareTuples".into(), compare_tuples)?;
        Ok(load)
    })();
    interp.pop_scope(outer);
    interp.source_lines = old_source_lines;
    result
}

fn node_gyp_build_parse_tags(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let Some(Value::String(filename)) = args.first() else {
        return Err(VmErr::Msg(
            "TypeError: node-gyp-build.parseTags expects a filename string".into(),
        ));
    };
    let Some(tags) = parse_prebuild_tags(filename) else {
        return Ok(Value::Undefined);
    };
    prebuild_tags_to_value(tags)
}

fn prebuild_tags_to_value(tags: PrebuildTags) -> Result<Value, VmErr> {
    let mut entries = vec![
        ("file".to_string(), Value::String(tags.file)),
        (
            "specificity".to_string(),
            Value::Number(tags.specificity as f64),
        ),
    ];
    for field in tags.field_order {
        match field.as_str() {
            "runtime" => {
                if let Some(value) = &tags.runtime {
                    entries.push((field, Value::String(value.clone())));
                }
            }
            "napi" if tags.napi => entries.push((field, Value::Bool(true))),
            "abi" => {
                if let Some(value) = &tags.abi {
                    entries.push((field, Value::String(value.clone())));
                }
            }
            "uv" => {
                if let Some(value) = &tags.uv {
                    entries.push((field, Value::String(value.clone())));
                }
            }
            "armv" => {
                if let Some(value) = &tags.armv {
                    entries.push((field, Value::String(value.clone())));
                }
            }
            "libc" => {
                if let Some(value) = &tags.libc {
                    entries.push((field, Value::String(value.clone())));
                }
            }
            _ => {}
        }
    }
    Value::checked_object(entries)
}

fn node_gyp_build_match_tags(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let runtime = args.first().and_then(value_string).unwrap_or_default();
    let Some(tags) = args.get(2) else {
        return Ok(Value::Bool(false));
    };
    match_tags_for_rust_node_api(runtime, tags)
}

fn match_tags_for_rust_node_api(runtime: &str, tags: &Value) -> Result<Value, VmErr> {
    let napi = matches!(tags.get_prop("napi"), Some(Value::Bool(true)));
    if !napi {
        // The Rust backend implements Node-API. A matching Node ABI tag alone
        // cannot make a V8/NAN addon safe to load in this runtime.
        return Ok(Value::Bool(false));
    }
    if let Some(tag_runtime) = property_string(tags, "runtime")
        && tag_runtime != runtime
        && !(tag_runtime == "node" && napi)
    {
        return Ok(Value::Bool(false));
    }
    if tags
        .get_prop("uv")
        .and_then(|value| value_string(&value).map(str::to_owned))
        .is_some_and(|uv| !uv.is_empty())
    {
        // A uv-tagged addon depends on libuv's ABI, which this host does not
        // provide as part of Node-API compatibility.
        return Ok(Value::Bool(false));
    }
    let target = NodeApiPrebuildTarget::current();
    if let Some(libc) = property_string(tags, "libc")
        && !libc.is_empty()
        && target.libc.as_deref() != Some(libc.as_str())
    {
        return Ok(Value::Bool(false));
    }
    if let Some(armv) = property_string(tags, "armv")
        && !armv.is_empty()
        && target.armv.as_deref() != Some(armv.as_str())
    {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}

fn value_string(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) => Some(value),
        _ => None,
    }
}

fn node_gyp_build_compare_tags(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let runtime = args.first().and_then(value_string).unwrap_or_default();
    let left = args.get(1).cloned().unwrap_or(Value::Undefined);
    let right = args.get(2).cloned().unwrap_or(Value::Undefined);
    let left_runtime = property_string(&left, "runtime");
    let right_runtime = property_string(&right, "runtime");
    if left_runtime != right_runtime {
        return Ok(Value::Number(if left_runtime.as_deref() == Some(runtime) {
            -1.0
        } else {
            1.0
        }));
    }
    let left_abi = property_string(&left, "abi");
    let right_abi = property_string(&right, "abi");
    if left_abi != right_abi {
        return Ok(Value::Number(
            if left_abi.as_deref().is_some_and(|abi| !abi.is_empty()) {
                -1.0
            } else {
                1.0
            },
        ));
    }
    let left_specificity = left
        .get_prop("specificity")
        .and_then(|value| match value {
            Value::Number(value) => Some(value),
            _ => None,
        })
        .unwrap_or(0.0);
    let right_specificity = right
        .get_prop("specificity")
        .and_then(|value| match value {
            Value::Number(value) => Some(value),
            _ => None,
        })
        .unwrap_or(0.0);
    Ok(Value::Number(if left_specificity > right_specificity {
        -1.0
    } else if right_specificity > left_specificity {
        1.0
    } else {
        0.0
    }))
}

fn property_string(value: &Value, key: &str) -> Option<String> {
    value
        .get_prop(key)
        .and_then(|value| value_string(&value).map(str::to_owned))
}

fn node_gyp_build_parse_tuple(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let Some(Value::String(name)) = args.first() else {
        return Err(VmErr::Msg(
            "TypeError: node-gyp-build.parseTuple expects a tuple name string".into(),
        ));
    };
    let Some(tuple) = parse_prebuild_tuple(name) else {
        return Ok(Value::Undefined);
    };
    Value::checked_object(vec![
        ("name".to_string(), Value::String(tuple.name)),
        ("platform".to_string(), Value::String(tuple.platform)),
        (
            "architectures".to_string(),
            Value::checked_array(tuple.architectures.into_iter().map(Value::String).collect())?,
        ),
    ])
}

fn node_gyp_build_match_tuple(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let platform = args.first().and_then(value_string).unwrap_or_default();
    let architecture = args.get(1).and_then(value_string).unwrap_or_default();
    let Some(tuple) = args.get(2) else {
        return Ok(Value::Bool(false));
    };
    let matches_platform = tuple
        .get_prop("platform")
        .and_then(|value| value_string(&value).map(str::to_owned))
        .as_deref()
        == Some(platform);
    let matches_architecture = tuple
        .get_prop("architectures")
        .and_then(|value| value.as_array())
        .is_some_and(|values| {
            values
                .borrow()
                .iter()
                .any(|value| value_string(value) == Some(architecture))
        });
    Ok(Value::Bool(matches_platform && matches_architecture))
}

fn node_gyp_build_compare_tuples(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let architecture_count = |value: Option<&Value>| {
        value
            .and_then(|value| value.get_prop("architectures"))
            .and_then(|value| value.as_array())
            .map(|array| array.borrow().len())
            .unwrap_or(0)
    };
    let left = architecture_count(args.first());
    let right = architecture_count(args.get(1));
    Ok(Value::Number(left as f64 - right as f64))
}

fn compile_guest_function(
    interp: &mut crate::interpreter::Interpreter,
    source: &str,
) -> Result<Value, VmErr> {
    interp.set_source(source);
    let tokens = crate::lexer::Lexer::new(source).tokenize_with_spans();
    let mut parser = crate::parser::Parser::new_with_spans(tokens);
    let statements = parser
        .parse_program()
        .map_err(|error| VmErr::Msg(error.to_string()))?;
    interp.run_program_body(&statements)
}

fn node_gyp_build_resolve(
    interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let Some(Value::String(package_root)) = args.first() else {
        return Err(VmErr::Msg(
            "TypeError: node-gyp-build expects a package directory string".into(),
        ));
    };
    let loader = interp
        .commonjs_loader
        .clone()
        .ok_or_else(|| VmErr::Msg("node-gyp-build requires a configured CommonJS loader".into()))?;
    let module = loader.resolve_node_api_prebuild_for_package(Path::new(package_root))?;
    Ok(Value::String(module.filename))
}

fn node_gyp_build_load(
    interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let filename = node_gyp_build_resolve(interp, Value::Undefined, args)?;
    let Value::String(filename) = &filename else {
        unreachable!("node-gyp-build resolve returns a string")
    };
    interp.require_commonjs(filename, None)
}

pub(super) fn json_to_guest(value: JsonValue) -> Result<Value, VmErr> {
    Ok(match value {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(value) => Value::Bool(value),
        JsonValue::Number(value) => {
            Value::Number(value.as_f64().ok_or_else(|| {
                VmErr::Msg("JSON number is outside the VM number range".to_string())
            })?)
        }
        JsonValue::String(value) => Value::String(value),
        JsonValue::Array(values) => Value::checked_array(
            values
                .into_iter()
                .map(json_to_guest)
                .collect::<Result<Vec<_>, _>>()?,
        )?,
        JsonValue::Object(entries) => Value::checked_object(
            entries
                .into_iter()
                .map(|(key, value)| Ok((key, json_to_guest(value)?)))
                .collect::<Result<Vec<_>, VmErr>>()?,
        )?,
    })
}

#[cfg(test)]
mod tests {
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
                Some("module.exports = async function() { return require('./dependency.cjs').answer; };"),
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
        assert!(
            matches!(first, Err(VmErr::Msg(message)) if message == "fixture initializer failed")
        );
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
            0x68, 0xe8, 0x9f, 0x8b, 0x20, 0x74, 0xe2, 0x62, 0x7d, 0x62, 0xbe, 0xe3, 0xa2, 0xb6,
            0x94, 0xe2, 0x81, 0x43, 0x2e, 0xf9, 0x09, 0xeb, 0x7a, 0x55, 0x05, 0xf0, 0x7b, 0xbf,
            0xfd, 0x91, 0x7c, 0xbf,
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
}
