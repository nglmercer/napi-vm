use napi_vm_plugin_protocol::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Explicit OS, architecture and optional libc identity for a native artifact.
pub struct Target {
    pub os: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub libc: Option<String>,
}
impl Target {
    pub fn current() -> Self {
        Self {
            os: match std::env::consts::OS {
                "macos" => "darwin",
                "windows" => "win32",
                o => o,
            }
            .into(),
            arch: match std::env::consts::ARCH {
                "x86_64" => "x64",
                "aarch64" => "arm64",
                a => a,
            }
            .into(),
            libc: if cfg!(target_os = "linux") {
                Some(
                    if cfg!(target_env = "musl") {
                        "musl"
                    } else {
                        "gnu"
                    }
                    .into(),
                )
            } else {
                None
            },
        }
    }
    pub fn matches_current(&self) -> bool {
        let t = Self::current();
        self.os == t.os && self.arch == t.arch && self.libc == t.libc
    }
    pub fn validate(&self) -> PluginResult<()> {
        if !["linux", "darwin", "win32"].contains(&self.os.as_str())
            || !["x64", "arm64", "ia32", "arm"].contains(&self.arch.as_str())
            || (self.os == "linux" && !matches!(self.libc.as_deref(), Some("gnu" | "musl")))
            || (self.os != "linux" && self.libc.is_some())
        {
            return Err(invalid(
                "target requires explicit supported OS, architecture and Linux libc",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProtocolRange {
    pub major: u32,
    pub min_minor: u32,
    pub max_minor: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
/// Declared launch route: supported JavaScript runtime or a target-specific executable.
pub enum Launch {
    Javascript {
        entry: String,
        runtimes: Vec<String>,
        #[serde(rename = "preferredRuntime")]
        preferred_runtime: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(
            default,
            rename = "versions",
            skip_serializing_if = "BTreeMap::is_empty"
        )]
        runtime_versions: BTreeMap<String, String>,
    },
    Executable {
        entry: String,
        #[serde(default)]
        args: Vec<String>,
        target: Target,
    },
}
impl Launch {
    pub fn entry(&self) -> &str {
        match self {
            Self::Javascript { entry, .. } | Self::Executable { entry, .. } => entry,
        }
    }
    pub fn args(&self) -> &[String] {
        match self {
            Self::Javascript { args, .. } | Self::Executable { args, .. } => args,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Development {
    pub entry: String,
    pub runtime: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeDependency {
    pub name: String,
    pub path: String,
    pub target: Target,
    pub abi: String,
    pub runtime: String,
    pub status: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceRequirement {
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub readiness: Option<ServiceReadiness>,
    #[serde(default)]
    pub shutdown: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServiceReadiness {
    pub path: String,
    pub status: u16,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependencies {
    #[serde(default)]
    pub native: Vec<NativeDependency>,
    #[serde(default)]
    pub services: Vec<ServiceRequirement>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Validated package metadata describing contracts, launch settings and dependencies.
/// Runtime connection credentials belong in `LoadOptions`, never in this document.
pub struct Manifest {
    pub manifest_version: u32,
    pub execution: String,
    pub id: String,
    pub version: String,
    pub protocol: ProtocolRange,
    pub provides: BTreeMap<String, String>,
    #[serde(default)]
    pub requires_host: BTreeMap<String, String>,
    pub launch: Launch,
    #[serde(default)]
    pub assets: Vec<String>,
    #[serde(default)]
    pub contracts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub development: Option<Development>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default)]
    pub dependencies: Dependencies,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Value>,
}
#[derive(Clone)]
/// Preflight result with validated contracts and one selected launch command.
/// Preparing JavaScript plugins probes their declared runtime; native preparation does
/// not invoke Node, Bun or npm. This is authoring/introspection data, not a running instance.
pub struct Prepared {
    pub manifest_path: PathBuf,
    pub directory: PathBuf,
    pub manifest: Manifest,
    pub contracts: BTreeMap<String, Contract>,
    pub command: PathBuf,
    pub arguments: Vec<String>,
    pub selected_runtime: String,
    pub runtime_version: String,
    pub native_paths: BTreeMap<String, Value>,
}
#[derive(Clone, Default)]
/// Explicit runtime paths/override and integrity/development policy.
/// `LoadOptions::default()` enables integrity verification; constructing this type
/// directly with its default leaves `verify_integrity` false for authoring preflight.
pub struct RuntimeOptions {
    pub runtime: Option<String>,
    pub runtime_paths: BTreeMap<String, PathBuf>,
    pub development: bool,
    pub verify_integrity: bool,
}
fn invalid(m: impl Into<String>) -> RpcError {
    RpcError::new("INVALID_ARGUMENT", m)
}
pub fn relative_path(value: &str) -> PluginResult<PathBuf> {
    let s = value.strip_prefix("./").unwrap_or(value);
    if s.is_empty()
        || s.len() > 4096
        || s.contains('\0')
        || s.contains('\\')
        || s.starts_with('/')
        || s.contains(':')
        || s.split('/').any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err(invalid(format!("invalid package-relative path {value}")));
    }
    Ok(PathBuf::from(s))
}
pub async fn contained(root: &Path, value: &str) -> PluginResult<PathBuf> {
    let r = relative_path(value)?;
    let path = tokio::fs::canonicalize(root.join(r))
        .await
        .map_err(|e| invalid(format!("missing packaged path {value}: {e}")))?;
    if !path.starts_with(root) || path == root {
        return Err(invalid(
            "packaged path or symlink escapes package directory",
        ));
    }
    Ok(path)
}
pub async fn read_manifest(
    path: &Path,
    verify_integrity: bool,
) -> PluginResult<(Manifest, PathBuf, BTreeMap<String, Contract>)> {
    let full = tokio::fs::canonicalize(path)
        .await
        .map_err(|e| invalid(format!("manifest is unavailable: {e}")))?;
    let root = full
        .parent()
        .ok_or_else(|| invalid("manifest has no parent"))?
        .to_path_buf();
    let bytes = tokio::fs::read(&full).await?;
    if bytes.len() > 1024 * 1024 {
        return Err(invalid("manifest exceeds 1 MiB"));
    }
    let v: Value = serde_json::from_slice(&bytes)?;
    if v["manifestVersion"] != 2 || v["execution"] != "trusted-process" {
        return Err(invalid(
            "wrong manifest format: trusted host requires manifestVersion 2 and execution trusted-process; use the legacy VM host for old plugin.json files",
        ));
    }
    let m: Manifest = serde_json::from_value(v)?;
    validate_manifest(&m)?;
    contained(&root, m.launch.entry()).await?;
    for a in &m.assets {
        contained(&root, a).await?;
    }
    let mut contracts = BTreeMap::new();
    for p in &m.contracts {
        let bytes = tokio::fs::read(contained(&root, p).await?).await?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(invalid("contract artifact exceeds 8 MiB"));
        }
        let c = Contract::from_value(serde_json::from_slice(&bytes)?)?;
        if contracts.insert(c.id().into(), c).is_some() {
            return Err(invalid("duplicate contract artifact ID"));
        }
    }
    for (id, v) in m.provides.iter().chain(&m.requires_host) {
        if contracts.get(id).is_none_or(|c| c.version() != v) {
            return Err(RpcError::new(
                "CONTRACT_MISMATCH",
                format!("missing or wrong-version generated contract for {id}"),
            ));
        }
    }
    for n in &m.dependencies.native {
        contained(&root, &n.path).await?;
    }
    if verify_integrity {
        verify_lock(&root, &m, &contracts).await?;
    }
    Ok((m, root, contracts))
}
pub fn validate_manifest(m: &Manifest) -> PluginResult<()> {
    if !identifier(&m.id) || !version(&m.version) {
        return Err(invalid("invalid plugin identifier or semantic version"));
    }
    if m.protocol.major != 1
        || m.protocol.min_minor > 0
        || m.protocol.min_minor > m.protocol.max_minor
    {
        return Err(RpcError::new(
            "PROTOCOL_MISMATCH",
            "protocol v1.0 is required",
        ));
    }
    if m.provides.is_empty()
        || m.provides.len() > 128
        || m.requires_host.len() > 128
        || m.assets.len() > 4096
        || m.contracts.len() > 256
        || m.dependencies.services.len() > 128
        || m.dependencies.native.len() > 256
        || m.dependencies.capabilities.len() > 128
    {
        return Err(invalid("manifest array/map bounds exceeded"));
    }
    for (id, v) in m.provides.iter().chain(&m.requires_host) {
        if !identifier(id) || !version(v) {
            return Err(invalid("invalid interface ID or semantic version"));
        }
    }
    if m.launch.args().len() > 256
        || m.launch
            .args()
            .iter()
            .any(|a| a.len() > 32768 || a.contains('\0'))
    {
        return Err(invalid("invalid launch arguments"));
    }
    relative_path(m.launch.entry())?;
    match &m.launch {
        Launch::Javascript {
            entry,
            runtimes,
            preferred_runtime,
            runtime_versions,
            ..
        } => {
            let mut set = BTreeSet::new();
            if runtimes.is_empty()
                || runtimes.len() > 2
                || runtimes
                    .iter()
                    .any(|r| !["bun", "node"].contains(&r.as_str()) || !set.insert(r))
                || !runtimes.contains(preferred_runtime)
            {
                return Err(invalid(
                    "runtimes must be unique node/bun names and include preferredRuntime",
                ));
            }
            if !entry.ends_with(".mjs") && !entry.ends_with(".js") {
                return Err(invalid(
                    "production JavaScript entry must be emitted .js/.mjs",
                ));
            }
            for (r, v) in runtime_versions {
                if !runtimes.contains(r) || !version(v.trim_start_matches(">=")) {
                    return Err(invalid(
                        "versions supports exact or >= major.minor.patch versions",
                    ));
                }
            }
        }
        Launch::Executable { target, .. } => {
            target.validate()?;
            if m.profile
                .as_deref()
                .is_some_and(|p| p != "native-executable")
            {
                return Err(invalid(
                    "executable launch requires native-executable profile",
                ));
            }
        }
    }
    if let Some(p) = &m.profile {
        if !["portable-js", "external-runtime", "native-executable"].contains(&p.as_str()) {
            return Err(invalid("unknown artifact profile"));
        }
        if p == "portable-js" && !m.dependencies.native.is_empty() {
            return Err(invalid("portable-js rejects native dependencies"));
        }
    }
    if let Some(d) = &m.development {
        relative_path(&d.entry)?;
        if d.runtime != "bun" && d.runtime != "node" {
            return Err(invalid("development runtime must be bun or node"));
        }
        if d.runtime == "node" && d.entry.ends_with(".ts") {
            return Err(invalid("Node development requires emitted JavaScript"));
        }
    }
    let mut names = BTreeSet::new();
    for n in &m.dependencies.native {
        if !identifier(&n.name)
            || !names.insert(&n.name)
            || n.abi.is_empty()
            || n.abi.len() > 128
            || !["tested", "buildable", "distributed"].contains(&n.status.as_str())
            || !["bun", "node", "native"].contains(&n.runtime.as_str())
        {
            return Err(invalid("invalid native dependency declaration"));
        }
        n.target.validate()?;
        relative_path(&n.path)?;
    }
    names.clear();
    for s in &m.dependencies.services {
        match s.owner.as_deref().unwrap_or("external") {
            "external" => {
                if s.readiness.is_some() || s.shutdown.is_some() {
                    return Err(invalid(
                        "external services cannot declare host readiness/shutdown",
                    ));
                }
            }
            "host" => {
                let r = s
                    .readiness
                    .as_ref()
                    .ok_or_else(|| invalid("host services require HTTP readiness"))?;
                if !r.path.starts_with('/')
                    || r.path.chars().any(|c| c.is_control())
                    || !(200..=599).contains(&r.status)
                    || s.shutdown.as_deref() != Some("terminate")
                {
                    return Err(invalid(
                        "host service requires safe readiness path/status and shutdown:terminate",
                    ));
                }
            }
            _ => return Err(invalid("unknown service owner")),
        }
        if !identifier(&s.name)
            || !names.insert(&s.name)
            || !["http", "mcp"].contains(&s.kind.as_str())
        {
            return Err(invalid("invalid or duplicate external service requirement"));
        }
    }
    let mut caps = BTreeSet::new();
    for c in &m.dependencies.capabilities {
        if !identifier(c) || !caps.insert(c) {
            return Err(invalid("invalid or duplicate host capability"));
        }
    }
    for e in m.extensions.keys() {
        if !e.contains('.') || !identifier(e) {
            return Err(invalid("extensions require namespaced ASCII keys"));
        }
    }
    Ok(())
}
async fn verify_lock(
    root: &Path,
    m: &Manifest,
    contracts: &BTreeMap<String, Contract>,
) -> PluginResult<()> {
    let bytes=tokio::fs::read(root.join("plugin.lock.json")).await.map_err(|_|invalid("production package requires plugin.lock.json; build/pack explicitly, or choose development mode"))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(invalid("package inventory too large"));
    }
    let lock: Value = serde_json::from_slice(&bytes)?;
    let o = lock
        .as_object()
        .ok_or_else(|| invalid("invalid package inventory"))?;
    if o.keys().any(|k| {
        ![
            "lockVersion",
            "pluginId",
            "pluginVersion",
            "artifact",
            "interfaces",
            "files",
        ]
        .contains(&k.as_str())
    }) || lock["lockVersion"] != 1
        || lock["pluginId"] != m.id
        || lock["pluginVersion"] != m.version
    {
        return Err(invalid("package inventory identity mismatch"));
    }
    let interfaces = lock["interfaces"]
        .as_object()
        .ok_or_else(|| invalid("inventory interfaces missing"))?;
    for (id, c) in contracts {
        if interfaces.get(id) != Some(&serde_json::to_value(c.identity())?) {
            return Err(RpcError::new(
                "CONTRACT_MISMATCH",
                "inventory contract identity mismatch",
            ));
        }
    }
    let artifact = &lock["artifact"];
    if artifact["profile"]
        != m.profile.clone().unwrap_or_else(|| {
            if matches!(m.launch, Launch::Executable { .. }) {
                "native-executable"
            } else {
                "external-runtime"
            }
            .into()
        })
    {
        return Err(invalid("inventory artifact profile mismatch"));
    }
    match &m.launch {
        Launch::Executable { target, .. } => {
            if artifact["target"] != serde_json::to_value(target)?
                || artifact["abi"] != "native-executable"
            {
                return Err(invalid("inventory executable target/ABI mismatch"));
            }
        }
        Launch::Javascript { runtimes, .. } => {
            if artifact["runtime"] != serde_json::to_value(runtimes)?
                || artifact["abi"] != "javascript-esm"
            {
                return Err(invalid("inventory JS runtime/ABI mismatch"));
            }
        }
    }
    let files = lock["files"]
        .as_object()
        .filter(|f| !f.is_empty() && f.len() <= 100_000)
        .ok_or_else(|| invalid("inventory files missing or exceeds bounds"))?;
    let required = std::iter::once("plugin.json")
        .chain(std::iter::once(m.launch.entry()))
        .chain(m.contracts.iter().map(String::as_str))
        .chain(m.assets.iter().map(String::as_str))
        .chain(m.dependencies.native.iter().map(|n| n.path.as_str()));
    for p in required {
        let normalized = relative_path(p)?.to_string_lossy().replace('\\', "/");
        if !files.contains_key(&normalized)
            && !files
                .keys()
                .any(|k| k.starts_with(&(normalized.clone() + "/")))
        {
            return Err(invalid(format!(
                "required packaged path {p} is missing from inventory"
            )));
        }
    }
    for (p, hash) in files {
        let h = hash
            .as_str()
            .filter(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
            .ok_or_else(|| invalid("invalid SHA256 inventory digest"))?;
        let path = contained(root, p).await?;
        let data = tokio::fs::read(&path).await?;
        if format!("{:x}", Sha256::digest(&data)) != h {
            return Err(invalid(format!("package integrity failure for {p}")));
        }
    }
    Ok(())
}
async fn executable(name: &str, paths: &BTreeMap<String, PathBuf>) -> PluginResult<PathBuf> {
    if let Some(p) = paths.get(name) {
        let p = tokio::fs::canonicalize(p)
            .await
            .map_err(|_| invalid(format!("configured {name} executable missing")))?;
        return Ok(p);
    }
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        for suffix in if cfg!(windows) {
            vec![".exe", ""]
        } else {
            vec![""]
        } {
            let p = dir.join(format!("{name}{suffix}"));
            if tokio::fs::metadata(&p).await.is_ok_and(|m| m.is_file()) {
                return tokio::fs::canonicalize(p).await.map_err(Into::into);
            }
        }
    }
    Err(invalid(format!(
        "runtime {name} was not found; install it explicitly or configure runtime_paths"
    )))
}
pub async fn prepare(path: &Path, options: &RuntimeOptions) -> PluginResult<Prepared> {
    let (m, root, contracts) =
        read_manifest(path, options.verify_integrity && !options.development).await?;
    let mut runtime_napi = 0u32;
    let mut runtime_target = Target::current();
    let (command, mut arguments, selected_runtime, runtime_version) = match &m.launch {
        Launch::Executable {
            entry,
            args,
            target,
        } => {
            if options.runtime.is_some() {
                return Err(invalid(
                    "runtime override does not apply to executable artifacts",
                ));
            }
            if !target.matches_current() {
                return Err(invalid(
                    "executable target does not match current OS/architecture/libc",
                ));
            }
            (
                contained(&root, entry).await?,
                args.clone(),
                "executable".into(),
                "native".into(),
            )
        }
        Launch::Javascript {
            entry,
            runtimes,
            preferred_runtime,
            args,
            runtime_versions,
        } => {
            let (entry, forced) = if options.development {
                if let Some(d) = &m.development {
                    (d.entry.as_str(), Some(d.runtime.clone()))
                } else {
                    (entry.as_str(), None)
                }
            } else {
                (entry.as_str(), None)
            };
            let explicit = options.runtime.clone().or(forced);
            if explicit.as_ref().is_some_and(|r| !runtimes.contains(r)) {
                return Err(invalid("selected runtime is not supported by the plugin"));
            }
            let mut order = if let Some(r) = explicit {
                vec![r]
            } else {
                let mut r = vec![preferred_runtime.clone()];
                r.extend(runtimes.iter().filter(|r| *r != preferred_runtime).cloned());
                r
            };
            let mut selected = None;
            let mut errors = Vec::new();
            for runtime in order.drain(..) {
                let probe = async {
                    let path = executable(&runtime, &options.runtime_paths).await?;
                    let out=probe_output(&path,&["--version"]).await?;
                    let v = String::from_utf8_lossy(&out)
                        .trim()
                        .trim_start_matches('v')
                        .to_string();
                    if !version(&v) {
                        return Err(invalid("runtime returned invalid version"));
                    }
                    if let Some(requirement) = runtime_versions.get(&runtime) {
                        let parse = |s: &str| {
                            s.split('.')
                                .map(|x| x.parse::<u32>().unwrap_or(0))
                                .collect::<Vec<_>>()
                        };
                        let ok = if let Some(min) = requirement.strip_prefix(">=") {
                            parse(&v) >= parse(min)
                        } else {
                            &v == requirement
                        };
                        if !ok {
                            return Err(invalid(
                                "runtime version does not meet manifest requirement",
                            ));
                        }
                    }
                    let script="process.stdout.write(JSON.stringify({versions:process.versions,target:{os:process.platform,arch:process.arch,...(process.platform==='linux'?{libc:process.report?.getReport().header.glibcVersionRuntime?'gnu':'musl'}:{})}}))";
                    let details:Value=serde_json::from_slice(&probe_output(&path,&["-e",script]).await?)?;
                    if !details["versions"]["node"].is_string() || (runtime=="bun")!=details["versions"]["bun"].is_string(){return Err(invalid("configured executable is the wrong JavaScript runtime"));}
                    let napi=details["versions"]["napi"].as_str().and_then(|s|s.parse::<u32>().ok()).unwrap_or(0);
                    let target:Target=serde_json::from_value(details["target"].clone())?;target.validate()?;
                    Ok::<_, RpcError>((path, v,napi,target))
                }
                .await;
                match probe {
                    Ok((p, v, napi, target)) => {
                        runtime_napi = napi;
                        runtime_target = target;
                        selected = Some((runtime, p, v));
                        break;
                    }
                    Err(e) => errors.push(e.message),
                }
            }
            let (runtime, path, v) = selected.ok_or_else(|| {
                invalid(format!(
                    "no declared runtime available: {}",
                    errors.join("; ")
                ))
            })?;
            let file = contained(&root, entry).await?;
            if runtime == "node" && file.extension().is_some_and(|e| e == "ts") {
                return Err(invalid("Node requires emitted JS"));
            }
            let mut a = if runtime == "bun" {
                vec!["--no-install".into()]
            } else {
                vec![]
            };
            // Windows canonicalization produces verbatim (\\?\) paths, which
            // JavaScript entry-point loaders need not support. The child cwd
            // is the verified package root, so pass the contained canonical
            // file relative to that root, retaining argument/path boundaries.
            a.push(javascript_entry_argument(&root, &file)?);
            a.extend(args.clone());
            (path, a, runtime, v)
        }
    };
    let mut native_paths = BTreeMap::new();
    for n in &m.dependencies.native {
        if n.target.os == runtime_target.os
            && n.target.arch == runtime_target.arch
            && n.target.libc == runtime_target.libc
            && (n.runtime == selected_runtime
                || n.runtime == "native" && selected_runtime == "executable")
        {
            if n.status != "tested" {
                return Err(invalid(format!(
                    "native dependency {} has no runtime-tested support claim for selected target",
                    n.name
                )));
            }
            if selected_runtime != "executable" {
                let abi = n
                    .abi
                    .strip_prefix("napi-")
                    .filter(|v| !v.starts_with('0'))
                    .and_then(|v| v.parse::<u32>().ok())
                    .filter(|v| *v > 0)
                    .ok_or_else(|| {
                        invalid("JavaScript native dependency must declare napi-N ABI")
                    })?;
                if abi > runtime_napi {
                    return Err(invalid(
                        "native dependency requires newer Node-API ABI than selected runtime",
                    ));
                }
            }
            native_paths.insert(n.name.clone(),serde_json::json!({"path":contained(&root,&n.path).await?.to_string_lossy().into_owned(),"target":n.target,"abi":n.abi,"runtime":n.runtime}));
        } else {
            return Err(invalid(format!(
                "native dependency {} is incompatible with selected runtime/target",
                n.name
            )));
        }
    }
    arguments.shrink_to_fit();
    Ok(Prepared {
        manifest_path: tokio::fs::canonicalize(path).await?,
        directory: root,
        manifest: m,
        contracts,
        command,
        arguments,
        selected_runtime,
        runtime_version,
        native_paths,
    })
}

fn javascript_entry_argument(root: &Path, file: &Path) -> PluginResult<String> {
    let entry = file
        .strip_prefix(root)
        .map_err(|_| invalid("JavaScript entry escapes package directory"))?;
    if entry.as_os_str().is_empty()
        || entry
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(invalid("invalid contained JavaScript entry"));
    }
    // The leading dot also prevents an entry named -script.js becoming a
    // runtime command-line option. Path joining preserves Unicode and spaces.
    Ok(Path::new(".").join(entry).to_string_lossy().into_owned())
}

async fn probe_output(path: &Path, args: &[&str]) -> PluginResult<Vec<u8>> {
    let mut command = Command::new(path);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let mut pipe = child
        .stdout
        .take()
        .ok_or_else(|| invalid("runtime probe stdout unavailable"))?
        .take(8193);
    // Use the same bounded version-probe deadline as the JS host.
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut out = Vec::new();
        pipe.read_to_end(&mut out).await?;
        if out.len() > 8192 {
            return Err(invalid("runtime probe output exceeds 8 KiB"));
        }
        let status = child.wait().await?;
        if !status.success() {
            return Err(invalid("runtime probe exited unsuccessfully"));
        }
        Ok(out)
    })
    .await;
    match result {
        Ok(Ok(out)) => Ok(out),
        other => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            match other {
                Ok(Err(e)) => Err(e),
                Err(_) => Err(invalid("runtime probe timed out")),
                _ => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod launch_tests {
    use super::*;

    #[test]
    fn javascript_entries_are_relative_to_the_verified_package() {
        #[cfg(windows)]
        let root = Path::new(r"\\?\C:\package with spaces");
        #[cfg(not(windows))]
        let root = Path::new("/package with spaces");
        for name in ["dist/main.js", "-entry.mjs", "dist/日本語 file.js"] {
            let argument = javascript_entry_argument(root, &root.join(name)).unwrap();
            assert!(Path::new(&argument).is_relative());
            assert!(argument.starts_with('.'));
            assert_eq!(Path::new(&argument), Path::new(".").join(name));
            assert!(!argument.contains("package with spaces"));
        }
        assert!(javascript_entry_argument(root, root).is_err());
        assert!(javascript_entry_argument(root, &root.join("../escape.js")).is_err());
        assert!(
            javascript_entry_argument(root, &root.with_file_name("other").join("entry.js"))
                .is_err()
        );
    }
}
