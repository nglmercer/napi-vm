//! Package `exports`/`imports` resolution and guest module `require()`.

use super::*;

pub(super) fn read_package_json(path: &Path) -> Result<JsonValue, VmErr> {
    let source = fs::read_to_string(path)
        .map_err(|error| VmErr::Msg(format!("cannot read {}: {error}", path.display())))?;
    serde_json::from_str(&source)
        .map_err(|error| VmErr::Msg(format!("invalid {}: {error}", path.display())))
}

pub(super) fn split_package_request(request: &str) -> Result<(String, String), VmErr> {
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

pub(super) fn exports_target(
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

pub(super) enum ImportTarget {
    Relative(String),
    External(String),
}

pub(super) enum ImportTargetSelection {
    Target(ImportTarget),
    NoTarget,
    Invalid(String),
}

pub(super) fn imports_target(
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

pub(super) fn select_import_target(
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

pub(super) fn is_bare_import_target(target: &str) -> bool {
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

pub(super) fn match_export_pattern(pattern: &str, key: &str) -> Option<String> {
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

pub(super) enum ExportTargetSelection {
    Target(String),
    NoTarget,
    Invalid(String),
}

pub(super) fn finish_export_selection(
    selection: ExportTargetSelection,
) -> Result<Option<String>, VmErr> {
    match selection {
        ExportTargetSelection::Target(target) => Ok(Some(target)),
        ExportTargetSelection::NoTarget => Ok(None),
        ExportTargetSelection::Invalid(message) => Err(VmErr::Msg(message)),
    }
}

pub(super) fn select_export_target(
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

pub(super) fn normalize_exports_target(target: &str) -> Option<String> {
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

pub(super) fn percent_decode_path(path: &str) -> Option<String> {
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

pub(super) fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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
    if let Some(module) = resolve_guest_module_builtin(interp, request) {
        return Ok(Value::String(module.filename));
    }
    let module = loader.resolve(request, parent)?;
    Ok(Value::String(module.filename))
}

pub(super) fn resolve_guest_module_builtin(
    interp: &crate::interpreter::Interpreter,
    request: &str,
) -> Option<ResolvedCommonJsModule> {
    let module_name = match request {
        "fs" | "node:fs" => "node:fs",
        "path" | "node:path" => "node:path",
        _ => return None,
    };
    let registered = interp.module_sources.borrow().contains_key(module_name)
        || interp.modules.borrow().contains_key(module_name);
    registered.then(|| ResolvedCommonJsModule {
        id: format!("napi-vm:guest-module:{module_name}"),
        filename: request.to_owned(),
        format: CommonJsModuleFormat::RuntimeBuiltin,
        source: None,
    })
}
