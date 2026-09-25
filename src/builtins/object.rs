//! `Object` static methods, including the property-descriptor surface.

use std::rc::Rc;

use super::nf;
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter, is_internal_key};
use crate::value::{BoxedPrimitive, ObjectCell, PropAttrs, Value};

pub(super) fn install(e: &mut Environment) {
    let Some(o) = e.get("Object") else { return };
    let prototype = Value::object(vec![
        ("constructor".into(), o.clone()),
        (
            "__defineGetter__".into(),
            nf("__defineGetter__", object_define_getter),
        ),
        (
            "__defineSetter__".into(),
            nf("__defineSetter__", object_define_setter),
        ),
        (
            "hasOwnProperty".into(),
            nf("hasOwnProperty", object_has_own_property),
        ),
        (
            "__lookupGetter__".into(),
            nf("__lookupGetter__", object_lookup_getter),
        ),
        (
            "__lookupSetter__".into(),
            nf("__lookupSetter__", object_lookup_setter),
        ),
        (
            "isPrototypeOf".into(),
            nf("isPrototypeOf", object_is_prototype_of),
        ),
        (
            "propertyIsEnumerable".into(),
            nf("propertyIsEnumerable", object_property_is_enumerable),
        ),
        (
            "toLocaleString".into(),
            nf("toLocaleString", object_to_locale_string),
        ),
        ("toString".into(), nf("toString", object_to_string)),
        ("valueOf".into(), nf("valueOf", object_value_of)),
    ]);
    if let Value::Object { props } = &prototype {
        let mut metadata = props.meta.borrow_mut();
        for key in [
            "constructor",
            "__defineGetter__",
            "__defineSetter__",
            "hasOwnProperty",
            "__lookupGetter__",
            "__lookupSetter__",
            "isPrototypeOf",
            "propertyIsEnumerable",
            "toLocaleString",
            "toString",
            "valueOf",
        ] {
            metadata.set_attrs(
                key,
                PropAttrs {
                    enumerable: false,
                    ..PropAttrs::default()
                },
            );
        }
    }
    let proto_descriptor = Value::object(vec![
        ("get".into(), nf("get __proto__", object_get_prototype)),
        ("set".into(), nf("set __proto__", object_set_prototype)),
        ("enumerable".into(), Value::Bool(false)),
        ("configurable".into(), Value::Bool(true)),
    ]);
    define_property(&prototype, "__proto__", &proto_descriptor)
        .expect("built-in Object.prototype __proto__ accessor");
    o.set_prop("prototype".to_string(), prototype.clone())
        .expect("built-in Object.prototype");
    if let Value::Object { props } = &o {
        props.meta.borrow_mut().set_attrs(
            "prototype",
            PropAttrs {
                writable: false,
                enumerable: false,
                configurable: false,
            },
        );
    }
    let methods: &[(&str, super::NativeFn)] = &[
        ("keys", object_keys),
        ("values", object_values),
        ("entries", object_entries),
        ("assign", object_assign),
        ("getOwnPropertyNames", object_get_own_property_names),
        ("create", object_create),
        ("defineProperty", object_define_property),
        ("defineProperties", object_define_properties),
        ("getOwnPropertyDescriptor", object_get_own_descriptor),
        ("getOwnPropertyDescriptors", object_get_own_descriptors),
        ("getPrototypeOf", object_get_prototype_of),
        ("setPrototypeOf", object_set_prototype_of),
        ("hasOwn", object_has_own),
        ("fromEntries", object_from_entries),
        ("freeze", object_freeze),
        ("isFrozen", object_is_frozen),
        ("seal", object_seal),
        ("isSealed", object_is_sealed),
        ("preventExtensions", object_prevent_extensions),
        ("isExtensible", object_is_extensible),
        ("is", object_is),
    ];
    for (name, callable) in methods {
        o.set_prop(name.to_string(), nf(name, *callable))
            .expect("built-in Object property");
    }
}

// --- Shared helpers ---------------------------------------------------------

fn cell(v: &Value) -> Option<&Rc<ObjectCell>> {
    match v {
        Value::Object { props } => Some(props),
        Value::Class(class) => Some(&class.statics),
        Value::Function(function) => {
            function.ensure_name_length_properties();
            function.prototype_value(v);
            Some(&function.properties)
        }
        Value::HostFunction { properties, .. } => Some(properties),
        _ => None,
    }
}

fn type_err(msg: &str) -> VmErr {
    VmErr::Msg(format!("TypeError: {}", msg))
}

/// Own property names in insertion order, excluding the VM's internal
/// symbol slots. `enumerable_only` applies the `enumerable` attribute.
fn own_names(v: &Value, enumerable_only: bool) -> Vec<String> {
    match v {
        Value::Object { props } => own_object_names(props, enumerable_only),
        Value::Class(class) => own_object_names(&class.statics, enumerable_only),
        Value::Function(function) => {
            function.ensure_name_length_properties();
            function.prototype_value(v);
            own_object_names(&function.properties, enumerable_only)
        }
        Value::HostFunction { properties, .. } => own_object_names(properties, enumerable_only),
        Value::Array(items) => {
            let mut names: Vec<String> = (0..items.borrow().len())
                .filter(|index| items.has_index(*index))
                .map(|index| index.to_string())
                .collect();
            if !enumerable_only {
                names.push("length".to_string());
            }
            let metadata = items.meta.borrow();
            names.extend(
                items
                    .named
                    .borrow()
                    .iter()
                    .filter(|(key, _)| {
                        !is_internal_key(key)
                            && (!enumerable_only || metadata.attrs_of(key).enumerable)
                    })
                    .map(|(key, _)| key.clone()),
            );
            names
        }
        _ => Vec::new(),
    }
}

fn own_object_names(props: &Rc<ObjectCell>, enumerable_only: bool) -> Vec<String> {
    let meta = props.meta.borrow();
    props
        .borrow()
        .iter()
        .filter(|(k, _)| !is_internal_key(k))
        .filter(|(k, _)| !enumerable_only || meta.attrs_of(k).enumerable)
        .map(|(k, _)| k.clone())
        .collect()
}

fn own_names_for(
    interp: &mut Interpreter,
    value: &Value,
    enumerable_only: bool,
) -> Result<Vec<String>, VmErr> {
    if matches!(value, Value::Proxy(_)) {
        // The current Proxy model exposes ownKeys as a string array. Native
        // addon proxies return the host object's enumerable own keys here.
        return interp.keys_with_proxy_trap(value);
    }
    Ok(own_names(value, enumerable_only))
}

/// Read an own property slot without walking the prototype chain and without
/// invoking a getter.
fn own_slot(v: &Value, key: &str) -> Option<Value> {
    if let Value::Function(function) = v {
        function.ensure_name_length_properties();
        if key == "prototype" {
            function.prototype_value(v);
        }
    }
    if let Value::Array(array) = v {
        if key == "length" {
            return Some(Value::Number(array.borrow().len() as f64));
        }
        if let Some(index) = crate::value::array_index(key) {
            return array
                .has_index(index)
                .then(|| array.borrow().get(index).cloned())
                .flatten();
        }
        return array.named_prop(key);
    }
    cell(v)?
        .borrow()
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, value)| value.deref_binding())
}

/// Is this slot an accessor stored under the `get …` / `set …` naming that the
/// evaluator uses to recognize getters and setters?
fn accessor_kind(key: &str, value: &Value) -> Option<&'static str> {
    let name = match value {
        Value::Function(function) => function.name.as_deref(),
        Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
            Some(name.as_ref())
        }
        _ => None,
    }?;
    if name == format!("get {}", key) {
        Some("get")
    } else if name == format!("set {}", key) {
        Some("set")
    } else {
        None
    }
}

fn is_callable(value: &Value) -> bool {
    matches!(
        value,
        Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
    )
}

fn name_callable(value: &Value, name: &str) -> Option<Value> {
    Some(match value {
        Value::Function(function) => {
            let mut renamed = function.as_ref().clone();
            renamed.name = Some(name.into());
            Value::Function(Rc::new(renamed))
        }
        Value::NativeFunction { callable, .. } => Value::NativeFunction {
            name: name.into(),
            callable: *callable,
        },
        Value::HostFunction { .. } => value.host_function_named(name)?,
        _ => return None,
    })
}

// --- Enumeration ------------------------------------------------------------

fn object_keys(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    Value::checked_array(
        own_names_for(interp, &v, true)?
            .into_iter()
            .map(Value::String)
            .collect(),
    )
}

fn object_values(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let mut out = Vec::new();
    for key in own_names_for(interp, &v, true)? {
        out.push(interp.member(&v, &key)?);
    }
    Value::checked_array(out)
}

fn object_entries(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let mut out = Vec::new();
    for key in own_names_for(interp, &v, true)? {
        let value = interp.member(&v, &key)?;
        out.push(Value::array(vec![Value::String(key), value]));
    }
    Value::checked_array(out)
}

fn object_assign(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let target = a.first().cloned().unwrap_or_else(|| Value::object(vec![]));
    for src in a.iter().skip(1) {
        // Snapshot the keys first so `Object.assign(o, o)` does not hold a
        // borrow on the object it is about to write to.
        for key in own_names_for(interp, src, true)? {
            let value = interp.member(src, &key)?;
            interp.assign_member(&target, &Value::String(key), value)?;
        }
    }
    Ok(target)
}

fn object_get_own_property_names(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let names = match v {
        Value::GlobalObject => interp.global_keys(),
        ref other => own_names_for(interp, other, false)?,
    };
    Value::checked_array(names.into_iter().map(Value::String).collect())
}

fn object_from_entries(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let source = a.first().cloned().unwrap_or(Value::Undefined);
    let mut props: Vec<(String, Value)> = Vec::new();
    let mut symbol_keys = Vec::new();
    for entry in interp.iterate(&source)? {
        let raw_key = interp.member(&entry, "0")?;
        let symbol = match &raw_key {
            Value::Symbol(symbol) => Some(symbol.clone()),
            _ => None,
        };
        let key = interp.property_key(&raw_key)?;
        let value = interp.member(&entry, "1")?;
        match props.iter_mut().find(|(k, _)| *k == key) {
            Some((_, slot)) => *slot = value,
            None => props.push((key.clone(), value)),
        }
        if let Some(symbol) = symbol {
            symbol_keys.push((key, symbol));
        }
    }
    let object = Value::checked_object(props)?;
    if let Value::Object { props } = &object {
        let mut meta = props.meta.borrow_mut();
        for (key, symbol) in symbol_keys {
            meta.set_symbol_key(&key, symbol);
        }
    }
    Ok(object)
}

fn object_has_own(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    if matches!(v, Value::Undefined | Value::Null) {
        return Err(type_err("Cannot convert undefined or null to object"));
    }
    let key = interp.property_key(a.get(1).unwrap_or(&Value::Undefined))?;
    let found = if matches!(v, Value::GlobalObject) {
        interp.global_keys().iter().any(|name| name == &key)
    } else {
        object_property_attributes(&v, &key).is_some()
    };
    Ok(Value::Bool(found))
}

fn object_has_own_property(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    object_has_own(
        interp,
        Value::Undefined,
        vec![this, args.first().cloned().unwrap_or(Value::Undefined)],
    )
}

fn is_ecmascript_object(value: &Value) -> bool {
    matches!(
        value,
        Value::Object { .. }
            | Value::Array(_)
            | Value::Function(_)
            | Value::NativeFunction { .. }
            | Value::HostFunction { .. }
            | Value::GlobalObject
            | Value::Class(_)
            | Value::Promise(_)
            | Value::Generator { .. }
            | Value::StringIterator { .. }
            | Value::Date(_)
            | Value::Proxy(_)
            | Value::ArrayBuffer(_)
            | Value::SharedArrayBuffer(_)
            | Value::TypedArray(_)
            | Value::DataView(_)
            | Value::RegExp(_)
            | Value::Error(_)
    )
}

fn to_object_receiver(value: &Value) -> Result<Value, VmErr> {
    let value = value.deref_binding();
    if matches!(value, Value::Undefined | Value::Null) {
        return Err(type_err("Cannot convert undefined or null to object"));
    }
    if is_ecmascript_object(&value) {
        return Ok(value);
    }
    Value::boxed_primitive(value).ok_or_else(|| type_err("Cannot convert value to object"))
}

fn object_property_attributes(value: &Value, key: &str) -> Option<PropAttrs> {
    match value {
        Value::GlobalObject => return None,
        Value::Object { props } => {
            if let Some(BoxedPrimitive::String(string)) =
                props.meta.borrow().boxed_primitive.as_ref()
            {
                if key == "length" {
                    return Some(PropAttrs {
                        writable: false,
                        enumerable: false,
                        configurable: false,
                    });
                }
                return crate::value::array_index(key)
                    .filter(|index| *index < string.chars().count())
                    .map(|_| PropAttrs {
                        writable: false,
                        enumerable: true,
                        configurable: false,
                    });
            }
        }
        Value::Array(array) => {
            if key == "length" {
                return Some(array.meta.borrow().attrs_of(key));
            }
            if let Some(index) = crate::value::array_index(key) {
                return array
                    .has_index(index)
                    .then(|| array.meta.borrow().attrs_of(key));
            }
            return array
                .named_prop(key)
                .map(|_| array.meta.borrow().attrs_of(key));
        }
        Value::String(string) => {
            if key == "length" {
                return Some(PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: false,
                });
            }
            return crate::value::array_index(key)
                .filter(|index| *index < string.chars().count())
                .map(|_| PropAttrs {
                    writable: false,
                    enumerable: true,
                    configurable: false,
                });
        }
        Value::Error(error) => {
            return match key {
                "name" | "message" | "stack" => Some(PropAttrs {
                    writable: true,
                    enumerable: false,
                    configurable: true,
                }),
                "code" if error.code.is_some() => Some(PropAttrs::default()),
                _ => None,
            };
        }
        Value::RegExp(_) if key == "lastIndex" => {
            return Some(PropAttrs {
                writable: true,
                enumerable: false,
                configurable: false,
            });
        }
        Value::TypedArray(view) => {
            return crate::value::array_index(key)
                .filter(|index| *index < view.effective_length())
                .map(|_| PropAttrs::default());
        }
        _ => {}
    }

    if let Some(properties) = cell(value)
        && own_slot(value, key).is_some()
    {
        return Some(properties.meta.borrow().attrs_of(key));
    }
    None
}

fn object_property_is_enumerable(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let receiver = to_object_receiver(&this)?;
    let key = interp.property_key(args.first().unwrap_or(&Value::Undefined))?;
    Ok(Value::Bool(
        object_property_attributes(&receiver, &key).is_some_and(|attributes| attributes.enumerable),
    ))
}

fn object_is_prototype_of(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let receiver = to_object_receiver(&this)?;
    let mut current = args.first().cloned().unwrap_or(Value::Undefined);
    if !is_ecmascript_object(&current) {
        return Ok(Value::Bool(false));
    }
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        let prototype = interp.get_prototype_of(&current)?;
        if matches!(prototype, Value::Null) {
            return Ok(Value::Bool(false));
        }
        if crate::interpreter::strict_equals(&receiver, &prototype) {
            return Ok(Value::Bool(true));
        }
        current = prototype;
    }
    Err(crate::value::limit_err("Maximum prototype depth exceeded"))
}

fn object_to_string_tag(value: &Value) -> String {
    match value {
        Value::Undefined => "Undefined".into(),
        Value::Null => "Null".into(),
        Value::Bool(_) => "Boolean".into(),
        Value::Number(_) => "Number".into(),
        Value::String(_) => "String".into(),
        Value::Symbol(_) => "Symbol".into(),
        Value::BigInt(_) => "BigInt".into(),
        Value::Object { props } => match props.meta.borrow().boxed_primitive.as_ref() {
            Some(BoxedPrimitive::Bool(_)) => "Boolean".into(),
            Some(BoxedPrimitive::Number(_)) => "Number".into(),
            Some(BoxedPrimitive::String(_)) => "String".into(),
            Some(BoxedPrimitive::Symbol(_)) => "Symbol".into(),
            Some(BoxedPrimitive::BigInt(_)) => "BigInt".into(),
            None => crate::builtins::collection_tag(value)
                .unwrap_or("Object")
                .into(),
        },
        Value::Array(_) => "Array".into(),
        Value::Function(_)
        | Value::NativeFunction { .. }
        | Value::HostFunction { .. }
        | Value::Class(_) => "Function".into(),
        Value::GlobalObject => "global".into(),
        Value::Promise(_) => "Promise".into(),
        Value::Generator { .. } => "Generator".into(),
        Value::StringIterator { .. } => "String Iterator".into(),
        Value::HostPending { .. } | Value::Binding(_) => "Object".into(),
        #[cfg(stackful_coroutines)]
        Value::AsyncTask(_) => "AsyncTask".into(),
        Value::Date(_) => "Date".into(),
        Value::Proxy(_) => "Object".into(),
        Value::ArrayBuffer(_) => "ArrayBuffer".into(),
        Value::SharedArrayBuffer(_) => "SharedArrayBuffer".into(),
        Value::TypedArray(view) if view.is_buffer => "Uint8Array".into(),
        Value::TypedArray(view) => view.kind.name().into(),
        Value::DataView(_) => "DataView".into(),
        Value::RegExp(_) => "RegExp".into(),
        Value::Error(_) => "Error".into(),
    }
}

fn object_to_string(interp: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let this = this.deref_binding();
    let mut tag = object_to_string_tag(&this);
    if !matches!(this, Value::Undefined | Value::Null)
        && let Some(Value::Symbol(to_string_tag)) =
            crate::builtins::well_known("toStringTag").as_ref()
    {
        let custom = interp.prop(&this, &Value::Symbol(to_string_tag.clone()))?;
        if let Value::String(custom) = &custom {
            tag.clone_from(custom);
        }
    }
    Ok(Value::String(format!("[object {tag}]")))
}

fn object_to_locale_string(
    interp: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    let method = interp.prop_str(&this, "toString")?;
    if !is_callable(&method) {
        return Err(type_err("toString is not callable"));
    }
    interp.call_this(&method, this, vec![])
}

fn object_value_of(_: &mut Interpreter, this: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let this = this.deref_binding();
    if is_ecmascript_object(&this) {
        Ok(this)
    } else {
        Value::boxed_primitive(this)
            .ok_or_else(|| type_err("Cannot convert undefined or null to object"))
    }
}

fn object_define_getter(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    object_define_accessor(interp, this, args, true)
}

fn object_define_setter(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    object_define_accessor(interp, this, args, false)
}

fn object_define_accessor(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
    is_getter: bool,
) -> Result<Value, VmErr> {
    let target = this.deref_binding();
    if !is_ecmascript_object(&target)
        || (cell(&target).is_none() && !matches!(&target, Value::Array(_)))
    {
        return Err(type_err("Object.prototype accessor called on non-object"));
    }
    let raw_key = args.first().cloned().unwrap_or(Value::Undefined);
    let symbol = match &raw_key {
        Value::Symbol(symbol) => Some(symbol.clone()),
        _ => None,
    };
    let key = interp.property_key(&raw_key)?;
    let accessor = args.get(1).cloned().unwrap_or(Value::Undefined);
    if !is_callable(&accessor) {
        return Err(type_err("Getter and setter must be callable"));
    }

    let previous = descriptor_for(&target, &key);
    let existing_getter = previous.get_prop("get").unwrap_or(Value::Undefined);
    let existing_setter = previous.get_prop("set").unwrap_or(Value::Undefined);
    let mut fields = vec![
        (if is_getter { "get" } else { "set" }.into(), accessor),
        ("enumerable".into(), Value::Bool(true)),
        ("configurable".into(), Value::Bool(true)),
    ];
    if is_getter && is_callable(&existing_setter) {
        fields.push(("set".into(), existing_setter));
    }
    if !is_getter && is_callable(&existing_getter) {
        fields.push(("get".into(), existing_getter));
    }
    define_property(&target, &key, &Value::object(fields))?;
    if let Some(symbol) = symbol {
        if let Value::Array(array) = &target {
            array.set_symbol_key(&key, symbol);
        } else if let Some(properties) = cell(&target) {
            properties.meta.borrow_mut().set_symbol_key(&key, symbol);
        }
    }
    Ok(Value::Undefined)
}

fn object_lookup_getter(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    object_lookup_accessor(interp, this, args, false)
}

fn object_lookup_setter(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    object_lookup_accessor(interp, this, args, true)
}

fn object_lookup_accessor(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
    want_setter: bool,
) -> Result<Value, VmErr> {
    let receiver = to_object_receiver(&this)?;
    let key = interp.property_key(args.first().unwrap_or(&Value::Undefined))?;
    let mut current = receiver;
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        if let Some(value) = own_slot(&current, &key) {
            if accessor_kind(&key, &value) == Some("get") {
                if !want_setter {
                    return Ok(value);
                }
                return Ok(
                    own_slot(&current, &format!("__setter:{key}__")).unwrap_or(Value::Undefined)
                );
            }
            if accessor_kind(&key, &value) == Some("set") {
                return Ok(if want_setter { value } else { Value::Undefined });
            }
            return Ok(Value::Undefined);
        }
        let Some(prototype) = interp.prototype_of(&current) else {
            return Ok(Value::Undefined);
        };
        current = prototype.as_ref().clone();
    }
    Err(crate::value::limit_err("Maximum prototype depth exceeded"))
}

fn object_get_prototype(
    interp: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    let receiver = to_object_receiver(&this)?;
    interp.get_prototype_of(&receiver)
}

fn object_set_prototype(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = this.deref_binding();
    if !is_ecmascript_object(&target) || args.is_empty() {
        return Ok(Value::Undefined);
    }
    let requested = match args[0].deref_binding() {
        Value::Null => None,
        value if is_ecmascript_object(&value) => Some(Rc::new(value)),
        _ => return Ok(Value::Undefined),
    };
    if !matches!(
        &target,
        Value::Object { .. } | Value::Array(_) | Value::Function(_) | Value::Class(_)
    ) {
        return Ok(Value::Undefined);
    }
    if !object_is_extensible_value(&target)
        && !crate::interpreter::strict_equals(
            &interp
                .prototype_of(&target)
                .map_or(Value::Null, |prototype| prototype.as_ref().clone()),
            &requested
                .as_ref()
                .map_or(Value::Null, |prototype| prototype.as_ref().clone()),
        )
    {
        return Ok(Value::Undefined);
    }
    let mut current = requested
        .clone()
        .map(|prototype| prototype.as_ref().clone());
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        let Some(prototype) = current else { break };
        if crate::interpreter::strict_equals(&target, &prototype) {
            return Ok(Value::Undefined);
        }
        current = interp
            .prototype_of(&prototype)
            .map(|prototype| prototype.as_ref().clone());
    }
    match &target {
        Value::Object { props } => props.set_proto(requested),
        Value::Array(array) => array.set_proto(requested),
        Value::Function(function) => function.properties.set_proto(requested),
        Value::Class(class) => class.statics.set_proto(requested),
        _ => {}
    }
    Ok(Value::Undefined)
}

fn object_is_extensible_value(value: &Value) -> bool {
    if let Value::Array(array) = value {
        return !array.meta.borrow().non_extensible;
    }
    cell(value).is_some_and(|properties| !properties.meta.borrow().non_extensible)
}

fn object_is(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let x = a.first().cloned().unwrap_or(Value::Undefined);
    let y = a.get(1).cloned().unwrap_or(Value::Undefined);
    // `Object.is` differs from `===` exactly at NaN and signed zero.
    let same = match (&x, &y) {
        (Value::Number(a), Value::Number(b)) => {
            if a.is_nan() && b.is_nan() {
                true
            } else if *a == 0.0 && *b == 0.0 {
                a.is_sign_positive() == b.is_sign_positive()
            } else {
                a == b
            }
        }
        _ => interp.seq(&x, &y),
    };
    Ok(Value::Bool(same))
}

// --- Prototypes -------------------------------------------------------------

fn object_get_prototype_of(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    interp.get_prototype_of(&v)
}

fn object_set_prototype_of(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let proto = a.get(1).cloned().unwrap_or(Value::Null);
    let proto = proto_arg(&proto)?;
    if let Value::Array(array) = &v {
        array.set_proto(proto);
    } else if let Some(c) = cell(&v) {
        c.set_proto(proto);
    }
    Ok(v)
}

/// Validate and wrap the prototype argument shared by `create` and
/// `setPrototypeOf`: an object, or `null` for a null prototype.
fn proto_arg(proto: &Value) -> Result<Option<Rc<Value>>, VmErr> {
    match proto {
        Value::Null | Value::Undefined => Ok(None),
        Value::Object { .. } | Value::Array(_) | Value::Function(_) | Value::Class(_) => {
            Ok(Some(Rc::new(proto.clone())))
        }
        _ => Err(type_err("Object prototype may only be an Object or null")),
    }
}

fn object_create(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let proto = a.first().cloned().unwrap_or(Value::Null);
    let created = Value::object_with_proto(vec![], proto_arg(&proto)?);
    if let Some(descriptors) = a.get(1)
        && !matches!(descriptors, Value::Undefined | Value::Null)
    {
        apply_descriptor_map(interp, &created, descriptors)?;
    }
    Ok(created)
}

// --- Descriptors ------------------------------------------------------------

fn object_define_property(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    if cell(&target).is_none() && !matches!(target, Value::Array(_)) {
        return Err(type_err("Object.defineProperty called on non-object"));
    }
    let raw_key = a.get(1).cloned().unwrap_or(Value::Undefined);
    let symbol = match &raw_key {
        Value::Symbol(symbol) => Some(symbol.clone()),
        _ => None,
    };
    let key = interp.property_key(&raw_key)?;
    let descriptor = a.get(2).cloned().unwrap_or(Value::Undefined);
    define_property(&target, &key, &descriptor)?;
    if let Some(symbol) = symbol {
        match &target {
            Value::Array(array) => array.set_symbol_key(&key, symbol),
            _ => {
                if let Some(object) = cell(&target) {
                    object.meta.borrow_mut().set_symbol_key(&key, symbol);
                }
            }
        }
    }
    Ok(target)
}

fn object_define_properties(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    if cell(&target).is_none() && !matches!(target, Value::Array(_)) {
        return Err(type_err("Object.defineProperties called on non-object"));
    }
    let descriptors = a.get(1).cloned().unwrap_or(Value::Undefined);
    apply_descriptor_map(interp, &target, &descriptors)?;
    Ok(target)
}

fn apply_descriptor_map(
    _interp: &mut Interpreter,
    target: &Value,
    descriptors: &Value,
) -> Result<(), VmErr> {
    for key in own_names(descriptors, true) {
        let descriptor = own_slot(descriptors, &key).unwrap_or(Value::Undefined);
        define_property(target, &key, &descriptor)?;
    }
    Ok(())
}

/// Install one property from a descriptor object.
///
/// Data descriptors write the slot directly; accessor descriptors store the
/// function under the `get …` / `set …` name the evaluator recognizes. A
/// descriptor omitting an attribute gets `false`, per the specification —
/// which is why `defineProperty` produces a non-enumerable property by
/// default while plain assignment produces an enumerable one.
pub(crate) fn define_property(target: &Value, key: &str, descriptor: &Value) -> Result<(), VmErr> {
    if let Value::Array(array) = target {
        return define_array_property(array, key, descriptor);
    }
    if let Value::Function(function) = target {
        function.ensure_name_length_properties();
        function.prototype_value(target);
    }
    let Some(c) = cell(target) else {
        return Err(type_err("Object.defineProperty called on non-object"));
    };
    if cell(descriptor).is_none() {
        return Err(type_err("Property description must be an object"));
    }

    let old_value = own_slot(target, key);
    let existing = old_value.is_some();
    let old_attributes = c.meta.borrow().attrs_of(key);
    let getter = own_slot(descriptor, "get");
    let setter = own_slot(descriptor, "set");
    let value = own_slot(descriptor, "value");
    let writable = own_slot(descriptor, "writable");
    let enumerable = own_slot(descriptor, "enumerable");
    let configurable = own_slot(descriptor, "configurable");
    let accessor_fields = getter.is_some() || setter.is_some();
    if accessor_fields && (value.is_some() || writable.is_some()) {
        return Err(type_err("Invalid property descriptor"));
    }

    let old_accessor_kind = old_value
        .as_ref()
        .and_then(|value| accessor_kind(key, value));
    let old_is_accessor = old_accessor_kind.is_some();
    let old_is_data = existing && !old_is_accessor;
    let new_is_accessor =
        accessor_fields || (value.is_none() && writable.is_none() && old_is_accessor);

    // Omitted attributes preserve the current ones on redefine (and default
    // to `false` on first definition — which is why `defineProperty`
    // produces a non-enumerable property by default while plain assignment
    // produces an enumerable one). An accessor has no `writable` attribute:
    // assignment dispatches a setter before checking this flag, so accessors
    // stay non-writable here and getter-only properties cannot be
    // overwritten as data.
    let attrs = PropAttrs {
        writable: if new_is_accessor {
            false
        } else {
            writable
                .as_ref()
                .map(Value::is_truthy)
                .unwrap_or(existing && old_is_data && old_attributes.writable)
        },
        enumerable: enumerable
            .as_ref()
            .map(Value::is_truthy)
            .unwrap_or(existing && old_attributes.enumerable),
        configurable: configurable
            .as_ref()
            .map(Value::is_truthy)
            .unwrap_or(existing && old_attributes.configurable),
    };

    // ValidateAndApplyPropertyDescriptor for a non-configurable property:
    // same value, same attributes, same kind — except `writable` may narrow
    // from `true` to `false`, which is what class transpiler output relies on.
    if existing && !old_attributes.configurable {
        if attrs.configurable
            || attrs.enumerable != old_attributes.enumerable
            || old_is_accessor != new_is_accessor
        {
            return Err(type_err(&format!("Cannot redefine property: {key}")));
        }
        if old_is_data
            && !old_attributes.writable
            && (attrs.writable
                || value.as_ref().is_some_and(|new_value| {
                    old_value.as_ref().is_some_and(|old_value| {
                        !crate::interpreter::strict_equals(old_value, new_value)
                    })
                }))
        {
            return Err(type_err(&format!("Cannot redefine property: {key}")));
        }
        if old_is_accessor {
            let old_getter = (old_accessor_kind == Some("get"))
                .then(|| old_value.as_ref().cloned())
                .flatten()
                .unwrap_or(Value::Undefined);
            let old_setter = if old_accessor_kind == Some("set") {
                old_value.clone()
            } else {
                own_slot(target, &format!("__setter:{}__", key))
            };
            if getter
                .as_ref()
                .is_some_and(|new| !crate::interpreter::strict_equals(&old_getter, new))
                || setter.as_ref().is_some_and(|new| {
                    old_setter
                        .as_ref()
                        .is_none_or(|old| !crate::interpreter::strict_equals(old, new))
                })
            {
                return Err(type_err(&format!("Cannot redefine property: {key}")));
            }
        }
    }
    if !existing && c.meta.borrow().non_extensible {
        return Err(type_err(&format!(
            "Cannot define property {}, object is not extensible",
            key
        )));
    }

    let mut values: Vec<(String, Value)> = Vec::new();
    // Converting an accessor back to data drops the setter companion slot.
    let mut drop_companion = false;
    if new_is_accessor {
        for accessor in [getter.as_ref(), setter.as_ref()].into_iter().flatten() {
            if !matches!(accessor, Value::Undefined) && !is_callable(accessor) {
                return Err(type_err("Getter and setter must be callable"));
            }
        }
        let old_getter = (old_accessor_kind == Some("get"))
            .then(|| old_value.clone())
            .flatten();
        let old_setter = if old_accessor_kind == Some("set") {
            old_value.clone()
        } else if old_is_accessor {
            own_slot(target, &format!("__setter:{}__", key))
        } else {
            None
        };
        let getter = getter.or(old_getter).unwrap_or(Value::Undefined);
        let setter = setter.or(old_setter).unwrap_or(Value::Undefined);
        let getter = getter
            .is_truthy()
            .then(|| name_callable(&getter, &format!("get {key}")))
            .flatten();
        let setter = setter
            .is_truthy()
            .then(|| name_callable(&setter, &format!("set {key}")))
            .flatten();
        if let Some(getter) = getter {
            values.push((key.to_string(), getter));
        }
        if let Some(setter) = setter {
            // A setter lives in the same slot when there is no getter; with a
            // getter present it is stored under a companion slot the assign
            // path looks up.
            let slot = if values.is_empty() {
                key.to_string()
            } else {
                format!("__setter:{}__", key)
            };
            values.push((slot, setter));
        }
        if values.is_empty() {
            // `{ get: undefined, set: undefined }` still converts the slot.
            values.push((key.to_string(), Value::Undefined));
        }
    } else {
        let property_value = value
            .or_else(|| old_is_data.then(|| old_value.clone()).flatten())
            .unwrap_or(Value::Undefined);
        values.push((key.to_string(), property_value));
        drop_companion = old_is_accessor;
    }

    {
        let mut slots = c.borrow_mut();
        if drop_companion {
            let companion = format!("__setter:{}__", key);
            slots.retain(|(slot, _)| *slot != companion);
        }
        for (slot, value) in values {
            match slots.iter_mut().find(|(k, _)| *k == slot) {
                Some((_, existing)) => *existing = value,
                None => {
                    if slots.len() >= crate::value::MAX_OBJECT_PROPS {
                        return Err(crate::value::limit_err(
                            "Maximum object property count exceeded",
                        ));
                    }
                    slots.push((slot, value));
                }
            }
        }
    }
    // Redefines add, drop, or convert slots (accessor companions): the
    // layout changed in ways a single transition cannot describe.
    c.note_mutated();
    let mut meta = c.meta.borrow_mut();
    meta.set_attrs(key, attrs);
    if new_is_accessor {
        meta.has_accessors = true;
    }
    Ok(())
}

fn array_property_value(array: &crate::value::ArrayCell, key: &str) -> Option<Value> {
    if key == "length" {
        return Some(Value::Number(array.borrow().len() as f64));
    }
    if let Some(index) = crate::value::array_index(key) {
        return array
            .has_index(index)
            .then(|| array.borrow().get(index).cloned())
            .flatten();
    }
    array.named_prop(key)
}

fn array_store_property(
    array: &crate::value::ArrayCell,
    key: &str,
    value: Value,
) -> Result<(), VmErr> {
    if key == "length" {
        return Err(type_err("Invalid array length property definition"));
    }
    if let Some(index) = crate::value::array_index(key) {
        if index >= crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum array length exceeded"));
        }
        let old_length = array.borrow().len();
        if index >= old_length {
            let new_length = index + 1;
            array.borrow_mut().resize(new_length, Value::Undefined);
            array.resize_presence(old_length, new_length, false);
        }
        array.borrow_mut()[index] = value;
        array.set_index_presence(index, true);
    } else {
        array.set_named(key.to_owned(), value);
    }
    Ok(())
}

fn array_set_accessor_setter(array: &crate::value::ArrayCell, key: &str, setter: Option<Value>) {
    let companion = format!("__setter:{}__", key);
    match setter {
        Some(setter) => array.set_named(companion, setter),
        None => array
            .named
            .borrow_mut()
            .retain(|(name, _)| name != &companion),
    }
}

fn define_array_property(
    array: &crate::value::ArrayCell,
    key: &str,
    descriptor: &Value,
) -> Result<(), VmErr> {
    if cell(descriptor).is_none() && !matches!(descriptor, Value::Array(_)) {
        return Err(type_err("Property description must be an object"));
    }

    let old_value = array_property_value(array, key);
    let existing = old_value.is_some();
    let old_attributes = array.meta.borrow().attrs_of(key);
    let getter = own_slot(descriptor, "get");
    let setter = own_slot(descriptor, "set");
    let value = own_slot(descriptor, "value");
    let writable = own_slot(descriptor, "writable");
    let enumerable = own_slot(descriptor, "enumerable");
    let configurable = own_slot(descriptor, "configurable");
    let accessor_fields = getter.is_some() || setter.is_some();
    if accessor_fields && (value.is_some() || writable.is_some()) {
        return Err(type_err("Invalid property descriptor"));
    }

    if key == "length" {
        if accessor_fields {
            return Err(type_err("Cannot redefine array length as an accessor"));
        }
        if enumerable.is_some_and(|value| value.is_truthy())
            || configurable.is_some_and(|value| value.is_truthy())
        {
            return Err(type_err("Cannot redefine array length attributes"));
        }
        let old_length = array.borrow().len();
        let requested_length = match value {
            Some(Value::Number(length)) => {
                if !length.is_finite()
                    || length < 0.0
                    || length.fract() != 0.0
                    || length > crate::value::MAX_ARRAY_LEN as f64
                {
                    return Err(type_err("Invalid array length"));
                }
                Some(length as usize)
            }
            Some(_) => return Err(type_err("Invalid array length")),
            None => None,
        };
        let requested_writable = writable
            .as_ref()
            .map(Value::is_truthy)
            .unwrap_or(old_attributes.writable);
        if !old_attributes.writable
            && (requested_writable || requested_length.is_some_and(|length| length != old_length))
        {
            return Err(type_err("Cannot redefine non-writable array length"));
        }
        if let Some(length) = requested_length {
            array.set_length(length);
            if array.borrow().len() != length {
                return Err(type_err("Cannot remove a non-configurable array element"));
            }
        }
        let mut attributes = array.meta.borrow().attrs_of("length");
        attributes.enumerable = false;
        attributes.configurable = false;
        attributes.writable = requested_writable;
        array.meta.borrow_mut().set_attrs("length", attributes);
        return Ok(());
    }

    let index = crate::value::array_index(key);
    if index.is_some_and(|index| index >= crate::value::MAX_ARRAY_LEN) {
        return Err(crate::value::limit_err("Maximum array length exceeded"));
    }
    let old_accessor_kind = old_value
        .as_ref()
        .and_then(|value| accessor_kind(key, value));
    let old_is_accessor = old_accessor_kind.is_some();
    let old_is_data = existing && !old_is_accessor;
    let new_is_accessor =
        accessor_fields || (value.is_none() && writable.is_none() && old_is_accessor);

    let attributes = PropAttrs {
        writable: if new_is_accessor {
            false
        } else {
            writable
                .as_ref()
                .map(Value::is_truthy)
                .unwrap_or_else(|| existing && old_is_data && old_attributes.writable)
        },
        enumerable: enumerable
            .as_ref()
            .map(Value::is_truthy)
            .unwrap_or(existing && old_attributes.enumerable),
        configurable: configurable
            .as_ref()
            .map(Value::is_truthy)
            .unwrap_or(existing && old_attributes.configurable),
    };

    if existing && !old_attributes.configurable {
        if attributes.configurable
            || attributes.enumerable != old_attributes.enumerable
            || old_is_accessor != new_is_accessor
        {
            return Err(type_err(&format!("Cannot redefine property: {key}")));
        }
        if old_is_data
            && !old_attributes.writable
            && (attributes.writable
                || value.as_ref().is_some_and(|new_value| {
                    old_value.as_ref().is_some_and(|old_value| {
                        !crate::interpreter::strict_equals(old_value, new_value)
                    })
                }))
        {
            return Err(type_err(&format!("Cannot redefine property: {key}")));
        }
        if old_is_accessor {
            let old_getter = (old_accessor_kind == Some("get"))
                .then(|| old_value.as_ref().cloned())
                .flatten()
                .unwrap_or(Value::Undefined);
            let old_setter = if old_accessor_kind == Some("set") {
                old_value.clone()
            } else {
                array.named_prop(&format!("__setter:{}__", key))
            };
            if getter
                .as_ref()
                .is_some_and(|new| !crate::interpreter::strict_equals(&old_getter, new))
                || setter.as_ref().is_some_and(|new| {
                    old_setter
                        .as_ref()
                        .is_none_or(|old| !crate::interpreter::strict_equals(old, new))
                })
            {
                return Err(type_err(&format!("Cannot redefine property: {key}")));
            }
        }
    }
    if !existing && array.meta.borrow().non_extensible {
        return Err(type_err(&format!(
            "Cannot define property {key}, object is not extensible"
        )));
    }
    if let Some(index) = index {
        let length = array.borrow().len();
        if index >= length && !array.meta.borrow().attrs_of("length").writable {
            return Err(type_err("Cannot extend array with non-writable length"));
        }
    }

    if new_is_accessor {
        for accessor in [getter.as_ref(), setter.as_ref()].into_iter().flatten() {
            if !matches!(accessor, Value::Undefined) && !is_callable(accessor) {
                return Err(type_err("Getter and setter must be callable"));
            }
        }
        let old_getter = (old_accessor_kind == Some("get"))
            .then(|| old_value.clone())
            .flatten();
        let old_setter = if old_accessor_kind == Some("set") {
            old_value.clone()
        } else if old_is_accessor {
            array.named_prop(&format!("__setter:{}__", key))
        } else {
            None
        };
        let getter = getter.or(old_getter).unwrap_or(Value::Undefined);
        let setter = setter.or(old_setter).unwrap_or(Value::Undefined);
        let getter = getter
            .is_truthy()
            .then(|| name_callable(&getter, &format!("get {key}")))
            .flatten();
        let setter = setter
            .is_truthy()
            .then(|| name_callable(&setter, &format!("set {key}")))
            .flatten();
        let primary = getter
            .clone()
            .or_else(|| setter.clone())
            .unwrap_or(Value::Undefined);
        array_store_property(array, key, primary)?;
        array_set_accessor_setter(array, key, getter.and(setter));
        array.meta.borrow_mut().has_accessors = true;
    } else {
        let property_value = value
            .or_else(|| old_is_data.then(|| old_value.clone()).flatten())
            .unwrap_or(Value::Undefined);
        array_store_property(array, key, property_value)?;
        array_set_accessor_setter(array, key, None);
    }
    array.meta.borrow_mut().set_attrs(key, attributes);
    Ok(())
}

fn object_get_own_descriptor(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let key = interp.property_key(&a.get(1).cloned().unwrap_or(Value::Undefined))?;
    Ok(descriptor_for(&target, &key))
}

fn object_get_own_descriptors(
    _: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    let props = own_names(&target, false)
        .into_iter()
        .map(|key| {
            let descriptor = descriptor_for(&target, &key);
            (key, descriptor)
        })
        .collect();
    Value::checked_object(props)
}

/// Build the descriptor object for one own property, or `undefined` when the
/// property does not exist.
fn descriptor_for(target: &Value, key: &str) -> Value {
    if let Value::Array(items) = target {
        let array = items;
        let items = array.borrow();
        if let Some(index) = crate::value::array_index(key)
            && index < items.len()
            && array.has_index(index)
        {
            let value = items[index].clone();
            let attrs = array.meta.borrow().attrs_of(key);
            return descriptor_for_array_value(array, key, value, attrs);
        }
        if key == "length" {
            let attrs = array.meta.borrow().attrs_of("length");
            return Value::object(vec![
                ("value".to_string(), Value::Number(items.len() as f64)),
                ("writable".to_string(), Value::Bool(attrs.writable)),
                ("enumerable".to_string(), Value::Bool(attrs.enumerable)),
                ("configurable".to_string(), Value::Bool(attrs.configurable)),
            ]);
        }
        if let Some(value) = array.named_prop(key) {
            let attrs = array.meta.borrow().attrs_of(key);
            return descriptor_for_array_value(array, key, value, attrs);
        }
        return Value::Undefined;
    }

    let Some(c) = cell(target) else {
        return Value::Undefined;
    };
    let Some(value) = own_slot(target, key) else {
        return Value::Undefined;
    };
    let attrs = c.meta.borrow().attrs_of(key);
    let mut fields = Vec::new();
    match accessor_kind(key, &value) {
        Some("get") => {
            fields.push(("get".to_string(), value));
            let setter =
                own_slot(target, &format!("__setter:{}__", key)).unwrap_or(Value::Undefined);
            fields.push(("set".to_string(), setter));
        }
        Some("set") => {
            fields.push(("get".to_string(), Value::Undefined));
            fields.push(("set".to_string(), value));
        }
        _ => {
            fields.push(("value".to_string(), value));
            fields.push(("writable".to_string(), Value::Bool(attrs.writable)));
        }
    }
    fields.push(("enumerable".to_string(), Value::Bool(attrs.enumerable)));
    fields.push(("configurable".to_string(), Value::Bool(attrs.configurable)));
    Value::object(fields)
}

fn descriptor_for_array_value(
    array: &crate::value::ArrayCell,
    key: &str,
    value: Value,
    attrs: PropAttrs,
) -> Value {
    match accessor_kind(key, &value) {
        Some("get") => Value::object(vec![
            ("get".to_string(), value),
            (
                "set".to_string(),
                array
                    .named_prop(&format!("__setter:{}__", key))
                    .unwrap_or(Value::Undefined),
            ),
            ("enumerable".to_string(), Value::Bool(attrs.enumerable)),
            ("configurable".to_string(), Value::Bool(attrs.configurable)),
        ]),
        Some("set") => Value::object(vec![
            ("get".to_string(), Value::Undefined),
            ("set".to_string(), value),
            ("enumerable".to_string(), Value::Bool(attrs.enumerable)),
            ("configurable".to_string(), Value::Bool(attrs.configurable)),
        ]),
        _ => Value::object(vec![
            ("value".to_string(), value),
            ("writable".to_string(), Value::Bool(attrs.writable)),
            ("enumerable".to_string(), Value::Bool(attrs.enumerable)),
            ("configurable".to_string(), Value::Bool(attrs.configurable)),
        ]),
    }
}

// --- Integrity levels -------------------------------------------------------

/// Apply `seal`/`freeze`: mark the object non-extensible, and clear
/// `configurable` (and, when freezing, `writable`) on every own property.
fn lock(target: &Value, freeze: bool) {
    if let Value::Array(array) = target {
        array.set_integrity(freeze);
        return;
    }
    let Some(c) = cell(target) else { return };
    let keys: Vec<String> = c.borrow().iter().map(|(k, _)| k.clone()).collect();
    let mut meta = c.meta.borrow_mut();
    meta.non_extensible = true;
    for key in keys {
        let mut attrs = meta.attrs_of(&key);
        attrs.configurable = false;
        if freeze {
            attrs.writable = false;
        }
        meta.set_attrs(&key, attrs);
    }
}

/// Do every own property, and the object itself, already satisfy the
/// integrity level? An object with no properties is frozen as soon as it is
/// non-extensible.
fn locked(target: &Value, freeze: bool) -> bool {
    if let Value::Array(array) = target {
        return array.is_integrity_locked(freeze);
    }
    let Some(c) = cell(target) else {
        // Primitives are frozen and sealed vacuously.
        return !matches!(target, Value::Array(_));
    };
    let meta = c.meta.borrow();
    if !meta.non_extensible {
        return false;
    }
    c.borrow().iter().all(|(k, v)| {
        let attrs = meta.attrs_of(k);
        // Accessors have no writable attribute, so freezing does not require
        // one to be cleared.
        !attrs.configurable && (!freeze || !attrs.writable || accessor_kind(k, v).is_some())
    })
}

fn object_freeze(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    lock(&v, true);
    Ok(v)
}
fn object_seal(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    lock(&v, false);
    Ok(v)
}
fn object_is_frozen(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Bool(locked(&v, true)))
}
fn object_is_sealed(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Bool(locked(&v, false)))
}
fn object_prevent_extensions(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    if let Value::Array(array) = &v {
        array.meta.borrow_mut().non_extensible = true;
    } else if let Some(c) = cell(&v) {
        c.meta.borrow_mut().non_extensible = true;
    }
    Ok(v)
}
fn object_is_extensible(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Bool(match &v {
        Value::Array(array) => !array.meta.borrow().non_extensible,
        _ => cell(&v).is_some_and(|c| !c.meta.borrow().non_extensible),
    }))
}
