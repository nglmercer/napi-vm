pub(super) unsafe extern "C" fn api_create_array(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::array(Vec::new()))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_array_with_length(
    env: NapiEnv,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        if length > crate::value::MAX_ARRAY_LEN {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let environment = environment(env)?;
        let array = Value::array_with_presence(vec![Value::Undefined; length], vec![false; length]);
        let handle = environment.handles.borrow_mut().create(array)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_is_array(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        unsafe { result.write(matches!(value, Value::Array(_))) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_array_length(env: NapiEnv, value: NapiValue, result: *mut u32) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Array(array) = &value else {
            return Err(NAPI_ARRAY_EXPECTED);
        };
        let length = array.borrow().len();
        unsafe { result.write(length as u32) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_prototype(
    env: NapiEnv,
    object: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        let prototype = if matches!(object, Value::Proxy(_)) {
            // Node's current Node-API implementation reports null for Proxy
            // values without invoking `getPrototypeOf` traps. Keep this
            // native API behavior separate from guest Object.getPrototypeOf,
            // which uses the full Proxy internal method.
            Value::Null
        } else {
            napi_effective_prototype(&environment, &object)?
        };
        let handle = environment.handles.borrow_mut().create(prototype)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) fn napi_effective_prototype(environment: &NapiEnvironment, object: &Value) -> Result<Value, i32> {
    let (prototype, uses_default_prototype, default_constructor) = match object {
        Value::Object { props } => {
            let meta = props.meta.borrow();
            let default_constructor = match meta.boxed_primitive.as_ref() {
                Some(BoxedPrimitive::Bool(_)) => "Boolean",
                Some(BoxedPrimitive::Number(_)) => "Number",
                Some(BoxedPrimitive::String(_)) => "String",
                Some(BoxedPrimitive::Symbol(_)) => "Symbol",
                Some(BoxedPrimitive::BigInt(_)) => "BigInt",
                None => "Object",
            };
            (
                meta.proto.clone(),
                meta.uses_default_prototype,
                default_constructor,
            )
        }
        Value::Function(function) => {
            let meta = function.properties.meta.borrow();
            (meta.proto.clone(), meta.uses_default_prototype, "Function")
        }
        Value::HostFunction { properties, .. } => {
            let meta = properties.meta.borrow();
            (meta.proto.clone(), meta.uses_default_prototype, "Function")
        }
        Value::Class(class) => {
            let meta = class.statics.meta.borrow();
            (meta.proto.clone(), meta.uses_default_prototype, "Function")
        }
        Value::Array(array) => {
            let meta = array.meta.borrow();
            (meta.proto.clone(), meta.uses_default_prototype, "Array")
        }
        Value::Promise(_) => return napi_default_builtin_prototype(environment, "Promise"),
        Value::Date(_) => return napi_default_builtin_prototype(environment, "Date"),
        Value::RegExp(_) => return napi_default_builtin_prototype(environment, "RegExp"),
        Value::ArrayBuffer(_) => {
            return napi_default_builtin_prototype(environment, "ArrayBuffer");
        }
        Value::SharedArrayBuffer(_) => {
            return napi_default_builtin_prototype(environment, "SharedArrayBuffer");
        }
        Value::TypedArray(view) => {
            let constructor = if view.is_buffer {
                "Buffer"
            } else {
                view.kind.name()
            };
            return napi_default_builtin_prototype(environment, constructor);
        }
        Value::DataView(_) => return napi_default_builtin_prototype(environment, "DataView"),
        Value::GlobalObject => return napi_default_object_prototype(environment),
        Value::NativeFunction { .. } => {
            return napi_default_function_prototype(environment);
        }
        value if !is_napi_property_object(value) => return Err(NAPI_OBJECT_EXPECTED),
        _ => return Err(NAPI_GENERIC_FAILURE),
    };
    if let Some(prototype) = prototype {
        return Ok(prototype.as_ref().clone());
    }
    if !uses_default_prototype {
        return Ok(Value::Null);
    }
    let default_prototype = napi_default_builtin_prototype(environment, default_constructor)?;
    if crate::interpreter::strict_equals(object, &default_prototype) {
        if default_constructor == "Object" {
            return Ok(Value::Null);
        }
        if default_constructor == "Array" {
            return napi_default_object_prototype(environment);
        }
    }
    Ok(default_prototype)
}

/// The experimental Node-API prototype setter supports ordinary objects,
/// arrays, functions, and classes whose prototype links are represented by
/// the VM's shared object metadata. Proxies and specialized built-ins fail
/// explicitly instead of reporting a successful no-op.
pub(super) unsafe extern "C" fn api_set_prototype(
    env: NapiEnv,
    object: NapiValue,
    prototype: NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let handles = environment.handles.borrow();
        let object = handles.get(object)?;
        let prototype = handles.get(prototype)?;

        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let prototype = match &prototype {
            Value::Null => None,
            Value::Object { .. }
            | Value::Array(_)
            | Value::Function(_)
            | Value::HostFunction { .. }
            | Value::Class(_) => Some(Rc::new(prototype)),
            other if !is_napi_property_object(other) => return Err(NAPI_OBJECT_EXPECTED),
            // Proxies and specialized built-ins do not expose a mutable
            // ordinary [[Prototype]] slot in the current VM model.
            _ => return Err(NAPI_GENERIC_FAILURE),
        };
        let target_meta = match &object {
            Value::Object { props } => &props.meta,
            Value::Array(array) => &array.meta,
            Value::Function(function) => &function.properties.meta,
            Value::HostFunction { properties, .. } => &properties.meta,
            Value::Class(class) => &class.statics.meta,
            // Be explicit when the input is a genuine JS object that the
            // current VM data model cannot mutate as an ordinary object.
            _ => return Err(NAPI_GENERIC_FAILURE),
        };

        let non_extensible = target_meta.borrow().non_extensible;
        if non_extensible {
            let old_prototype = napi_effective_prototype(&environment, &object)?;
            let same_prototype = prototype.as_ref().map_or_else(
                || matches!(old_prototype, Value::Null),
                |prototype| crate::interpreter::strict_equals(&old_prototype, prototype.as_ref()),
            );
            if !same_prototype {
                return Err(NAPI_GENERIC_FAILURE);
            }
            return Ok(());
        }

        if let Some(candidate) = prototype.as_ref() {
            let target_identity = napi_object_identity(&object)?;
            let mut current = Some(candidate.clone());
            let mut seen = HashSet::new();
            let mut depth = 0;
            while let Some(value) = current {
                if depth > crate::value::MAX_PROTOTYPE_DEPTH {
                    return Err(NAPI_GENERIC_FAILURE);
                }
                let value = value.as_ref();
                let identity = napi_object_identity(value)?;
                if identity == target_identity || !seen.insert(identity) {
                    return Err(NAPI_GENERIC_FAILURE);
                }
                let next = napi_effective_prototype(&environment, value)?;
                current = (!matches!(next, Value::Null)).then(|| Rc::new(next));
                depth += 1;
            }
        }

        match &object {
            Value::Object { props } => props.set_proto(prototype),
            Value::Array(array) => array.set_proto(prototype),
            Value::Function(function) => function.properties.set_proto(prototype),
            Value::HostFunction { properties, .. } => properties.set_proto(prototype),
            Value::Class(class) => class.statics.set_proto(prototype),
            _ => unreachable!("target metadata was validated above"),
        }
        Ok(())
    })
}

/// Experimental fast object creation with a supplied prototype and ordered
/// data properties. Only prototype types represented by the VM's ordinary
/// object-chain lookup are accepted; specialized prototypes fail explicitly.
pub(super) unsafe extern "C" fn api_create_object_with_properties(
    env: NapiEnv,
    prototype_or_null: NapiValue,
    property_names: *const NapiValue,
    property_values: *const NapiValue,
    property_count: usize,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        if property_count > crate::value::MAX_OBJECT_PROPS {
            return Err(NAPI_GENERIC_FAILURE);
        }
        if property_count > 0 && (property_names.is_null() || property_values.is_null()) {
            return Err(NAPI_INVALID_ARG);
        }

        let environment = environment(env)?;
        let handles = environment.handles.borrow();
        let prototype_value = if prototype_or_null.is_null() {
            Value::Null
        } else {
            handles.get(prototype_or_null)?
        };
        let prototype = match &prototype_value {
            Value::Null => None,
            Value::Object { .. } | Value::Function(_) | Value::Class(_) => {
                Some(Rc::new(prototype_value))
            }
            other if !is_napi_property_object(other) => return Err(NAPI_OBJECT_EXPECTED),
            _ => return Err(NAPI_GENERIC_FAILURE),
        };

        let mut properties: Vec<(String, Value)> = Vec::with_capacity(property_count);
        let mut symbol_keys = Vec::new();
        for index in 0..property_count {
            let name_handle = unsafe { *property_names.add(index) };
            let value_handle = unsafe { *property_values.add(index) };
            let name = handles.get(name_handle)?;
            let symbol = match &name {
                Value::Symbol(symbol) => Some(symbol.clone()),
                _ => None,
            };
            let key = napi_property_key(&name)?;
            let value = handles.get(value_handle)?;
            match properties.iter_mut().find(|(existing, _)| existing == &key) {
                Some((_, existing_value)) => *existing_value = value,
                None => properties.push((key.clone(), value)),
            }
            if let Some(symbol) = symbol {
                if let Some((_, existing_symbol)) = symbol_keys
                    .iter_mut()
                    .find(|(existing, _)| existing == &key)
                {
                    *existing_symbol = symbol;
                } else {
                    symbol_keys.push((key, symbol));
                }
            }
        }
        drop(handles);

        let object = Value::object_with_proto(properties, prototype);
        if let Value::Object { props } = &object {
            let mut metadata = props.meta.borrow_mut();
            for (key, symbol) in symbol_keys {
                metadata.set_symbol_key(&key, symbol);
            }
        }
        let handle = environment.handles.borrow_mut().create(object)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

/// Queue a finalizer for the runtime's owner-thread event loop. This entry
/// point deliberately avoids the thread-local environment lookup used by
/// ordinary Node-API functions, so native finalization work can safely post
/// from a thread that cannot enter the interpreter.
pub(super) unsafe extern "C" fn api_post_finalizer(
    env: NapiEnv,
    finalize: Option<NapiFinalize>,
    data: *mut c_void,
    hint: *mut c_void,
) -> i32 {
    let status = match (env.is_null(), finalize) {
        (false, Some(finalize)) => {
            let notification = HostRuntimeNotification::PostedFinalizer(PostedFinalizer {
                environment: env as usize,
                finalize,
                data: data as usize,
                hint: hint as usize,
            });
            match post_finalizer_senders().lock() {
                Ok(senders) => match senders.get(&(env as usize)) {
                    Some(sender) => sender
                        .send(notification)
                        .map_or(NAPI_GENERIC_FAILURE, |_| NAPI_OK),
                    None => NAPI_INVALID_ARG,
                },
                Err(_) => NAPI_GENERIC_FAILURE,
            }
        }
        _ => NAPI_INVALID_ARG,
    };
    // `last_error` is owner-thread state. Update it only when this call is
    // made on that thread; a worker must not touch the interpreter's `Cell`.
    if let Ok(environment) = environment(env) {
        environment.last_error.set(napi_extended_error_info(status));
    }
    status
}

pub(super) fn napi_default_builtin_prototype(
    environment: &NapiEnvironment,
    constructor_name: &str,
) -> Result<Value, i32> {
    let owner = environment.owner.upgrade().ok_or(NAPI_INVALID_ARG)?;
    let global = owner.borrow().global.clone();
    global
        .borrow()
        .get(constructor_name)
        .and_then(|constructor| constructor.get_prop("prototype"))
        .ok_or(NAPI_GENERIC_FAILURE)
}

pub(super) fn napi_default_function_prototype(environment: &NapiEnvironment) -> Result<Value, i32> {
    napi_default_builtin_prototype(environment, "Function")
}

pub(super) fn napi_default_object_prototype(environment: &NapiEnvironment) -> Result<Value, i32> {
    let owner = environment.owner.upgrade().ok_or(NAPI_INVALID_ARG)?;
    owner
        .borrow()
        .object_prototype
        .clone()
        .ok_or(NAPI_GENERIC_FAILURE)
}

pub(super) unsafe extern "C" fn api_get_element(
    env: NapiEnv,
    value: NapiValue,
    index: u32,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Array(array) = &value else {
            return Err(NAPI_ARRAY_EXPECTED);
        };
        let value = if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_get_element",
                napi_guest_get_property,
                value.clone(),
                vec![Value::Number(index as f64)],
            )?
        } else {
            array
                .borrow()
                .get(index as usize)
                .cloned()
                .unwrap_or(Value::Undefined)
        };
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_set_element(
    env: NapiEnv,
    value: NapiValue,
    index: u32,
    element: NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let element = environment.handles.borrow().get(element)?;
        let Value::Array(array) = &value else {
            return Err(NAPI_ARRAY_EXPECTED);
        };
        let index = index as usize;
        if index >= crate::value::MAX_ARRAY_LEN {
            return Err(NAPI_GENERIC_FAILURE);
        }
        if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_set_element",
                napi_guest_set_property,
                value.clone(),
                vec![Value::Number(index as f64), element],
            )?;
            Ok(())
        } else {
            napi_array_set_property(array, &index.to_string(), element)
        }
    })
}

pub(super) unsafe extern "C" fn api_has_element(
    env: NapiEnv,
    value: NapiValue,
    index: u32,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Array(array) = &value else {
            return Err(NAPI_ARRAY_EXPECTED);
        };
        unsafe { result.write(array.has_index(index as usize)) };
        Ok(())
    })
}
