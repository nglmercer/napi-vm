//! `plugin.json` parsing, permission compilation, and entry validation.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PermissionRule {
    pub(super) absolute: bool,
    pub(super) pattern: String,
}

#[derive(Clone, Debug, Default)]
pub(super) struct FsPermissions {
    pub(super) read: Vec<PermissionRule>,
    pub(super) write: Vec<PermissionRule>,
}
pub(super) fn parse_manifest(source: &str) -> Result<RustPluginManifest, PluginHostError> {
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

pub(super) fn required_nonempty_string(
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

pub(super) fn parse_permissions(
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

pub(super) fn compile_permission_rules(
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

pub(super) fn compile_host_fs_permissions(
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

pub(super) fn compile_one_pattern(
    pattern: &str,
    field: &str,
) -> Result<PermissionRule, PluginHostError> {
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

pub(super) fn validate_entry_path(entry: &str) -> Result<PathBuf, PluginHostError> {
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
