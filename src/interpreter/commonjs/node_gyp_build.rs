//! `node-gyp-build` compatibility helpers for guest packages.

use super::*;

pub(super) fn make_node_gyp_build(
    interp: &mut crate::interpreter::Interpreter,
) -> Result<Value, VmErr> {
    const LOAD_SOURCE: &str =
        "(function nodeGypBuild(directory) { return __napi_vm_node_gyp_build_load(directory); })";
    const RESOLVE_SOURCE: &str = "(function nodeGypBuildResolve(directory) { return __napi_vm_node_gyp_build_resolve(directory); })";
    const PARSE_TAGS_SOURCE: &str =
        "(function parseTags(file) { return __napi_vm_node_gyp_build_parse_tags(file); })";
    const MATCH_TAGS_SOURCE: &str = "(function matchTags(runtime, abi) { return function match(tags) { return __napi_vm_node_gyp_build_match_tags(runtime, abi, tags); }; })";
    const COMPARE_TAGS_SOURCE: &str = "(function compareTags(runtime) { return function compare(a, b) { return __napi_vm_node_gyp_build_compare_tags(runtime, a, b); }; })";
    const PARSE_TUPLE_SOURCE: &str =
        "(function parseTuple(name) { return __napi_vm_node_gyp_build_parse_tuple(name); })";
    const MATCH_TUPLE_SOURCE: &str = "(function matchTuple(platform, architecture) { return function match(tuple) { return __napi_vm_node_gyp_build_match_tuple(platform, architecture, tuple); }; })";
    const COMPARE_TUPLES_SOURCE: &str =
        "(function compareTuples(a, b) { return __napi_vm_node_gyp_build_compare_tuples(a, b); })";

    let outer = interp.push_scope();
    let old_source_lines = std::mem::take(&mut interp.source_lines);
    let result = (|| {
        interp.set_binding(
            "__napi_vm_node_gyp_build_load",
            Value::NativeFunction {
                name: "node-gyp-build".into(),
                callable: node_gyp_build_load,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_resolve",
            Value::NativeFunction {
                name: "node-gyp-build.resolve".into(),
                callable: node_gyp_build_resolve,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_parse_tags",
            Value::NativeFunction {
                name: "node-gyp-build.parseTags".into(),
                callable: node_gyp_build_parse_tags,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_match_tags",
            Value::NativeFunction {
                name: "node-gyp-build.matchTags".into(),
                callable: node_gyp_build_match_tags,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_compare_tags",
            Value::NativeFunction {
                name: "node-gyp-build.compareTags".into(),
                callable: node_gyp_build_compare_tags,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_parse_tuple",
            Value::NativeFunction {
                name: "node-gyp-build.parseTuple".into(),
                callable: node_gyp_build_parse_tuple,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_match_tuple",
            Value::NativeFunction {
                name: "node-gyp-build.matchTuple".into(),
                callable: node_gyp_build_match_tuple,
            },
        )?;
        interp.set_binding(
            "__napi_vm_node_gyp_build_compare_tuples",
            Value::NativeFunction {
                name: "node-gyp-build.compareTuples".into(),
                callable: node_gyp_build_compare_tuples,
            },
        )?;
        let load = compile_guest_function(interp, LOAD_SOURCE)?;
        let resolve = compile_guest_function(interp, RESOLVE_SOURCE)?;
        let parse_tags = compile_guest_function(interp, PARSE_TAGS_SOURCE)?;
        let match_tags = compile_guest_function(interp, MATCH_TAGS_SOURCE)?;
        let compare_tags = compile_guest_function(interp, COMPARE_TAGS_SOURCE)?;
        let parse_tuple = compile_guest_function(interp, PARSE_TUPLE_SOURCE)?;
        let match_tuple = compile_guest_function(interp, MATCH_TUPLE_SOURCE)?;
        let compare_tuples = compile_guest_function(interp, COMPARE_TUPLES_SOURCE)?;
        load.set_prop("path".into(), resolve.clone())?;
        load.set_prop("resolve".into(), resolve)?;
        load.set_prop("parseTags".into(), parse_tags)?;
        load.set_prop("matchTags".into(), match_tags)?;
        load.set_prop("compareTags".into(), compare_tags)?;
        load.set_prop("parseTuple".into(), parse_tuple)?;
        load.set_prop("matchTuple".into(), match_tuple)?;
        load.set_prop("compareTuples".into(), compare_tuples)?;
        Ok(load)
    })();
    interp.pop_scope(outer);
    interp.source_lines = old_source_lines;
    result
}

pub(super) fn node_gyp_build_parse_tags(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let Some(Value::String(filename)) = args.first() else {
        return Err(VmErr::Msg(
            "TypeError: node-gyp-build.parseTags expects a filename string".into(),
        ));
    };
    let Some(tags) = parse_prebuild_tags(filename) else {
        return Ok(Value::Undefined);
    };
    prebuild_tags_to_value(tags)
}

pub(super) fn prebuild_tags_to_value(tags: PrebuildTags) -> Result<Value, VmErr> {
    let mut entries = vec![
        ("file".to_string(), Value::String(tags.file)),
        (
            "specificity".to_string(),
            Value::Number(tags.specificity as f64),
        ),
    ];
    for field in tags.field_order {
        match field.as_str() {
            "runtime" => {
                if let Some(value) = &tags.runtime {
                    entries.push((field, Value::String(value.clone())));
                }
            }
            "napi" if tags.napi => entries.push((field, Value::Bool(true))),
            "abi" => {
                if let Some(value) = &tags.abi {
                    entries.push((field, Value::String(value.clone())));
                }
            }
            "uv" => {
                if let Some(value) = &tags.uv {
                    entries.push((field, Value::String(value.clone())));
                }
            }
            "armv" => {
                if let Some(value) = &tags.armv {
                    entries.push((field, Value::String(value.clone())));
                }
            }
            "libc" => {
                if let Some(value) = &tags.libc {
                    entries.push((field, Value::String(value.clone())));
                }
            }
            _ => {}
        }
    }
    Value::checked_object(entries)
}

pub(super) fn node_gyp_build_match_tags(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let runtime = args.first().and_then(value_string).unwrap_or_default();
    let Some(tags) = args.get(2) else {
        return Ok(Value::Bool(false));
    };
    match_tags_for_rust_node_api(runtime, tags)
}

pub(super) fn match_tags_for_rust_node_api(runtime: &str, tags: &Value) -> Result<Value, VmErr> {
    let napi = matches!(tags.get_prop("napi"), Some(Value::Bool(true)));
    if !napi {
        // The Rust backend implements Node-API. A matching Node ABI tag alone
        // cannot make a V8/NAN addon safe to load in this runtime.
        return Ok(Value::Bool(false));
    }
    if let Some(tag_runtime) = property_string(tags, "runtime")
        && tag_runtime != runtime
        && !(tag_runtime == "node" && napi)
    {
        return Ok(Value::Bool(false));
    }
    if tags
        .get_prop("uv")
        .and_then(|value| value_string(&value).map(str::to_owned))
        .is_some_and(|uv| !uv.is_empty())
    {
        // A uv-tagged addon depends on libuv's ABI, which this host does not
        // provide as part of Node-API compatibility.
        return Ok(Value::Bool(false));
    }
    let target = NodeApiPrebuildTarget::current();
    if let Some(libc) = property_string(tags, "libc")
        && !libc.is_empty()
        && target.libc.as_deref() != Some(libc.as_str())
    {
        return Ok(Value::Bool(false));
    }
    if let Some(armv) = property_string(tags, "armv")
        && !armv.is_empty()
        && target.armv.as_deref() != Some(armv.as_str())
    {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}

pub(super) fn value_string(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) => Some(value),
        _ => None,
    }
}

pub(super) fn node_gyp_build_compare_tags(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let runtime = args.first().and_then(value_string).unwrap_or_default();
    let left = args.get(1).cloned().unwrap_or(Value::Undefined);
    let right = args.get(2).cloned().unwrap_or(Value::Undefined);
    let left_runtime = property_string(&left, "runtime");
    let right_runtime = property_string(&right, "runtime");
    if left_runtime != right_runtime {
        return Ok(Value::Number(if left_runtime.as_deref() == Some(runtime) {
            -1.0
        } else {
            1.0
        }));
    }
    let left_abi = property_string(&left, "abi");
    let right_abi = property_string(&right, "abi");
    if left_abi != right_abi {
        return Ok(Value::Number(
            if left_abi.as_deref().is_some_and(|abi| !abi.is_empty()) {
                -1.0
            } else {
                1.0
            },
        ));
    }
    let left_specificity = left
        .get_prop("specificity")
        .and_then(|value| match value {
            Value::Number(value) => Some(value),
            _ => None,
        })
        .unwrap_or(0.0);
    let right_specificity = right
        .get_prop("specificity")
        .and_then(|value| match value {
            Value::Number(value) => Some(value),
            _ => None,
        })
        .unwrap_or(0.0);
    Ok(Value::Number(if left_specificity > right_specificity {
        -1.0
    } else if right_specificity > left_specificity {
        1.0
    } else {
        0.0
    }))
}

pub(super) fn property_string(value: &Value, key: &str) -> Option<String> {
    value
        .get_prop(key)
        .and_then(|value| value_string(&value).map(str::to_owned))
}

pub(super) fn node_gyp_build_parse_tuple(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let Some(Value::String(name)) = args.first() else {
        return Err(VmErr::Msg(
            "TypeError: node-gyp-build.parseTuple expects a tuple name string".into(),
        ));
    };
    let Some(tuple) = parse_prebuild_tuple(name) else {
        return Ok(Value::Undefined);
    };
    Value::checked_object(vec![
        ("name".to_string(), Value::String(tuple.name)),
        ("platform".to_string(), Value::String(tuple.platform)),
        (
            "architectures".to_string(),
            Value::checked_array(tuple.architectures.into_iter().map(Value::String).collect())?,
        ),
    ])
}

pub(super) fn node_gyp_build_match_tuple(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let platform = args.first().and_then(value_string).unwrap_or_default();
    let architecture = args.get(1).and_then(value_string).unwrap_or_default();
    let Some(tuple) = args.get(2) else {
        return Ok(Value::Bool(false));
    };
    let matches_platform = tuple
        .get_prop("platform")
        .and_then(|value| value_string(&value).map(str::to_owned))
        .as_deref()
        == Some(platform);
    let matches_architecture = tuple
        .get_prop("architectures")
        .and_then(|value| value.as_array())
        .is_some_and(|values| {
            values
                .borrow()
                .iter()
                .any(|value| value_string(value) == Some(architecture))
        });
    Ok(Value::Bool(matches_platform && matches_architecture))
}

pub(super) fn node_gyp_build_compare_tuples(
    _interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let architecture_count = |value: Option<&Value>| {
        value
            .and_then(|value| value.get_prop("architectures"))
            .and_then(|value| value.as_array())
            .map(|array| array.borrow().len())
            .unwrap_or(0)
    };
    let left = architecture_count(args.first());
    let right = architecture_count(args.get(1));
    Ok(Value::Number(left as f64 - right as f64))
}

pub(super) fn compile_guest_function(
    interp: &mut crate::interpreter::Interpreter,
    source: &str,
) -> Result<Value, VmErr> {
    interp.set_source(source);
    let tokens = crate::lexer::Lexer::new(source).tokenize_with_spans();
    let mut parser = crate::parser::Parser::new_with_spans(tokens);
    let statements = parser
        .parse_program()
        .map_err(|error| VmErr::Msg(error.to_string()))?;
    interp.run_program_body(&statements)
}

pub(super) fn node_gyp_build_resolve(
    interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let Some(Value::String(package_root)) = args.first() else {
        return Err(VmErr::Msg(
            "TypeError: node-gyp-build expects a package directory string".into(),
        ));
    };
    let loader = interp
        .commonjs_loader
        .clone()
        .ok_or_else(|| VmErr::Msg("node-gyp-build requires a configured CommonJS loader".into()))?;
    let module = loader.resolve_node_api_prebuild_for_package(Path::new(package_root))?;
    Ok(Value::String(module.filename))
}

pub(super) fn node_gyp_build_load(
    interp: &mut crate::interpreter::Interpreter,
    _this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let filename = node_gyp_build_resolve(interp, Value::Undefined, args)?;
    let Value::String(filename) = &filename else {
        unreachable!("node-gyp-build resolve returns a string")
    };
    interp.require_commonjs(filename, None)
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
