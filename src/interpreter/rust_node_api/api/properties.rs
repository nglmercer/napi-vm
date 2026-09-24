pub(super) unsafe extern "C" fn api_define_properties(
    env: NapiEnv,
    object: NapiValue,
    property_count: usize,
    properties: *const NapiPropertyDescriptor,
) -> i32 {
    with_ffi_status(env, || {
        if property_count > crate::value::MAX_OBJECT_PROPS {
            return Err(NAPI_GENERIC_FAILURE);
        }
        if property_count > 0 && properties.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if napi_is_external_value(&environment, &object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        if let Value::Function(function) = &object {
            function.ensure_name_length_properties();
            function.prototype_value(&object);
        }
        let props_meta = match &object {
            Value::Object { props } => &props.meta,
            Value::Class(class) => &class.statics.meta,
            Value::Function(function) => &function.properties.meta,
            Value::HostFunction { properties, .. } => &properties.meta,
            Value::Array(array) => &array.meta,
            _ => return Err(NAPI_OBJECT_EXPECTED),
        };
        let descriptors = if property_count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(properties, property_count) }
        };
        for descriptor in descriptors {
            if descriptor.utf8name.is_null() == descriptor.name.is_null() {
                return Err(NAPI_INVALID_ARG);
            }
            let name = if descriptor.utf8name.is_null() {
                environment.handles.borrow().get(descriptor.name)?
            } else {
                Value::String(unsafe { read_c_string(descriptor.utf8name)? })
            };
            let symbol = match &name {
                Value::Symbol(symbol) => Some(symbol.clone()),
                Value::String(_) => None,
                _ => return Err(NAPI_INVALID_ARG),
            };
            let key = napi_property_key(&name)?;
            let has_accessor = descriptor.getter.is_some() || descriptor.setter.is_some();
            if has_accessor && (descriptor.method.is_some() || !descriptor.value.is_null()) {
                return Err(NAPI_INVALID_ARG);
            }
            if !has_accessor && descriptor.method.is_some() && !descriptor.value.is_null() {
                return Err(NAPI_INVALID_ARG);
            }
            let mut descriptor_properties = Vec::with_capacity(5);
            descriptor_properties.push((
                "enumerable".to_owned(),
                Value::Bool(descriptor.attributes & 0b010 != 0),
            ));
            descriptor_properties.push((
                "configurable".to_owned(),
                Value::Bool(descriptor.attributes & 0b100 != 0),
            ));
            if has_accessor {
                if let Some(getter) = descriptor.getter {
                    descriptor_properties.push((
                        "get".to_owned(),
                        create_native_callback_value(
                            &environment,
                            &format!("get {key}"),
                            getter,
                            descriptor.data,
                        )?,
                    ));
                }
                if let Some(setter) = descriptor.setter {
                    descriptor_properties.push((
                        "set".to_owned(),
                        create_native_callback_value(
                            &environment,
                            &format!("set {key}"),
                            setter,
                            descriptor.data,
                        )?,
                    ));
                }
            } else {
                let value = if let Some(method) = descriptor.method {
                    create_native_callback_value(&environment, &key, method, descriptor.data)?
                } else if descriptor.value.is_null() {
                    Value::Undefined
                } else {
                    environment.handles.borrow().get(descriptor.value)?
                };
                descriptor_properties.push(("value".to_owned(), value));
                descriptor_properties.push((
                    "writable".to_owned(),
                    Value::Bool(descriptor.attributes & 0b001 != 0),
                ));
            }
            let descriptor_value = Value::object(descriptor_properties);
            crate::builtins::object::define_property(&object, &key, &descriptor_value)
                .map_err(|_| NAPI_GENERIC_FAILURE)?;
            if let Some(symbol) = symbol {
                match &object {
                    Value::Array(array) => array.set_symbol_key(&key, symbol),
                    _ => props_meta.borrow_mut().set_symbol_key(&key, symbol),
                }
            }
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_define_class(
    env: NapiEnv,
    name: *const c_char,
    name_length: usize,
    constructor: Option<NapiCallback>,
    data: *mut c_void,
    property_count: usize,
    properties: *const NapiPropertyDescriptor,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        if property_count > crate::value::MAX_OBJECT_PROPS {
            return Err(NAPI_GENERIC_FAILURE);
        }
        if property_count > 0 && properties.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let constructor = constructor.ok_or(NAPI_FUNCTION_EXPECTED)?;
        let environment = environment(env)?;
        let class_name = if name_length == usize::MAX {
            unsafe { read_c_string(name)? }
        } else if name_length == 0 {
            String::new()
        } else {
            if name.is_null() {
                return Err(NAPI_INVALID_ARG);
            }
            let bytes = unsafe { std::slice::from_raw_parts(name.cast::<u8>(), name_length) };
            std::str::from_utf8(bytes)
                .map_err(|_| NAPI_INVALID_ARG)?
                .to_owned()
        };
        let descriptors = if property_count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(properties, property_count) }
        };
        let native_constructor =
            create_native_callback_value(&environment, &class_name, constructor, data)?;
        let prototype = Value::object(Vec::new());
        let statics = Rc::new(crate::value::ObjectCell::new_with_default_proto(vec![
            ("name".to_owned(), Value::String(class_name.clone())),
            ("prototype".to_owned(), prototype.clone()),
            ("length".to_owned(), Value::Number(0.0)),
        ]));
        {
            let mut meta = statics.meta.borrow_mut();
            meta.set_attrs(
                "name",
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
            meta.set_attrs(
                "prototype",
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: false,
                },
            );
            meta.set_attrs(
                "length",
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        let class = Value::Class(Box::new(ClassData {
            name: class_name,
            constructor: Box::new(native_constructor),
            prototype: Rc::new(prototype.clone()),
            statics,
        }));
        prototype
            .set_prop("constructor".to_owned(), class.clone())
            .map_err(|_| NAPI_GENERIC_FAILURE)?;
        if let Value::Object { props } = &prototype {
            props.meta.borrow_mut().set_attrs(
                "constructor",
                PropAttrs {
                    writable: true,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        let prototype_handle = environment.handles.borrow_mut().create(prototype)?;
        let class_handle = environment.handles.borrow_mut().create(class.clone())?;
        for descriptor in descriptors {
            let target = if descriptor.attributes & NAPI_PROPERTY_STATIC != 0 {
                class_handle
            } else {
                prototype_handle
            };
            let status = unsafe { api_define_properties(env, target, 1, descriptor) };
            if status != NAPI_OK {
                return Err(status);
            }
        }
        unsafe { result.write(class_handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_function(
    env: NapiEnv,
    name: *const c_char,
    name_length: usize,
    callback: Option<NapiCallback>,
    data: *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let callback = callback.ok_or(NAPI_FUNCTION_EXPECTED)?;
        let environment = environment(env)?;
        let function_name = if name_length == usize::MAX {
            if name.is_null() {
                String::new()
            } else {
                unsafe { read_c_string(name)? }
            }
        } else if name_length == 0 {
            String::new()
        } else {
            if name.is_null() {
                return Err(NAPI_INVALID_ARG);
            }
            let bytes = unsafe { std::slice::from_raw_parts(name.cast::<u8>(), name_length) };
            std::str::from_utf8(bytes)
                .map_err(|_| NAPI_INVALID_ARG)?
                .to_owned()
        };
        let value = create_native_callback_value(&environment, &function_name, callback, data)?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_set_named_property(
    env: NapiEnv,
    object: NapiValue,
    name: *const c_char,
    value: NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        let key = unsafe { read_c_string(name)? };
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        let value = environment.handles.borrow().get(value)?;
        if napi_is_external_value(&environment, &object) {
            // Node accepts these writes but an external has no property slots.
            return Ok(());
        }
        if !matches!(
            object,
            Value::Object { .. }
                | Value::Array(_)
                | Value::Class(_)
                | Value::Function(_)
                | Value::HostFunction { .. }
                | Value::GlobalObject
        ) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_set_named_property",
                napi_guest_set_property,
                object,
                vec![Value::String(key), value],
            )?;
            Ok(())
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_set(&environment, &key, value)
            } else {
                object
                    .set_prop(key, value)
                    .map_err(|_| NAPI_GENERIC_FAILURE)
            }
        }
    })
}

pub(super) unsafe extern "C" fn api_get_named_property(
    env: NapiEnv,
    object: NapiValue,
    name: *const c_char,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let key = unsafe { read_c_string(name)? };
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) && !matches!(object, Value::String(_)) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let value = if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_get_named_property",
                napi_guest_get_property,
                object,
                vec![Value::String(key)],
            )?
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_get(&environment, &key)?
            } else {
                object.get_prop(&key).unwrap_or(Value::Undefined)
            }
        };
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_property(
    env: NapiEnv,
    object: NapiValue,
    key: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let key = environment.handles.borrow().get(key)?;
        let value = if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_get_property",
                napi_guest_get_property,
                object,
                vec![key],
            )?
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_get(&environment, &napi_property_key(&key)?)?
            } else {
                napi_direct_get_property(&object, &key)?
            }
        };
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_set_property(
    env: NapiEnv,
    object: NapiValue,
    key: NapiValue,
    value: NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let key = environment.handles.borrow().get(key)?;
        let value = environment.handles.borrow().get(value)?;
        if napi_is_external_value(&environment, &object) {
            // Match napi_set_named_property: native writes to an external are
            // successful but cannot add JavaScript properties.
            return Ok(());
        }
        if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_set_property",
                napi_guest_set_property,
                object,
                vec![key, value],
            )?;
            Ok(())
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_set(&environment, &napi_property_key(&key)?, value)
            } else {
                napi_direct_set_property(&object, &key, value)
            }
        }
    })
}

pub(super) unsafe extern "C" fn api_has_property(
    env: NapiEnv,
    object: NapiValue,
    key: NapiValue,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let key = environment.handles.borrow().get(key)?;
        let found = if has_guest_callback_dispatcher(&environment) {
            let value = run_napi_guest_operation(
                &environment,
                "napi_has_property",
                napi_guest_has_property,
                object,
                vec![key],
            )?;
            let Value::Bool(found) = value else {
                return Err(NAPI_GENERIC_FAILURE);
            };
            found
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_has(&environment, &napi_property_key(&key)?)?
            } else {
                object.has_prop(&napi_property_key(&key)?)
            }
        };
        unsafe { result.write(found) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_delete_property(
    env: NapiEnv,
    object: NapiValue,
    key: NapiValue,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        let key = environment.handles.borrow().get(key)?;
        let deleted =
            napi_delete_property_value(&environment, object, key, "napi_delete_property")?;
        if !result.is_null() {
            unsafe { result.write(deleted) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_delete_element(
    env: NapiEnv,
    object: NapiValue,
    index: u32,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        let key = Value::String(index.to_string());
        let deleted = napi_delete_property_value(&environment, object, key, "napi_delete_element")?;
        if !result.is_null() {
            unsafe { result.write(deleted) };
        }
        Ok(())
    })
}

pub(super) fn napi_delete_property_value(
    environment: &NapiEnvironment,
    object: Value,
    key: Value,
    operation_name: &'static str,
) -> Result<bool, i32> {
    if !is_napi_property_object(&object) {
        return Err(NAPI_OBJECT_EXPECTED);
    }
    if has_guest_callback_dispatcher(environment) {
        let value = run_napi_guest_operation(
            environment,
            operation_name,
            napi_guest_delete_property,
            object,
            vec![key],
        )?;
        let Value::Bool(deleted) = value else {
            return Err(NAPI_GENERIC_FAILURE);
        };
        Ok(deleted)
    } else if matches!(object, Value::GlobalObject) {
        napi_global_delete(environment, &napi_property_key(&key)?)
    } else {
        napi_direct_delete_property(&object, &key)
    }
}

pub(super) unsafe extern "C" fn api_has_own_property(
    env: NapiEnv,
    object: NapiValue,
    key: NapiValue,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let key = environment.handles.borrow().get(key)?;
        if !matches!(key, Value::String(_) | Value::Symbol(_)) {
            set_pending_exception(
                &environment,
                Value::Error(ErrorData::new(
                    "TypeError",
                    "property key must be a string or symbol",
                )),
            )?;
            return Err(NAPI_PENDING_EXCEPTION);
        }
        let found = if has_guest_callback_dispatcher(&environment) {
            let value = run_napi_guest_operation(
                &environment,
                "napi_has_own_property",
                napi_guest_has_own_property,
                object,
                vec![key],
            )?;
            let Value::Bool(found) = value else {
                return Err(NAPI_GENERIC_FAILURE);
            };
            found
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_has_own(&environment, &napi_property_key(&key)?)?
            } else {
                napi_direct_has_own_property(&object, &key)?
            }
        };
        unsafe { result.write(found) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_has_named_property(
    env: NapiEnv,
    object: NapiValue,
    name: *const c_char,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let name = unsafe { read_c_string(name)? };
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let found = if has_guest_callback_dispatcher(&environment) {
            let value = run_napi_guest_operation(
                &environment,
                "napi_has_named_property",
                napi_guest_has_property,
                object,
                vec![Value::String(name)],
            )?;
            let Value::Bool(found) = value else {
                return Err(NAPI_GENERIC_FAILURE);
            };
            found
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_has(&environment, &name)?
            } else {
                object.has_prop(&name)
            }
        };
        unsafe { result.write(found) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_property_names(
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
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let names = if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_get_property_names",
                napi_guest_get_property_names,
                object,
                Vec::new(),
            )?
        } else {
            if matches!(object, Value::GlobalObject) {
                Value::checked_array(
                    napi_global_scope(&environment)?
                        .borrow()
                        .all_keys()
                        .into_iter()
                        .filter(|key| !crate::interpreter::is_internal_key(key))
                        .map(Value::String)
                        .collect(),
                )
                .map_err(|_| NAPI_GENERIC_FAILURE)?
            } else {
                napi_direct_property_names(&environment, &object)?
            }
        };
        let handle = environment.handles.borrow_mut().create(names)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_all_property_names(
    env: NapiEnv,
    object: NapiValue,
    key_mode: i32,
    key_filter: i32,
    key_conversion: i32,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null()
            || !(0..=1).contains(&key_mode)
            || !(0..=0x1f).contains(&key_filter)
            || !(0..=1).contains(&key_conversion)
        {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }

        // Proxy ownKeys traps execute guest code. Enumerate a chain containing
        // a proxy through the paused-interpreter dispatcher; the direct path
        // below remains available to addon initializers before that dispatcher
        // exists.
        let mut probe = object.clone();
        let mut contains_proxy = false;
        let mut contains_global = false;
        for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
            match &probe {
                Value::Proxy(_) => contains_proxy = true,
                Value::GlobalObject => contains_global = true,
                _ => {}
            }
            let Some(prototype) = napi_direct_prototype(&environment, &probe)? else {
                break;
            };
            probe = (*prototype).clone();
        }
        if contains_global && key_filter & 0x07 != 0 {
            // Global bindings do not retain the per-property attributes that
            // Node-API's writable/enumerable/configurable filters require.
            return Err(NAPI_GENERIC_FAILURE);
        }
        if contains_proxy || contains_global {
            if !has_guest_callback_dispatcher(&environment) {
                return Err(NAPI_GENERIC_FAILURE);
            }
            let names = run_napi_guest_operation(
                &environment,
                "napi_get_all_property_names",
                napi_guest_get_all_property_names,
                object,
                vec![
                    Value::Number(key_mode as f64),
                    Value::Number(key_filter as f64),
                    Value::Number(key_conversion as f64),
                ],
            )?;
            let handle = environment.handles.borrow_mut().create(names)?;
            unsafe { result.write(handle) };
            return Ok(());
        }

        let mut current = object;
        let mut seen = Vec::<NapiPropertyKey>::new();
        let mut names = Vec::new();
        for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
            for (key, attributes) in napi_direct_all_property_keys(&current)? {
                if seen.iter().any(|existing| existing.matches(&key)) {
                    continue;
                }
                // A filtered own key still shadows a matching key on the
                // prototype chain, just as it does during JS property lookup.
                seen.push(key.clone());
                if seen.len() > crate::value::MAX_ARRAY_LEN {
                    return Err(NAPI_GENERIC_FAILURE);
                }
                let filtered = (key_filter & 1 != 0 && !attributes.writable)
                    || (key_filter & 2 != 0 && !attributes.enumerable)
                    || (key_filter & 4 != 0 && !attributes.configurable)
                    || (key_filter & 8 != 0 && matches!(key, NapiPropertyKey::String(_)))
                    || (key_filter & 16 != 0 && matches!(key, NapiPropertyKey::Symbol(_)));
                if filtered {
                    continue;
                }
                names.push(match key {
                    NapiPropertyKey::String(key) if key_conversion == 0 => {
                        crate::value::array_index(&key)
                            .map_or_else(|| Value::String(key), |index| Value::Number(index as f64))
                    }
                    NapiPropertyKey::String(key) => Value::String(key),
                    NapiPropertyKey::Symbol(symbol) => Value::Symbol(symbol),
                });
            }
            if key_mode == 1 {
                break;
            }
            let Some(prototype) = napi_direct_prototype(&environment, &current)? else {
                break;
            };
            current = (*prototype).clone();
        }
        let names = Value::checked_array(names).map_err(|_| NAPI_GENERIC_FAILURE)?;
        let handle = environment.handles.borrow_mut().create(names)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}
