//! Plugin preparation and `onLoad`/`onUnload`/`onReload` lifecycle driving.

use super::*;

pub(super) fn prepare_plugin(
    directory: &Path,
    max_file_bytes: u64,
) -> Result<PreparedPlugin, PluginHostError> {
    let root = fs::canonicalize(directory).map_err(|error| {
        PluginHostError::Io(format!("cannot resolve plugin directory: {error}"))
    })?;
    if !root.is_dir() {
        return Err(PluginHostError::Load(
            "plugin directory is not a directory".into(),
        ));
    }
    let manifest_path = root.join(PLUGIN_MANIFEST_FILENAME);
    let manifest_path = fs::canonicalize(&manifest_path).map_err(|_| {
        PluginHostError::Load(format!(
            "missing {PLUGIN_MANIFEST_FILENAME} in plugin directory"
        ))
    })?;
    if !manifest_path.starts_with(&root) || !manifest_path.is_file() {
        return Err(PluginHostError::Load(
            "plugin manifest must be a file inside its directory".into(),
        ));
    }
    let manifest_bytes = read_limited(&manifest_path, max_file_bytes)?;
    let manifest_source = std::str::from_utf8(&manifest_bytes)
        .map_err(|_| PluginHostError::Manifest("plugin.json must be UTF-8".into()))?;
    let manifest = parse_manifest(manifest_source)?;
    let (fs_permissions, path_enabled, capability_requests) =
        parse_permissions(&manifest.permissions)?;
    let entry_relative = validate_entry_path(&manifest.entry)?;
    let entry_path = root.join(&entry_relative);
    let entry_path = fs::canonicalize(&entry_path)
        .map_err(|_| PluginHostError::Load(format!("entry file not found: {}", manifest.entry)))?;
    if !entry_path.starts_with(&root) || !entry_path.is_file() {
        return Err(PluginHostError::Manifest(
            "entry must be a file inside the plugin directory".into(),
        ));
    }
    let (mut sources, module_aliases) =
        collect_guest_module_graph(&root, &entry_path, &manifest.name, max_file_bytes)?;
    let relative_entry_id = module_id(&root, &entry_path, &manifest.name)?;
    let entry_id = relative_entry_id
        .strip_prefix("./")
        .expect("module ids have a relative alias")
        .to_owned();
    // The host bootstrap has no importing module, so it uses a bare virtual
    // entry ID. A re-export wrapper keeps the actual entry in its canonical
    // path module, where package aliases remain scoped to that source file.
    let entry_filename = entry_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| PluginHostError::Load("entry filename must be UTF-8".into()))?;
    sources.push((
        entry_id.clone(),
        format!(
            "export {{ default }} from {:?};",
            format!("./{entry_filename}")
        ),
    ));
    Ok(PreparedPlugin {
        manifest,
        root,
        entry_path,
        entry_id,
        sources,
        module_aliases,
        fs_permissions,
        path_enabled,
        capability_requests,
    })
}

pub(super) fn lifecycle_bootstrap(entry_id: &str) -> String {
    let entry = serde_json::to_string(entry_id).expect("module id is serializable");
    format!(
        r#"
import __Plugin from {entry};
const __pluginInstance = typeof __Plugin === "function" ? new __Plugin() : __Plugin;
function __plugin_describe() {{
  return {{ hasInstance: __pluginInstance !== undefined && __pluginInstance !== null }};
}}
function __plugin_onLoad(context) {{
  if (__pluginInstance && typeof __pluginInstance.onLoad === "function") return __pluginInstance.onLoad(context);
}}
function __plugin_onUnload(context) {{
  if (__pluginInstance && typeof __pluginInstance.onUnload === "function") return __pluginInstance.onUnload(context);
}}
function __plugin_onReload(context, previousState) {{
  if (__pluginInstance && typeof __pluginInstance.onReload === "function") return __pluginInstance.onReload(context, previousState);
  return __plugin_onLoad(context);
}}
undefined;
"#
    )
}

pub(super) fn context_value(manifest: &RustPluginManifest, reason: Option<&str>) -> JsonValue {
    let mut context = serde_json::Map::new();
    context.insert("name".into(), JsonValue::String(manifest.name.clone()));
    context.insert(
        "version".into(),
        JsonValue::String(manifest.version.clone()),
    );
    if let Some(reason) = reason {
        context.insert("reason".into(), JsonValue::String(reason.into()));
    }
    JsonValue::Object(context)
}

/// Invoke one guest lifecycle hook directly: no generated source, no
/// parsing, no JSON trampoline. A missing hook (`None`, or a non-function
/// value) is not an error — it behaves like the old wrapper's
/// `typeof hook === "function"` guard and yields no state. An `undefined`
/// result likewise yields no state; anything else converts to JSON with
/// the same serializer semantics `JSON.stringify` would apply.
pub(super) fn invoke_plugin_hook(
    interpreter: &mut Interpreter,
    function: Option<&Value>,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Option<JsonValue>, PluginHostError> {
    let Some(function) = function else {
        return Ok(None);
    };
    if !crate::interpreter::call::is_callable_value(function) {
        return Ok(None);
    }
    let result = interpreter.call_host_function(function, receiver, args)?;
    if matches!(result, Value::Undefined) {
        return Ok(None);
    }
    crate::convert::value_to_json(interpreter, &result)
        .map(Some)
        .map_err(PluginHostError::from)
}

/// Resolve a lifecycle hook by name from the cached plugin instance.
/// Member lookup (with getter semantics) preserves the dynamic dispatch
/// the generated wrappers had; only the parse/eval round-trip is gone.
pub(super) fn resolve_hook(
    interpreter: &mut Interpreter,
    instance: &Value,
    name: &str,
) -> Result<Option<Value>, PluginHostError> {
    let hook = interpreter.member(instance, name)?;
    if crate::interpreter::call::is_callable_value(&hook) {
        Ok(Some(hook))
    } else {
        Ok(None)
    }
}

/// Resolve one hook by name and invoke it with JSON arguments. Arguments
/// convert directly to guest values; the awaited result converts back,
/// with `undefined` (or a missing hook) yielding no state.
pub(super) fn invoke_named_hook(
    plugin: &mut RustLoadedPlugin,
    name: &str,
    args: Vec<JsonValue>,
) -> Result<Option<JsonValue>, PluginHostError> {
    let instance = plugin.plugin_instance.clone();
    let hook = resolve_hook(&mut plugin.interpreter, &instance, name)?;
    let mut converted = Vec::with_capacity(args.len());
    for arg in &args {
        converted.push(crate::convert::value_from_json(arg)?);
    }
    invoke_plugin_hook(&mut plugin.interpreter, hook.as_ref(), instance, converted)
}
