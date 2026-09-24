//! Sandboxed `node:fs` facade backed by compiled permission rules.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum FsOperation {
    Read,
    Write,
}

#[derive(Debug)]
pub(super) struct GuestCallError {
    name: &'static str,
    message: String,
}

pub(super) struct PluginFileSystem {
    root: PathBuf,
    requested_permissions: FsPermissions,
    host_permissions: FsPermissions,
    max_file_bytes: u64,
}

impl PluginFileSystem {
    pub(super) fn new(
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

    pub(super) fn resolve(
        &self,
        requested: &str,
        operation: FsOperation,
    ) -> Result<PathBuf, GuestCallError> {
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

    pub(super) fn read_text(&self, requested: &str) -> Result<String, GuestCallError> {
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

    pub(super) fn write_text(&self, requested: &str, contents: &str) -> Result<(), GuestCallError> {
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

    pub(super) fn exists(&self, requested: &str) -> Result<bool, GuestCallError> {
        let path = self.resolve(requested, FsOperation::Read)?;
        Ok(path.exists())
    }
}

pub(super) fn native_path_string(path: &str) -> String {
    if std::path::MAIN_SEPARATOR == '/' {
        path.to_owned()
    } else {
        path.replace('/', std::path::MAIN_SEPARATOR_STR)
    }
}

pub(super) fn slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(super) fn canonicalize_missing_tail(path: &Path) -> std::io::Result<PathBuf> {
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

pub(super) fn permission_error(message: &str) -> GuestCallError {
    GuestCallError {
        name: "PermissionDenied",
        message: message.to_owned(),
    }
}

pub(super) fn resource_error(message: &str) -> GuestCallError {
    GuestCallError {
        name: "ResourceLimit",
        message: message.to_owned(),
    }
}

pub(super) fn io_guest_error(message: &str) -> GuestCallError {
    GuestCallError {
        name: "Error",
        message: message.to_owned(),
    }
}

pub(super) fn guest_result(result: Result<Value, GuestCallError>) -> Value {
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
