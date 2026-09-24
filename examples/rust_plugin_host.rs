//! Run the repository's JavaScript plugin from a Rust host without Node.js.
//!
//! This is an embedding example, not a general-purpose plugin manager. It
//! shows the important boundary: guest modules use `node:fs` and `node:path`,
//! while the Rust host owns filesystem access and checks both plugin requests
//! and host policy before each I/O operation.
//!
//! Run with:
//! `cargo run --no-default-features --example rust-plugin-host -- examples/plugins/example-plugin`

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use napi_vm::{HostBridge, Interpreter, Value, VmErr};
use serde::Deserialize;

const FS_READ: usize = 1;
const FS_WRITE: usize = 2;
const FS_EXISTS: usize = 3;
const PATH_JOIN: usize = 4;
const PATH_SEP: usize = 5;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    name: String,
    version: String,
    api_version: u32,
    entry: String,
    permissions: Option<ManifestPermissions>,
}

#[derive(Debug, Default, Deserialize)]
struct ManifestPermissions {
    fs: Option<FsPermissions>,
    path: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct FsPermissions {
    read: Option<Permission>,
    write: Option<Permission>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Permission {
    Boolean(bool),
    Pattern(String),
    Patterns(Vec<String>),
}

impl Permission {
    fn patterns(&self) -> Vec<&str> {
        match self {
            Self::Boolean(true) => vec!["*"],
            Self::Boolean(false) => Vec::new(),
            Self::Pattern(pattern) => vec![pattern],
            Self::Patterns(patterns) => patterns.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Clone, Copy)]
enum FsOperation {
    Read,
    Write,
}

struct PluginCapabilities {
    root: PathBuf,
    requested: FsPermissions,
    // The host policy is independent of plugin.json. A permission is active
    // only when both sets match the canonical path.
    policy_read: Vec<String>,
    policy_write: Vec<String>,
    enable_path: bool,
}

impl PluginCapabilities {
    fn permission_allows(&self, operation: FsOperation, relative: &Path) -> bool {
        let (requested, policy) = match operation {
            FsOperation::Read => (&self.requested.read, &self.policy_read),
            FsOperation::Write => (&self.requested.write, &self.policy_write),
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        requested.as_ref().is_some_and(|permission| {
            permission
                .patterns()
                .into_iter()
                .any(|pattern| pattern_matches(pattern, &relative))
        }) && policy
            .iter()
            .any(|pattern| pattern_matches(pattern, &relative))
    }

    fn resolve_guest_path(&self, requested: &str) -> Result<(PathBuf, PathBuf), String> {
        let request_path = Path::new(requested);
        let candidate = if request_path.is_absolute() {
            request_path.to_path_buf()
        } else {
            self.root.join(request_path)
        };

        // Canonicalize existing paths through symlinks. If the requested path
        // does not exist yet, find its nearest existing ancestor, canonicalize
        // that ancestor, and append the missing suffix before checking root
        // confinement. This also makes existsSync return false for a missing
        // nested path instead of treating it as an I/O failure.
        let canonical = match candidate.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut unresolved = Vec::<OsString>::new();
                let mut ancestor = candidate.as_path();
                loop {
                    match ancestor.canonicalize() {
                        Ok(mut canonical_ancestor) => {
                            for component in unresolved.iter().rev() {
                                canonical_ancestor.push(component);
                            }
                            break canonical_ancestor;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            let name = ancestor.file_name().ok_or_else(|| {
                                format!("path is not permitted for {requested:?}")
                            })?;
                            unresolved.push(name.to_os_string());
                            ancestor = ancestor.parent().ok_or_else(|| {
                                format!("path is not permitted for {requested:?}")
                            })?;
                        }
                        Err(_) => {
                            return Err(format!("path is not permitted for {requested:?}"));
                        }
                    }
                }
            }
            Err(_) => return Err(format!("path is not permitted for {requested:?}")),
        };

        let relative = canonical
            .strip_prefix(&self.root)
            .map_err(|_| format!("path is not permitted for {requested:?}"))?
            .to_path_buf();
        Ok((canonical, relative))
    }

    fn read_text(&self, requested: &str) -> Result<String, String> {
        let (path, relative) = self.resolve_guest_path(requested)?;
        if !self.permission_allows(FsOperation::Read, &relative) {
            return Err(format!("fs.read is not permitted for {requested:?}"));
        }
        let metadata = fs::metadata(&path)
            .map_err(|_| format!("cannot read requested plugin file {requested:?}"))?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return Err("plugin file exceeds the host read limit".into());
        }
        fs::read_to_string(path)
            .map_err(|_| format!("cannot read requested plugin file {requested:?}"))
    }

    fn write_text(&self, requested: &str, contents: &str) -> Result<(), String> {
        if contents.len() as u64 > MAX_FILE_BYTES {
            return Err("plugin write exceeds the host write limit".into());
        }
        let (path, relative) = self.resolve_guest_path(requested)?;
        if !self.permission_allows(FsOperation::Write, &relative) {
            return Err(format!("fs.write is not permitted for {requested:?}"));
        }
        fs::write(path, contents)
            .map_err(|_| format!("cannot write requested plugin file {requested:?}"))
    }

    fn exists(&self, requested: &str) -> Result<bool, String> {
        let (path, relative) = self.resolve_guest_path(requested)?;
        if !self.permission_allows(FsOperation::Read, &relative) {
            return Err(format!("fs.read is not permitted for {requested:?}"));
        }
        Ok(path.is_file())
    }
}

impl HostBridge for PluginCapabilities {
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        let result = match id {
            FS_READ => result_value(
                args.first()
                    .and_then(value_string)
                    .ok_or_else(|| "path must be a string".to_string())
                    .and_then(|path| self.read_text(path))
                    .map(Value::String),
            ),
            FS_WRITE => {
                let path = args.first().and_then(value_string);
                let contents = args.get(1).and_then(value_string);
                result_value(match (path, contents) {
                    (Some(path), Some(contents)) => {
                        self.write_text(path, contents).map(|()| Value::Undefined)
                    }
                    _ => Err("path and contents must be strings".into()),
                })
            }
            FS_EXISTS => result_value(
                args.first()
                    .and_then(value_string)
                    .ok_or_else(|| "path must be a string".to_string())
                    .and_then(|path| self.exists(path))
                    .map(Value::Bool),
            ),
            PATH_JOIN => {
                let mut joined = PathBuf::new();
                for part in &args {
                    let Some(part) = value_string(part) else {
                        return Err(VmErr::Msg("path segments must be strings".into()));
                    };
                    if !part.is_empty() {
                        joined.push(part);
                    }
                }
                Value::String(joined.to_string_lossy().into_owned())
            }
            PATH_SEP => Value::String(std::path::MAIN_SEPARATOR.to_string()),
            _ => {
                return Err(VmErr::Msg(format!(
                    "unknown Rust plugin host function {id}"
                )));
            }
        };
        Ok(result)
    }
}

fn value_string(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) => Some(value),
        _ => None,
    }
}

fn result_value(result: Result<Value, String>) -> Value {
    match result {
        Ok(value) => Value::object(vec![
            ("ok".into(), Value::Bool(true)),
            ("value".into(), value),
        ]),
        Err(message) => Value::object(vec![
            ("ok".into(), Value::Bool(false)),
            ("name".into(), Value::String("PermissionDenied".into())),
            ("message".into(), Value::String(message)),
        ]),
    }
}

fn pattern_matches(pattern: &str, relative: &str) -> bool {
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    if pattern == "*" || pattern == "**" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return relative == prefix || relative.starts_with(&format!("{prefix}/"));
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        let Some(rest) = relative.strip_prefix(&format!("{prefix}/")) else {
            return false;
        };
        return !rest.is_empty() && !rest.contains('/');
    }
    pattern == relative
}

fn expose_host_function(interpreter: &mut Interpreter, name: &str, id: usize) {
    interpreter
        .global
        .borrow_mut()
        .set(name, Value::host_function(name, id));
}

fn run() -> Result<(), String> {
    let plugin_dir = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/plugins/example-plugin"));
    let root = fs::canonicalize(&plugin_dir)
        .map_err(|error| format!("cannot open plugin directory: {error}"))?;
    let manifest_path = root
        .join("plugin.json")
        .canonicalize()
        .map_err(|error| format!("cannot resolve plugin.json: {error}"))?;
    if !manifest_path.starts_with(&root) || !manifest_path.is_file() {
        return Err("plugin.json must be a file inside its plugin directory".into());
    }
    let manifest_size = fs::metadata(&manifest_path)
        .map_err(|error| format!("cannot inspect plugin.json: {error}"))?
        .len();
    if manifest_size > MAX_FILE_BYTES {
        return Err("plugin.json exceeds the host read limit".into());
    }
    let manifest_source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("cannot read plugin.json: {error}"))?;
    let manifest: Manifest = serde_json::from_str(&manifest_source)
        .map_err(|error| format!("invalid plugin.json: {error}"))?;
    if manifest.api_version != 1 {
        return Err(format!(
            "unsupported plugin API version {}",
            manifest.api_version
        ));
    }
    let permissions = manifest.permissions.unwrap_or_default();
    let requested_fs = permissions.fs.unwrap_or_default();
    let entry_path = Path::new(&manifest.entry);
    if entry_path.is_absolute()
        || entry_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("plugin entry must be a relative path without `..`".into());
    }
    let plugin_entry = root
        .join(entry_path)
        .canonicalize()
        .map_err(|error| format!("cannot resolve plugin entry: {error}"))?;
    if !plugin_entry.starts_with(&root) || !plugin_entry.is_file() {
        return Err("plugin entry must be a file inside its plugin directory".into());
    }
    if fs::metadata(&plugin_entry)
        .map_err(|error| format!("cannot inspect plugin entry: {error}"))?
        .len()
        > MAX_FILE_BYTES
    {
        return Err("plugin entry exceeds the host read limit".into());
    }
    let plugin_source = fs::read_to_string(&plugin_entry)
        .map_err(|error| format!("cannot read plugin entry: {error}"))?;

    // These grants belong to the application and remain narrower than any
    // permissions requested by a plugin manifest.
    let policy_read = vec!["config.json".into(), "assets/**".into()];
    let policy_write = vec!["cache/**".into()];
    let policy_path = true;
    let capabilities = Rc::new(PluginCapabilities {
        root,
        requested: requested_fs,
        policy_read,
        policy_write,
        enable_path: permissions.path.unwrap_or(false) && policy_path,
    });
    let mut interpreter = Interpreter::with_builtins();
    interpreter.set_host_bridge(capabilities.clone());

    expose_host_function(&mut interpreter, "__nvm_fs_read", FS_READ);
    expose_host_function(&mut interpreter, "__nvm_fs_write", FS_WRITE);
    expose_host_function(&mut interpreter, "__nvm_fs_exists", FS_EXISTS);
    if capabilities.enable_path {
        expose_host_function(&mut interpreter, "__nvm_path_join", PATH_JOIN);
        expose_host_function(&mut interpreter, "__nvm_path_sep", PATH_SEP);
    }

    interpreter.define_module(
        "node:fs",
        r#"
const hostRead = __nvm_fs_read;
const hostWrite = __nvm_fs_write;
const hostExists = __nvm_fs_exists;
function unwrapResult(result) {
  if (result.ok) return result.value;
  const error = new Error(result.message);
  error.name = result.name;
  throw error;
}
export function readFileSync(path, encoding) {
  if (encoding !== undefined && encoding !== "utf8" && encoding !== "utf-8") {
    throw new TypeError("the Rust plugin host supports UTF-8 text reads only");
  }
  return unwrapResult(hostRead(path));
}
export function writeFileSync(path, contents) {
  return unwrapResult(hostWrite(path, contents));
}
export function existsSync(path) {
  return unwrapResult(hostExists(path));
}
"#
        .to_string(),
    );
    if capabilities.enable_path {
        interpreter.define_module(
            "node:path",
            r#"
const hostJoin = __nvm_path_join;
const hostSep = __nvm_path_sep;
export function join(...parts) { return hostJoin(...parts); }
export const sep = hostSep();
"#
            .to_string(),
        );
    }

    // Evaluate facades once so their export records retain the host functions,
    // then remove the bootstrap-only globals before loading guest source.
    if !interpreter
        .ensure_module("node:fs")
        .map_err(|error| error.to_string())?
    {
        return Err("failed to register node:fs facade".into());
    }
    for name in ["__nvm_fs_read", "__nvm_fs_write", "__nvm_fs_exists"] {
        interpreter.global.borrow_mut().remove(name);
    }
    if capabilities.enable_path {
        if !interpreter
            .ensure_module("node:path")
            .map_err(|error| error.to_string())?
        {
            return Err("failed to register node:path facade".into());
        }
        for name in ["__nvm_path_join", "__nvm_path_sep"] {
            interpreter.global.borrow_mut().remove(name);
        }
    }

    let module_id = "plugin:desktop-plugin";
    interpreter.define_module(module_id, plugin_source);
    let context = serde_json::json!({
        "name": manifest.name,
        "version": manifest.version,
    });
    let load_result = interpreter
        .eval_source(&format!(
            "import Plugin from '{module_id}'; globalThis.__plugin = new Plugin(); globalThis.__plugin.onLoad({context});"
        ))
        .map_err(|error| format!("plugin onLoad failed: {error}"))?;
    println!("plugin loaded; onLoad returned {load_result:?}");

    let unload_context = serde_json::json!({
        "name": manifest.name,
        "version": manifest.version,
        "reason": "unload",
    });
    let unload_state = interpreter
        .eval_source(&format!("globalThis.__plugin.onUnload({unload_context});"))
        .map_err(|error| format!("plugin onUnload failed: {error}"))?;
    println!("plugin unloaded; state {unload_state:?}");

    let status_path = capabilities.root.join("cache/status.json");
    if let Ok(status) = fs::read_to_string(status_path) {
        println!("plugin wrote cache/status.json: {}", status.trim());
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("rust-plugin-host: {error}");
        std::process::exit(1);
    }
}
