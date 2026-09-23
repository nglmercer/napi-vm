//! `Object` static methods, including the property-descriptor surface.

use std::rc::Rc;

use super::nf;
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter, is_internal_key};
use crate::value::{ObjectCell, PropAttrs, Value};

pub(super) fn install(e: &mut Environment) {
    let Some(o) = e.get("Object") else { return };
    let prototype = Value::object(vec![]);
    prototype
        .set_prop(
            "hasOwnProperty".to_string(),
            nf("hasOwnProperty", object_has_own_property),
        )
        .expect("built-in Object.prototype property");
    if let Value::Object { props } = &prototype {
        props.meta.borrow_mut().set_attrs(
            "hasOwnProperty",
            PropAttrs {
                enumerable: false,
                ..PropAttrs::default()
            },
        );
    }
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
        Value::Function(function) => Some(&function.properties),
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
        Value::Array(items) => {
            let mut names: Vec<String> = (0..items.borrow().len())
                .filter(|index| items.has_index(*index))
                .map(|index| index.to_string())
                .collect();
            if !enumerable_only {
                names.push("length".to_string());
            }
            names.extend(items.named.borrow().iter().map(|(key, _)| key.clone()));
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
            let mut function = function.clone();
            function.name = Some(name.into());
            Value::Function(function)
        }
        Value::NativeFunction { callable, .. } => Value::NativeFunction {
            name: name.into(),
            callable: *callable,
        },
        Value::HostFunction { id, .. } => Value::HostFunction {
            name: name.into(),
            id: *id,
        },
        _ => return None,
    })
}

fn desc_bool(desc: &Value, key: &str, default: bool) -> bool {
    match own_slot(desc, key) {
        Some(v) => v.is_truthy(),
        None => default,
    }
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
    let key = interp.property_key(a.get(1).unwrap_or(&Value::Undefined))?;
    let found = match &v {
        Value::GlobalObject => interp.global_keys().iter().any(|name| name == &key),
        Value::Object { props } => props.borrow().iter().any(|(k, _)| *k == key),
        Value::Array(items) => {
            key == "length"
                || crate::value::array_index(&key).is_some_and(|i| items.has_index(i))
                || items.named_prop(&key).is_some()
        }
        Value::String(s) => {
            key == "length" || key.parse::<usize>().is_ok_and(|i| i < s.chars().count())
        }
        _ => false,
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

fn object_get_prototype_of(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(match v.proto_of() {
        Some(p) => p.as_ref().clone(),
        None => Value::Null,
    })
}

fn object_set_prototype_of(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let proto = a.get(1).cloned().unwrap_or(Value::Null);
    if let Some(c) = cell(&v) {
        c.set_proto(proto_arg(&proto)?);
    }
    Ok(v)
}

/// Validate and wrap the prototype argument shared by `create` and
/// `setPrototypeOf`: an object, or `null` for a null prototype.
fn proto_arg(proto: &Value) -> Result<Option<Rc<Value>>, VmErr> {
    match proto {
        Value::Null | Value::Undefined => Ok(None),
        Value::Object { .. } | Value::Function(_) | Value::Class(_) => {
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
    if cell(&target).is_none() {
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
    if let (Some(object), Some(symbol)) = (cell(&target), symbol) {
        object.meta.borrow_mut().set_symbol_key(&key, symbol);
    }
    Ok(target)
}

fn object_define_properties(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    if cell(&target).is_none() {
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

    let existing = c.borrow().iter().any(|(k, _)| k == key);
    if existing && !c.meta.borrow().attrs_of(key).configurable {
        return Err(type_err(&format!("Cannot redefine property: {}", key)));
    }
    if !existing && c.meta.borrow().non_extensible {
        return Err(type_err(&format!(
            "Cannot define property {}, object is not extensible",
            key
        )));
    }

    let getter = own_slot(descriptor, "get");
    let setter = own_slot(descriptor, "set");
    let is_accessor =
        getter.as_ref().is_some_and(is_callable) || setter.as_ref().is_some_and(is_callable);

    let attrs = PropAttrs {
        // An accessor has no `writable` attribute. Assignment dispatches a
        // setter before checking this flag, so accessors can stay non-writable
        // here and getter-only properties cannot be overwritten as data.
        writable: if is_accessor {
            false
        } else {
            desc_bool(descriptor, "writable", false)
        },
        enumerable: desc_bool(descriptor, "enumerable", false),
        configurable: desc_bool(descriptor, "configurable", false),
    };

    let mut values: Vec<(String, Value)> = Vec::new();
    if is_accessor {
        if let Some(getter) = getter.as_ref().filter(|value| is_callable(value)) {
            let getter = name_callable(getter, &format!("get {}", key))
                .expect("callable getter can retain its value type");
            values.push((key.to_string(), getter));
        }
        if let Some(setter) = setter.as_ref().filter(|value| is_callable(value)) {
            let setter = name_callable(setter, &format!("set {}", key))
                .expect("callable setter can retain its value type");
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
    } else {
        values.push((
            key.to_string(),
            own_slot(descriptor, "value").unwrap_or(Value::Undefined),
        ));
    }

    {
        let mut slots = c.borrow_mut();
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
    let mut meta = c.meta.borrow_mut();
    meta.set_attrs(key, attrs);
    if is_accessor {
        meta.has_accessors = true;
    }
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
        let items = items.borrow();
        if let Some(index) = crate::value::array_index(key)
            && index < items.len()
            && target
                .as_array()
                .is_some_and(|array| array.has_index(index))
        {
            return Value::object(vec![
                ("value".to_string(), items[index].clone()),
                ("writable".to_string(), Value::Bool(true)),
                ("enumerable".to_string(), Value::Bool(true)),
                ("configurable".to_string(), Value::Bool(true)),
            ]);
        }
        if key == "length" {
            return Value::object(vec![
                ("value".to_string(), Value::Number(items.len() as f64)),
                ("writable".to_string(), Value::Bool(true)),
                ("enumerable".to_string(), Value::Bool(false)),
                ("configurable".to_string(), Value::Bool(false)),
            ]);
        }
        if let Some(value) = target.as_array().and_then(|array| array.named_prop(key)) {
            return Value::object(vec![
                ("value".to_string(), value),
                ("writable".to_string(), Value::Bool(true)),
                ("enumerable".to_string(), Value::Bool(true)),
                ("configurable".to_string(), Value::Bool(true)),
            ]);
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

// --- Integrity levels -------------------------------------------------------

/// Apply `seal`/`freeze`: mark the object non-extensible, and clear
/// `configurable` (and, when freezing, `writable`) on every own property.
fn lock(target: &Value, freeze: bool) {
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
    if let Some(c) = cell(&v) {
        c.meta.borrow_mut().non_extensible = true;
    }
    Ok(v)
}
fn object_is_extensible(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Bool(
        cell(&v).is_some_and(|c| !c.meta.borrow().non_extensible),
    ))
}
