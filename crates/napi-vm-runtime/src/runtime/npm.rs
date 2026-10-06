//! Local npm package resolution. Every manifest and source read uses read grants.
use super::permissions::Permissions;
use crate::{
    CommonJsModuleFormat, CommonJsModuleLoader, ModuleLoader, ModuleSource, ResolvedCommonJsModule,
    VmErr,
};
use serde_json::Value as Json;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy)]
enum Mode {
    Import,
    Require,
}
pub struct NpmLoader {
    permissions: Permissions,
    base: PathBuf,
    max_bytes: usize,
    #[cfg(feature = "runtime-node")]
    node: bool,
}
fn err(message: impl Into<String>) -> VmErr {
    VmErr::Msg(message.into())
}
impl NpmLoader {
    pub fn new(
        permissions: Permissions,
        base: impl AsRef<Path>,
        max_bytes: usize,
    ) -> std::io::Result<Self> {
        let base = if base.as_ref().is_absolute() {
            base.as_ref().to_path_buf()
        } else {
            std::env::current_dir()?.join(base)
        };
        Ok(Self {
            permissions,
            base,
            max_bytes,
            #[cfg(feature = "runtime-node")]
            node: false,
        })
    }
    #[cfg(feature = "runtime-node")]
    pub fn node_builtins(mut self) -> Self {
        self.node = true;
        self
    }
    fn normalize(path: &Path) -> Result<PathBuf, VmErr> {
        let mut out = PathBuf::new();
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    if !out.pop() {
                        return Err(err("Invalid module path"));
                    }
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        Ok(out)
    }
    fn from_id(id: &str) -> Result<PathBuf, VmErr> {
        if id.starts_with("file:") {
            url::Url::parse(id)
                .map_err(|e| err(e.to_string()))?
                .to_file_path()
                .map_err(|_| err("Invalid file URL"))
        } else {
            Ok(PathBuf::from(id))
        }
    }
    fn id(path: &Path) -> Result<String, VmErr> {
        url::Url::from_file_path(path)
            .map(String::from)
            .map_err(|_| err("Invalid module path"))
    }
    fn text(&self, path: &Path) -> Result<String, VmErr> {
        self.permissions.read_text(path, self.max_bytes)
    }
    fn manifest(&self, directory: &Path) -> Result<Json, VmErr> {
        let source = self.text(&directory.join("package.json"))?;
        serde_json::from_str(&source).map_err(|e| err(format!("Invalid package.json: {e}")))
    }
    fn candidate(&self, path: &Path, mode: Mode, directory: bool) -> Result<PathBuf, VmErr> {
        let path = Self::normalize(path)?;
        let initial_error = match self.text(&path) {
            Ok(_) => return Ok(path),
            Err(error) => error,
        };
        if matches!(mode, Mode::Import) && !directory {
            return Err(initial_error);
        }
        if matches!(mode, Mode::Require) {
            for extension in ["js", "json", "cjs"] {
                let candidate = PathBuf::from(format!("{}.{}", path.display(), extension));
                if self.text(&candidate).is_ok() {
                    return Ok(candidate);
                }
            }
        }
        if directory {
            if let Ok(manifest) = self.manifest(&path) {
                let field = if matches!(mode, Mode::Import) {
                    manifest.get("module").or_else(|| manifest.get("main"))
                } else {
                    manifest.get("main")
                };
                if let Some(main) = field.and_then(Json::as_str) {
                    let main_path = Self::normalize(&path.join(main))?;
                    if !main_path.starts_with(&path) {
                        return Err(err("Package main escapes package"));
                    }
                    if let Ok(entry) = self.candidate(&main_path, Mode::Require, false) {
                        return Ok(entry);
                    }
                }
            }
            for filename in ["index.js", "index.json", "index.cjs"] {
                let candidate = path.join(filename);
                if self.text(&candidate).is_ok() {
                    return Ok(candidate);
                }
            }
        }
        Err(err(format!(
            "Module not found: {} ({initial_error})",
            path.display()
        )))
    }
    fn select(value: &Json, mode: Mode, capture: Option<&str>) -> Result<Option<String>, VmErr> {
        match value {
            Json::String(target) => Ok(Some(if let Some(capture) = capture {
                target.replace('*', capture)
            } else {
                target.clone()
            })),
            Json::Null => Ok(None),
            Json::Array(values) => {
                for value in values {
                    if let Some(target) = Self::select(value, mode, capture)? {
                        return Ok(Some(target));
                    }
                }
                Ok(None)
            }
            Json::Object(conditions) => {
                for (condition, target) in conditions {
                    if (condition == "default"
                        || condition == "node"
                        || condition
                            == if matches!(mode, Mode::Import) {
                                "import"
                            } else {
                                "require"
                            })
                        && let Some(selected) = Self::select(target, mode, capture)?
                    {
                        return Ok(Some(selected));
                    }
                }
                Ok(None)
            }
            _ => Err(err("Invalid package target")),
        }
    }
    fn mapping(
        value: &Json,
        key: &str,
        mode: Mode,
        imports: bool,
    ) -> Result<Option<String>, VmErr> {
        if let Some(map) = value.as_object() {
            let subpaths = map.keys().any(|k| k.starts_with('.') || k.starts_with('#'));
            if subpaths || imports {
                if let Some(target) = map.get(key) {
                    return Self::select(target, mode, None);
                }
                let mut patterns = map
                    .iter()
                    .filter_map(|(pattern, target)| {
                        let (prefix, suffix) = pattern.split_once('*')?;
                        if suffix.contains('*')
                            || !key.starts_with(prefix)
                            || !key.ends_with(suffix)
                            || key.len() < prefix.len() + suffix.len()
                        {
                            return None;
                        }
                        Some((
                            prefix.len(),
                            pattern.len(),
                            &key[prefix.len()..key.len() - suffix.len()],
                            target,
                        ))
                    })
                    .collect::<Vec<_>>();
                patterns.sort_by_key(|a| std::cmp::Reverse((a.0, a.1)));
                return if let Some((_, _, capture, target)) = patterns.first() {
                    Self::select(target, mode, Some(capture))
                } else {
                    Ok(None)
                };
            }
        }
        if key == "." {
            Self::select(value, mode, None)
        } else {
            Ok(None)
        }
    }
    fn package_target(&self, root: &Path, target: &str, mode: Mode) -> Result<PathBuf, VmErr> {
        if !target.starts_with("./")
            || target.contains('\\')
            || target.contains('%')
            || target.split('/').any(|p| p == ".." || p == "node_modules")
        {
            return Err(err("Invalid package target"));
        }
        self.candidate(&root.join(target), mode, false)
    }
    fn resolve_path(
        &self,
        request: &str,
        parent: Option<&str>,
        mode: Mode,
        depth: usize,
    ) -> Result<PathBuf, VmErr> {
        if depth > 32 {
            return Err(err("Package import resolution cycle"));
        }
        let from = parent
            .map(Self::from_id)
            .transpose()?
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| self.base.clone());
        if request.starts_with("file:") {
            let url = url::Url::parse(request).map_err(|e| err(e.to_string()))?;
            if url.query().is_some() || url.fragment().is_some() {
                return Err(err("Unsupported file URL"));
            }
            return self.candidate(&Self::from_id(request)?, mode, false);
        }
        if request.starts_with('.') || Path::new(request).is_absolute() {
            return self.candidate(&from.join(request), mode, matches!(mode, Mode::Require));
        }
        if request.starts_with('#') {
            for root in from.ancestors() {
                if let Ok(manifest) = self.manifest(root) {
                    let target = Self::mapping(
                        manifest
                            .get("imports")
                            .ok_or_else(|| err("Package has no imports"))?,
                        request,
                        mode,
                        true,
                    )?
                    .ok_or_else(|| err("Package import not defined"))?;
                    return if target.starts_with("./") {
                        self.package_target(root, &target, mode)
                    } else {
                        self.resolve_path(
                            &target,
                            Some(&Self::id(&root.join("package.json"))?),
                            mode,
                            depth + 1,
                        )
                    };
                }
            }
            return Err(err("No package scope for import"));
        }
        let request = request.strip_prefix("npm:").unwrap_or(request);
        if request.contains(':')
            || request.contains('\\')
            || request.contains('%')
            || request
                .split('/')
                .any(|p| p == "." || p == ".." || p.is_empty())
        {
            return Err(err("Invalid package specifier"));
        }
        let segments = request.split('/').collect::<Vec<_>>();
        let count = if request.starts_with('@') { 2 } else { 1 };
        if segments.len() < count {
            return Err(err("Invalid scoped package"));
        }
        let name = segments[..count].join("/");
        let subpath = segments[count..].join("/");
        // Self-references use the nearest package's exports map.
        for directory in from.ancestors() {
            if let Ok(manifest) = self.manifest(directory) {
                if manifest.get("name").and_then(Json::as_str) == Some(name.as_str())
                    && manifest.get("exports").is_some()
                {
                    return self.package_entry(directory, &manifest, &subpath, mode);
                }
                break;
            }
        }
        for directory in from.ancestors() {
            let root = directory.join("node_modules").join(&name);
            if let Ok(manifest) = self.manifest(&root) {
                return self.package_entry(&root, &manifest, &subpath, mode);
            }
        }
        Err(err(format!("Package not installed or read denied: {name}")))
    }
    fn package_entry(
        &self,
        root: &Path,
        manifest: &Json,
        subpath: &str,
        mode: Mode,
    ) -> Result<PathBuf, VmErr> {
        if let Some(exports) = manifest.get("exports") {
            let key = if subpath.is_empty() {
                ".".into()
            } else {
                format!("./{subpath}")
            };
            let target = Self::mapping(exports, &key, mode, false)?
                .ok_or_else(|| err(format!("Package path not exported: {key}")))?;
            self.package_target(root, &target, mode)
        } else if !subpath.is_empty() {
            self.candidate(&root.join(subpath), mode, matches!(mode, Mode::Require))
        } else {
            self.candidate(root, mode, true)
        }
    }
    fn is_esm(&self, path: &Path) -> bool {
        if path
            .extension()
            .is_some_and(|e| e == "mjs" || e == "ts" || e == "tsx" || e == "mts")
        {
            return true;
        }
        if path.extension().is_some_and(|e| e == "cjs" || e == "json") {
            return false;
        }
        for directory in path.parent().into_iter().flat_map(Path::ancestors) {
            if let Ok(manifest) = self.manifest(directory) {
                return manifest.get("type").and_then(Json::as_str) == Some("module");
            }
        }
        true
    }
}
impl ModuleLoader for NpmLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, VmErr> {
        #[cfg(feature = "runtime-node")]
        if self.node && super::node::is_builtin(specifier) {
            return Ok(if specifier.starts_with("node:") {
                specifier.into()
            } else {
                format!("node:{specifier}")
            });
        }
        Self::id(&self.resolve_path(specifier, referrer, Mode::Import, 0)?)
    }
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
        let path = Self::from_id(id)?;
        let source = self.text(&path)?;
        let source = if path.extension().is_some_and(|e| e == "json") {
            let value: Json = serde_json::from_str(&source).map_err(|e| err(e.to_string()))?;
            format!("export default {};", value)
        } else if self.is_esm(&path) {
            source
        } else {
            format!(
                "export default require({});",
                serde_json::to_string(&path.to_string_lossy()).map_err(|e| err(e.to_string()))?
            )
        };
        Ok(ModuleSource {
            id: id.into(),
            source,
        })
    }
}
impl CommonJsModuleLoader for NpmLoader {
    fn resolve(
        &self,
        request: &str,
        parent: Option<&str>,
    ) -> Result<ResolvedCommonJsModule, VmErr> {
        #[cfg(feature = "runtime-node")]
        if self.node
            && let Some(source) = super::node::commonjs_source(request)
        {
            return Ok(ResolvedCommonJsModule {
                id: request.into(),
                filename: request.into(),
                format: CommonJsModuleFormat::JavaScript,
                source: Some(source),
            });
        }
        let path = self.resolve_path(request, parent, Mode::Require, 0)?;
        if path.extension().is_some_and(|e| e == "node") {
            self.permissions.check_ffi()?;
            return Err(err("Native addon provider is not configured"));
        }
        if self.is_esm(&path) {
            return Err(err("ERR_REQUIRE_ESM: use import() for an ES module"));
        }
        let source = self.text(&path)?;
        Ok(ResolvedCommonJsModule {
            id: Self::id(&path)?,
            filename: path.to_string_lossy().into(),
            format: if path.extension().is_some_and(|e| e == "json") {
                CommonJsModuleFormat::Json
            } else {
                CommonJsModuleFormat::JavaScript
            },
            source: Some(source),
        })
    }
}

/// Resolve a registry version without conflating semver with package identity.
pub fn select_version(metadata: &Json, requirement: &str) -> Result<String, VmErr> {
    if let Some(version) = metadata
        .get("dist-tags")
        .and_then(|tags| tags.get(requirement))
        .and_then(Json::as_str)
    {
        return Ok(version.into());
    }
    let requirement = if requirement.is_empty() {
        "*"
    } else {
        requirement
    };
    let _ = version_matches("0.0.0", requirement)?;
    metadata
        .get("versions")
        .and_then(Json::as_object)
        .into_iter()
        .flat_map(|versions| versions.keys())
        .filter_map(|version| semver::Version::parse(version).ok())
        .filter(|version| version_matches(&version.to_string(), requirement).unwrap_or(false))
        .max()
        .map(|version| version.to_string())
        .ok_or_else(|| err("No npm version satisfies the requested range"))
}

/// npm bare complete versions are exact, unlike Rust semver's implicit caret.
pub fn version_matches(version: &str, requirement: &str) -> Result<bool, VmErr> {
    let version = semver::Version::parse(version).map_err(|e| err(e.to_string()))?;
    for alternative in requirement.split("||") {
        let mut range = alternative
            .trim()
            .trim_start_matches('v')
            .replace(['x', 'X'], "*");
        if range.is_empty() {
            range = "*".into();
        }
        if let Ok(exact) = semver::Version::parse(&range) {
            if exact == version {
                return Ok(true);
            }
            continue;
        }
        if let Some((start, end)) = range.split_once(" - ") {
            range = format!(">={start}, <={end}");
        } else if range.chars().all(|c| c.is_ascii_digit() || c == '.')
            && range.split('.').count() < 3
        {
            range.push_str(".*");
        } else if range.contains(' ') && !range.contains(',') {
            range = range.split_whitespace().collect::<Vec<_>>().join(", ");
        }
        let range = semver::VersionReq::parse(&range)
            .map_err(|e| err(format!("Unsupported npm version range: {e}")))?;
        if range.matches(&version) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Exact package artifacts recorded independently of ECMAScript compatibility.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PackageLock {
    pub name: String,
    pub version: String,
    pub integrity: String,
    pub tarball: String,
    #[serde(default)]
    pub requirement: String,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Lockfile {
    pub version: u32,
    pub packages: std::collections::BTreeMap<String, PackageLock>,
}
pub fn verify_integrity(bytes: &[u8], integrity: &str) -> Result<(), VmErr> {
    use base64::Engine;
    use sha2::Digest;
    let tokens = integrity.split_whitespace().collect::<Vec<_>>();
    let algorithm = if tokens.iter().any(|v| v.starts_with("sha512-")) {
        "sha512"
    } else if tokens.iter().any(|v| v.starts_with("sha256-")) {
        "sha256"
    } else {
        return Err(err("Unsupported or missing package integrity hash"));
    };
    let actual = if algorithm == "sha512" {
        sha2::Sha512::digest(bytes).to_vec()
    } else {
        sha2::Sha256::digest(bytes).to_vec()
    };
    for token in tokens {
        if let Some(encoded) = token.strip_prefix(&format!("{algorithm}-"))
            && let Ok(expected) = base64::engine::general_purpose::STANDARD.decode(encoded)
            && expected == actual
        {
            return Ok(());
        }
    }
    Err(err("Package integrity check failed"))
}

/// Network and write capabilities are supplied by the host. Install scripts
/// and native compilation are never part of package source installation.
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
pub struct NpmInstaller {
    permissions: Permissions,
    registry: url::Url,
    root: PathBuf,
    max_bytes: usize,
    lock: Lockfile,
    locked: bool,
    installed: usize,
}
#[cfg(all(feature = "runtime-net", not(target_arch = "wasm32")))]
impl NpmInstaller {
    pub fn new(
        permissions: Permissions,
        root: impl AsRef<Path>,
        registry: &str,
        max_bytes: usize,
        locked: bool,
    ) -> Result<Self, VmErr> {
        let root = if root.as_ref().is_absolute() {
            root.as_ref().to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| err(e.to_string()))?
                .join(root)
        };
        let registry = url::Url::parse(registry).map_err(|e| err(e.to_string()))?;
        super::network::validate_url(registry.as_str(), &permissions).map_err(VmErr::Msg)?;
        let lock_path = root.join("napi-vm.lock");
        let lock = match permissions.read_text(&lock_path, max_bytes) {
            Ok(source) => {
                let lock: Lockfile = serde_json::from_str(&source)
                    .map_err(|e| err(format!("Invalid lockfile: {e}")))?;
                if lock.version != 1 {
                    return Err(err("Unsupported lockfile version"));
                }
                lock
            }
            Err(error) if !locked && error.to_string().contains("No such file") => Lockfile {
                version: 1,
                ..Lockfile::default()
            },
            Err(error) => return Err(error),
        };
        Ok(Self {
            permissions,
            registry,
            root,
            max_bytes,
            lock,
            locked,
            installed: 0,
        })
    }
    fn download(&self, url: &str) -> Result<Vec<u8>, VmErr> {
        let response = super::network::fetch(
            super::network::FetchRequest {
                url: url.into(),
                method: "GET".into(),
                headers: Vec::new(),
                body: None,
                redirect: "follow".into(),
            },
            self.permissions.clone(),
            self.max_bytes,
            std::time::Duration::from_secs(30),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .map_err(VmErr::Msg)?;
        let status = response["status"].as_u64().unwrap_or(0);
        if !(200..300).contains(&status) {
            return Err(err(format!("Registry returned HTTP {status}")));
        }
        super::network::response_bytes(&response, self.max_bytes).map_err(VmErr::Msg)
    }
    fn request(specifier: &str) -> Result<(String, String), VmErr> {
        let specifier = specifier.strip_prefix("npm:").unwrap_or(specifier);
        let (name, requirement) = specifier
            .rsplit_once('@')
            .filter(|(name, _)| !name.is_empty())
            .unwrap_or((specifier, "latest"));
        let segments = name.split('/').collect::<Vec<_>>();
        if segments.len() != if name.starts_with('@') { 2 } else { 1 }
            || segments.iter().any(|s| {
                s.is_empty()
                    || *s == "."
                    || *s == ".."
                    || !s
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '_' | '-' | '.'))
            })
        {
            return Err(err("Invalid npm package name"));
        }
        Ok((name.into(), requirement.into()))
    }
    /// Install a package graph with nested dependency identities. A graph is
    /// bounded to 256 package instances and depth 32; cycles reuse ancestors.
    pub fn install(&mut self, specifier: &str) -> Result<&Lockfile, VmErr> {
        let (name, requirement) = Self::request(specifier)?;
        self.install_package(&name, &requirement, PathBuf::from(&name), &mut Vec::new())?;
        if !self.locked {
            let source =
                serde_json::to_string_pretty(&self.lock).map_err(|e| err(e.to_string()))?;
            self.permissions
                .write_text(self.root.join("napi-vm.lock"), &source, self.max_bytes)?;
        }
        Ok(&self.lock)
    }
    fn install_package(
        &mut self,
        name: &str,
        requirement: &str,
        relative: PathBuf,
        ancestors: &mut Vec<(String, String)>,
    ) -> Result<(), VmErr> {
        if ancestors.len() >= 32 || self.installed >= 256 {
            return Err(err("ResourceLimit: package graph exceeded"));
        }
        let key = relative.to_string_lossy().into_owned();
        let pin = if let Some(pin) = self.lock.packages.get(&key) {
            if pin.name != name {
                return Err(err("Lockfile package identity mismatch"));
            }
            match version_matches(&pin.version, requirement) {
                Ok(false) => return Err(err("Lockfile version does not satisfy dependency")),
                Err(_) if pin.requirement != requirement => {
                    return Err(err("Lockfile dependency requirement mismatch"));
                }
                _ => {}
            }
            pin.clone()
        } else {
            if self.locked {
                return Err(err(format!("Package missing from lockfile: {key}")));
            }
            let mut url = self.registry.clone();
            url.path_segments_mut()
                .map_err(|_| err("Invalid registry URL"))?
                .pop_if_empty()
                .push(name);
            let metadata: Json = serde_json::from_slice(&self.download(url.as_str())?)
                .map_err(|e| err(e.to_string()))?;
            let version = select_version(&metadata, requirement)?;
            let selected = &metadata["versions"][&version];
            if selected["name"].as_str() != Some(name)
                || selected["version"].as_str() != Some(version.as_str())
            {
                return Err(err("Registry package identity mismatch"));
            }
            PackageLock {
                name: name.into(),
                requirement: requirement.into(),
                version,
                integrity: selected["dist"]["integrity"]
                    .as_str()
                    .ok_or_else(|| err("Package integrity required"))?
                    .into(),
                tarball: selected["dist"]["tarball"]
                    .as_str()
                    .ok_or_else(|| err("Package tarball required"))?
                    .into(),
            }
        };
        if ancestors
            .iter()
            .any(|(n, v)| n == name && v == &pin.version)
        {
            return Ok(());
        }
        use sha2::Digest;
        let cache_key = sha2::Sha256::digest(pin.integrity.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let cache = self
            .root
            .join(".napi-vm/cache")
            .join(format!("{cache_key}.tgz"));
        let archive = match self.permissions.read_bytes(&cache, self.max_bytes) {
            Ok(bytes) => bytes,
            Err(error) if error.to_string().contains("No such file") => {
                let bytes = self.download(&pin.tarball)?;
                verify_integrity(&bytes, &pin.integrity)?;
                self.permissions
                    .create_dir_all(cache.parent().ok_or_else(|| err("Invalid cache path"))?)?;
                self.permissions
                    .write_bytes(&cache, &bytes, self.max_bytes)?;
                bytes
            }
            Err(error) => return Err(error),
        };
        verify_integrity(&archive, &pin.integrity)?;
        let entries = unpack_archive(&archive, self.max_bytes.saturating_mul(4))?;
        let package_json = entries
            .iter()
            .find(|(path, _)| path == Path::new("package.json"))
            .ok_or_else(|| err("Archive has no package.json"))?;
        let manifest: Json =
            serde_json::from_slice(&package_json.1).map_err(|e| err(e.to_string()))?;
        if manifest["name"].as_str() != Some(name)
            || manifest["version"].as_str() != Some(pin.version.as_str())
        {
            return Err(err("Archive package identity mismatch"));
        }
        let directory = self.root.join("node_modules").join(&relative);
        self.permissions.create_dir_all(&directory)?;
        for (path, bytes) in entries {
            let target = directory.join(path);
            self.permissions
                .create_dir_all(target.parent().ok_or_else(|| err("Invalid archive path"))?)?;
            self.permissions
                .write_bytes(target, &bytes, self.max_bytes)?;
        }
        self.installed += 1;
        self.lock.packages.insert(key, pin.clone());
        ancestors.push((name.into(), pin.version));
        if let Some(dependencies) = manifest["dependencies"].as_object() {
            for (dependency, range) in dependencies {
                let range = range
                    .as_str()
                    .ok_or_else(|| err("Invalid dependency version"))?;
                let (dependency_name, _) = Self::request(dependency)?;
                self.install_package(
                    &dependency_name,
                    range,
                    relative.join("node_modules").join(&dependency_name),
                    ancestors,
                )?;
            }
        }
        ancestors.pop();
        Ok(())
    }
}
/// Validate an entire archive before writing any package source. No archive
/// links, device nodes, parent components, or duplicate file names are accepted.
pub fn unpack_archive(bytes: &[u8], max_bytes: usize) -> Result<Vec<(PathBuf, Vec<u8>)>, VmErr> {
    use std::io::Read;
    let decoder =
        flate2::read::GzDecoder::new(bytes).take(max_bytes.saturating_add(512 * 10002) as u64);
    let mut archive = tar::Archive::new(decoder);
    let mut files = Vec::new();
    let mut total = 0usize;
    let mut seen = std::collections::HashSet::new();
    let mut count = 0;
    for entry in archive.entries().map_err(|e| err(e.to_string()))? {
        count += 1;
        if count > 10000 {
            return Err(err("ResourceLimit: package archive entry count exceeded"));
        }
        let mut entry = entry.map_err(|e| err(e.to_string()))?;
        let path = entry.path().map_err(|e| err(e.to_string()))?.into_owned();
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(err("Unsafe package archive path"));
        }
        let path = path
            .strip_prefix("package")
            .map_err(|_| err("Package archive must use the package/ root"))?
            .to_path_buf();
        if entry.header().entry_type().is_dir() {
            continue;
        }
        if !entry.header().entry_type().is_file()
            || path.as_os_str().is_empty()
            || !seen.insert(path.clone())
        {
            return Err(err("Unsupported or duplicate archive entry"));
        }
        let size =
            usize::try_from(entry.size()).map_err(|_| err("Package archive size exceeded"))?;
        total = total
            .checked_add(size)
            .filter(|n| *n <= max_bytes)
            .ok_or_else(|| err("ResourceLimit: unpacked package size exceeded"))?;
        if files.len() >= 10000 {
            return Err(err("ResourceLimit: package file count exceeded"));
        }
        let mut content = Vec::new();
        entry
            .by_ref()
            .take(size as u64 + 1)
            .read_to_end(&mut content)
            .map_err(|e| err(e.to_string()))?;
        if content.len() != size {
            return Err(err("Truncated package archive"));
        }
        files.push((path, content));
    }
    Ok(files)
}
