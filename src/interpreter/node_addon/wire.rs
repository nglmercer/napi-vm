//! Guest value encoding, graph snapshots, and wire mutation decoding.

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{Value as JsonValue, json};

use crate::error::VmErr;
use crate::value::{
    MAX_ARRAY_LEN, MAX_OBJECT_PROPS, MAX_STRING_LEN, PropAttrs, SymbolData, TypedKind, Value,
};

use super::wire_apply::{apply_guest_mutation, wire_to_guest_with_context};
use super::{MAX_WIRE_DEPTH, NodeAddonSidecar, WireDecodeContext, WireEncodeContext};

pub(super) fn required_string_arg(args: &[Value], index: usize) -> Result<String, VmErr> {
    match args.get(index) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(VmErr::Msg("native object property key is invalid".into())),
    }
}

pub(super) fn typed_kind(name: &str) -> Option<TypedKind> {
    Some(match name {
        "Int8Array" => TypedKind::Int8,
        "Uint8Array" => TypedKind::Uint8,
        "Uint8ClampedArray" => TypedKind::Uint8Clamped,
        "Int16Array" => TypedKind::Int16,
        "Uint16Array" => TypedKind::Uint16,
        "Int32Array" => TypedKind::Int32,
        "Uint32Array" => TypedKind::Uint32,
        "Float32Array" => TypedKind::Float32,
        "Float64Array" => TypedKind::Float64,
        "BigInt64Array" => TypedKind::BigInt64,
        "BigUint64Array" => TypedKind::BigUint64,
        _ => return None,
    })
}

pub(super) fn wire_bytes(value: &JsonValue) -> Result<Vec<u8>, VmErr> {
    value
        .as_array()
        .ok_or_else(|| VmErr::Msg("invalid Node bytes".into()))?
        .iter()
        .map(|byte| {
            byte.as_u64()
                .filter(|value| *value <= 255)
                .map(|value| value as u8)
                .ok_or_else(|| VmErr::Msg("invalid Node byte".into()))
        })
        .collect()
}

pub(super) fn guest_accessor_kind(key: &str, value: &Value) -> Option<&'static str> {
    let name = match value {
        Value::Function(function) => function.name.as_deref(),
        Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
            Some(name.as_ref())
        }
        _ => None,
    }?;
    if name == format!("get {key}") {
        Some("get")
    } else if name == format!("set {key}") {
        Some("set")
    } else {
        None
    }
}

pub(super) fn guest_symbol_key_wire(sidecar: &NodeAddonSidecar, symbol: &SymbolData) -> JsonValue {
    let remote_id = sidecar
        .state
        .borrow()
        .symbol_remote_ids
        .get(&symbol.id)
        .cloned()
        .unwrap_or_else(|| format!("g:{}", symbol.id));
    json!({
        "t": "symbol",
        "v": remote_id,
        "description": symbol.description,
    })
}

pub(super) fn guest_to_wire(
    sidecar: &NodeAddonSidecar,
    v: &Value,
    depth: usize,
    graph: &mut WireEncodeContext,
    proxy_ids: &HashMap<usize, u64>,
) -> Result<JsonValue, VmErr> {
    if depth > MAX_WIRE_DEPTH {
        return Err(VmErr::Msg("guest value exceeds bridge depth limit".into()));
    }
    Ok(match v {
        Value::Undefined => json!({"t":"undefined"}),
        Value::Null => json!({"t":"null"}),
        Value::Bool(x) => json!({"t":"boolean","v":x}),
        Value::Number(x) => {
            json!({"t":"number","v":if x.is_nan(){"NaN".into()}else if *x==f64::INFINITY{"Infinity".into()}else if *x==f64::NEG_INFINITY{"-Infinity".into()}else if *x==0.0&&x.is_sign_negative(){"-0".into()}else{x.to_string()}})
        }
        Value::String(x) => {
            if x.len() > MAX_STRING_LEN {
                return Err(VmErr::Msg("guest string exceeds bridge limit".into()));
            }
            json!({"t":"string","v":x})
        }
        Value::Array(a) => {
            let id = Rc::as_ptr(a) as usize;
            if let Some(node_id) = graph.seen.get(&id) {
                return Ok(json!({"t":"ref","v":format!("g:{node_id}")}));
            }
            let items = a.borrow().clone();
            let presence = a.presence_snapshot();
            if items.len() > MAX_ARRAY_LEN {
                return Err(VmErr::Msg("guest array exceeds limit".into()));
            }
            let node_id = graph.register(id, v.clone())?;
            let wire = items
                .iter()
                .enumerate()
                .map(|(index, x)| {
                    if presence.get(index).copied().unwrap_or(true) {
                        guest_to_wire(sidecar, x, depth + 1, graph, proxy_ids)
                    } else {
                        Ok(json!({"t":"hole"}))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            let named = a
                .named
                .borrow()
                .iter()
                .filter(|(key, _)| !crate::interpreter::is_internal_key(key))
                .map(|(key, value)| {
                    Ok(json!([
                        key,
                        guest_to_wire(sidecar, value, depth + 1, graph, proxy_ids)?
                    ]))
                })
                .collect::<Result<Vec<_>, VmErr>>()?;
            json!({"t":"array","id":format!("g:{node_id}"),"v":wire,"named":named})
        }
        Value::Object { props } => {
            let id = Rc::as_ptr(props) as usize;
            if let Some(node_id) = graph.seen.get(&id) {
                return Ok(json!({"t":"ref","v":format!("g:{node_id}")}));
            }
            let entries = props.borrow().clone();
            if entries.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg("guest object exceeds limit".into()));
            }
            let meta = props.meta.borrow();
            let extensible = !meta.non_extensible;
            let node_id = graph.register(id, v.clone())?;
            let mut wire = Vec::with_capacity(entries.len());
            for (key, value) in &entries {
                let wire_key = if let Some(symbol) = meta.symbol_key(key) {
                    guest_symbol_key_wire(sidecar, &symbol)
                } else if crate::interpreter::symbol_id_from_slot(key).is_some() {
                    return Err(VmErr::Msg(
                        "guest symbol property has no symbol metadata".into(),
                    ));
                } else if crate::interpreter::is_internal_key(key) {
                    continue;
                } else {
                    json!(key)
                };
                let attrs = meta.attrs_of(key);
                let kind = guest_accessor_kind(key, value);
                let getter = (kind == Some("get")).then_some(value);
                let setter = if kind == Some("set") {
                    Some(value)
                } else if getter.is_some() {
                    entries
                        .iter()
                        .find(|(slot, _)| slot == &format!("__setter:{key}__"))
                        .and_then(|(_, candidate)| {
                            (guest_accessor_kind(key, candidate) == Some("set"))
                                .then_some(candidate)
                        })
                } else {
                    None
                };
                if getter.is_some() || setter.is_some() {
                    let getter = getter
                        .map(|getter| guest_to_wire(sidecar, getter, depth + 1, graph, proxy_ids))
                        .transpose()?
                        .unwrap_or(JsonValue::Null);
                    let setter = setter
                        .map(|setter| guest_to_wire(sidecar, setter, depth + 1, graph, proxy_ids))
                        .transpose()?
                        .unwrap_or(JsonValue::Null);
                    wire.push(json!([
                        wire_key,
                        {"t":"undefined"},
                        true,
                        attrs.enumerable,
                        attrs.configurable,
                        getter,
                        setter
                    ]));
                } else {
                    wire.push(json!([
                        wire_key,
                        guest_to_wire(sidecar, value, depth + 1, graph, proxy_ids)?,
                        attrs.writable,
                        attrs.enumerable,
                        attrs.configurable
                    ]));
                }
            }
            let prototype = if meta.uses_default_prototype {
                json!({"t":"defaultPrototype"})
            } else {
                match meta.proto.as_deref() {
                    Some(prototype) => {
                        guest_to_wire(sidecar, prototype, depth + 1, graph, proxy_ids)?
                    }
                    None => json!({"t":"null"}),
                }
            };
            json!({"t":"object","id":format!("g:{node_id}"),"v":wire,"prototype":prototype,"extensible":extensible})
        }
        Value::SharedArrayBuffer(_) => {
            return Err(VmErr::Msg(
                "SharedArrayBuffer cannot cross the Node sidecar bridge because shared memory cannot be preserved".into(),
            ));
        }
        Value::ArrayBuffer(bytes) => json!({"t":"arrayBuffer","v":&*bytes.borrow()}),
        Value::TypedArray(view) => {
            if view.buffer.is_shared() {
                return Err(VmErr::Msg(
                    "a typed array over SharedArrayBuffer cannot cross the Node sidecar bridge because shared memory cannot be preserved".into(),
                ));
            }
            let start = view.effective_byte_offset();
            let byte_len = view
                .effective_length()
                .checked_mul(view.kind.size())
                .ok_or_else(|| VmErr::Msg("guest typed array exceeds the bridge limit".into()))?;
            let end = start
                .checked_add(byte_len)
                .ok_or_else(|| VmErr::Msg("guest typed array exceeds the bridge limit".into()))?;
            let bytes = view
                .buffer
                .read(start, end - start)
                .ok_or_else(|| VmErr::Msg("guest typed array has an invalid byte range".into()))?;
            json!({"t":"typedArray","kind":view.kind.name(),"length":view.effective_length(),"isBuffer":view.is_buffer,"bytes":bytes})
        }
        Value::DataView(view) => {
            if view.buffer.is_shared() {
                return Err(VmErr::Msg(
                    "a DataView over SharedArrayBuffer cannot cross the Node sidecar bridge because shared memory cannot be preserved".into(),
                ));
            }
            let start = view.effective_byte_offset();
            let end = start
                .checked_add(view.effective_length())
                .ok_or_else(|| VmErr::Msg("guest DataView exceeds the bridge limit".into()))?;
            let bytes = view
                .buffer
                .read(start, end - start)
                .ok_or_else(|| VmErr::Msg("guest DataView has an invalid byte range".into()))?;
            json!({"t":"dataView","length":view.effective_length(),"bytes":bytes})
        }
        Value::BigInt(x) => json!({"t":"bigint","v":x.to_string()}),
        Value::Date(milliseconds) => {
            let value = milliseconds.get();
            let wire = if value.is_nan() {
                "NaN".to_string()
            } else if value == f64::INFINITY {
                "Infinity".to_string()
            } else if value == f64::NEG_INFINITY {
                "-Infinity".to_string()
            } else {
                value.to_string()
            };
            json!({"t":"date","v":wire})
        }
        Value::RegExp(data) => json!({
            "t":"regexp",
            "source":data.regex.source,
            "flags":data.regex.flags,
            "lastIndex":data.last_index.get().to_string(),
        }),
        Value::Symbol(symbol) => {
            let remote_id = sidecar
                .state
                .borrow()
                .symbol_remote_ids
                .get(&symbol.id)
                .cloned()
                .unwrap_or_else(|| format!("g:{}", symbol.id));
            json!({
                "t":"symbol",
                "v":remote_id,
                "description":symbol.description,
            })
        }
        Value::Error(error) => json!({
            "t":"error",
            "name":error.name,
            "message":error.message,
            "code":error.code,
        }),
        Value::Proxy(proxy) => {
            let proxy_id = Rc::as_ptr(proxy) as usize;
            match proxy_ids.get(&proxy_id) {
                Some(object_id) => json!({"t":"hostObject","v":object_id}),
                None => {
                    if let Some(node_id) = graph.seen.get(&proxy_id) {
                        if graph.active_proxies.contains(&proxy_id) {
                            return Err(VmErr::Msg(
                                "cyclic guest Proxy graphs cannot cross the Node addon bridge yet"
                                    .into(),
                            ));
                        }
                        json!({"t":"ref","v":format!("g:{node_id}")})
                    } else {
                        let node_id = graph.register(proxy_id, v.clone())?;
                        graph.active_proxies.insert(proxy_id);
                        let target =
                            guest_to_wire(sidecar, &proxy.target, depth + 1, graph, proxy_ids)?;
                        let handler =
                            guest_to_wire(sidecar, &proxy.handler, depth + 1, graph, proxy_ids)?;
                        graph.active_proxies.remove(&proxy_id);
                        json!({
                            "t":"proxy",
                            "id":format!("g:{node_id}"),
                            "target":target,
                            "handler":handler,
                        })
                    }
                }
            }
        }
        Value::Class(class) => {
            let identity = Rc::as_ptr(&class.statics) as usize;
            if let Some(node_id) = graph.seen.get(&identity) {
                return Ok(json!({"t":"ref","v":format!("g:{node_id}")}));
            }
            if class.statics.borrow().len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg(
                    "guest class exceeds bridge property limit".into(),
                ));
            }
            let node_id = graph.register(identity, v.clone())?;
            let callback_id = sidecar.register_guest_callback(v.clone())?;
            graph.callbacks.insert(callback_id, v.clone());
            let prototype = guest_to_wire(
                sidecar,
                class.prototype.as_ref(),
                depth + 1,
                graph,
                proxy_ids,
            )?;
            let statics = class
                .statics
                .borrow()
                .iter()
                .filter(|(key, _)| {
                    !matches!(key.as_str(), "name" | "length" | "prototype")
                        && !crate::interpreter::is_internal_key(key)
                })
                .map(|(key, value)| {
                    Ok(json!([
                        key,
                        guest_to_wire(sidecar, value, depth + 1, graph, proxy_ids)?
                    ]))
                })
                .collect::<Result<Vec<_>, VmErr>>()?;
            json!({
                "t":"guestClass",
                "id":format!("g:{node_id}"),
                "callbackId":callback_id,
                "name":class.name,
                "prototype":prototype,
                "statics":statics,
            })
        }
        Value::Function(function) => {
            let callback_id = sidecar.register_guest_callback(v.clone())?;
            graph.callbacks.insert(callback_id, v.clone());
            json!({
                "t":"guestCallback",
                "v":callback_id,
                "constructable":!function.is_arrow,
            })
        }
        Value::NativeFunction { .. } => {
            let callback_id = sidecar.register_guest_callback(v.clone())?;
            graph.callbacks.insert(callback_id, v.clone());
            json!({"t":"guestCallback","v":callback_id,"constructable":false})
        }
        Value::HostFunction { properties, .. } => {
            let id = properties
                .meta
                .borrow()
                .host_function_id
                .expect("host function identity is initialized");
            if sidecar.state.borrow().local_handles.contains_key(&id) {
                return Err(VmErr::Msg(
                    "native object proxy traps cannot be passed as callbacks".into(),
                ));
            }
            json!({"t":"function","v":id})
        }
        _ => {
            return Err(VmErr::Msg(
                "this guest value cannot cross the Node addon bridge yet".into(),
            ));
        }
    })
}

pub(super) fn guest_graph_node_snapshot(
    sidecar: &NodeAddonSidecar,
    node_id: u64,
    value: &Value,
    graph: &mut WireEncodeContext,
) -> Result<Option<JsonValue>, VmErr> {
    let snapshot = match value {
        Value::Array(array) => {
            let items = array.borrow().clone();
            if items.len() > MAX_ARRAY_LEN {
                return Err(VmErr::Msg(
                    "guest array exceeds limit during callback sync".into(),
                ));
            }
            let values = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    if array.has_index(index) {
                        sidecar.guest_to_wire_with_context(item, 0, graph)
                    } else {
                        Ok(json!({"t":"hole"}))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            let named = array
                .named
                .borrow()
                .iter()
                .filter(|(key, _)| !crate::interpreter::is_internal_key(key))
                .map(|(key, item)| {
                    Ok(json!([
                        key,
                        sidecar.guest_to_wire_with_context(item, 0, graph)?
                    ]))
                })
                .collect::<Result<Vec<_>, VmErr>>()?;
            json!({"t":"array","id":format!("g:{node_id}"),"v":values,"named":named})
        }
        Value::Object { props } => {
            let entries = props.borrow().clone();
            if entries.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg(
                    "guest object exceeds limit during callback sync".into(),
                ));
            }
            let meta = props.meta.borrow();
            let prototype = if meta.uses_default_prototype {
                json!({"t":"defaultPrototype"})
            } else {
                match meta.proto.as_deref() {
                    Some(prototype) => sidecar.guest_to_wire_with_context(prototype, 0, graph)?,
                    None => json!({"t":"null"}),
                }
            };
            let mut wire = Vec::with_capacity(entries.len());
            for (key, item) in &entries {
                let wire_key = if let Some(symbol) = meta.symbol_key(key) {
                    guest_symbol_key_wire(sidecar, &symbol)
                } else if crate::interpreter::symbol_id_from_slot(key).is_some() {
                    return Err(VmErr::Msg(
                        "guest symbol property has no symbol metadata".into(),
                    ));
                } else if crate::interpreter::is_internal_key(key) {
                    continue;
                } else {
                    json!(key)
                };
                let attrs = meta.attrs_of(key);
                let kind = guest_accessor_kind(key, item);
                let getter = (kind == Some("get")).then_some(item);
                let setter = if kind == Some("set") {
                    Some(item)
                } else if getter.is_some() {
                    entries
                        .iter()
                        .find(|(slot, _)| slot == &format!("__setter:{key}__"))
                        .and_then(|(_, candidate)| {
                            (guest_accessor_kind(key, candidate) == Some("set"))
                                .then_some(candidate)
                        })
                } else {
                    None
                };
                if getter.is_some() || setter.is_some() {
                    let getter = getter
                        .map(|getter| sidecar.guest_to_wire_with_context(getter, 0, graph))
                        .transpose()?
                        .unwrap_or(JsonValue::Null);
                    let setter = setter
                        .map(|setter| sidecar.guest_to_wire_with_context(setter, 0, graph))
                        .transpose()?
                        .unwrap_or(JsonValue::Null);
                    wire.push(json!([
                        wire_key,
                        {"t":"undefined"},
                        true,
                        attrs.enumerable,
                        attrs.configurable,
                        getter,
                        setter
                    ]));
                } else {
                    wire.push(json!([
                        wire_key,
                        sidecar.guest_to_wire_with_context(item, 0, graph)?,
                        attrs.writable,
                        attrs.enumerable,
                        attrs.configurable
                    ]));
                }
            }
            json!({
                "t":"object",
                "id":format!("g:{node_id}"),
                "v":wire,
                "prototype":prototype,
                "extensible":!meta.non_extensible,
            })
        }
        Value::Class(class) => {
            let statics = class.statics.borrow();
            if statics.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg(
                    "guest class exceeds limit during callback sync".into(),
                ));
            }
            let statics = statics
                .iter()
                .filter(|(key, _)| {
                    !matches!(key.as_str(), "name" | "length" | "prototype")
                        && !crate::interpreter::is_internal_key(key)
                })
                .map(|(key, item)| {
                    Ok(json!([
                        key,
                        sidecar.guest_to_wire_with_context(item, 0, graph)?
                    ]))
                })
                .collect::<Result<Vec<_>, VmErr>>()?;
            json!({"t":"guestClass","id":format!("g:{node_id}"),"statics":statics})
        }
        _ => return Ok(None),
    };
    Ok(Some(snapshot))
}

pub(super) fn wire_to_guest(
    sidecar: &NodeAddonSidecar,
    v: &JsonValue,
    depth: usize,
) -> Result<Value, VmErr> {
    wire_to_guest_with_context(sidecar, v, depth, &mut WireDecodeContext::default())
}

pub(super) fn wire_graph_id(value: Option<&JsonValue>) -> Result<String, VmErr> {
    let value = value.ok_or_else(|| VmErr::Msg("Node graph value has no id".into()))?;
    if let Some(id) = value.as_str() {
        if id.len() > 64 || !id.contains(':') {
            return Err(VmErr::Msg("invalid Node graph id".into()));
        }
        return Ok(id.to_string());
    }
    value
        .as_u64()
        .map(|id| format!("n:{id}"))
        .ok_or_else(|| VmErr::Msg("invalid Node graph id".into()))
}

pub(super) fn guest_call_result_to_value(
    sidecar: &NodeAddonSidecar,
    envelope: &JsonValue,
    encoded: &WireEncodeContext,
) -> Result<Value, VmErr> {
    if envelope.get("t").and_then(JsonValue::as_str) != Some("guestCallResult") {
        return Err(VmErr::Msg(
            "Node call response has no guest graph result".into(),
        ));
    }
    let mut graph = WireDecodeContext::from_encode_context(encoded);
    {
        let state = sidecar.state.borrow();
        graph.nodes.extend(
            state
                .guest_graph_nodes
                .iter()
                .map(|(id, value)| (format!("g:{id}"), value.clone())),
        );
        graph.callbacks.extend(state.guest_callbacks.clone());
    }
    let result = envelope
        .get("result")
        .ok_or_else(|| VmErr::Msg("Node call response has no result".into()))?;
    let result = wire_to_guest_with_context(sidecar, result, 0, &mut graph)?;
    let thrown = envelope
        .get("thrown")
        .map(|thrown| wire_to_guest_with_context(sidecar, thrown, 0, &mut graph))
        .transpose()?;
    let mutations = envelope
        .get("mutations")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| VmErr::Msg("Node call response has invalid guest mutations".into()))?;
    for mutation in mutations {
        apply_guest_mutation(sidecar, mutation, &mut graph)?;
    }
    if let Some(reason) = thrown {
        Err(VmErr::Throw(reason))
    } else {
        Ok(result)
    }
}

pub(super) fn mutation_property(
    sidecar: &NodeAddonSidecar,
    item: &JsonValue,
    graph: &mut WireDecodeContext,
) -> Result<GuestPropertyMutation, VmErr> {
    let pair = item
        .as_array()
        .filter(|pair| pair.len() == 5 || pair.len() == 7)
        .ok_or_else(|| VmErr::Msg("invalid Node guest mutation property".into()))?;
    let (key, symbol) = wire_property_slot(sidecar, &pair[0], 0, graph)?;
    let attrs = PropAttrs {
        writable: pair[2]
            .as_bool()
            .ok_or_else(|| VmErr::Msg("invalid Node property writable flag".into()))?,
        enumerable: pair[3]
            .as_bool()
            .ok_or_else(|| VmErr::Msg("invalid Node property enumerable flag".into()))?,
        configurable: pair[4]
            .as_bool()
            .ok_or_else(|| VmErr::Msg("invalid Node property configurable flag".into()))?,
    };
    let value = wire_to_guest_with_context(sidecar, &pair[1], 0, graph)?;
    let getter = pair
        .get(5)
        .filter(|value| !value.is_null())
        .map(|value| wire_to_guest_with_context(sidecar, value, 0, graph))
        .transpose()?;
    let setter = pair
        .get(6)
        .filter(|value| !value.is_null())
        .map(|value| wire_to_guest_with_context(sidecar, value, 0, graph))
        .transpose()?;
    Ok(GuestPropertyMutation {
        key,
        symbol,
        value,
        attrs,
        getter,
        setter,
    })
}

pub(super) fn wire_property_slot(
    sidecar: &NodeAddonSidecar,
    key: &JsonValue,
    depth: usize,
    graph: &mut WireDecodeContext,
) -> Result<(String, Option<Rc<SymbolData>>), VmErr> {
    if let Some(key) = key.as_str() {
        return Ok((key.to_string(), None));
    }
    let value = wire_to_guest_with_context(sidecar, key, depth, graph)?;
    match &value {
        Value::Symbol(symbol) => Ok((
            crate::interpreter::symbol_slot_key(symbol),
            Some(symbol.clone()),
        )),
        _ => Err(VmErr::Msg(
            "invalid Node guest mutation property key".into(),
        )),
    }
}

pub(super) fn is_reserved_guest_property_key(key: &str, symbol: bool) -> bool {
    !symbol && crate::interpreter::is_internal_key(key)
}

pub(super) struct GuestPropertyMutation {
    pub(super) key: String,
    pub(super) symbol: Option<Rc<SymbolData>>,
    pub(super) value: Value,
    pub(super) attrs: PropAttrs,
    pub(super) getter: Option<Value>,
    pub(super) setter: Option<Value>,
}

pub(super) fn callable_value(value: &Value) -> bool {
    matches!(
        value,
        Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
    )
}

pub(super) fn named_accessor(value: Value, name: String) -> Result<Value, VmErr> {
    Ok(match &value {
        Value::Function(function) => {
            let mut renamed = function.as_ref().clone();
            renamed.name = Some(name.into());
            Value::Function(Rc::new(renamed))
        }
        Value::NativeFunction { callable, .. } => Value::NativeFunction {
            name: name.into(),
            callable: *callable,
        },
        Value::HostFunction { .. } => value
            .host_function_named(name)
            .ok_or_else(|| VmErr::Msg("Node accessor value is not callable".into()))?,
        _ => {
            return Err(VmErr::Msg("Node accessor value is not callable".into()));
        }
    })
}
