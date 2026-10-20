//! `Object` static methods, including the property-descriptor surface.

use std::rc::Rc;

use super::nf;
use crate::error::VmErr;
use crate::interpreter::{Environment, Interpreter, is_internal_key};
use crate::value::{BoxedPrimitive, ObjectCell, PropAttrs, Value};

pub(super) fn install(e: &mut Environment) {
    let Some(o) = e.get("Object") else { return };
    for (key, value) in [
        ("length", Value::Number(1.)),
        ("name", Value::String("Object".into())),
    ] {
        o.set_prop(key.into(), value)
            .expect("Object constructor metadata");
        o.property_cell()
            .expect("Object constructor")
            .meta
            .borrow_mut()
            .set_attrs(
                key,
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
    }
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
        metadata.immutable_prototype = true;
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
    let proto_descriptor = Value::descriptor_record(vec![
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
        ("groupBy", super::collections::object_group_by),
        ("values", object_values),
        ("entries", object_entries),
        ("assign", object_assign),
        ("getOwnPropertyNames", object_get_own_property_names),
        ("getOwnPropertySymbols", object_get_own_property_symbols),
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

/// Complete native method objects after Function.prototype is available.
pub(super) fn install_function_metadata(e: &Environment) {
    let Some(object) = e.get("Object") else {
        return;
    };
    let function_prototype = e.get("Function").and_then(|v| v.get_prop("prototype"));
    object
        .property_cell()
        .expect("Object constructor")
        .set_proto(function_prototype.clone().map(Rc::new));
    for (target, methods) in [
        (
            object.clone(),
            vec![
                ("keys", 1),
                ("values", 1),
                ("entries", 1),
                ("assign", 2),
                ("groupBy", 2),
                ("getOwnPropertyNames", 1),
                ("getOwnPropertySymbols", 1),
                ("create", 2),
                ("defineProperty", 3),
                ("defineProperties", 2),
                ("getOwnPropertyDescriptor", 2),
                ("getOwnPropertyDescriptors", 1),
                ("getPrototypeOf", 1),
                ("setPrototypeOf", 2),
                ("hasOwn", 2),
                ("fromEntries", 1),
                ("freeze", 1),
                ("isFrozen", 1),
                ("seal", 1),
                ("isSealed", 1),
                ("preventExtensions", 1),
                ("isExtensible", 1),
                ("is", 2),
            ],
        ),
        (
            object
                .get_prop("prototype")
                .expect("Object.prototype installed"),
            vec![
                ("__defineGetter__", 2),
                ("__defineSetter__", 2),
                ("hasOwnProperty", 1),
                ("__lookupGetter__", 1),
                ("__lookupSetter__", 1),
                ("isPrototypeOf", 1),
                ("propertyIsEnumerable", 1),
                ("toLocaleString", 0),
                ("toString", 0),
                ("valueOf", 0),
            ],
        ),
    ] {
        for (name, length) in methods {
            if let Some(Value::NativeFunction { callable, .. }) = target.get_prop(name) {
                target
                    .set_prop(
                        name.into(),
                        super::native_method(name, length, callable, function_prototype.clone()),
                    )
                    .expect("built-in Object method");
                target
                    .property_cell()
                    .expect("Object method owner")
                    .meta
                    .borrow_mut()
                    .set_attrs(
                        name,
                        PropAttrs {
                            writable: true,
                            enumerable: false,
                            configurable: true,
                        },
                    );
            }
        }
    }
}

fn cell(v: &Value) -> Option<Rc<ObjectCell>> {
    if let Value::Function(function) = v {
        function.ensure_name_length_properties();
        function.prototype_value(v);
    }
    v.property_cell()
}

fn type_err(msg: &str) -> VmErr {
    VmErr::Msg(format!("TypeError: {}", msg))
}

/// Own property names in insertion order, excluding the VM's internal
/// symbol slots. `enumerable_only` applies the `enumerable` attribute.
fn own_names(v: &Value, enumerable_only: bool) -> Vec<String> {
    match v {
        Value::Object { props } => {
            let string = match props.meta.borrow().boxed_primitive.as_ref() {
                Some(BoxedPrimitive::String(string)) => Some(string.clone()),
                _ => None,
            };
            let mut names = Vec::new();
            if let Some(string) = string {
                names.extend((0..string.len()).map(|index| index.to_string()));
                if !enumerable_only {
                    names.push("length".into());
                }
            }
            names.extend(own_object_names(props, enumerable_only));
            names
        }
        Value::RegExp(data) => {
            let mut names = Vec::new();
            if !enumerable_only {
                names.push("lastIndex".into());
            }
            names.extend(
                own_object_names(&data.properties, enumerable_only)
                    .into_iter()
                    .filter(|name| name != "lastIndex"),
            );
            names
        }
        Value::TypedArray(view) => {
            let mut names: Vec<_> = (0..view.effective_length())
                .map(|index| index.to_string())
                .collect();
            names.extend(own_object_names(&view.properties, enumerable_only));
            names
        }
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
        _ => cell(v).map_or_else(Vec::new, |properties| {
            own_object_names(&properties, enumerable_only)
        }),
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

pub(crate) fn own_names_for(
    interp: &mut Interpreter,
    value: &Value,
    enumerable_only: bool,
) -> Result<Vec<String>, VmErr> {
    // Internal enumeration also serves for-in, where a nullish source is empty.
    // Object reflection entry points perform their own ToObject first.
    if matches!(value, Value::Null | Value::Undefined) {
        return Ok(Vec::new());
    }
    let value = to_object_receiver(value)?;
    let mut names = Vec::new();
    for key in interp.own_property_keys(&value)? {
        if let Value::String(name) = &key {
            if enumerable_only {
                let descriptor = descriptor_for_key_in(interp, &value, &key)?;
                if !descriptor
                    .get_prop("enumerable")
                    .is_some_and(|value| value.is_truthy())
                {
                    continue;
                }
            }
            names.push(name.to_key());
        }
    }
    Ok(names)
}

/// Ordinary [[OwnPropertyKeys]], including the indexed exotic keys and
/// symbols stored separately from string slots.
pub(crate) fn ordinary_own_property_keys(
    interp: &Interpreter,
    value: &Value,
) -> Result<Vec<Value>, VmErr> {
    let mut names = if let Some(global) = interp.global_scope_of(value) {
        global.borrow().global_property_keys()
    } else {
        own_names(value, false)
    };
    let mut seen = std::collections::HashSet::new();
    names.retain(|name| seen.insert(name.clone()));
    // Integer index keys precede other strings; the sort is stable for the
    // remaining strings, which retain their creation order.
    names.sort_by_key(|name| crate::value::array_index(name).map_or((1, 0), |index| (0, index)));
    let mut keys: Vec<Value> = names
        .into_iter()
        .map(|name| Value::String(crate::JsString::from_key(&name)))
        .collect();
    if let Value::Array(array) = value {
        for (slot, symbol) in array.symbol_keys.borrow().iter() {
            if array.named_prop(slot).is_some() {
                keys.push(Value::Symbol(symbol.clone()));
            }
        }
    } else if let Some(properties) = cell(value) {
        let meta = properties.meta.borrow();
        let slots = properties.borrow();
        for (slot, symbol) in &meta.symbol_keys {
            if slots.iter().any(|(name, _)| name == slot) {
                keys.push(Value::Symbol(symbol.clone()));
            }
        }
    }
    Ok(keys)
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
            return Some(Value::Number(array.array_length() as f64));
        }
        if let Some(index) = crate::value::array_index(key) {
            return array.index_value(index);
        }
        return array.named_prop(key);
    }
    if let Value::Object { props } = v
        && let Some(BoxedPrimitive::String(string)) = &props.meta.borrow().boxed_primitive
    {
        if key == "length" {
            return Some(Value::Number(string.len() as f64));
        }
        if let Some(index) = crate::value::array_index(key) {
            return crate::value::str_char_at(string, index);
        }
    }
    if let Value::RegExp(data) = v
        && key == "lastIndex"
    {
        return data
            .properties
            .own_value(key)
            .or_else(|| Some(Value::Number(data.last_index.get() as f64)));
    }
    if let Value::TypedArray(view) = v
        && let Some(index) = crate::value::array_index(key)
    {
        return crate::builtins::read_element(view, index);
    }
    cell(v)?.own_value(key).map(|value| value.deref_binding())
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

fn accessor_kind_for(target: &Value, key: &str, value: &Value) -> Option<&'static str> {
    let empty = match target {
        Value::Array(array) => array.meta.borrow().is_empty_accessor(key),
        _ => cell(target).is_some_and(|props| props.meta.borrow().is_empty_accessor(key)),
    };
    if empty {
        return Some("empty");
    }
    let symbol = match target {
        Value::Array(array) => array.meta.borrow().symbol_key(key),
        _ => cell(target).and_then(|cell| cell.meta.borrow().symbol_key(key)),
    };
    accessor_kind_with_symbol(symbol, key, value)
}

fn accessor_kind_with_symbol(
    symbol: Option<Rc<crate::value::SymbolData>>,
    key: &str,
    value: &Value,
) -> Option<&'static str> {
    if let Some(symbol) = symbol {
        let name = symbol
            .description
            .as_ref()
            .map_or_else(String::new, |description| format!("[{description}]"));
        accessor_kind(&name, value).or_else(|| accessor_kind(key, value))
    } else {
        accessor_kind(key, value)
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

/// Install a trusted intrinsic accessor using the same callable identity
/// representation as ordinary descriptor definitions.
pub(crate) fn install_intrinsic_accessor(
    target: &Value,
    key: &str,
    getter: &Value,
    setter: &Value,
    configurable: bool,
) {
    let properties = target
        .property_cell()
        .expect("intrinsic accessor property cell");
    let display = properties.meta.borrow().symbol_key(key).map_or_else(
        || key.to_string(),
        |symbol| {
            symbol
                .description
                .as_ref()
                .map_or_else(String::new, |description| format!("[{description}]"))
        },
    );
    let getter =
        name_callable(getter, &format!("get {display}")).expect("intrinsic getter is callable");
    let setter =
        name_callable(setter, &format!("set {display}")).expect("intrinsic setter is callable");
    target
        .set_prop(key.into(), getter)
        .expect("intrinsic getter slot");
    target
        .set_prop(format!("__setter:{key}__"), setter)
        .expect("intrinsic setter slot");
    let mut metadata = properties.meta.borrow_mut();
    metadata.has_accessors = true;
    metadata.set_attrs(
        key,
        PropAttrs {
            writable: false,
            enumerable: false,
            configurable,
        },
    );
}

// --- Enumeration ------------------------------------------------------------

#[derive(Clone, Copy)]
enum EnumerableOwnKind {
    Key,
    Value,
    Entry,
}

fn enumerable_own_properties(
    interp: &mut Interpreter,
    source: &Value,
    kind: EnumerableOwnKind,
) -> Result<Value, VmErr> {
    let source = to_object_receiver(source)?;
    let mut result = Vec::new();
    for key in interp.own_property_keys(&source)? {
        interp.check_execution()?;
        // EnumerableOwnProperties considers string keys only. In particular,
        // it must not consult a Proxy descriptor trap for a symbol key.
        if !matches!(key, Value::String(_)) {
            continue;
        }
        let descriptor = descriptor_for_key_in(interp, &source, &key)?;
        if !descriptor
            .get_prop("enumerable")
            .is_some_and(|value| value.is_truthy())
        {
            continue;
        }
        let entry = match kind {
            EnumerableOwnKind::Key => key,
            EnumerableOwnKind::Value => interp.get_prop_value(&source, &key)?,
            EnumerableOwnKind::Entry => {
                let value = interp.get_prop_value(&source, &key)?;
                Value::array(vec![key, value])
            }
        };
        if result.len() >= crate::value::MAX_ARRAY_LEN {
            return Err(crate::value::limit_err("Maximum array length exceeded"));
        }
        result.push(entry);
    }
    Value::checked_array(result)
}

fn object_keys(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    enumerable_own_properties(
        interp,
        a.first().unwrap_or(&Value::Undefined),
        EnumerableOwnKind::Key,
    )
}

fn object_values(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    enumerable_own_properties(
        interp,
        a.first().unwrap_or(&Value::Undefined),
        EnumerableOwnKind::Value,
    )
}

fn object_entries(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    enumerable_own_properties(
        interp,
        a.first().unwrap_or(&Value::Undefined),
        EnumerableOwnKind::Entry,
    )
}

fn object_assign(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let target = to_object_receiver(a.first().unwrap_or(&Value::Undefined))?;
    for src in a.iter().skip(1) {
        if matches!(src, Value::Null | Value::Undefined) {
            continue;
        }
        let src = to_object_receiver(src)?;
        // Assign interleaves each Get with a throwing Set. A setter may change
        // the next source descriptor, so do not materialize all values first.
        for key in interp.own_property_keys(&src)? {
            interp.check_execution()?;
            let descriptor = descriptor_for_key_in(interp, &src, &key)?;
            if !descriptor
                .get_prop("enumerable")
                .is_some_and(|value| value.is_truthy())
            {
                continue;
            }
            let value = interp.get_prop_value(&src, &key)?;
            if !interp.set_member_with_receiver(&target, &key, value, &target)? {
                return Err(type_err("Cannot assign to property"));
            }
        }
    }
    Ok(target)
}

fn object_get_own_property_names(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let v = to_object_receiver(&a.first().cloned().unwrap_or(Value::Undefined))?;
    let names = own_names_for(interp, &v, false)?;
    Value::checked_array(
        names
            .into_iter()
            .map(|value| Value::String(crate::JsString::from_key(&value)))
            .collect(),
    )
}

fn object_get_own_property_symbols(
    interp: &mut Interpreter,
    _: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = to_object_receiver(&args.first().cloned().unwrap_or(Value::Undefined))?;
    Value::checked_array(
        interp
            .own_property_keys(&target)?
            .into_iter()
            .filter(|key| matches!(key, Value::Symbol(_)))
            .collect(),
    )
}

fn object_from_entries(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let source = a.first().cloned().unwrap_or(Value::Undefined);
    let object = Value::checked_object(Vec::new())?;
    let iterator = interp.iterator_for(&source)?;
    let next = interp.member(&iterator, "next")?;
    if !crate::interpreter::is_callable_value(&next) {
        return Err(type_err("Iterator next is not callable"));
    }

    loop {
        interp.consume_loop()?;
        let step_result = (|| {
            let step = interp.call_this(&next, iterator.clone(), vec![])?;
            interp.iterator_result_fields(&step)
        })();
        let (done, entry) = match step_result {
            Ok(result) => result,
            Err(error) => {
                let close_result = interp.close_guest_iterator_for_abrupt(&iterator, false, &error);
                return Err(close_result.err().unwrap_or(error));
            }
        };
        if done {
            return Ok(object);
        }

        let add_entry = (|| {
            if !is_ecmascript_object(&entry) {
                return Err(type_err("Iterator entry must be an object"));
            }

            // AddEntriesFromIterable reads both entry properties before it
            // performs ToPropertyKey, then creates the data property before
            // requesting the next iterator result.
            let raw_key = interp.member(&entry, "0")?;
            let value = interp.member(&entry, "1")?;
            let key = interp.ecmascript_to_property_key(&raw_key)?;

            let slot = interp.property_key(&key)?;
            let properties = object.property_cell();
            let has_own_property = properties
                .as_ref()
                .is_some_and(|properties| properties.own_value(&slot).is_some());
            let property_count = object
                .property_cell()
                .map(|properties| properties.borrow().len())
                .unwrap_or(0);
            if !has_own_property && property_count >= crate::value::MAX_OBJECT_PROPS {
                return Err(crate::value::limit_err(
                    "Maximum object property count exceeded",
                ));
            }

            let descriptor = Value::descriptor_record(vec![
                ("value".into(), value),
                ("writable".into(), Value::Bool(true)),
                ("enumerable".into(), Value::Bool(true)),
                ("configurable".into(), Value::Bool(true)),
            ]);
            if !interp.define_own_property(&object, &key, &descriptor)? {
                return Err(type_err("Cannot create property from entry"));
            }
            Ok(())
        })();

        if let Err(error) = add_entry {
            let close_result = interp.close_guest_iterator_for_abrupt(&iterator, false, &error);
            return Err(close_result.err().unwrap_or(error));
        }
    }
}

fn object_has_own(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let target = to_object_receiver(&a.first().cloned().unwrap_or(Value::Undefined))?;
    let key = interp.ecmascript_to_property_key(a.get(1).unwrap_or(&Value::Undefined))?;
    Ok(Value::Bool(!matches!(
        descriptor_for_key_in(interp, &target, &key)?,
        Value::Undefined
    )))
}

fn object_has_own_property(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = interp.ecmascript_to_property_key(args.first().unwrap_or(&Value::Undefined))?;
    let target = to_object_receiver(&this)?;
    Ok(Value::Bool(!matches!(
        descriptor_for_key_in(interp, &target, &key)?,
        Value::Undefined
    )))
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
            | Value::RealmGlobal(_)
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

pub(crate) fn to_object_receiver(value: &Value) -> Result<Value, VmErr> {
    let value = value.deref_binding();
    if matches!(value, Value::Undefined | Value::Null) {
        return Err(type_err("Cannot convert undefined or null to object"));
    }
    if is_ecmascript_object(&value) {
        return Ok(value);
    }
    Value::boxed_primitive(value).ok_or_else(|| type_err("Cannot convert value to object"))
}

pub(crate) fn object_property_attributes(value: &Value, key: &str) -> Option<PropAttrs> {
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
                    .filter(|index| *index < string.len())
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
                .filter(|index| *index < string.len())
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
        Value::RegExp(data) if key == "lastIndex" => {
            if let Some((_, attrs)) = data
                .properties
                .meta
                .borrow()
                .attrs
                .iter()
                .find(|(name, _)| name == key)
            {
                return Some(*attrs);
            }
            return Some(PropAttrs {
                writable: true,
                enumerable: false,
                configurable: false,
            });
        }
        Value::TypedArray(view) => {
            return crate::value::array_index(key)
                .filter(|index| *index < view.effective_length())
                .map(|_| PropAttrs {
                    writable: !view.buffer.is_immutable(),
                    enumerable: true,
                    configurable: !view.buffer.is_immutable(),
                });
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
    let key = interp.ecmascript_to_property_key(args.first().unwrap_or(&Value::Undefined))?;
    let receiver = to_object_receiver(&this)?;
    let descriptor = descriptor_for_key_in(interp, &receiver, &key)?;
    Ok(Value::Bool(
        descriptor
            .get_prop("enumerable")
            .is_some_and(|v| v.is_truthy()),
    ))
}

fn object_is_prototype_of(
    interp: &mut Interpreter,
    this: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let mut current = args.first().cloned().unwrap_or(Value::Undefined);
    if !is_ecmascript_object(&current) {
        return Ok(Value::Bool(false));
    }
    // Preserve the specified argument-first order: primitive arguments return
    // false before ToObject can reject a nullish receiver.
    let receiver = to_object_receiver(&this)?;
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

/// The `builtinTag` fallback of `Object.prototype.toString`: only the
/// specification's internal-slot brands apply here. Named host brands
/// (`Map`, `Promise`, iterators, …) come from `@@toStringTag` properties
/// on prototype objects, so deleting or shadowing a tag falls back to
/// `"Object"` instead of leaking the value kind.
fn object_to_string_tag(value: &Value) -> Result<String, VmErr> {
    // Only IsArray and IsCallable forward through proxy targets. Other
    // builtin tags depend on internal slots on the receiver itself.
    if super::array::is_array(value)? {
        return Ok("Array".into());
    }
    let callable = crate::interpreter::is_callable_value(value);
    let errored = value.is_error_object();
    Ok(match value {
        Value::Uninitialized | Value::Undefined => "Undefined".into(),
        Value::Null => "Null".into(),
        Value::Object { .. } | Value::Proxy(_) if callable => "Function".into(),
        Value::Object { .. } if errored => "Error".into(),
        Value::Object { props } => {
            let meta = props.meta.borrow();
            if meta.is_arguments {
                "Arguments".into()
            } else {
                // Primitive wrappers report their brand when no string tag
                // is found, matching observed engine behavior for both
                // `ToObject` wrappers and genuine `new` wrappers. Boxed
                // symbols and bigints stay ordinary objects.
                match meta.boxed_primitive.as_ref() {
                    Some(BoxedPrimitive::Bool(_)) => "Boolean".into(),
                    Some(BoxedPrimitive::Number(_)) => "Number".into(),
                    Some(BoxedPrimitive::String(_)) => "String".into(),
                    _ => "Object".into(),
                }
            }
        }
        Value::Array(_) => "Array".into(),
        Value::Function(_)
        | Value::NativeFunction { .. }
        | Value::HostFunction { .. }
        | Value::Class(_) => "Function".into(),
        Value::GlobalObject | Value::RealmGlobal(_) => "global".into(),
        Value::HostPending { .. } | Value::Binding(_) => "Object".into(),
        #[cfg(stackful_coroutines)]
        Value::AsyncTask(_) => "AsyncTask".into(),
        Value::Date(_) => "Date".into(),
        Value::RegExp(_) => "RegExp".into(),
        Value::Error(_) => "Error".into(),
        // Other objects obtain their modern brands through @@toStringTag.
        _ => "Object".into(),
    })
}

pub(super) fn object_to_string(
    interp: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    let this = this.deref_binding();
    if matches!(this, Value::Undefined | Value::Uninitialized) {
        return Ok(Value::String(crate::JsString::from("[object Undefined]")));
    }
    if matches!(this, Value::Null) {
        return Ok(Value::String(crate::JsString::from("[object Null]")));
    }
    // Primitives box through `ToObject`, so their prototype's
    // `@@toStringTag` (or its absence) governs the result.
    let object = to_object_receiver(&this)?;
    let mut tag = crate::JsString::from(object_to_string_tag(&object)?.as_str());
    if let Some(Value::Symbol(to_string_tag)) = crate::builtins::well_known("toStringTag").as_ref()
    {
        let custom = interp.get_prop_value(&object, &Value::Symbol(to_string_tag.clone()))?;
        if let Value::String(custom) = &custom {
            tag = custom.clone();
        }
    }
    Ok(Value::String(
        crate::JsString::from("[object ")
            .concat(&tag)
            .concat(&crate::JsString::from("]")),
    ))
}

fn object_to_locale_string(
    interp: &mut Interpreter,
    this: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    let method = interp.get_prop_value_str(&this, "toString")?;
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
    if !is_ecmascript_object(&target) {
        return Err(type_err("Object.prototype accessor called on non-object"));
    }
    let accessor = args.get(1).cloned().unwrap_or(Value::Undefined);
    // Spec order: the callable check precedes key coercion, so a
    // non-callable getter never coerces (or observes) the key.
    if !is_callable(&accessor) {
        return Err(type_err("Getter and setter must be callable"));
    }
    // ToPropertyKey is abrupt: a throwing key coercion propagates.
    let key_value = interp.ecmascript_to_property_key(args.first().unwrap_or(&Value::Undefined))?;
    let symbol = match &key_value {
        Value::Symbol(symbol) => Some(symbol.clone()),
        _ => None,
    };
    let key = match &key_value {
        Value::String(string) => string.to_key(),
        Value::Symbol(symbol) => crate::interpreter::symbol_slot_key(symbol),
        _ => unreachable!("ToPropertyKey yields a String or a Symbol"),
    };

    let previous = descriptor_for_key_in(interp, &target, &key_value)?;
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
    // DefinePropertyOrThrow dispatches through Proxy defineProperty traps.
    if !interp.define_own_property(&target, &key_value, &Value::descriptor_record(fields))? {
        return Err(type_err("Cannot define property"));
    }
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
    let mut current = to_object_receiver(&this)?;
    // ToPropertyKey is abrupt: a key whose coercion throws (for example a
    // throwing `toString`) propagates instead of silently stringifying.
    let key = interp.ecmascript_to_property_key(args.first().unwrap_or(&Value::Undefined))?;
    // Both [[GetOwnProperty]] and [[GetPrototypeOf]] dispatch through
    // Proxy traps, so revoked proxies and throwing traps surface here.
    let field = if want_setter { "set" } else { "get" };
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        let desc = descriptor_for_key_in(interp, &current, &key)?;
        if !matches!(desc, Value::Undefined) {
            return Ok(desc.get_prop(field).unwrap_or(Value::Undefined));
        }
        let prototype = interp.get_prototype_of(&current)?;
        if matches!(prototype, Value::Null) {
            return Ok(Value::Undefined);
        }
        current = prototype;
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
    if matches!(target, Value::Null | Value::Undefined) {
        return Err(type_err("Cannot convert undefined or null to object"));
    }
    let Some(requested) = args.first() else {
        return Ok(Value::Undefined);
    };
    if (!matches!(requested, Value::Null) && !is_ecmascript_object(requested))
        || !is_ecmascript_object(&target)
    {
        return Ok(Value::Undefined);
    }
    if !interp.set_prototype_of(&target, requested)? {
        return Err(type_err("Cannot set object prototype"));
    }
    Ok(Value::Undefined)
}

fn object_is(_: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let x = a.first().cloned().unwrap_or(Value::Undefined);
    let y = a.get(1).cloned().unwrap_or(Value::Undefined);
    // `Object.is` differs from `===` exactly at NaN and signed zero.
    Ok(Value::Bool(same_value(&x, &y)))
}

pub(crate) fn same_value(x: &Value, y: &Value) -> bool {
    match (x, y) {
        (Value::Number(a), Value::Number(b)) => {
            if a.is_nan() && b.is_nan() {
                true
            } else if *a == 0.0 && *b == 0.0 {
                a.is_sign_positive() == b.is_sign_positive()
            } else {
                a == b
            }
        }
        _ => crate::interpreter::strict_equals(x, y),
    }
}

// --- Prototypes -------------------------------------------------------------

fn object_get_prototype_of(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    let object = to_object_receiver(&v)?;
    interp.get_prototype_of(&object)
}

fn object_set_prototype_of(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let value = a.first().cloned().unwrap_or(Value::Undefined);
    let prototype = a.get(1).cloned().unwrap_or(Value::Undefined);
    if matches!(value, Value::Null | Value::Undefined) {
        return Err(type_err("Cannot convert undefined or null to object"));
    }
    if !matches!(prototype, Value::Null) && !crate::interpreter::call::is_js_object(&prototype) {
        return Err(type_err("Object prototype may only be an Object or null"));
    }
    if crate::interpreter::call::is_js_object(&value)
        && !interp.set_prototype_of(&value, &prototype)?
    {
        return Err(type_err("Cannot set object prototype"));
    }
    Ok(value)
}

/// Object.create accepts every object kind, including Proxy and realm globals.
fn proto_arg(proto: &Value) -> Result<Option<Rc<Value>>, VmErr> {
    if matches!(proto, Value::Null) {
        Ok(None)
    } else if crate::interpreter::call::is_js_object(proto) {
        Ok(Some(Rc::new(proto.clone())))
    } else {
        Err(type_err("Object prototype may only be an Object or null"))
    }
}

fn object_create(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let proto = a.first().cloned().unwrap_or(Value::Undefined);
    let created = Value::object_with_proto(vec![], proto_arg(&proto)?);
    if let Some(descriptors) = a.get(1)
        && !matches!(descriptors, Value::Undefined)
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
    if !crate::interpreter::call::is_js_object(&target) {
        return Err(type_err("Object.defineProperty called on non-object"));
    }
    let key = interp.ecmascript_to_property_key(&a.get(1).cloned().unwrap_or(Value::Undefined))?;
    let descriptor =
        to_property_descriptor(interp, &a.get(2).cloned().unwrap_or(Value::Undefined))?;
    if !interp.define_own_property(&target, &key, &descriptor)? {
        return Err(type_err("Cannot define property"));
    }
    Ok(target)
}

fn object_define_properties(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = a.first().cloned().unwrap_or(Value::Undefined);
    if !crate::interpreter::call::is_js_object(&target) {
        return Err(type_err("Object.defineProperties called on non-object"));
    }
    let descriptors = a.get(1).cloned().unwrap_or(Value::Undefined);
    apply_descriptor_map(interp, &target, &descriptors)?;
    Ok(target)
}

fn apply_descriptor_map(
    interp: &mut Interpreter,
    target: &Value,
    descriptors: &Value,
) -> Result<(), VmErr> {
    let descriptors = to_object_receiver(descriptors)?;
    let mut pending = Vec::new();
    for key in interp.own_property_keys(&descriptors)? {
        interp.check_execution()?;
        let own = descriptor_for_key_in(interp, &descriptors, &key)?;
        if own.get_prop("enumerable").is_some_and(|v| v.is_truthy()) {
            let value = interp.get_prop_value(&descriptors, &key)?;
            let descriptor = to_property_descriptor(interp, &value)?;
            pending.push((key, descriptor));
        }
    }
    // All descriptor conversions precede the first mutation of the target.
    for (key, descriptor) in pending {
        interp.check_execution()?;
        if !interp.define_own_property(target, &key, &descriptor)? {
            return Err(type_err("Cannot define property"));
        }
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
    let mapped = target.property_cell().and_then(|props| {
        if !props.meta.borrow().is_arguments {
            return None;
        }
        match props.own_value(key) {
            Some(Value::Binding(ref cell)) => Some(cell.clone()),
            _ => None,
        }
    });
    let Some(mapped) = mapped else {
        return define_ordinary_property(target, key, descriptor);
    };
    let accessor = descriptor.get_prop("get").is_some() || descriptor.get_prop("set").is_some();
    let readonly = descriptor
        .get_prop("writable")
        .is_some_and(|value| !value.is_truthy());
    let snapshot;
    let actual = if !accessor && readonly && descriptor.get_prop("value").is_none() {
        let Value::Object { props } = descriptor else {
            unreachable!("normalized descriptor");
        };
        let mut fields = props.borrow().clone();
        fields.push(("value".into(), mapped.borrow().clone_for_execution()));
        snapshot = Value::descriptor_record(fields);
        &snapshot
    } else {
        descriptor
    };
    define_ordinary_property(target, key, actual)?;
    if !accessor {
        if let Some(value) = descriptor.get_prop("value") {
            *mapped.borrow_mut() = value;
        }
        if !readonly {
            let props = target.property_cell().expect("arguments properties");
            let index = props.own_index(key).expect("defined mapped index");
            props.borrow_mut()[index].1 = Value::Binding(mapped);
        }
    }
    Ok(())
}

fn define_ordinary_property(target: &Value, key: &str, descriptor: &Value) -> Result<(), VmErr> {
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

    if let Value::TypedArray(view) = target {
        // Integer-indexed elements never become ordinary property slots.
        let numeric = key.parse::<f64>().ok();
        let canonical = key == "-0"
            || key == "NaN"
            || key == "Infinity"
            || key == "-Infinity"
            || numeric.is_some_and(|index| index.to_string() == key);
        if canonical {
            let index = numeric.unwrap_or(f64::NAN);
            if view.buffer.is_immutable() {
                if !crate::builtins::valid_integer_index(view, index) {
                    return Err(type_err("Cannot redefine an invalid typed array index"));
                }
                if own_slot(descriptor, "get").is_some()
                    || own_slot(descriptor, "set").is_some()
                    || own_slot(descriptor, "configurable").is_some_and(|v| v.is_truthy())
                    || own_slot(descriptor, "writable").is_some_and(|v| v.is_truthy())
                    || own_slot(descriptor, "enumerable").is_some_and(|v| !v.is_truthy())
                    || own_slot(descriptor, "value").is_some_and(|v| {
                        !same_value(
                            &v,
                            &crate::builtins::read_element(view, index as usize)
                                .unwrap_or(Value::Undefined),
                        )
                    })
                {
                    return Err(type_err("Cannot redefine an immutable typed array element"));
                }
                return Ok(());
            }

            if key == "-0"
                || !index.is_finite()
                || index < 0.0
                || index.fract() != 0.0
                || index >= view.effective_length() as f64
                || own_slot(descriptor, "get").is_some()
                || own_slot(descriptor, "set").is_some()
                || ["configurable", "enumerable", "writable"]
                    .iter()
                    .any(|name| own_slot(descriptor, name).is_some_and(|value| !value.is_truthy()))
            {
                return Err(type_err("Cannot redefine a typed array element"));
            }
            if let Some(value) = own_slot(descriptor, "value") {
                crate::builtins::write_element(view, index as usize, &value)?;
            }
            return Ok(());
        }
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
    if c.meta.borrow().module_namespace && c.meta.borrow().symbol_key(key).is_none() {
        let same = match (&old_value, &value) {
            (_, None) => true,
            (Some(Value::Number(a)), Some(Value::Number(b))) => {
                (a.is_nan() && b.is_nan())
                    || (a == b && (a != &0. || a.is_sign_negative() == b.is_sign_negative()))
            }
            (Some(a), Some(b)) => crate::interpreter::strict_equals(a, b),
            _ => false,
        };
        if !existing
            || getter.is_some()
            || setter.is_some()
            || configurable.as_ref().is_some_and(Value::is_truthy)
            || enumerable.as_ref().is_some_and(|v| !v.is_truthy())
            || writable.as_ref().is_some_and(|v| !v.is_truthy())
            || !same
        {
            return Err(type_err("Cannot redefine a module namespace export"));
        }
        return Ok(());
    }

    let accessor_fields = getter.is_some() || setter.is_some();
    if accessor_fields && (value.is_some() || writable.is_some()) {
        return Err(type_err("Invalid property descriptor"));
    }

    let old_accessor_kind = old_value
        .as_ref()
        .and_then(|value| accessor_kind_for(target, key, value));
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
    if existing && !compatible_descriptor(&descriptor_for(target, key), descriptor) {
        return Err(type_err(&format!("Cannot redefine property: {key}")));
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
    let mut empty_accessor = false;
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
        // A present field overrides even when it is `undefined`
        // (`{ set: undefined }` clears the setter); only an absent field
        // inherits the current accessor.
        let getter = getter.unwrap_or_else(|| old_getter.unwrap_or(Value::Undefined));
        let setter = setter.unwrap_or_else(|| old_setter.unwrap_or(Value::Undefined));
        let getter = getter
            .is_truthy()
            .then(|| name_callable(&getter, &format!("get {key}")))
            .flatten();
        let setter = setter
            .is_truthy()
            .then(|| name_callable(&setter, &format!("set {key}")))
            .flatten();
        let has_getter = getter.is_some();
        if let Some(getter) = getter {
            values.push((key.to_string(), getter));
        }
        let has_setter = setter.is_some();
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
            empty_accessor = true;
            // `{ get: undefined, set: undefined }` still converts the slot.
            values.push((key.to_string(), Value::Undefined));
        }
        // Clearing one side of an accessor pair must also drop the companion
        // setter slot; otherwise the old function stays visible through the
        // descriptor and assignment paths.
        if old_is_accessor && (!has_getter || !has_setter) {
            drop_companion = true;
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
    meta.set_empty_accessor(key, empty_accessor);
    if new_is_accessor {
        meta.has_accessors = true;
    }
    Ok(())
}

fn array_property_value(array: &crate::value::ArrayCell, key: &str) -> Option<Value> {
    if key == "length" {
        return Some(Value::Number(array.array_length() as f64));
    }
    if let Some(index) = crate::value::array_index(key) {
        return array.index_value(index);
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
        array.store_index(index, value);
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
        let old_length = array.array_length();
        let requested_length = match value {
            Some(Value::Number(length)) => {
                // Array lengths are Uint32 values; only huge ones stay sparse.
                if !length.is_finite()
                    || length < 0.0
                    || length.fract() != 0.0
                    || length > u32::MAX as f64
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
            if array.array_length() != length {
                if !requested_writable {
                    let mut attributes = array.meta.borrow().attrs_of("length");
                    attributes.writable = false;
                    array.meta.borrow_mut().set_attrs("length", attributes);
                }
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
    let old_accessor_kind = if array.meta.borrow().is_empty_accessor(key) {
        Some("empty")
    } else {
        old_value
            .as_ref()
            .and_then(|value| accessor_kind(key, value))
    };
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

    if existing
        && !compatible_descriptor(
            &descriptor_for_array_value(
                array,
                key,
                old_value.clone().unwrap_or(Value::Undefined),
                old_attributes,
            ),
            descriptor,
        )
    {
        return Err(type_err(&format!("Cannot redefine property: {key}")));
    }
    if !existing && array.meta.borrow().non_extensible {
        return Err(type_err(&format!(
            "Cannot define property {key}, object is not extensible"
        )));
    }
    if let Some(index) = index {
        let length = array.array_length();
        if index >= length && !array.meta.borrow().attrs_of("length").writable {
            return Err(type_err("Cannot extend array with non-writable length"));
        }
    }

    let empty_accessor;
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
        // A present field overrides even when it is `undefined`
        // (`{ set: undefined }` clears the setter); only an absent field
        // inherits the current accessor.
        let getter = getter.unwrap_or_else(|| old_getter.unwrap_or(Value::Undefined));
        let setter = setter.unwrap_or_else(|| old_setter.unwrap_or(Value::Undefined));
        let getter = getter
            .is_truthy()
            .then(|| name_callable(&getter, &format!("get {key}")))
            .flatten();
        let setter = setter
            .is_truthy()
            .then(|| name_callable(&setter, &format!("set {key}")))
            .flatten();
        empty_accessor = getter.is_none() && setter.is_none();
        let primary = getter
            .clone()
            .or_else(|| setter.clone())
            .unwrap_or(Value::Undefined);
        array_store_property(array, key, primary)?;
        array_set_accessor_setter(array, key, getter.and(setter));
        array.meta.borrow_mut().has_accessors = true;
    } else {
        empty_accessor = false;
        let property_value = value
            .or_else(|| old_is_data.then(|| old_value.clone()).flatten())
            .unwrap_or(Value::Undefined);
        array_store_property(array, key, property_value)?;
        array_set_accessor_setter(array, key, None);
    }
    array.meta.borrow_mut().set_attrs(key, attributes);
    array
        .meta
        .borrow_mut()
        .set_empty_accessor(key, empty_accessor);
    Ok(())
}

fn object_get_own_descriptor(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = to_object_receiver(&a.first().cloned().unwrap_or(Value::Undefined))?;
    let key = interp.ecmascript_to_property_key(&a.get(1).cloned().unwrap_or(Value::Undefined))?;
    descriptor_for_key_in(interp, &target, &key).map(from_property_descriptor)
}

fn object_get_own_descriptors(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let target = to_object_receiver(a.first().unwrap_or(&Value::Undefined))?;
    let mut props = Vec::new();
    let mut symbols = Vec::new();
    for key in interp.own_property_keys(&target)? {
        interp.check_execution()?;
        let descriptor = descriptor_for_key_in(interp, &target, &key)?;
        if !matches!(descriptor, Value::Undefined) {
            let slot = interp.property_key(&key)?;
            if let Value::Symbol(symbol) = &key {
                symbols.push((slot.clone(), symbol.clone()));
            }
            if props.len() >= crate::value::MAX_OBJECT_PROPS {
                return Err(crate::value::limit_err(
                    "Maximum object property count exceeded",
                ));
            }
            props.push((slot, from_property_descriptor(descriptor)));
        }
    }
    let result = Value::checked_object(props)?;
    if let Value::Object { props } = &result {
        let mut metadata = props.meta.borrow_mut();
        for (slot, symbol) in symbols {
            metadata.set_symbol_key(&slot, symbol);
        }
    }
    Ok(result)
}

/// FromPropertyDescriptor creates an ordinary object only when a descriptor
/// escapes to guest code; internal operations keep prototype-free records.
pub(crate) fn from_property_descriptor(descriptor: Value) -> Value {
    match descriptor {
        Value::Object { ref props } => Value::object(props.borrow().clone()),
        other => other,
    }
}

/// Build the descriptor object for one own property, or `undefined` when the
/// property does not exist.
/// ToPropertyDescriptor preserves missing fields and performs observable reads
/// once, in specification order. All public and Proxy descriptor operations
/// consume this same normalized representation.
pub(crate) fn to_property_descriptor(
    interp: &mut Interpreter,
    result: &Value,
) -> Result<Value, VmErr> {
    if !is_ecmascript_object(result) {
        return Err(type_err("Property description must be an object"));
    }
    let mut fields = Vec::new();
    for name in [
        "enumerable",
        "configurable",
        "value",
        "writable",
        "get",
        "set",
    ] {
        if interp.has_property(result, &Value::String(name.into()))? {
            let mut value = interp.get_prop_value_str(result, name)?;
            if matches!(name, "enumerable" | "configurable" | "writable") {
                value = Value::Bool(value.is_truthy());
            }
            if matches!(name, "get" | "set")
                && !matches!(value, Value::Undefined)
                && !is_callable(&value)
            {
                return Err(type_err("Property descriptor accessor must be callable"));
            }
            fields.push((name.into(), value));
        }
    }
    let accessor = fields
        .iter()
        .any(|(name, _)| name == "get" || name == "set");
    if accessor
        && fields
            .iter()
            .any(|(name, _)| name == "value" || name == "writable")
    {
        return Err(type_err("Invalid property descriptor"));
    }
    Ok(Value::descriptor_record(fields))
}

/// IsCompatiblePropertyDescriptor over the shared normalized descriptor model.
/// Missing fields preserve current attributes; values and accessors use SameValue.
pub(crate) fn compatible_descriptor(current: &Value, descriptor: &Value) -> bool {
    if matches!(current, Value::Undefined)
        || current
            .get_prop("configurable")
            .is_some_and(|value| value.is_truthy())
    {
        return true;
    }
    if descriptor
        .get_prop("configurable")
        .is_some_and(|value| value.is_truthy())
        || descriptor.get_prop("enumerable").is_some_and(|value| {
            value.is_truthy()
                != current
                    .get_prop("enumerable")
                    .is_some_and(|value| value.is_truthy())
        })
    {
        return false;
    }
    let data = descriptor.get_prop("value").is_some() || descriptor.get_prop("writable").is_some();
    let accessor = descriptor.get_prop("get").is_some() || descriptor.get_prop("set").is_some();
    if !data && !accessor {
        return true;
    }
    let current_data =
        current.get_prop("value").is_some() || current.get_prop("writable").is_some();
    if data != current_data {
        return false;
    }
    if data {
        if !current
            .get_prop("writable")
            .is_some_and(|value| value.is_truthy())
        {
            if descriptor
                .get_prop("writable")
                .is_some_and(|value| value.is_truthy())
            {
                return false;
            }
            if let Some(value) = descriptor.get_prop("value") {
                return same_value(
                    &current.get_prop("value").unwrap_or(Value::Undefined),
                    &value,
                );
            }
        }
    } else {
        for name in ["get", "set"] {
            if let Some(value) = descriptor.get_prop(name)
                && !same_value(&current.get_prop(name).unwrap_or(Value::Undefined), &value)
            {
                return false;
            }
        }
    }
    true
}

pub(crate) fn descriptor_for_in(
    interp: &mut Interpreter,
    target: &Value,
    key: &str,
) -> Result<Value, VmErr> {
    descriptor_for_key_in(
        interp,
        target,
        &Value::String(crate::JsString::from_key(key)),
    )
}

pub(crate) fn descriptor_for_key_in(
    interp: &mut Interpreter,
    target: &Value,
    property_key: &Value,
) -> Result<Value, VmErr> {
    if let Value::Proxy(proxy) = target {
        let (target, handler) = proxy.snapshot()?;
        let trap = interp.get_prop_value_str(&handler, "getOwnPropertyDescriptor")?;
        if matches!(trap, Value::Undefined | Value::Null) {
            return descriptor_for_key_in(interp, &target, property_key);
        }
        if !crate::interpreter::is_callable_value(&trap) {
            return Err(type_err(
                "Proxy getOwnPropertyDescriptor trap must be callable",
            ));
        }
        let result =
            interp.call_this(&trap, handler, vec![target.clone(), property_key.clone()])?;
        if !matches!(result, Value::Undefined) && !is_ecmascript_object(&result) {
            return Err(type_err(
                "Proxy descriptor trap must return an object or undefined",
            ));
        }
        let previous = descriptor_for_key_in(interp, &target, property_key)?;
        let exists = !matches!(previous, Value::Undefined);
        let extensible = interp.is_extensible(&target)?;
        let configurable = previous
            .get_prop("configurable")
            .is_some_and(|v| v.is_truthy());
        if matches!(result, Value::Undefined) {
            if exists && (!configurable || !extensible) {
                return Err(type_err("Proxy cannot hide a protected target property"));
            }
            return Ok(Value::Undefined);
        }
        let normalized = to_property_descriptor(interp, &result)?;
        let Value::Object { ref props } = normalized else {
            unreachable!("normalized descriptor");
        };
        let mut fields = props.borrow().clone();
        // CompletePropertyDescriptor preserves accessor fields even when
        // both are undefined. Ordinary storage cannot represent that case
        // solely by the callable name of its property slot.
        let accessor = fields
            .iter()
            .any(|(name, _)| name == "get" || name == "set");
        let defaults = if accessor {
            vec![("get", Value::Undefined), ("set", Value::Undefined)]
        } else {
            vec![
                ("value", Value::Undefined),
                ("writable", Value::Bool(false)),
            ]
        };
        for (name, value) in defaults.into_iter().chain([
            ("enumerable", Value::Bool(false)),
            ("configurable", Value::Bool(false)),
        ]) {
            if !fields.iter().any(|(field, _)| field == name) {
                fields.push((name.into(), value));
            }
        }
        let descriptor = Value::descriptor_record(fields);
        // Validate descriptor shape without changing the target or returning
        // a reconstructed accessor whose callable name was rewritten.
        if !exists && !extensible {
            return Err(type_err(
                "Proxy cannot add a property to a non-extensible target",
            ));
        }
        if exists && !compatible_descriptor(&previous, &descriptor) {
            return Err(type_err("Proxy descriptor is incompatible with the target"));
        }
        if !descriptor
            .get_prop("configurable")
            .is_some_and(|v| v.is_truthy())
        {
            if !exists || configurable {
                return Err(type_err("Proxy cannot invent a non-configurable property"));
            }
            if previous.get_prop("writable").is_some_and(|v| v.is_truthy())
                && descriptor
                    .get_prop("writable")
                    .is_some_and(|v| !v.is_truthy())
            {
                return Err(type_err(
                    "Proxy cannot report a writable property as frozen",
                ));
            }
        }
        return Ok(descriptor);
    }
    interp.evaluate_deferred_namespace_property(target, property_key)?;
    let key_storage = interp.property_key(property_key)?;
    let key = key_storage.as_str();
    if let Some(global) = interp.global_scope_of(target) {
        let object = global
            .borrow()
            .global_object()
            .expect("realm object record");
        return Ok(descriptor_for(&object, key));
    }
    let descriptor = descriptor_for(target, key);
    if !matches!(property_key, Value::Symbol(_))
        && cell(target).is_some_and(|properties| properties.meta.borrow().module_namespace)
        && !matches!(descriptor, Value::Undefined)
    {
        // Namespace [[GetOwnProperty]] reads the live export binding. A slot
        // exists during its TDZ, but descriptor queries must still throw.
        let value = interp.member(target, key)?;
        descriptor.set_prop("value".into(), value)?;
    }
    Ok(descriptor)
}

fn descriptor_for(target: &Value, key: &str) -> Value {
    if let Value::Array(items) = target {
        let array = items;
        if let Some(index) = crate::value::array_index(key)
            && let Some(value) = array.index_value(index)
        {
            let attrs = array.meta.borrow().attrs_of(key);
            return descriptor_for_array_value(array, key, value, attrs);
        }
        if key == "length" {
            let attrs = array.meta.borrow().attrs_of("length");
            return Value::descriptor_record(vec![
                (
                    "value".to_string(),
                    Value::Number(array.array_length() as f64),
                ),
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
    let attrs =
        object_property_attributes(target, key).unwrap_or_else(|| c.meta.borrow().attrs_of(key));
    let mut fields = Vec::new();
    match c
        .meta
        .borrow()
        .has_accessors
        .then(|| accessor_kind_for(target, key, &value))
        .flatten()
    {
        Some("get" | "set" | "empty") => {
            let slots = c.borrow();
            let getter = slots
                .iter()
                .find(|(name, value)| {
                    name == key && accessor_kind_for(target, key, value) == Some("get")
                })
                .map(|(_, value)| value.clone())
                .unwrap_or(Value::Undefined);
            let companion = format!("__setter:{key}__");
            let setter = slots
                .iter()
                .find(|(name, value)| {
                    (name == key || name == &companion)
                        && accessor_kind_for(target, key, value) == Some("set")
                })
                .map(|(_, value)| value.clone())
                .unwrap_or(Value::Undefined);
            fields.push(("get".to_string(), getter));
            fields.push(("set".to_string(), setter));
        }
        _ => {
            fields.push(("value".to_string(), value));
            fields.push(("writable".to_string(), Value::Bool(attrs.writable)));
        }
    }
    fields.push(("enumerable".to_string(), Value::Bool(attrs.enumerable)));
    fields.push(("configurable".to_string(), Value::Bool(attrs.configurable)));
    Value::descriptor_record(fields)
}

fn descriptor_for_array_value(
    array: &crate::value::ArrayCell,
    key: &str,
    value: Value,
    attrs: PropAttrs,
) -> Value {
    if array.meta.borrow().is_empty_accessor(key) {
        return Value::descriptor_record(vec![
            ("get".into(), Value::Undefined),
            ("set".into(), Value::Undefined),
            ("enumerable".into(), Value::Bool(attrs.enumerable)),
            ("configurable".into(), Value::Bool(attrs.configurable)),
        ]);
    }
    match accessor_kind_with_symbol(array.meta.borrow().symbol_key(key), key, &value) {
        Some("get") => Value::descriptor_record(vec![
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
        Some("set") => Value::descriptor_record(vec![
            ("get".to_string(), Value::Undefined),
            ("set".to_string(), value),
            ("enumerable".to_string(), Value::Bool(attrs.enumerable)),
            ("configurable".to_string(), Value::Bool(attrs.configurable)),
        ]),
        _ => Value::descriptor_record(vec![
            ("value".to_string(), value),
            ("writable".to_string(), Value::Bool(attrs.writable)),
            ("enumerable".to_string(), Value::Bool(attrs.enumerable)),
            ("configurable".to_string(), Value::Bool(attrs.configurable)),
        ]),
    }
}

// --- Integrity levels -------------------------------------------------------

/// Apply the specification's SetIntegrityLevel algorithm through the
/// interpreter's internal methods, so Proxy traps and abrupt completions stay
/// observable in own-key order.
fn set_integrity_level(
    interp: &mut Interpreter,
    target: &Value,
    freeze: bool,
) -> Result<bool, VmErr> {
    if !crate::interpreter::call::is_js_object(target) {
        return Ok(true);
    }
    if !interp.prevent_extensions(target)? {
        return Ok(false);
    }

    for key in interp.own_property_keys(target)? {
        interp.check_execution()?;
        let current = descriptor_for_key_in(interp, target, &key)?;
        if matches!(current, Value::Undefined) {
            continue;
        }
        let mut fields = vec![("configurable".into(), Value::Bool(false))];
        if freeze && current.get_prop("get").is_none() && current.get_prop("set").is_none() {
            fields.push(("writable".into(), Value::Bool(false)));
        }
        let descriptor = Value::descriptor_record(fields);
        if !interp.define_own_property(target, &key, &descriptor)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Test the integrity level using [[IsExtensible]], [[OwnPropertyKeys]], and
/// [[GetOwnProperty]] so proxies observe the same operations as the spec.
fn test_integrity_level(
    interp: &mut Interpreter,
    target: &Value,
    freeze: bool,
) -> Result<bool, VmErr> {
    if !crate::interpreter::call::is_js_object(target) {
        return Ok(true);
    }
    if interp.is_extensible(target)? {
        return Ok(false);
    }
    for key in interp.own_property_keys(target)? {
        interp.check_execution()?;
        let current = descriptor_for_key_in(interp, target, &key)?;
        if matches!(current, Value::Undefined) {
            continue;
        }
        if current
            .get_prop("configurable")
            .is_some_and(|configurable| configurable.is_truthy())
        {
            return Ok(false);
        }
        if freeze
            && current.get_prop("get").is_none()
            && current.get_prop("set").is_none()
            && current
                .get_prop("writable")
                .is_some_and(|writable| writable.is_truthy())
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn object_freeze(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    if !set_integrity_level(interp, &v, true)? {
        return Err(type_err("Cannot freeze object"));
    }
    Ok(v)
}
fn object_seal(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    if !set_integrity_level(interp, &v, false)? {
        return Err(type_err("Cannot seal object"));
    }
    Ok(v)
}
fn object_is_frozen(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Bool(test_integrity_level(interp, &v, true)?))
}
fn object_is_sealed(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let v = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Bool(test_integrity_level(interp, &v, false)?))
}
fn object_prevent_extensions(
    interp: &mut Interpreter,
    _: Value,
    a: Vec<Value>,
) -> Result<Value, VmErr> {
    let value = a.first().cloned().unwrap_or(Value::Undefined);
    if crate::interpreter::call::is_js_object(&value) && !interp.prevent_extensions(&value)? {
        return Err(type_err("Cannot prevent extensions"));
    }
    Ok(value)
}
fn object_is_extensible(interp: &mut Interpreter, _: Value, a: Vec<Value>) -> Result<Value, VmErr> {
    let value = a.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Bool(
        crate::interpreter::call::is_js_object(&value) && interp.is_extensible(&value)?,
    ))
}
