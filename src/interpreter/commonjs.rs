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
}

/// One resolved CommonJS module. `id` is the stable cache key; `filename` is
/// passed to guest `__filename`/`__dirname` and to the native addon provider.
#[derive(Clone, Debug)]
pub struct ResolvedCommonJsModule {
    pub id: String,
    pub filename: String,
    pub format: CommonJsModuleFormat,
    /// The exact source text for JavaScript and JSON. Native addons have no
    /// source text and are represented by `None`.
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
}

/// Allowlisted native addon provider hook.
///
/// Implementors bridge an addon through an actual Node-API implementation or
/// another explicitly chosen host runtime. Merely opening the shared library
/// is insufficient: its initializer expects a valid `napi_env`.
pub trait NativeAddonLoader {
    /// Initialize the addon at `filename` and return its `module.exports`.
    fn load(&self, filename: &Path) -> Result<Value, VmErr>;
}

/// Filesystem resolver for JavaScript, JSON, and `.node` CommonJS modules.
///
/// Resolution is limited to configured roots. Symlinks are canonicalized and
/// rejected when their targets leave those roots. Built-in modules are not
/// loaded implicitly. Package `exports` supports exact subpaths and the
/// `require`, `node`, and `default` conditions; wildcard exports are rejected
/// explicitly until implemented.
pub struct FileCommonJsLoader {
    roots: Vec<PathBuf>,
    native_addons: Option<Rc<dyn NativeAddonLoader>>,
    allowed_native_addons: HashMap<PathBuf, [u8; 32]>,
}

impl std::fmt::Debug for FileCommonJsLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileCommonJsLoader")
            .field("roots", &self.roots)
            .field("native_addons", &self.native_addons.is_some())
            .field("allowed_native_addons", &self.allowed_native_addons)
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
        })
    }

    /// Attach an explicitly trusted native addon provider. Without this, a
    /// resolved `.node` file fails with a clear configuration error.
    pub fn with_native_addon_loader(mut self, loader: Rc<dyn NativeAddonLoader>) -> Self {
        self.native_addons = Some(loader);
        self
    }

    /// Allow one specific `.node` binary and pin its SHA-256 digest. Native
    /// addons are never enabled for every package under a root by default;
    /// each binary must be opted in and remain byte-for-byte unchanged.
    pub fn allow_native_addon(mut self, path: impl AsRef<Path>) -> Result<Self, VmErr> {
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
        let digest = sha256_file(&canonical).map_err(|error| {
            VmErr::Msg(format!(
                "cannot pin native addon {}: {error}",
                canonical.display()
            ))
        })?;
        self.allowed_native_addons.insert(canonical, digest);
        Ok(self)
    }

    /// The canonical roots this loader may read from.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
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
                    exports_target(exports, ".")?.ok_or_else(|| {
                        VmErr::Msg(format!(
                            "package does not export its root entry: {}",
                            path.display()
                        ))
                    })?
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

    fn package_request(&self, request: &str, parent: Option<&str>) -> Result<PathBuf, VmErr> {
        let (package_name, subpath) = split_package_request(request)?;
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

            let mut uses_exports = false;
            let target = if let Some(exports) = package.get("exports") {
                uses_exports = true;
                let export_key = if subpath.is_empty() {
                    ".".to_string()
                } else {
                    format!("./{subpath}")
                };
                let target = exports_target(exports, &export_key)?.ok_or_else(|| {
                    VmErr::Msg(format!(
                        "package {package_name} does not export subpath {export_key}"
                    ))
                })?;
                package_root.join(target)
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
                if uses_exports && !found.starts_with(&package_root) {
                    return Err(VmErr::Msg(format!(
                        "package exports target escapes package root: {}",
                        found.display()
                    )));
                }
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
        self.resolved_file(self.resolve_request(request, parent)?)
    }

    fn load_native_addon(&self, module: &ResolvedCommonJsModule) -> Result<Value, VmErr> {
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
        loader.load(&path)
    }
}

fn sha256_file(path: &Path) -> std::io::Result<[u8; 32]> {
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

fn exports_target(exports: &JsonValue, key: &str) -> Result<Option<String>, VmErr> {
    let target = match exports {
        JsonValue::String(target) if key == "." => Some(exports),
        JsonValue::Array(_) | JsonValue::Null => Some(exports),
        JsonValue::Object(entries) if entries.keys().any(|entry| entry.starts_with('.')) => {
            entries.get(key)
        }
        JsonValue::Object(_) if key == "." => Some(exports),
        _ => None,
    };
    let Some(target) = target else {
        return Ok(None);
    };
    export_target_value(target)
}

fn export_target_value(value: &JsonValue) -> Result<Option<String>, VmErr> {
    match value {
        JsonValue::String(path) => {
            if !path.starts_with("./") || path.contains('*') {
                return Err(VmErr::Msg(format!(
                    "unsupported package exports target '{path}'"
                )));
            }
            Ok(Some(path[2..].to_string()))
        }
        JsonValue::Array(entries) => {
            for entry in entries {
                if let Some(target) = export_target_value(entry)? {
                    return Ok(Some(target));
                }
            }
            Ok(None)
        }
        JsonValue::Object(entries) => {
            for condition in ["require", "node", "default"] {
                if let Some(value) = entries.get(condition)
                    && let Some(target) = export_target_value(value)?
                {
                    return Ok(Some(target));
                }
            }
            Ok(None)
        }
        JsonValue::Null => Ok(None),
        _ => Err(VmErr::Msg(
            "invalid package exports entry; expected a path, conditions, or array".to_string(),
        )),
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
            let exports = loader.load_native_addon(&module)?;
            interp.commonjs_cache.borrow_mut().insert(
                module.id,
                CommonJsCacheEntry {
                    exports: exports.clone(),
                    module: None,
                },
            );
            Ok(exports)
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
        interp.set_source(SOURCE);
        let tokens = crate::lexer::Lexer::new(SOURCE).tokenize_with_spans();
        let mut parser = crate::parser::Parser::new_with_spans(tokens);
        let statements = parser
            .parse_program()
            .map_err(|error| VmErr::Msg(error.to_string()))?;
        interp.run_program_body(&statements)
    })();
    interp.pop_scope(outer);
    interp.source_lines = old_source_lines;
    result
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

        let loader = Rc::new(
            FileCommonJsLoader::new([&root])
                .unwrap()
                .allow_native_addon(root.join("addon.node"))
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
