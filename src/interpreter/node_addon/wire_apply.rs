//! Applying wire mutations onto guest objects, accessors, and prototypes.

use std::collections::HashSet;
use std::rc::Rc;

use serde_json::Value as JsonValue;

use crate::error::VmErr;
use crate::value::{
    Buffer, MAX_ARRAY_LEN, MAX_OBJECT_PROPS, MAX_STRING_LEN, PropAttrs, SymbolData, TypedArrayData,
    TypedKind, Value,
};

use super::wire::{
    callable_value, is_reserved_guest_property_key, mutation_property, named_accessor, typed_kind,
    wire_bytes, wire_graph_id, wire_property_slot,
};
use super::{MAX_NATIVE_HANDLES, MAX_WIRE_DEPTH, NodeAddonSidecar, WireDecodeContext};

#[derive(Clone)]
pub(super) enum GuestPrototypeState {
    Default,
    Explicit(Option<Rc<Value>>),
}

pub(super) fn decode_guest_prototype(
    sidecar: &NodeAddonSidecar,
    wire: &JsonValue,
    depth: usize,
    graph: &mut WireDecodeContext,
) -> Result<GuestPrototypeState, VmErr> {
    if wire.get("t").and_then(JsonValue::as_str) == Some("defaultPrototype") {
        return Ok(GuestPrototypeState::Default);
    }
    let value = wire_to_guest_with_context(sidecar, wire, depth, graph)?;
    match &value {
        Value::Null => Ok(GuestPrototypeState::Explicit(None)),
        Value::Object { .. } => Ok(GuestPrototypeState::Explicit(Some(Rc::new(value.clone())))),
        Value::Class(class) => Ok(GuestPrototypeState::Explicit(Some(class.prototype.clone()))),
        _ => Err(VmErr::Msg(
            "Node object prototype must be an object or null".into(),
        )),
    }
}

pub(super) fn ensure_no_guest_prototype_cycle(
    target: &Rc<crate::value::ObjectCell>,
    prototype: &Value,
) -> Result<(), VmErr> {
    let mut current = Some(Rc::new(prototype.clone()));
    let mut visited = HashSet::new();
    while let Some(value) = current {
        let Value::Object { props } = value.as_ref() else {
            break;
        };
        if Rc::ptr_eq(target, props) {
            return Err(VmErr::Msg(
                "Node addon prototype mutation would create a cycle".into(),
            ));
        }
        if !visited.insert(Rc::as_ptr(props) as usize) {
            break;
        }
        current = props.proto();
    }
    Ok(())
}

pub(super) fn apply_guest_mutation(
    sidecar: &NodeAddonSidecar,
    mutation: &JsonValue,
    graph: &mut WireDecodeContext,
) -> Result<(), VmErr> {
    let id = mutation
        .get("id")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| VmErr::Msg("Node mutation has an invalid guest id".into()))?;
    if !id.starts_with("g:") {
        return Err(VmErr::Msg(
            "Node mutation targets a non-guest graph node".into(),
        ));
    }
    let target = graph
        .nodes
        .get(id)
        .cloned()
        .ok_or_else(|| VmErr::Msg("Node mutation references an unknown guest node".into()))?;
    let entries = mutation
        .get("entries")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| VmErr::Msg("Node mutation has invalid properties".into()))?;
    let kind = mutation
        .get("kind")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| VmErr::Msg("Node mutation has no kind".into()))?;
    let prototype = mutation
        .get("prototype")
        .map(|wire| decode_guest_prototype(sidecar, wire, 0, graph))
        .transpose()?;
    match (kind, &target) {
        ("object", Value::Object { props }) => {
            if let Some(GuestPrototypeState::Explicit(Some(prototype))) = &prototype {
                ensure_no_guest_prototype_cycle(props, prototype)?;
            }
            if entries.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg("Node object mutation exceeds VM limit".into()));
            }
            let mut updates = Vec::with_capacity(entries.len());
            let mut attributes = Vec::with_capacity(entries.len());
            let mut keys = HashSet::with_capacity(entries.len());
            for item in entries {
                let property = mutation_property(sidecar, item, graph)?;
                if is_reserved_guest_property_key(&property.key, property.symbol.is_some()) {
                    return Err(VmErr::Msg(
                        "Node addon mutation uses a reserved VM property name".into(),
                    ));
                }
                if !keys.insert(property.key.clone()) {
                    return Err(VmErr::Msg(
                        "Node object mutation has duplicate properties".into(),
                    ));
                }
                if property.getter.is_some() || property.setter.is_some() {
                    if property
                        .getter
                        .as_ref()
                        .is_some_and(|getter| !callable_value(getter))
                        || property
                            .setter
                            .as_ref()
                            .is_some_and(|setter| !callable_value(setter))
                    {
                        return Err(VmErr::Msg(
                            "Node accessor properties require callable getter/setter values".into(),
                        ));
                    }
                    if property.getter.is_none() && property.setter.is_none() {
                        return Err(VmErr::Msg(
                            "empty Node accessor properties cannot be represented by the VM".into(),
                        ));
                    }
                }
                attributes.push((
                    property.key.clone(),
                    property.attrs,
                    property.symbol.clone(),
                ));
                updates.push(property);
            }
            let has_accessor_updates = updates
                .iter()
                .any(|property| property.getter.is_some() || property.setter.is_some());
            let deleted_entries = mutation
                .get("deleted")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("Node object mutation has invalid deletions".into()))?;
            let mut deleted = Vec::with_capacity(deleted_entries.len());
            for key in deleted_entries {
                let (key, symbol) = wire_property_slot(sidecar, key, 0, graph)?;
                if is_reserved_guest_property_key(&key, symbol.is_some()) {
                    return Err(VmErr::Msg(
                        "Node addon mutation deletes a reserved VM property name".into(),
                    ));
                }
                deleted.push(key);
            }
            let mut slots = props.borrow_mut();
            slots.retain(|(key, _)| {
                !deleted
                    .iter()
                    .any(|deleted| deleted == key || key == &format!("__setter:{deleted}__"))
            });
            for property in updates {
                let key = property.key;
                let companion = format!("__setter:{key}__");
                slots.retain(|(slot, _)| slot != &companion);
                let (primary, setter) = match (property.getter, property.setter) {
                    (Some(getter), Some(setter)) => (
                        named_accessor(getter, format!("get {key}"))?,
                        Some(named_accessor(setter, format!("set {key}"))?),
                    ),
                    (Some(getter), None) => (named_accessor(getter, format!("get {key}"))?, None),
                    (None, Some(setter)) => (named_accessor(setter, format!("set {key}"))?, None),
                    (None, None) => (property.value, None),
                };
                if let Some((_, slot)) = slots.iter_mut().find(|(name, _)| name == &key) {
                    *slot = primary;
                } else {
                    if slots.len() >= MAX_OBJECT_PROPS {
                        return Err(VmErr::Msg("Node object mutation exceeds VM limit".into()));
                    }
                    slots.push((key, primary));
                }
                if let Some(setter) = setter {
                    if slots.len() >= MAX_OBJECT_PROPS {
                        return Err(VmErr::Msg("Node object mutation exceeds VM limit".into()));
                    }
                    slots.push((companion, setter));
                }
            }
            drop(slots);
            let mut meta = props.meta.borrow_mut();
            for key in &deleted {
                meta.forget(key);
            }
            for (key, attrs, symbol) in attributes {
                meta.forget(&key);
                meta.set_attrs(&key, attrs);
                if let Some(symbol) = symbol {
                    meta.set_symbol_key(&key, symbol);
                }
            }
            meta.has_accessors |= has_accessor_updates;
            if let Some(extensible) = mutation.get("extensible").and_then(JsonValue::as_bool) {
                meta.non_extensible = !extensible;
            }
            match prototype {
                Some(GuestPrototypeState::Default) => {
                    meta.proto = None;
                    meta.uses_default_prototype = true;
                }
                Some(GuestPrototypeState::Explicit(prototype)) => {
                    meta.proto = prototype;
                    meta.uses_default_prototype = false;
                }
                None => {}
            }
        }
        ("object", Value::Class(class)) => {
            if entries.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg("Node class mutation exceeds VM limit".into()));
            }
            let mut updates = Vec::with_capacity(entries.len());
            let mut keys = HashSet::with_capacity(entries.len());
            for item in entries {
                let property = mutation_property(sidecar, item, graph)?;
                if property.symbol.is_some()
                    || property.getter.is_some()
                    || property.setter.is_some()
                    || property.attrs != PropAttrs::default()
                {
                    return Err(VmErr::Msg(
                        "Node class accessor, symbol, or descriptor mutation is not supported"
                            .into(),
                    ));
                }
                if property.key == "name"
                    || property.key == "prototype"
                    || property.key == "length"
                    || property.key == "arguments"
                    || property.key == "caller"
                    || is_reserved_guest_property_key(&property.key, false)
                {
                    return Err(VmErr::Msg(
                        "Node class mutation targets a reserved constructor property".into(),
                    ));
                }
                if !keys.insert(property.key.clone()) {
                    return Err(VmErr::Msg(
                        "Node class mutation has duplicate properties".into(),
                    ));
                }
                updates.push((property.key, property.value));
            }
            let deleted_entries = mutation
                .get("deleted")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("Node class mutation has invalid deletions".into()))?;
            let mut deleted = Vec::with_capacity(deleted_entries.len());
            for key in deleted_entries {
                let (key, symbol) = wire_property_slot(sidecar, key, 0, graph)?;
                if symbol.is_some()
                    || key == "name"
                    || key == "prototype"
                    || key == "length"
                    || key == "arguments"
                    || key == "caller"
                    || is_reserved_guest_property_key(&key, false)
                {
                    return Err(VmErr::Msg(
                        "Node class deletion targets a reserved constructor property".into(),
                    ));
                }
                deleted.push(key);
            }
            let mut statics = class.statics.borrow_mut();
            statics.retain(|(key, _)| !deleted.iter().any(|deleted| deleted == key));
            for (key, value) in updates {
                if let Some((_, existing)) = statics.iter_mut().find(|(name, _)| name == &key) {
                    *existing = value;
                } else {
                    if statics.len() >= MAX_OBJECT_PROPS {
                        return Err(VmErr::Msg("Node class mutation exceeds VM limit".into()));
                    }
                    statics.push((key, value));
                }
            }
        }
        ("array", Value::Array(array)) => {
            let requested_length = mutation
                .get("length")
                .map(|length| {
                    length
                        .as_u64()
                        .and_then(|length| usize::try_from(length).ok())
                        .filter(|length| *length <= MAX_ARRAY_LEN)
                        .ok_or_else(|| {
                            VmErr::Msg("Node array mutation has an invalid length".into())
                        })
                })
                .transpose()?;
            let mut items = array.borrow().clone();
            let mut presence = array.presence_snapshot();
            if let Some(length) = requested_length {
                items.truncate(length);
                items.resize(length, Value::Undefined);
                presence.resize(length, false);
                presence.truncate(length);
            }
            let mut named = array.named.borrow().clone();
            let mut keys = HashSet::with_capacity(entries.len());
            for item in entries {
                let property = mutation_property(sidecar, item, graph)?;
                if property.symbol.is_some() {
                    return Err(VmErr::Msg(
                        "symbol-keyed array mutation is not supported by the VM".into(),
                    ));
                }
                if !keys.insert(property.key.clone()) {
                    return Err(VmErr::Msg(
                        "Node array mutation has duplicate properties".into(),
                    ));
                }
                if property.getter.is_some() || property.setter.is_some() {
                    return Err(VmErr::Msg(
                        "array accessor mutations cannot be written back to the VM".into(),
                    ));
                }
                if property.attrs != PropAttrs::default() {
                    return Err(VmErr::Msg(
                        "non-default array property attributes cannot be written back to the VM"
                            .into(),
                    ));
                }
                if let Ok(index) = property.key.parse::<usize>()
                    && property.key == index.to_string()
                    && index < items.len()
                {
                    items[index] = property.value;
                    presence[index] = true;
                    continue;
                }
                if crate::interpreter::is_internal_key(&property.key) {
                    return Err(VmErr::Msg(
                        "Node addon mutation uses a reserved VM property name".into(),
                    ));
                }
                if let Some((_, slot)) = named.iter_mut().find(|(name, _)| name == &property.key) {
                    *slot = property.value;
                } else {
                    named.push((property.key, property.value));
                }
            }
            let deleted = mutation
                .get("deleted")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("Node array mutation has invalid deletions".into()))?;
            for key in deleted {
                let (key, symbol) = wire_property_slot(sidecar, key, 0, graph)?;
                if symbol.is_some() {
                    return Err(VmErr::Msg(
                        "symbol-keyed array mutation is not supported by the VM".into(),
                    ));
                }
                if let Ok(index) = key.parse::<usize>()
                    && key == index.to_string()
                    && index < items.len()
                {
                    presence[index] = false;
                    items[index] = Value::Undefined;
                    continue;
                }
                named.retain(|(name, _)| name != &key);
            }
            *array.borrow_mut() = items;
            array.replace_presence(presence);
            *array.named.borrow_mut() = named;
        }
        _ => {
            return Err(VmErr::Msg(
                "Node mutation kind does not match guest value".into(),
            ));
        }
    }
    Ok(())
}

pub(super) fn wire_to_guest_with_context(
    sidecar: &NodeAddonSidecar,
    v: &JsonValue,
    depth: usize,
    graph: &mut WireDecodeContext,
) -> Result<Value, VmErr> {
    if depth > MAX_WIRE_DEPTH {
        return Err(VmErr::Msg(
            "native result exceeds bridge depth limit".into(),
        ));
    }
    let t = v
        .get("t")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| VmErr::Msg("invalid Node value tag".into()))?;
    match t {
        "ref" | "guestRef" => {
            let id = wire_graph_id(v.get("v"))?;
            graph
                .nodes
                .get(&id)
                .cloned()
                .ok_or_else(|| VmErr::Msg("Node value references an unknown graph node".into()))
        }
        "guestCallbackRef" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| VmErr::Msg("invalid guest callback reference id".into()))?;
            graph
                .callbacks
                .get(&id)
                .cloned()
                .ok_or_else(|| VmErr::Msg("Node value references an unknown guest callback".into()))
        }
        "undefined" => Ok(Value::Undefined),
        "hole" => Err(VmErr::Msg("array hole appeared outside an array".into())),
        "null" => Ok(Value::Null),
        "boolean" => v
            .get("v")
            .and_then(JsonValue::as_bool)
            .map(Value::Bool)
            .ok_or_else(|| VmErr::Msg("invalid Node boolean".into())),
        "number" => {
            let s = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node number".into()))?;
            let n = match s {
                "NaN" => f64::NAN,
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                "-0" => -0.0,
                _ => s
                    .parse()
                    .map_err(|e| VmErr::Msg(format!("invalid Node number: {e}")))?,
            };
            Ok(Value::Number(n))
        }
        "string" => v
            .get("v")
            .and_then(JsonValue::as_str)
            .map(|s| Value::String(s.to_string()))
            .ok_or_else(|| VmErr::Msg("invalid Node string".into())),
        "bigint" => {
            let s = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node BigInt".into()))?;
            let n = crate::bigint::BigInt::parse(s).map_err(VmErr::Msg)?;
            Ok(Value::BigInt(Rc::new(n)))
        }
        "date" => {
            let value = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node date".into()))?;
            let milliseconds = parse_wire_number(value)?;
            Ok(Value::Date(Rc::new(std::cell::Cell::new(milliseconds))))
        }
        "regexp" => {
            let source = v
                .get("source")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node regular expression source".into()))?;
            let flags = v
                .get("flags")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node regular expression flags".into()))?;
            if source.len() > MAX_STRING_LEN || flags.len() > MAX_STRING_LEN {
                return Err(VmErr::Msg(
                    "Node regular expression exceeds the VM string limit".into(),
                ));
            }
            let last_index = v
                .get("lastIndex")
                .and_then(JsonValue::as_str)
                .unwrap_or("0")
                .parse::<f64>()
                .unwrap_or(0.0);
            let regex = crate::builtins::compile_regex(source, flags)?;
            let Value::RegExp(data) = &regex else {
                unreachable!("regex compiler returns a RegExp")
            };
            if last_index.is_finite() && last_index >= 0.0 {
                data.last_index.set(last_index as usize);
            }
            Ok(regex)
        }
        "symbol" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node symbol id".into()))?;
            let description = v
                .get("description")
                .and_then(JsonValue::as_str)
                .map(str::to_string);
            if description
                .as_ref()
                .is_some_and(|description| description.len() > MAX_STRING_LEN)
            {
                return Err(VmErr::Msg(
                    "Node symbol description exceeds VM limit".into(),
                ));
            }
            if let Some(guest_id) = id.strip_prefix("g:") {
                let guest_id = guest_id
                    .parse::<u64>()
                    .map_err(|_| VmErr::Msg("invalid guest symbol id".into()))?;
                return Ok(Value::Symbol(Rc::new(SymbolData {
                    id: guest_id,
                    description,
                })));
            }
            if let Some(symbol) = sidecar.state.borrow().host_symbols.get(id).cloned() {
                return Ok(symbol);
            }
            if !id.starts_with("n:") {
                return Err(VmErr::Msg("invalid native symbol id".into()));
            }
            if sidecar.state.borrow().host_symbols.len() >= MAX_NATIVE_HANDLES {
                return Err(VmErr::Msg("native symbol handle limit exceeded".into()));
            }
            let symbol = crate::builtins::new_symbol(description);
            let Value::Symbol(data) = &symbol else {
                unreachable!("new_symbol returns a Symbol")
            };
            let mut state = sidecar.state.borrow_mut();
            state.symbol_remote_ids.insert(data.id, id.to_string());
            state.host_symbols.insert(id.to_string(), symbol.clone());
            Ok(symbol)
        }
        "hostObject" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| VmErr::Msg("invalid Node object id".into()))?;
            sidecar.host_object(id)
        }
        "hostPromise" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| VmErr::Msg("invalid Node promise id".into()))?;
            sidecar.host_promise(id)
        }
        "error" => {
            let name = v.get("name").and_then(JsonValue::as_str).unwrap_or("Error");
            let message = v
                .get("message")
                .and_then(JsonValue::as_str)
                .unwrap_or("native promise result could not be marshalled");
            let error = match v.get("code").and_then(JsonValue::as_str) {
                Some(code) => crate::value::ErrorData::with_code(name, message, code),
                None => crate::value::ErrorData::new(name, message),
            };
            Ok(Value::Error(error))
        }
        "arrayBuffer" | "bytes" => {
            let bytes = wire_bytes(
                v.get("v")
                    .ok_or_else(|| VmErr::Msg("invalid Node bytes".into()))?,
            )?;
            Ok(Value::ArrayBuffer(Buffer::owned(bytes)))
        }
        "typedArray" => {
            let kind_name = v
                .get("kind")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node typed array kind".into()))?;
            let kind = typed_kind(kind_name)
                .ok_or_else(|| VmErr::Msg("unsupported Node typed array kind".into()))?;
            let length = v
                .get("length")
                .and_then(JsonValue::as_u64)
                .and_then(|length| usize::try_from(length).ok())
                .ok_or_else(|| VmErr::Msg("invalid Node typed array length".into()))?;
            let bytes = wire_bytes(
                v.get("bytes")
                    .ok_or_else(|| VmErr::Msg("invalid Node typed array bytes".into()))?,
            )?;
            if length.checked_mul(kind.size()) != Some(bytes.len()) {
                return Err(VmErr::Msg(
                    "Node typed array has an invalid byte length".into(),
                ));
            }
            Ok(Value::TypedArray(Rc::new(TypedArrayData {
                kind,
                buffer: Buffer::owned(bytes).into(),
                byte_offset: 0,
                length,
                is_buffer: false,
            })))
        }
        "dataView" => {
            let length = v
                .get("length")
                .and_then(JsonValue::as_u64)
                .and_then(|length| usize::try_from(length).ok())
                .ok_or_else(|| VmErr::Msg("invalid Node DataView length".into()))?;
            let bytes = wire_bytes(
                v.get("bytes")
                    .ok_or_else(|| VmErr::Msg("invalid Node DataView bytes".into()))?,
            )?;
            if length != bytes.len() {
                return Err(VmErr::Msg(
                    "Node DataView has an invalid byte length".into(),
                ));
            }
            Ok(Value::DataView(Rc::new(TypedArrayData {
                kind: TypedKind::Uint8,
                buffer: Buffer::owned(bytes).into(),
                byte_offset: 0,
                length,
                is_buffer: false,
            })))
        }
        "function" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| VmErr::Msg("invalid Node function id".into()))?;
            Ok(Value::host_function(
                v.get("n")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("nodeAddon"),
                id,
            ))
        }
        "array" => {
            let a = v
                .get("v")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("invalid Node array".into()))?;
            if a.len() > MAX_ARRAY_LEN {
                return Err(VmErr::Msg("Node array exceeds VM limit".into()));
            }
            let array = Value::checked_array(Vec::new())?;
            if let Some(id) = v.get("id") {
                graph.register(wire_graph_id(Some(id))?, array.clone())?;
            }
            let mut presence = Vec::with_capacity(a.len());
            let items = a
                .iter()
                .map(|x| {
                    let is_hole = x.get("t").and_then(JsonValue::as_str) == Some("hole");
                    presence.push(!is_hole);
                    if is_hole {
                        Ok(Value::Undefined)
                    } else {
                        wire_to_guest_with_context(sidecar, x, depth + 1, graph)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let Value::Array(cell) = &array {
                *cell.borrow_mut() = items;
                cell.replace_presence(presence);
            }
            if let Some(named) = v.get("named").and_then(JsonValue::as_array) {
                for property in named {
                    let pair = property
                        .as_array()
                        .filter(|pair| pair.len() == 2)
                        .ok_or_else(|| VmErr::Msg("invalid Node array property".into()))?;
                    let key = pair[0]
                        .as_str()
                        .ok_or_else(|| VmErr::Msg("invalid Node array property key".into()))?;
                    let value = wire_to_guest_with_context(sidecar, &pair[1], depth + 1, graph)?;
                    if let Value::Array(cell) = &array {
                        cell.set_named(key.to_string(), value);
                    }
                }
            }
            Ok(array)
        }
        "object" => {
            let a = v
                .get("v")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("invalid Node object".into()))?;
            if a.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg("Node object exceeds VM limit".into()));
            }
            let object = Value::checked_object(Vec::new())?;
            if let Some(id) = v.get("id") {
                graph.register(wire_graph_id(Some(id))?, object.clone())?;
            }
            let mut slots = Vec::with_capacity(a.len());
            let mut attrs = Vec::with_capacity(a.len());
            let mut has_accessors = false;
            for item in a {
                let pair = item
                    .as_array()
                    .filter(|p| p.len() == 2 || p.len() == 5 || p.len() == 7)
                    .ok_or_else(|| VmErr::Msg("invalid Node property".into()))?;
                let (key, symbol) = wire_property_slot(sidecar, &pair[0], depth + 1, graph)?;
                if pair.len() == 7 {
                    let getter = (!pair[5].is_null())
                        .then(|| wire_to_guest_with_context(sidecar, &pair[5], depth + 1, graph))
                        .transpose()?;
                    let setter = (!pair[6].is_null())
                        .then(|| wire_to_guest_with_context(sidecar, &pair[6], depth + 1, graph))
                        .transpose()?;
                    if getter.is_none() && setter.is_none() {
                        return Err(VmErr::Msg(
                            "empty Node accessor properties cannot be represented by the VM".into(),
                        ));
                    }
                    if let Some(getter) = getter {
                        if !callable_value(&getter) {
                            return Err(VmErr::Msg("Node accessor getter is not callable".into()));
                        }
                        slots.push((key.clone(), named_accessor(getter, format!("get {key}"))?));
                    }
                    if let Some(setter) = setter {
                        if !callable_value(&setter) {
                            return Err(VmErr::Msg("Node accessor setter is not callable".into()));
                        }
                        let slot = if slots.last().is_some_and(|(slot, _)| slot == &key) {
                            format!("__setter:{key}__")
                        } else {
                            key.to_string()
                        };
                        slots.push((slot, named_accessor(setter, format!("set {key}"))?));
                    }
                    has_accessors = true;
                } else {
                    slots.push((
                        key.clone(),
                        wire_to_guest_with_context(sidecar, &pair[1], depth + 1, graph)?,
                    ));
                }
                attrs.push((
                    key.clone(),
                    PropAttrs {
                        writable: pair.get(2).and_then(JsonValue::as_bool).unwrap_or(true),
                        enumerable: pair.get(3).and_then(JsonValue::as_bool).unwrap_or(true),
                        configurable: pair.get(4).and_then(JsonValue::as_bool).unwrap_or(true),
                    },
                    symbol,
                ));
            }
            let prototype = v
                .get("prototype")
                .map(|wire| decode_guest_prototype(sidecar, wire, depth + 1, graph))
                .transpose()?
                .unwrap_or(GuestPrototypeState::Default);
            if let Value::Object { props } = &object {
                *props.borrow_mut() = slots;
                let mut meta = props.meta.borrow_mut();
                for (key, value, symbol) in attrs {
                    meta.set_attrs(&key, value);
                    if let Some(symbol) = symbol {
                        meta.set_symbol_key(&key, symbol);
                    }
                }
                meta.has_accessors = has_accessors;
                match prototype {
                    GuestPrototypeState::Default => {
                        meta.proto = None;
                        meta.uses_default_prototype = true;
                    }
                    GuestPrototypeState::Explicit(prototype) => {
                        meta.proto = prototype;
                        meta.uses_default_prototype = false;
                    }
                }
                if v.get("extensible").and_then(JsonValue::as_bool) == Some(false) {
                    meta.non_extensible = true;
                }
            }
            Ok(object)
        }
        _ => Err(VmErr::Msg(format!("unknown Node value tag '{t}'"))),
    }
}

pub(super) fn parse_wire_number(value: &str) -> Result<f64, VmErr> {
    match value {
        "NaN" => Ok(f64::NAN),
        "Infinity" => Ok(f64::INFINITY),
        "-Infinity" => Ok(f64::NEG_INFINITY),
        "-0" => Ok(-0.0),
        _ => value
            .parse()
            .map_err(|error| VmErr::Msg(format!("invalid Node number: {error}"))),
    }
}
