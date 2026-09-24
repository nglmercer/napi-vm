pub(super) unsafe extern "C" fn api_get_undefined(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment.handles.borrow_mut().create(Value::Undefined)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_global(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::GlobalObject)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_null(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment.handles.borrow_mut().create(Value::Null)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_boolean(env: NapiEnv, value: bool, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Bool(value))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_coerce_to_bool(
    env: NapiEnv,
    value: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Bool(value.deref_binding().is_truthy()))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) fn napi_guest_coerce_to_number(
    interpreter: &mut Interpreter,
    _receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let value = args.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Number(interpreter.ecmascript_to_number(&value)?))
}

pub(super) unsafe extern "C" fn api_coerce_to_number(
    env: NapiEnv,
    value: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let value = run_napi_guest_operation(
            &environment,
            "napi_coerce_to_number",
            napi_guest_coerce_to_number,
            Value::Undefined,
            vec![value],
        )?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) fn napi_guest_coerce_to_string(
    interpreter: &mut Interpreter,
    _receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let value = args.first().cloned().unwrap_or(Value::Undefined);
    Value::checked_string(interpreter.napi_to_string(&value)?)
}

pub(super) unsafe extern "C" fn api_coerce_to_string(
    env: NapiEnv,
    value: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let value = run_napi_guest_operation(
            &environment,
            "napi_coerce_to_string",
            napi_guest_coerce_to_string,
            Value::Undefined,
            vec![value],
        )?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_coerce_to_object(
    env: NapiEnv,
    value: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let object = match value {
            Value::Undefined | Value::Null => {
                set_pending_exception(
                    &environment,
                    Value::Error(ErrorData::new(
                        "TypeError",
                        "Cannot convert undefined or null to object",
                    )),
                )?;
                return Err(NAPI_PENDING_EXCEPTION);
            }
            value @ (Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Symbol(_)
            | Value::BigInt(_)) => {
                Value::boxed_primitive(value).expect("primitive values have wrapper objects")
            }
            value => value,
        };
        let handle = environment.handles.borrow_mut().create(object)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_double(env: NapiEnv, value: f64, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Number(value))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_int32(env: NapiEnv, value: i32, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Number(value as f64))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_uint32(env: NapiEnv, value: u32, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Number(value as f64))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_int64(env: NapiEnv, value: i64, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Number(value as f64))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_bigint_int64(
    env: NapiEnv,
    value: i64,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let bigint = crate::bigint::BigInt::from_i64(value);
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::BigInt(Rc::new(bigint)))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_bigint_uint64(
    env: NapiEnv,
    value: u64,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let bigint = crate::bigint::BigInt::from_words(false, &[value]);
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::BigInt(Rc::new(bigint)))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_bigint_words(
    env: NapiEnv,
    sign_bit: i32,
    word_count: usize,
    words: *const u64,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() || (word_count > 0 && words.is_null()) {
            return Err(NAPI_INVALID_ARG);
        }
        if word_count > MAX_BIGINT_WORDS {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let words = if word_count == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(words, word_count) }
        };
        let environment = environment(env)?;
        let bigint = crate::bigint::BigInt::from_words(sign_bit != 0, words);
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::BigInt(Rc::new(bigint)))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_string_utf8(
    env: NapiEnv,
    value: *const c_char,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let value = unsafe { read_utf8(value, length)? };
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::String(value))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_string_latin1(
    env: NapiEnv,
    value: *const c_char,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let value = unsafe { read_latin1(value, length)? };
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::String(value))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_string_utf16(
    env: NapiEnv,
    value: *const u16,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    let utf16_input_error = Cell::new(false);
    let status = with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let value = unsafe { read_utf16(value, length) }.inspect_err(|status| {
            if *status == NAPI_GENERIC_FAILURE {
                utf16_input_error.set(true);
            }
        })?;
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::String(value))?;
        unsafe { result.write(handle) };
        Ok(())
    });
    if utf16_input_error.get()
        && status == NAPI_GENERIC_FAILURE
        && let Ok(environment) = environment(env)
    {
        environment
            .last_error
            .set(napi_extended_error_info_with_message(
                status,
                UTF16_INPUT_ERROR_MESSAGE,
            ));
    }
    status
}

pub(super) unsafe extern "C" fn api_create_external_string_latin1(
    env: NapiEnv,
    value: *mut c_char,
    length: usize,
    finalize: Option<NapiFinalize>,
    finalize_hint: *mut c_void,
    result: *mut NapiValue,
    copied: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() || copied.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let string = unsafe { read_latin1_allow_empty(value, length)? };
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::String(string))?;
        unsafe {
            result.write(handle);
            copied.write(true);
        }
        if let Some(finalize) = finalize {
            // The VM owns a decoded copy, so release the addon's source buffer
            // immediately as Node-API requires when `copied` is true.
            unsafe { finalize(env, value.cast(), finalize_hint) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_external_string_utf16(
    env: NapiEnv,
    value: *mut u16,
    length: usize,
    finalize: Option<NapiFinalize>,
    finalize_hint: *mut c_void,
    result: *mut NapiValue,
    copied: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() || copied.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let string = unsafe { read_utf16_allow_empty(value, length)? };
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::String(string))?;
        unsafe {
            result.write(handle);
            copied.write(true);
        }
        if let Some(finalize) = finalize {
            // The VM owns a decoded copy, so release the addon's source buffer
            // immediately as Node-API requires when `copied` is true.
            unsafe { finalize(env, value.cast(), finalize_hint) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_property_key_latin1(
    env: NapiEnv,
    value: *const c_char,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    unsafe { api_create_string_latin1(env, value, length, result) }
}

pub(super) unsafe extern "C" fn api_create_property_key_utf8(
    env: NapiEnv,
    value: *const c_char,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    unsafe { api_create_string_utf8(env, value, length, result) }
}

pub(super) unsafe extern "C" fn api_create_property_key_utf16(
    env: NapiEnv,
    value: *const u16,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    unsafe { api_create_string_utf16(env, value, length, result) }
}

pub(super) unsafe extern "C" fn api_create_symbol(
    env: NapiEnv,
    description: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let description = if description.is_null() {
            None
        } else {
            let description = environment.handles.borrow().get(description)?;
            let Value::String(description) = &description else {
                return Err(NAPI_STRING_EXPECTED);
            };
            Some(description.clone())
        };
        let handle = environment
            .handles
            .borrow_mut()
            .create(crate::builtins::new_symbol(description))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_node_symbol_for(
    env: NapiEnv,
    description: *const c_char,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        if length != usize::MAX && length > MAX_NAPI_BUFFER_BYTES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let description = unsafe { read_utf8(description, length)? };
        if description.len() > MAX_NAPI_BUFFER_BYTES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let environment = environment(env)?;
        let symbol = crate::builtins::symbol_for_key(&description);
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Symbol(symbol))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_external(
    env: NapiEnv,
    data: *mut c_void,
    finalize: Option<NapiFinalize>,
    finalize_hint: *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        if environment.finalizing.get() {
            return Err(NAPI_GENERIC_FAILURE);
        }

        // Node-API externals are opaque JS values: they behave like
        // non-extensible, null-prototype objects in JS, while napi_typeof
        // reports the distinct napi_external tag.
        let value = Value::object_with_proto(Vec::new(), None);
        let Value::Object { props } = &value else {
            unreachable!("object_with_proto creates an object")
        };
        props.meta.borrow_mut().non_extensible = true;
        let identity = napi_object_identity(&value)?;
        let handle = environment.handles.borrow_mut().create(value.clone())?;
        environment.externals.borrow_mut().insert(
            identity,
            NapiExternal {
                _value: value,
                data,
                finalize,
                hint: finalize_hint,
            },
        );
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_date(env: NapiEnv, time: f64, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Date(Rc::new(Cell::new(time))))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_is_date(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        unsafe { result.write(matches!(value, Value::Date(_))) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_date_value(env: NapiEnv, value: NapiValue, result: *mut f64) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Date(date) = &value else {
            return Err(NAPI_DATE_EXPECTED);
        };
        unsafe { result.write(date.get()) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_typeof(env: NapiEnv, value: NapiValue, result: *mut i32) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        // Values are the Node-API `napi_valuetype` discriminants from
        // js_native_api_types.h. Proxy `typeof` follows its target.
        let value_type = if napi_is_external_value(&environment, &value) {
            8 // napi_external
        } else {
            napi_value_type(&value)
        };
        unsafe { result.write(value_type) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_value_external(
    env: NapiEnv,
    value: NapiValue,
    result: *mut *mut c_void,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let identity = napi_object_identity(&value)?;
        let data = environment
            .externals
            .borrow()
            .get(&identity)
            .map(|external| external.data)
            .ok_or(NAPI_INVALID_ARG)?;
        unsafe { result.write(data) };
        Ok(())
    })
}

pub(super) fn napi_value_type(value: &Value) -> i32 {
    let resolved = value.deref_binding();
    if is_napi_function(&resolved) {
        return 7; // napi_function
    }
    match &resolved {
        Value::Undefined => 0, // napi_undefined
        Value::Null => 1,      // napi_null
        Value::Bool(_) => 2,   // napi_boolean
        Value::Number(_) => 3, // napi_number
        Value::String(_) => 4, // napi_string
        Value::Symbol(_) => 5, // napi_symbol
        Value::BigInt(_) => 9, // napi_bigint
        Value::Proxy(proxy) => napi_value_type(&proxy.target),
        _ => 6, // napi_object
    }
}

pub(super) unsafe extern "C" fn api_get_value_double(env: NapiEnv, value: NapiValue, result: *mut f64) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Number(number) = value else {
            return Err(NAPI_NUMBER_EXPECTED);
        };
        unsafe { result.write(number) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_value_int32(env: NapiEnv, value: NapiValue, result: *mut i32) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Number(number) = value else {
            return Err(NAPI_NUMBER_EXPECTED);
        };
        unsafe { result.write(to_int32(number)) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_value_uint32(env: NapiEnv, value: NapiValue, result: *mut u32) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Number(number) = value else {
            return Err(NAPI_NUMBER_EXPECTED);
        };
        unsafe { result.write(to_int32(number) as u32) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_value_int64(env: NapiEnv, value: NapiValue, result: *mut i64) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Number(number) = value else {
            return Err(NAPI_NUMBER_EXPECTED);
        };
        // napi_get_value_int64 converts finite Numbers by truncating toward
        // zero, but maps NaN and infinities to zero. Rust's float-to-int cast
        // saturates infinities, so handle non-finite values explicitly.
        unsafe { result.write(if number.is_finite() { number as i64 } else { 0 }) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_value_bigint_int64(
    env: NapiEnv,
    value: NapiValue,
    result: *mut i64,
    lossless: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() || lossless.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::BigInt(value) = &value else {
            return Err(NAPI_BIGINT_EXPECTED);
        };
        let narrowed = value.as_n_bit(64, true).map_err(|_| NAPI_GENERIC_FAILURE)?;
        let number = narrowed
            .to_decimal()
            .parse::<i64>()
            .map_err(|_| NAPI_GENERIC_FAILURE)?;
        unsafe { result.write(number) };
        unsafe { lossless.write(value.compare(&narrowed) == std::cmp::Ordering::Equal) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_value_bigint_uint64(
    env: NapiEnv,
    value: NapiValue,
    result: *mut u64,
    lossless: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() || lossless.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::BigInt(value) = &value else {
            return Err(NAPI_BIGINT_EXPECTED);
        };
        let narrowed = value
            .as_n_bit(64, false)
            .map_err(|_| NAPI_GENERIC_FAILURE)?;
        let (_, words) = narrowed.to_words();
        unsafe { result.write(words.first().copied().unwrap_or(0)) };
        unsafe { lossless.write(value.compare(&narrowed) == std::cmp::Ordering::Equal) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_value_bigint_words(
    env: NapiEnv,
    value: NapiValue,
    sign_bit: *mut i32,
    word_count: *mut usize,
    words: *mut u64,
) -> i32 {
    with_ffi_status(env, || {
        if word_count.is_null() || sign_bit.is_null() != words.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::BigInt(value) = &value else {
            return Err(NAPI_BIGINT_EXPECTED);
        };
        let (negative, value_words) = value.to_words();
        if value_words.len() > MAX_BIGINT_WORDS {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let capacity = unsafe { word_count.read() };
        if !sign_bit.is_null() {
            unsafe { sign_bit.write(i32::from(negative)) };
            let copied = capacity.min(value_words.len());
            if copied > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(value_words.as_ptr(), words, copied);
                }
            }
        }
        unsafe { word_count.write(value_words.len()) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_value_bool(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Bool(value) = value else {
            return Err(NAPI_BOOLEAN_EXPECTED);
        };
        unsafe { result.write(value) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_value_string_utf8(
    env: NapiEnv,
    value: NapiValue,
    buffer: *mut c_char,
    buffer_size: usize,
    result: *mut usize,
) -> i32 {
    with_ffi_status(env, || {
        if buffer.is_null() && buffer_size != 0 || buffer.is_null() && result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::String(value) = &value else {
            return Err(NAPI_STRING_EXPECTED);
        };
        let bytes = value.as_bytes();
        let copied = if buffer.is_null() || buffer_size == 0 {
            0
        } else {
            let copied = bytes.len().min(buffer_size - 1);
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), copied);
                buffer.add(copied).write(0);
            }
            copied
        };
        if !result.is_null() {
            unsafe {
                result.write(if buffer.is_null() {
                    bytes.len()
                } else {
                    copied
                })
            };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_value_string_latin1(
    env: NapiEnv,
    value: NapiValue,
    buffer: *mut c_char,
    buffer_size: usize,
    result: *mut usize,
) -> i32 {
    with_ffi_status(env, || {
        if (buffer.is_null() && buffer_size != 0) || (buffer.is_null() && result.is_null()) {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::String(value) = &value else {
            return Err(NAPI_STRING_EXPECTED);
        };
        let length = value.encode_utf16().count();
        if buffer.is_null() {
            unsafe { result.write(length) };
            return Ok(());
        }
        let copied = if buffer_size == 0 {
            0
        } else {
            let copied = length.min(buffer_size - 1);
            for (index, unit) in value.encode_utf16().take(copied).enumerate() {
                unsafe { buffer.add(index).write(unit as u8 as c_char) };
            }
            unsafe { buffer.add(copied).write(0) };
            copied
        };
        if !result.is_null() {
            unsafe { result.write(copied) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_value_string_utf16(
    env: NapiEnv,
    value: NapiValue,
    buffer: *mut u16,
    buffer_size: usize,
    result: *mut usize,
) -> i32 {
    with_ffi_status(env, || {
        if (buffer.is_null() && buffer_size != 0) || (buffer.is_null() && result.is_null()) {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::String(value) = &value else {
            return Err(NAPI_STRING_EXPECTED);
        };
        let length = value.encode_utf16().count();
        if buffer.is_null() {
            unsafe { result.write(length) };
            return Ok(());
        }
        let copied = if buffer_size == 0 {
            0
        } else {
            let copied = length.min(buffer_size - 1);
            for (index, unit) in value.encode_utf16().take(copied).enumerate() {
                unsafe { buffer.add(index).write(unit) };
            }
            unsafe { buffer.add(copied).write(0) };
            copied
        };
        if !result.is_null() {
            unsafe { result.write(copied) };
        }
        Ok(())
    })
}
