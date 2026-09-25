//! Guest module graph collection and package/exports resolution.

use super::*;

pub(super) fn collect_guest_module_graph(
    root: &Path,
    entry: &Path,
    name: &str,
    max_file_bytes: u64,
) -> Result<(GuestModuleSources, GuestModuleAliases), PluginHostError> {
    let mut pending = vec![(entry.to_path_buf(), None)];
    let mut seen = HashSet::new();
    let mut sources = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    while let Some((path, imported_by)) = pending.pop() {
        let canonical = fs::canonicalize(&path).map_err(|error| {
            PluginHostError::Load(format!("cannot resolve guest module: {error}"))
        })?;
        if !canonical.starts_with(root) {
            return Err(PluginHostError::Load(
                "relative module import escapes the plugin directory".into(),
            ));
        }
        let id = module_id(root, &canonical, name)?;
        if let Some((importer, specifier)) = imported_by {
            aliases.insert((importer, specifier), id.clone());
        }
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let extension = canonical
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        if !matches!(extension, "js" | "mjs" | "json") {
            return Err(PluginHostError::Load(format!(
                "unsupported guest module extension .{extension}"
            )));
        }
        let bytes = read_limited(&canonical, max_file_bytes)?;
        let source = std::str::from_utf8(&bytes)
            .map_err(|_| PluginHostError::Load("guest modules must be UTF-8".into()))?;
        let guest_source = if extension == "json" {
            let json: JsonValue = serde_json::from_str(source).map_err(|error| {
                PluginHostError::Load(format!("guest JSON module is invalid: {error}"))
            })?;
            format!("export default {};", serde_json::to_string(&json).unwrap())
        } else {
            source.to_owned()
        };
        sources.insert(id.clone(), guest_source);
        if extension == "json" {
            continue;
        }
        for specifier in static_module_specifiers(source)? {
            if specifier.contains(':') && !specifier.starts_with('.') {
                // Namespaced names resolve from host-installed capability
                // modules (node:fs, tiktools:events, ...), never from
                // files; unknown ones fail at import time.
                continue;
            }
            let target = if specifier.starts_with('.') {
                Some(resolve_relative_module(root, &canonical, &specifier)?)
            } else {
                resolve_plugin_package(root, &canonical, &specifier, max_file_bytes)?
            };
            if let Some(target) = target {
                pending.push((target, Some((id.clone(), specifier))));
            }
        }
    }
    Ok((
        sources.into_iter().collect(),
        aliases
            .into_iter()
            .map(|((importer, specifier), target)| (importer, specifier, target))
            .collect(),
    ))
}

pub(super) fn resolve_plugin_package(
    root: &Path,
    importer: &Path,
    request: &str,
    max_file_bytes: u64,
) -> Result<Option<PathBuf>, PluginHostError> {
    let (package_name, subpath) = split_plugin_package_request(request)?;
    let mut directory = importer.parent().unwrap_or(root);
    loop {
        if directory.starts_with(root)
            && directory
                .file_name()
                .is_none_or(|name| name != "node_modules")
        {
            let candidate = directory.join("node_modules").join(&package_name);
            if candidate.is_dir() {
                let package_root = fs::canonicalize(&candidate).map_err(|error| {
                    PluginHostError::Load(format!("cannot resolve package {package_name}: {error}"))
                })?;
                if !package_root.starts_with(root) {
                    return Err(PluginHostError::Load(format!(
                        "package {package_name} escapes the plugin directory"
                    )));
                }
                return resolve_plugin_package_entry(
                    root,
                    &package_root,
                    &package_name,
                    &subpath,
                    max_file_bytes,
                )
                .map(Some);
            }
        }
        if directory == root {
            break;
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        if !parent.starts_with(root) {
            break;
        }
        directory = parent;
    }
    Ok(None)
}

pub(super) fn split_plugin_package_request(
    request: &str,
) -> Result<(String, String), PluginHostError> {
    if request.is_empty()
        || request.starts_with('.')
        || request.starts_with('/')
        || request.starts_with('#')
        || request.contains('\\')
        || request.contains(':')
        || request.contains('\0')
        || request.chars().any(char::is_whitespace)
    {
        return Err(PluginHostError::Load(format!(
            "unsupported guest package import '{request}'"
        )));
    }
    let parts: Vec<_> = request.split('/').collect();
    let package_end = if request.starts_with('@') { 2 } else { 1 };
    if parts.len() < package_end || parts[..package_end].iter().any(|part| part.is_empty()) {
        return Err(PluginHostError::Load(format!(
            "invalid guest package import '{request}'"
        )));
    }
    let package_name = parts[..package_end].join("/");
    let subpath = parts[package_end..].join("/");
    if package_name == "@" || parts.iter().any(|part| matches!(*part, "." | "..")) {
        return Err(PluginHostError::Load(format!(
            "invalid guest package import '{request}'"
        )));
    }
    Ok((package_name, subpath))
}

pub(super) fn resolve_plugin_package_entry(
    root: &Path,
    package_root: &Path,
    package_name: &str,
    subpath: &str,
    max_file_bytes: u64,
) -> Result<PathBuf, PluginHostError> {
    let manifest_path = package_root.join("package.json");
    let manifest = if manifest_path.is_file() {
        let bytes = read_limited(&manifest_path, max_file_bytes)?;
        serde_json::from_slice::<JsonValue>(&bytes).map_err(|error| {
            PluginHostError::Load(format!(
                "package {package_name} has invalid package.json: {error}"
            ))
        })?
    } else {
        JsonValue::Null
    };
    let target = if let Some(exports) = manifest.get("exports") {
        let export_key = if subpath.is_empty() {
            ".".to_string()
        } else {
            format!("./{subpath}")
        };
        let target = plugin_exports_target(exports, &export_key)?.ok_or_else(|| {
            PluginHostError::Load(format!(
                "package {package_name} does not export subpath {export_key} for ESM import"
            ))
        })?;
        package_root.join(target)
    } else if subpath.is_empty() {
        let entry = manifest
            .get("module")
            .and_then(JsonValue::as_str)
            .or_else(|| manifest.get("main").and_then(JsonValue::as_str))
            .unwrap_or("index");
        package_root.join(entry)
    } else {
        package_root.join(subpath)
    };
    resolve_plugin_package_file(root, package_root, &target).map_err(|error| {
        PluginHostError::Load(format!(
            "cannot resolve ESM entry for package {package_name}: {error}"
        ))
    })
}

pub(super) fn plugin_exports_target(
    exports: &JsonValue,
    key: &str,
) -> Result<Option<String>, PluginHostError> {
    let selected = match exports {
        JsonValue::String(_) | JsonValue::Array(_) | JsonValue::Null if key == "." => {
            select_plugin_export_condition(exports)?
        }
        JsonValue::Object(entries) if entries.keys().any(|entry| entry.starts_with('.')) => {
            if let Some(target) = entries.get(key) {
                select_plugin_export_condition(target)?
            } else {
                let mut best: Option<(usize, usize, String, &JsonValue)> = None;
                for (pattern, target) in entries {
                    let Some(capture) = match_plugin_export_pattern(pattern, key) else {
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
                if let Some((_, _, capture, target)) = best {
                    select_plugin_export_condition(target)?
                        .map(|target| target.replace('*', &capture))
                } else {
                    None
                }
            }
        }
        JsonValue::Object(_) if key == "." => select_plugin_export_condition(exports)?,
        _ => None,
    };
    if let Some(target) = selected {
        let Some(relative) = target.strip_prefix("./") else {
            return Err(PluginHostError::Load(format!(
                "unsupported package exports target '{target}'"
            )));
        };
        if relative.is_empty()
            || relative.contains('\\')
            || relative.split('/').any(|part| matches!(part, "." | ".."))
        {
            return Err(PluginHostError::Load(format!(
                "unsupported package exports target '{target}'"
            )));
        }
        Ok(Some(target))
    } else {
        Ok(None)
    }
}

pub(super) fn select_plugin_export_condition(
    value: &JsonValue,
) -> Result<Option<String>, PluginHostError> {
    match value {
        JsonValue::String(target) => Ok(Some(target.clone())),
        JsonValue::Array(targets) => {
            for target in targets {
                if let Some(selected) = select_plugin_export_condition(target)? {
                    return Ok(Some(selected));
                }
            }
            Ok(None)
        }
        JsonValue::Object(conditions) => {
            for (condition, value) in conditions {
                if matches!(condition.as_str(), "import" | "node" | "default")
                    && let Some(target) = select_plugin_export_condition(value)?
                {
                    return Ok(Some(target));
                }
            }
            Ok(None)
        }
        JsonValue::Null => Ok(None),
        _ => Err(PluginHostError::Load(
            "package exports entry must be a string, array, condition object, or null".into(),
        )),
    }
}

pub(super) fn match_plugin_export_pattern(pattern: &str, key: &str) -> Option<String> {
    let star = pattern.find('*')?;
    let prefix = &pattern[..star];
    let suffix = &pattern[star + 1..];
    let capture = key.strip_prefix(prefix)?.strip_suffix(suffix)?;
    (!capture.is_empty()).then(|| capture.to_string())
}

pub(super) fn resolve_plugin_package_file(
    root: &Path,
    package_root: &Path,
    target: &Path,
) -> Result<PathBuf, String> {
    let candidates = if target.extension().is_some() {
        vec![target.to_path_buf()]
    } else {
        vec![
            target.with_extension("mjs"),
            target.with_extension("js"),
            target.with_extension("json"),
            target.join("index.mjs"),
            target.join("index.js"),
            target.join("index.json"),
        ]
    };
    for candidate in candidates {
        let Ok(canonical) = fs::canonicalize(&candidate) else {
            continue;
        };
        if !canonical.starts_with(root) || !canonical.starts_with(package_root) {
            return Err("package target escapes its plugin or package root".into());
        }
        if !canonical.is_file() {
            continue;
        }
        let extension = canonical
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        if !matches!(extension, "js" | "mjs" | "json") {
            return Err(format!("unsupported package module extension .{extension}"));
        }
        return Ok(canonical);
    }
    Err(format!(
        "package target does not exist: {}",
        target.display()
    ))
}

pub(super) fn static_module_specifiers(source: &str) -> Result<Vec<String>, PluginHostError> {
    let tokens = Lexer::new(source).tokenize_with_spans();
    let mut parser = Parser::new_with_spans(tokens);
    let statements = parser
        .parse_program()
        .map_err(|error| PluginHostError::Load(format!("guest module parse failed: {error}")))?;
    let mut modules = Vec::new();
    for statement in statements {
        match statement {
            Statement::Import { module, .. } | Statement::ExportAll { source: module, .. } => {
                modules.push(module);
            }
            Statement::ExportNamed {
                source: Some(module),
                ..
            } => modules.push(module),
            _ => {}
        }
    }
    Ok(modules)
}

pub(super) fn resolve_relative_module(
    root: &Path,
    parent: &Path,
    request: &str,
) -> Result<PathBuf, PluginHostError> {
    let request = request.replace('\\', "/");
    let mut base = parent.parent().unwrap_or(root).to_path_buf();
    for component in request.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if !base.pop() || !base.starts_with(root) {
                    return Err(PluginHostError::Load(
                        "relative module import escapes the plugin directory".into(),
                    ));
                }
            }
            part => base.push(part),
        }
    }
    let candidates = if base.extension().is_some() {
        vec![base.clone()]
    } else {
        vec![
            base.with_extension("js"),
            base.with_extension("mjs"),
            base.with_extension("json"),
            base.join("index.js"),
            base.join("index.mjs"),
        ]
    };
    for candidate in candidates {
        if let Ok(canonical) = fs::canonicalize(&candidate) {
            if canonical.starts_with(root) && canonical.is_file() {
                return Ok(canonical);
            }
            return Err(PluginHostError::Load(
                "relative module import escapes the plugin directory".into(),
            ));
        }
    }
    Err(PluginHostError::Load(format!(
        "cannot resolve relative module \"{request}\""
    )))
}

pub(super) fn module_id(root: &Path, path: &Path, name: &str) -> Result<String, PluginHostError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| PluginHostError::Load("guest module is outside plugin directory".into()))?;
    let relative = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    Ok(format!("./plugin:{name}/{relative}"))
}
