//! Guest-facing capability modules installed into each plugin realm.

use super::*;

pub(super) fn install_fs_module(
    interpreter: &mut Interpreter,
    bridge: &PluginHostBridge,
    filesystem: Rc<PluginFileSystem>,
    module_ids: &mut Vec<String>,
    bridge_globals: &mut Vec<String>,
) {
    let read_fs = filesystem.clone();
    let read_name = "__napi_vm_plugin_fs_read";
    expose_plugin_function(interpreter, bridge, read_name, move |args| {
        let Some(Value::String(path)) = args.first() else {
            return Ok(guest_result(Err(permission_error(
                "fs.read requires a non-empty path string",
            ))));
        };
        Ok(guest_result(read_fs.read_text(path).map(Value::String)))
    })
    .expect("bootstrap callback registration is bounded");
    let write_fs = filesystem.clone();
    let write_name = "__napi_vm_plugin_fs_write";
    expose_plugin_function(interpreter, bridge, write_name, move |args| {
        let (Some(Value::String(path)), Some(Value::String(contents))) =
            (args.first(), args.get(1))
        else {
            return Ok(guest_result(Err(permission_error(
                "writeFileSync(path, contents): path and contents must be strings",
            ))));
        };
        Ok(guest_result(
            write_fs
                .write_text(path, contents)
                .map(|()| Value::Undefined),
        ))
    })
    .expect("bootstrap callback registration is bounded");
    let exists_fs = filesystem;
    let exists_name = "__napi_vm_plugin_fs_exists";
    expose_plugin_function(interpreter, bridge, exists_name, move |args| {
        let Some(Value::String(path)) = args.first() else {
            return Ok(guest_result(Err(permission_error(
                "fs.exists requires a non-empty path string",
            ))));
        };
        Ok(guest_result(exists_fs.exists(path).map(Value::Bool)))
    })
    .expect("bootstrap callback registration is bounded");
    bridge_globals.extend([read_name.into(), write_name.into(), exists_name.into()]);
    interpreter.define_module(
        "node:fs",
        r#"
const hostRead = __napi_vm_plugin_fs_read;
const hostWrite = __napi_vm_plugin_fs_write;
const hostExists = __napi_vm_plugin_fs_exists;
function unwrap(result) {
  if (result.ok) return result.value;
  const error = new Error(result.message);
  error.name = result.name;
  throw error;
}
export function readFileSync(path, encoding) {
  if (encoding !== "utf8" && encoding !== "utf-8") {
    throw new TypeError("the sandboxed node:fs facade supports UTF-8 text reads only");
  }
  return unwrap(hostRead(path));
}
export function writeFileSync(path, contents) {
  if (typeof contents !== "string") {
    throw new TypeError("writeFileSync(path, contents): contents must be a string");
  }
  unwrap(hostWrite(path, contents));
}
export function existsSync(path) {
  return unwrap(hostExists(path));
}
"#
        .into(),
    );
    module_ids.push("node:fs".into());
}

pub(super) fn install_path_module(
    interpreter: &mut Interpreter,
    bridge: &PluginHostBridge,
    module_ids: &mut Vec<String>,
    bridge_globals: &mut Vec<String>,
) {
    let mut exports = Vec::new();
    for (operation, export_name) in [
        (0, "join"),
        (1, "normalize"),
        (2, "dirname"),
        (3, "basename"),
        (4, "extname"),
        (5, "resolve"),
        (6, "relative"),
        (7, "isAbsolute"),
    ] {
        let global_name = format!("__napi_vm_plugin_path_{operation}");
        expose_plugin_function(interpreter, bridge, &global_name, move |args| {
            let strings = args.iter().map(value_to_guest_string).collect::<Vec<_>>();
            Ok(path_operation(operation, &strings))
        })
        .expect("bootstrap callback registration is bounded");
        bridge_globals.push(global_name.clone());
        exports.push((export_name, global_name));
    }
    let mut source = String::new();
    for (export_name, global_name) in exports {
        source.push_str(&format!(
            "const __host_{export_name} = {global_name};\nexport function {export_name}(...parts) {{ return __host_{export_name}(...parts.map(String)); }}\n"
        ));
    }
    source.push_str(&format!(
        "export const sep = {};\n",
        serde_json::to_string(std::path::MAIN_SEPARATOR_STR).unwrap()
    ));
    interpreter.define_module("node:path", source);
    module_ids.push("node:path".into());
}

pub(super) fn install_custom_capability(
    interpreter: &mut Interpreter,
    bridge: &PluginHostBridge,
    capability: &RustPluginCapability,
    module_ids: &mut Vec<String>,
    bridge_globals: &mut Vec<String>,
) -> Result<(), PluginHostError> {
    let mut source = String::new();
    for (index, (export_name, callback)) in capability.exports.iter().enumerate() {
        let global_name = format!(
            "__napi_vm_cap_{}_{}",
            sanitize_global(&capability.name),
            index
        );
        expose_plugin_function(interpreter, bridge, &global_name, {
            let callback = callback.clone();
            move |args| callback(args)
        })?;
        bridge_globals.push(global_name.clone());
        source.push_str(&format!(
            "const __host_{index} = {global_name};\nexport function {export_name}(...args) {{ return __host_{index}(...args); }}\n"
        ));
    }
    interpreter.define_module(&capability.name, source);
    module_ids.push(capability.name.clone());
    Ok(())
}

pub(super) fn sanitize_global(value: &str) -> String {
    value.bytes().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn value_to_guest_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => crate::format::number_string(*value),
        Value::Bool(value) => value.to_string(),
        Value::Null => "null".into(),
        Value::Undefined => "undefined".into(),
        other => format!("{other:?}"),
    }
}
