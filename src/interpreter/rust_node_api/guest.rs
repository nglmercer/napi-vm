//! Guest-side property, prototype, and callback helpers backing the N-API surface.

use std::collections::HashSet;
use std::ffi::c_void;
use std::rc::Rc;

use crate::error::VmErr;
use crate::host::{HostCallback, HostCallbackKind};
use crate::interpreter::{Env, Interpreter};
use crate::value::{PropAttrs, Value};

use super::api::napi_effective_prototype;
use super::state::{NapiEnvironment, NativeCallback, NativeCallbackRecord};
use super::{
    NAPI_GENERIC_FAILURE, NAPI_INVALID_ARG, NAPI_OBJECT_EXPECTED, NapiAsyncCompleteCallback,
    NapiCallback, NapiFinalize, NapiValue, call_guest_callback, exception_from_callback_error,
};

pub(super) fn napi_global_scope(environment: &NapiEnvironment) -> Result<Env, i32> {
    environment
        .owner
        .upgrade()
        .map(|owner| owner.borrow().global.clone())
        .ok_or(NAPI_INVALID_ARG)
}

pub(super) fn napi_global_get(environment: &NapiEnvironment, key: &str) -> Result<Value, i32> {
    Ok(napi_global_scope(environment)?
        .borrow()
        .get(key)
        .unwrap_or(Value::Undefined))
}

pub(super) fn napi_global_has(environment: &NapiEnvironment, key: &str) -> Result<bool, i32> {
    Ok(napi_global_scope(environment)?.borrow().get(key).is_some())
}

pub(super) fn napi_global_has_own(environment: &NapiEnvironment, key: &str) -> Result<bool, i32> {
    Ok(napi_global_scope(environment)?
        .borrow()
        .all_keys()
        .iter()
        .any(|name| name == key))
}

pub(super) fn napi_global_set(
    environment: &NapiEnvironment,
    key: &str,
    value: Value,
) -> Result<(), i32> {
    napi_global_scope(environment)?
        .borrow_mut()
        .try_set(key, value)
        .map_err(|_| NAPI_GENERIC_FAILURE)
}

pub(super) fn napi_global_delete(environment: &NapiEnvironment, key: &str) -> Result<bool, i32> {
    let global = napi_global_scope(environment)?;
    if global.borrow().has(key) {
        return Ok(global.borrow_mut().remove(key));
    }
    Ok(true)
}

pub(super) fn run_napi_guest_operation(
    environment: &NapiEnvironment,
    name: &'static str,
    operation: fn(&mut Interpreter, Value, Vec<Value>) -> Result<Value, VmErr>,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, i32> {
    call_guest_callback(
        environment,
        HostCallback {
            callback: Value::NativeFunction {
                name: Rc::from(name),
                callable: operation,
            },
            this_value: receiver,
            args,
            kind: HostCallbackKind::Call,
        },
    )
}

pub(super) fn is_napi_property_object(value: &Value) -> bool {
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

pub(super) fn callback_arguments(
    environment: &NapiEnvironment,
    argc: usize,
    argv: *const NapiValue,
) -> Result<Vec<Value>, i32> {
    if argc > 0 && argv.is_null() {
        return Err(NAPI_INVALID_ARG);
    }
    if argc == 0 {
        return Ok(Vec::new());
    }
    let handles = unsafe { std::slice::from_raw_parts(argv, argc) };
    let arena = environment.handles.borrow();
    handles.iter().map(|handle| arena.get(*handle)).collect()
}

pub(super) fn is_napi_function(value: &Value) -> bool {
    match value {
        Value::Function(_)
        | Value::NativeFunction { .. }
        | Value::HostFunction { .. }
        | Value::Class(_) => true,
        Value::Proxy(proxy) => is_napi_function(&proxy.target),
        Value::Object { .. } => value.get_prop("__symbol_call__").is_some_and(|target| {
            matches!(
                target,
                Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
            )
        }),
        _ => false,
    }
}

pub(super) fn create_native_callback_value(
    environment: &Rc<NapiEnvironment>,
    function_name: &str,
    callback: NapiCallback,
    data: *mut c_void,
) -> Result<Value, i32> {
    create_native_callback_value_with_kind(
        environment,
        function_name,
        NativeCallback::Function(callback),
        data,
        false,
    )
}

pub(super) fn create_native_async_complete_value(
    environment: &Rc<NapiEnvironment>,
    callback: NapiAsyncCompleteCallback,
    status: i32,
    data: *mut c_void,
    work_id: usize,
) -> Result<Value, i32> {
    create_native_callback_value_with_kind(
        environment,
        "napi_async_complete",
        NativeCallback::AsyncComplete {
            callback,
            status,
            work_id,
        },
        data,
        true,
    )
}

pub(super) fn create_posted_finalizer_value(
    environment: &Rc<NapiEnvironment>,
    finalize: NapiFinalize,
    data: *mut c_void,
    hint: *mut c_void,
) -> Result<Value, i32> {
    create_native_callback_value_with_kind(
        environment,
        "node_api_post_finalizer",
        NativeCallback::PostedFinalizer {
            finalize,
            data,
            hint,
        },
        std::ptr::null_mut(),
        true,
    )
}

pub(super) fn create_native_callback_value_with_kind(
    environment: &Rc<NapiEnvironment>,
    function_name: &str,
    callback: NativeCallback,
    data: *mut c_void,
    one_shot: bool,
) -> Result<Value, i32> {
    let owner = environment.owner.upgrade().ok_or(NAPI_GENERIC_FAILURE)?;
    let id = {
        let mut state = owner.borrow_mut();
        let id = state.next_callback_id;
        state.next_callback_id = id.checked_add(1).ok_or(NAPI_GENERIC_FAILURE)?;
        state.callbacks.insert(
            id,
            NativeCallbackRecord {
                env: environment.clone(),
                callback,
                data,
                one_shot,
            },
        );
        id
    };
    Ok(Value::napi_callback_function(Rc::from(function_name), id))
}

pub(super) fn napi_property_key(value: &Value) -> Result<String, i32> {
    match value {
        Value::String(key) => Ok(key.clone()),
        Value::Symbol(symbol) => Ok(crate::interpreter::symbol_slot_key(symbol)),
        _ => Err(NAPI_INVALID_ARG),
    }
}

pub(super) fn napi_direct_get_property(object: &Value, key: &Value) -> Result<Value, i32> {
    let key = napi_property_key(key)?;
    Ok(object.get_prop(&key).unwrap_or(Value::Undefined))
}

pub(super) fn napi_direct_set_property(
    object: &Value,
    key: &Value,
    value: Value,
) -> Result<(), i32> {
    let symbol = match key {
        Value::Symbol(symbol) => Some(symbol.clone()),
        _ => None,
    };
    let key = napi_property_key(key)?;
    match object {
        Value::Object { props } => {
            object
                .set_prop(key.clone(), value)
                .map_err(|_| NAPI_GENERIC_FAILURE)?;
            if let Some(symbol) = symbol {
                props.meta.borrow_mut().set_symbol_key(&key, symbol);
            }
            Ok(())
        }
        Value::Class(class) => {
            if class.statics.meta.borrow().has_accessors {
                let is_setter = |value: &Value| match value {
                    Value::Function(function) => function
                        .name
                        .as_ref()
                        .is_some_and(|name| name.starts_with("set ")),
                    Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                        name.starts_with("set ")
                    }
                    _ => false,
                };
                if class
                    .statics
                    .borrow()
                    .iter()
                    .any(|(name, value)| name == &key && is_setter(value))
                {
                    return Err(NAPI_GENERIC_FAILURE);
                }
            }
            object
                .set_prop(key.clone(), value)
                .map_err(|_| NAPI_GENERIC_FAILURE)?;
            if let Some(symbol) = symbol {
                class.statics.meta.borrow_mut().set_symbol_key(&key, symbol);
            }
            Ok(())
        }
        Value::Function(function) => {
            function.ensure_name_length_properties();
            function.prototype_value(object);
            object
                .set_prop(key.clone(), value)
                .map_err(|_| NAPI_GENERIC_FAILURE)?;
            if let Some(symbol) = symbol {
                function
                    .properties
                    .meta
                    .borrow_mut()
                    .set_symbol_key(&key, symbol);
            }
            Ok(())
        }
        Value::HostFunction { properties, .. } => {
            object
                .set_prop(key.clone(), value)
                .map_err(|_| NAPI_GENERIC_FAILURE)?;
            if let Some(symbol) = symbol {
                properties.meta.borrow_mut().set_symbol_key(&key, symbol);
            }
            Ok(())
        }
        Value::Array(array) => {
            napi_array_set_property(array, &key, value)?;
            if let Some(symbol) = symbol {
                array.set_symbol_key(&key, symbol);
            }
            Ok(())
        }
        _ => Err(NAPI_OBJECT_EXPECTED),
    }
}

pub(super) fn napi_array_set_property(
    array: &crate::value::ArrayCell,
    key: &str,
    value: Value,
) -> Result<(), i32> {
    if key == "length" {
        let Value::Number(length) = value else {
            return Err(NAPI_INVALID_ARG);
        };
        if !length.is_finite()
            || length < 0.0
            || length.fract() != 0.0
            || length > crate::value::MAX_ARRAY_LEN as f64
        {
            return Err(NAPI_INVALID_ARG);
        }
        if array.meta.borrow().attrs_of("length").writable {
            array.set_length(length as usize);
        }
        return Ok(());
    }
    if let Some(index) = crate::value::array_index(key) {
        if index >= crate::value::MAX_ARRAY_LEN {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let old_length = array.borrow().len();
        let exists = index < old_length && array.has_index(index);
        if (exists && !array.meta.borrow().attrs_of(key).writable)
            || (!exists && array.meta.borrow().non_extensible)
            || (index >= old_length && !array.meta.borrow().attrs_of("length").writable)
        {
            return Ok(());
        }
        if index >= old_length {
            let new_length = index + 1;
            array.borrow_mut().resize(new_length, Value::Undefined);
            array.resize_presence(old_length, new_length, false);
        }
        array.borrow_mut()[index] = value;
        array.set_index_presence(index, true);
        return Ok(());
    }
    let exists = array.named_prop(key).is_some();
    if (exists && !array.meta.borrow().attrs_of(key).writable)
        || (!exists && array.meta.borrow().non_extensible)
    {
        return Ok(());
    }
    array.set_named(key.to_owned(), value);
    Ok(())
}

pub(super) fn napi_direct_has_own_property(object: &Value, key: &Value) -> Result<bool, i32> {
    let key = napi_property_key(key)?;
    if let Value::Function(function) = object {
        function.ensure_name_length_properties();
        function.prototype_value(object);
    }
    Ok(match object {
        Value::Object { props } => props.borrow().iter().any(|(name, _)| name == &key),
        Value::Function(function) => function
            .properties
            .borrow()
            .iter()
            .any(|(name, _)| name == &key),
        Value::HostFunction { properties, .. } => {
            properties.borrow().iter().any(|(name, _)| name == &key)
        }
        Value::Class(class) => class.statics.borrow().iter().any(|(name, _)| name == &key),
        Value::Array(array) => {
            key == "length"
                || crate::value::array_index(&key).is_some_and(|index| array.has_index(index))
                || array.named_prop(&key).is_some()
        }
        Value::Error(error) => {
            matches!(key.as_str(), "name" | "message" | "stack")
                || (key == "code" && error.code.is_some())
        }
        Value::String(string) => {
            key == "length"
                || key
                    .parse::<usize>()
                    .is_ok_and(|index| index < string.chars().count())
        }
        _ => false,
    })
}

pub(super) fn napi_direct_delete_property(object: &Value, key: &Value) -> Result<bool, i32> {
    let key = napi_property_key(key)?;
    if let Value::Function(function) = object {
        function.ensure_name_length_properties();
        function.prototype_value(object);
    }
    match object {
        Value::Object { props } => {
            if !props.meta.borrow().attrs_of(&key).configurable
                && props.borrow().iter().any(|(name, _)| name == &key)
            {
                return Ok(false);
            }
            let mut slots = props.borrow_mut();
            if let Some(index) = slots.iter().position(|(name, _)| name == &key) {
                slots.remove(index);
                drop(slots);
                props.meta.borrow_mut().forget(&key);
                props.note_mutated();
            }
            Ok(true)
        }
        Value::Class(class) => {
            if !class.statics.meta.borrow().attrs_of(&key).configurable
                && class.statics.borrow().iter().any(|(name, _)| name == &key)
            {
                return Ok(false);
            }
            let companion = format!("__setter:{}__", key);
            let mut slots = class.statics.borrow_mut();
            slots.retain(|(name, _)| name != &key && name != &companion);
            drop(slots);
            class.statics.meta.borrow_mut().forget(&key);
            class.statics.meta.borrow_mut().forget(&companion);
            class.statics.note_mutated();
            Ok(true)
        }
        Value::Function(function) => {
            if !function
                .properties
                .meta
                .borrow()
                .attrs_of(&key)
                .configurable
                && function
                    .properties
                    .borrow()
                    .iter()
                    .any(|(name, _)| name == &key)
            {
                return Ok(false);
            }
            let companion = format!("__setter:{}__", key);
            function
                .properties
                .borrow_mut()
                .retain(|(name, _)| name != &key && name != &companion);
            function.properties.meta.borrow_mut().forget(&key);
            function.properties.meta.borrow_mut().forget(&companion);
            function.properties.note_mutated();
            Ok(true)
        }
        Value::HostFunction { properties, .. } => {
            if !properties.meta.borrow().attrs_of(&key).configurable
                && properties.borrow().iter().any(|(name, _)| name == &key)
            {
                return Ok(false);
            }
            let companion = format!("__setter:{}__", key);
            properties
                .borrow_mut()
                .retain(|(name, _)| name != &key && name != &companion);
            properties.meta.borrow_mut().forget(&key);
            properties.meta.borrow_mut().forget(&companion);
            properties.note_mutated();
            Ok(true)
        }
        Value::Array(array) => {
            if key == "length" {
                return Ok(false);
            }
            if let Some(index) = crate::value::array_index(&key) {
                if index < array.borrow().len() && array.has_index(index) {
                    if !array.meta.borrow().attrs_of(&key).configurable {
                        return Ok(false);
                    }
                    array.borrow_mut()[index] = Value::Undefined;
                    array.set_index_presence(index, false);
                }
            } else {
                if array.named_prop(&key).is_some()
                    && !array.meta.borrow().attrs_of(&key).configurable
                {
                    return Ok(false);
                }
                array.named.borrow_mut().retain(|(name, _)| name != &key);
                array.forget_symbol_key(&key);
            }
            Ok(true)
        }
        Value::Proxy(proxy) => napi_direct_delete_property(&proxy.target, &Value::String(key)),
        _ => Ok(true),
    }
}

pub(super) fn napi_direct_own_property_names(object: &Value) -> Vec<String> {
    if let Value::Function(function) = object {
        function.ensure_name_length_properties();
        function.prototype_value(object);
    }
    match object {
        Value::Object { props } => props.borrow().iter().map(|(key, _)| key.clone()).collect(),
        Value::Function(function) => function
            .properties
            .borrow()
            .iter()
            .map(|(key, _)| key.clone())
            .collect(),
        Value::HostFunction { properties, .. } => properties
            .borrow()
            .iter()
            .map(|(key, _)| key.clone())
            .collect(),
        Value::Class(class) => class
            .statics
            .borrow()
            .iter()
            .map(|(key, _)| key.clone())
            .collect(),
        Value::Array(array) => {
            let mut names = vec!["length".to_owned()];
            names.extend(
                (0..array.borrow().len())
                    .filter(|index| array.has_index(*index))
                    .map(|index| index.to_string()),
            );
            names.extend(array.named.borrow().iter().map(|(key, _)| key.clone()));
            names
        }
        Value::Proxy(proxy) => napi_direct_own_property_names(&proxy.target),
        Value::Error(error) => {
            let mut names = vec!["name".to_owned(), "message".to_owned(), "stack".to_owned()];
            if error.code.is_some() {
                names.push("code".to_owned());
            }
            names
        }
        _ => Vec::new(),
    }
}

pub(super) fn napi_direct_property_is_enumerable(object: &Value, key: &str) -> bool {
    match object {
        Value::Object { props } => {
            props.borrow().iter().any(|(name, _)| name == key)
                && props.meta.borrow().attrs_of(key).enumerable
        }
        Value::Class(class) => {
            class.statics.borrow().iter().any(|(name, _)| name == key)
                && class.statics.meta.borrow().attrs_of(key).enumerable
        }
        Value::Function(function) => {
            function
                .properties
                .borrow()
                .iter()
                .any(|(name, _)| name == key)
                && function.properties.meta.borrow().attrs_of(key).enumerable
        }
        Value::HostFunction { properties, .. } => {
            properties.borrow().iter().any(|(name, _)| name == key)
                && properties.meta.borrow().attrs_of(key).enumerable
        }
        Value::Array(array) => {
            key != "length"
                && (crate::value::array_index(key).is_some_and(|index| array.has_index(index))
                    || array.named_prop(key).is_some()
                        && array.meta.borrow().attrs_of(key).enumerable)
        }
        Value::Proxy(proxy) => napi_direct_property_is_enumerable(&proxy.target, key),
        Value::GlobalObject => true,
        Value::Error(error) => key == "code" && error.code.is_some(),
        _ => false,
    }
}

#[derive(Clone)]
pub(super) enum NapiPropertyKey {
    String(String),
    Symbol(Rc<crate::value::SymbolData>),
}

impl NapiPropertyKey {
    pub(super) fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::String(left), Self::String(right)) => left == right,
            (Self::Symbol(left), Self::Symbol(right)) => left.id == right.id,
            _ => false,
        }
    }
}

pub(super) fn napi_property_key_from_guest(value: &Value) -> Result<NapiPropertyKey, VmErr> {
    match value {
        Value::String(key) => Ok(NapiPropertyKey::String(key.clone())),
        Value::Symbol(symbol) => Ok(NapiPropertyKey::Symbol(symbol.clone())),
        _ => Err(VmErr::Msg(
            "TypeError: Proxy ownKeys trap returned a non-key".into(),
        )),
    }
}

pub(super) fn napi_guest_own_property_keys(
    interpreter: &mut Interpreter,
    object: &Value,
    depth: usize,
) -> Result<Vec<(NapiPropertyKey, PropAttrs)>, VmErr> {
    if depth >= crate::value::MAX_PROTOTYPE_DEPTH {
        return Err(crate::value::limit_err("Maximum prototype depth exceeded"));
    }
    if matches!(object, Value::GlobalObject) {
        let mut keys = interpreter
            .global_keys()
            .into_iter()
            .filter(|key| !crate::interpreter::is_internal_key(key))
            .map(|key| (NapiPropertyKey::String(key), PropAttrs::default()))
            .collect::<Vec<_>>();
        napi_sort_property_keys(&mut keys);
        return Ok(keys);
    }
    let Value::Proxy(proxy) = object else {
        return napi_direct_all_property_keys(object).map_err(|_| {
            VmErr::Msg("Node-API property key collection is unsupported for this value".into())
        });
    };

    let target = proxy.target.clone();
    let Some(trap) = interpreter.proxy_trap(proxy, "ownKeys") else {
        return napi_guest_own_property_keys(interpreter, &target, depth + 1);
    };
    let result = interpreter.call_this(&trap, proxy.handler.clone(), vec![target.clone()])?;
    let Value::Array(trap_keys) = &result else {
        return Err(VmErr::Msg(
            "TypeError: Proxy ownKeys trap must return an array".into(),
        ));
    };

    let length = trap_keys.borrow().len();
    if length > crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err(
            "Maximum proxy property key count exceeded",
        ));
    }
    let mut keys = Vec::with_capacity(length);
    for index in 0..length {
        let value = interpreter.get_prop_value(&result, &Value::Number(index as f64))?;
        let key = napi_property_key_from_guest(&value)?;
        if keys
            .iter()
            .any(|(existing, _): &(NapiPropertyKey, PropAttrs)| existing.matches(&key))
        {
            return Err(VmErr::Msg(
                "TypeError: Proxy ownKeys trap returned duplicate keys".into(),
            ));
        }
        keys.push((key, PropAttrs::default()));
    }

    let target_keys = napi_guest_own_property_keys(interpreter, &target, depth + 1)?;
    for (key, attributes) in &mut keys {
        let enumerable = target_keys
            .iter()
            .find(|(target_key, _)| target_key.matches(key))
            .is_some_and(|(_, target_attributes)| target_attributes.enumerable);
        // Node's napi_get_all_property_names preserves Proxy ownKeys results
        // for writable/configurable filters, while enumerable still consults
        // the target descriptor. Bun applies all three filters to descriptors.
        // The Rust host follows Node's behavior; the differential fixture
        // records Bun's distinct result.
        *attributes = PropAttrs {
            writable: true,
            enumerable,
            configurable: true,
        };
    }

    if target_keys.iter().any(|(key, attrs)| {
        !attrs.configurable && !keys.iter().any(|(found, _)| found.matches(key))
    }) {
        return Err(VmErr::Msg(
            "TypeError: Proxy ownKeys trap omitted a non-configurable key".into(),
        ));
    }
    if !napi_guest_object_is_extensible(&target)
        && (keys.len() != target_keys.len()
            || target_keys
                .iter()
                .any(|(key, _)| !keys.iter().any(|(found, _)| found.matches(key))))
    {
        return Err(VmErr::Msg(
            "TypeError: Proxy ownKeys trap returned keys for a non-extensible target".into(),
        ));
    }
    Ok(keys)
}

pub(super) fn napi_guest_object_is_extensible(object: &Value) -> bool {
    match object {
        Value::Object { props } => !props.meta.borrow().non_extensible,
        Value::Array(array) => !array.meta.borrow().non_extensible,
        Value::Function(function) => !function.properties.meta.borrow().non_extensible,
        Value::HostFunction { properties, .. } => !properties.meta.borrow().non_extensible,
        Value::Class(class) => !class.statics.meta.borrow().non_extensible,
        Value::Proxy(proxy) => napi_guest_object_is_extensible(&proxy.target),
        _ => true,
    }
}

pub(super) fn napi_guest_property_key_value(key: NapiPropertyKey, key_conversion: i32) -> Value {
    match key {
        NapiPropertyKey::String(key) if key_conversion == 0 => crate::value::array_index(&key)
            .map_or_else(|| Value::String(key), |index| Value::Number(index as f64)),
        NapiPropertyKey::String(key) => Value::String(key),
        NapiPropertyKey::Symbol(symbol) => Value::Symbol(symbol),
    }
}

pub(super) fn napi_guest_get_all_property_names(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let number_arg = |index: usize| match args.get(index) {
        Some(Value::Number(value)) => *value as i32,
        _ => 0,
    };
    let key_mode = number_arg(0);
    let key_filter = number_arg(1);
    let key_conversion = number_arg(2);

    let mut current = receiver;
    let mut seen = Vec::<NapiPropertyKey>::new();
    let mut names = Vec::new();
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        for (key, attributes) in napi_guest_own_property_keys(interpreter, &current, 0)? {
            if seen.iter().any(|existing| existing.matches(&key)) {
                continue;
            }
            seen.push(key.clone());
            if seen.len() > crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err(
                    "Maximum property name count exceeded",
                ));
            }
            let filtered = (key_filter & 1 != 0 && !attributes.writable)
                || (key_filter & 2 != 0 && !attributes.enumerable)
                || (key_filter & 4 != 0 && !attributes.configurable)
                || (key_filter & 8 != 0 && matches!(key, NapiPropertyKey::String(_)))
                || (key_filter & 16 != 0 && matches!(key, NapiPropertyKey::Symbol(_)));
            if !filtered {
                names.push(napi_guest_property_key_value(key, key_conversion));
            }
        }
        if key_mode == 1 {
            return Value::checked_array(names);
        }
        let prototype = match &current {
            Value::Proxy(proxy) => interpreter.prototype_of(&proxy.target),
            _ => interpreter.prototype_of(&current),
        };
        let Some(prototype) = prototype else {
            return Value::checked_array(names);
        };
        current = (*prototype).clone();
    }
    Err(crate::value::limit_err("Maximum prototype depth exceeded"))
}

pub(super) fn napi_push_direct_property_key(
    keys: &mut Vec<(NapiPropertyKey, PropAttrs)>,
    key: &str,
    symbol: Option<Rc<crate::value::SymbolData>>,
    attributes: PropAttrs,
) {
    if crate::interpreter::is_internal_key(key) && symbol.is_none() {
        return;
    }
    let key = symbol.map_or_else(
        || NapiPropertyKey::String(key.to_owned()),
        NapiPropertyKey::Symbol,
    );
    if !keys.iter().any(|(existing, _)| existing.matches(&key)) {
        keys.push((key, attributes));
    }
}

pub(super) fn napi_sort_property_keys(keys: &mut [(NapiPropertyKey, PropAttrs)]) {
    keys.sort_by(|(left, _), (right, _)| match (left, right) {
        (NapiPropertyKey::String(left), NapiPropertyKey::String(right)) => {
            match (
                crate::value::array_index(left),
                crate::value::array_index(right),
            ) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        }
        (NapiPropertyKey::String(_), NapiPropertyKey::Symbol(_)) => std::cmp::Ordering::Less,
        (NapiPropertyKey::Symbol(_), NapiPropertyKey::String(_)) => std::cmp::Ordering::Greater,
        (NapiPropertyKey::Symbol(_), NapiPropertyKey::Symbol(_)) => std::cmp::Ordering::Equal,
    });
}

pub(super) fn napi_direct_all_property_keys(
    object: &Value,
) -> Result<Vec<(NapiPropertyKey, PropAttrs)>, i32> {
    let mut keys = Vec::new();
    match object {
        Value::Object { props } => {
            let slots = props.borrow();
            let metadata = props.meta.borrow();
            for (key, _) in slots.iter() {
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    metadata.symbol_key(key),
                    metadata.attrs_of(key),
                );
            }
        }
        Value::Function(function) => {
            function.ensure_name_length_properties();
            function.prototype_value(object);
            let slots = function.properties.borrow();
            let metadata = function.properties.meta.borrow();
            for (key, _) in slots.iter() {
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    metadata.symbol_key(key),
                    metadata.attrs_of(key),
                );
            }
        }
        Value::HostFunction { properties, .. } => {
            let slots = properties.borrow();
            let metadata = properties.meta.borrow();
            for (key, _) in slots.iter() {
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    metadata.symbol_key(key),
                    metadata.attrs_of(key),
                );
            }
        }
        Value::Class(class) => {
            let slots = class.statics.borrow();
            let metadata = class.statics.meta.borrow();
            // Node's native class constructor creates these own properties
            // in the order length, name, prototype before addon statics.
            for key in ["length", "name", "prototype"] {
                if slots.iter().any(|(name, _)| name == key) {
                    napi_push_direct_property_key(
                        &mut keys,
                        key,
                        metadata.symbol_key(key),
                        metadata.attrs_of(key),
                    );
                }
            }
            for (key, _) in slots.iter() {
                if matches!(key.as_str(), "length" | "name" | "prototype") {
                    continue;
                }
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    metadata.symbol_key(key),
                    metadata.attrs_of(key),
                );
            }
        }
        Value::Array(array) => {
            let length = array.borrow().len();
            for index in 0..length {
                if array.has_index(index) {
                    let index_key = index.to_string();
                    napi_push_direct_property_key(
                        &mut keys,
                        &index_key,
                        None,
                        array.meta.borrow().attrs_of(&index_key),
                    );
                }
            }
            napi_push_direct_property_key(
                &mut keys,
                "length",
                None,
                array.meta.borrow().attrs_of("length"),
            );
            for (key, _) in array.named.borrow().iter() {
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    array.symbol_key(key),
                    array.meta.borrow().attrs_of(key),
                );
            }
        }
        Value::Error(error) => {
            for key in ["name", "message", "stack"] {
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    None,
                    PropAttrs {
                        writable: true,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
            if error.code.is_some() {
                napi_push_direct_property_key(&mut keys, "code", None, PropAttrs::default());
            }
        }
        Value::RegExp(_) => napi_push_direct_property_key(
            &mut keys,
            "lastIndex",
            None,
            PropAttrs {
                writable: true,
                enumerable: false,
                configurable: false,
            },
        ),
        Value::TypedArray(view) => {
            for index in 0..view.effective_length() {
                napi_push_direct_property_key(
                    &mut keys,
                    &index.to_string(),
                    None,
                    PropAttrs::default(),
                );
            }
        }
        Value::Date(_)
        | Value::Promise(_)
        | Value::ArrayBuffer(_)
        | Value::SharedArrayBuffer(_)
        | Value::DataView(_)
        | Value::StringIterator { .. }
        | Value::Generator { .. } => {}
        Value::Proxy(_) | Value::NativeFunction { .. } | Value::GlobalObject => {
            return Err(NAPI_GENERIC_FAILURE);
        }
        Value::Undefined
        | Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::HostPending { .. }
        | Value::Symbol(_)
        | Value::BigInt(_)
        | Value::Binding(_) => return Err(NAPI_OBJECT_EXPECTED),
        #[cfg(stackful_coroutines)]
        Value::AsyncTask(_) => return Err(NAPI_OBJECT_EXPECTED),
    }
    napi_sort_property_keys(&mut keys);
    Ok(keys)
}

pub(super) fn napi_direct_prototype(
    environment: &NapiEnvironment,
    object: &Value,
) -> Result<Option<Rc<Value>>, i32> {
    let direct = match object {
        Value::Proxy(proxy) => proxy.target.proto_of(),
        _ => object.proto_of(),
    };
    if direct.is_some() || matches!(object, Value::Proxy(_)) {
        return Ok(direct);
    }
    if !matches!(
        object,
        Value::Object { .. }
            | Value::Array(_)
            | Value::Function(_)
            | Value::Class(_)
            | Value::GlobalObject
            | Value::NativeFunction { .. }
            | Value::HostFunction { .. }
            | Value::Promise(_)
            | Value::Date(_)
            | Value::ArrayBuffer(_)
            | Value::SharedArrayBuffer(_)
            | Value::TypedArray(_)
            | Value::DataView(_)
    ) {
        return Ok(None);
    }
    let prototype = napi_effective_prototype(environment, object)?;
    Ok((!matches!(prototype, Value::Null)).then(|| Rc::new(prototype)))
}

pub(super) fn napi_direct_property_names(
    environment: &NapiEnvironment,
    object: &Value,
) -> Result<Value, i32> {
    let mut current = object.clone();
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        for key in napi_direct_own_property_names(&current) {
            if crate::interpreter::is_internal_key(&key) || !seen.insert(key.clone()) {
                continue;
            }
            if seen.len() > crate::value::MAX_ARRAY_LEN {
                return Err(NAPI_GENERIC_FAILURE);
            }
            if napi_direct_property_is_enumerable(&current, &key) {
                names.push(Value::String(key));
            }
        }
        let Some(prototype) = napi_direct_prototype(environment, &current)? else {
            return Value::checked_array(names).map_err(|_| NAPI_GENERIC_FAILURE);
        };
        current = (*prototype).clone();
    }
    Err(NAPI_GENERIC_FAILURE)
}

pub(super) fn napi_guest_get_property(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = args.first().cloned().unwrap_or(Value::Undefined);
    interpreter.get_prop_value(&receiver, &key)
}

pub(super) fn napi_guest_resolve_deferred(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let promise = receiver
        .as_promise()
        .ok_or_else(|| VmErr::Msg("Node-API deferred does not reference a promise".into()))?;
    let resolution = args.into_iter().next().unwrap_or(Value::Undefined);
    if let Err(error) = interpreter.resolve_promise(&promise, resolution) {
        // Promise resolution converts errors while reading/calling a thenable
        // into rejection. They must not escape as a synchronous N-API throw.
        interpreter.reject_promise(&promise, exception_from_callback_error(error));
    }
    Ok(Value::Undefined)
}

pub(super) fn napi_guest_reject_deferred(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let promise = receiver
        .as_promise()
        .ok_or_else(|| VmErr::Msg("Node-API deferred does not reference a promise".into()))?;
    let rejection = args.into_iter().next().unwrap_or(Value::Undefined);
    interpreter.reject_promise(&promise, rejection);
    Ok(Value::Undefined)
}

pub(super) fn napi_guest_set_property(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = args.first().cloned().unwrap_or(Value::Undefined);
    let value = args.get(1).cloned().unwrap_or(Value::Undefined);
    interpreter.assign_member(&receiver, &key, value.clone())?;
    Ok(value)
}

pub(super) fn napi_guest_has_property(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = args.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Bool(interpreter.has_property(&receiver, &key)?))
}

pub(super) fn napi_guest_has_own_property(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = args.first().cloned().unwrap_or(Value::Undefined);
    let constructor = interpreter
        .global
        .borrow()
        .get("Object")
        .ok_or_else(|| VmErr::Msg("Object constructor is unavailable".into()))?;
    let method = interpreter.member(&constructor, "hasOwn")?;
    interpreter.call_this(&method, constructor, vec![receiver, key])
}

pub(super) fn napi_guest_delete_property(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = args.first().cloned().unwrap_or(Value::Undefined);
    interpreter.delete_member(&receiver, &key)
}

pub(super) fn napi_guest_get_property_names(
    interpreter: &mut Interpreter,
    receiver: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    let mut current = receiver;
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        let trapped_keys = if matches!(current, Value::GlobalObject) {
            Some(interpreter.global_keys())
        } else if matches!(current, Value::Proxy(_)) {
            Some(interpreter.keys_with_proxy_trap(&current)?)
        } else {
            None
        };
        let own_keys = trapped_keys
            .as_ref()
            .cloned()
            .unwrap_or_else(|| napi_direct_own_property_names(&current));
        for key in &own_keys {
            if crate::interpreter::is_internal_key(key) || !seen.insert(key.clone()) {
                continue;
            }
            if seen.len() > crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err(
                    "Maximum property name count exceeded",
                ));
            }
            if napi_direct_property_is_enumerable(&current, key) {
                names.push(Value::String(key.clone()));
            }
        }
        // Non-enumerable own keys still shadow enumerable properties farther
        // up the prototype chain.
        seen.extend(napi_direct_own_property_names(&current));
        let prototype = match &current {
            Value::Proxy(proxy) => interpreter.prototype_of(&proxy.target),
            _ => interpreter.prototype_of(&current),
        };
        let Some(prototype) = prototype else {
            return Value::checked_array(names);
        };
        current = (*prototype).clone();
    }
    Err(crate::value::limit_err("Maximum prototype depth exceeded"))
}
