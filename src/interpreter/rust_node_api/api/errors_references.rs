pub(super) unsafe extern "C" fn api_create_error(
    env: NapiEnv,
    code: NapiValue,
    message: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    unsafe { api_create_error_with_name(env, code, message, result, "Error") }
}

pub(super) unsafe extern "C" fn api_create_type_error(
    env: NapiEnv,
    code: NapiValue,
    message: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    unsafe { api_create_error_with_name(env, code, message, result, "TypeError") }
}

pub(super) unsafe extern "C" fn api_create_range_error(
    env: NapiEnv,
    code: NapiValue,
    message: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    unsafe { api_create_error_with_name(env, code, message, result, "RangeError") }
}

pub(super) unsafe extern "C" fn api_create_syntax_error(
    env: NapiEnv,
    code: NapiValue,
    message: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    unsafe { api_create_error_with_name(env, code, message, result, "SyntaxError") }
}

pub(super) fn set_pending_exception(environment: &NapiEnvironment, exception: Value) -> Result<(), i32> {
    let mut pending = environment.pending_exception.borrow_mut();
    if pending.is_some() {
        return Err(NAPI_PENDING_EXCEPTION);
    }
    *pending = Some(exception);
    Ok(())
}

pub(super) unsafe extern "C" fn api_throw(env: NapiEnv, error: NapiValue) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let error = environment.handles.borrow().get(error)?;
        set_pending_exception(&environment, error)
    })
}

unsafe fn api_throw_error_with_name(
    env: NapiEnv,
    code: *const c_char,
    message: *const c_char,
    name: &'static str,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let message = unsafe { read_c_string(message)? };
        let code = if code.is_null() {
            None
        } else {
            Some(unsafe { read_c_string(code)? })
        };
        let mut error = ErrorData::new(name, message);
        error.code = code;
        set_pending_exception(&environment, Value::Error(error))
    })
}

pub(super) unsafe extern "C" fn api_throw_error(
    env: NapiEnv,
    code: *const c_char,
    message: *const c_char,
) -> i32 {
    unsafe { api_throw_error_with_name(env, code, message, "Error") }
}

pub(super) unsafe extern "C" fn api_throw_type_error(
    env: NapiEnv,
    code: *const c_char,
    message: *const c_char,
) -> i32 {
    unsafe { api_throw_error_with_name(env, code, message, "TypeError") }
}

pub(super) unsafe extern "C" fn api_throw_range_error(
    env: NapiEnv,
    code: *const c_char,
    message: *const c_char,
) -> i32 {
    unsafe { api_throw_error_with_name(env, code, message, "RangeError") }
}

pub(super) unsafe extern "C" fn api_throw_syntax_error(
    env: NapiEnv,
    code: *const c_char,
    message: *const c_char,
) -> i32 {
    unsafe { api_throw_error_with_name(env, code, message, "SyntaxError") }
}

pub(super) unsafe extern "C" fn api_get_module_file_name(env: NapiEnv, result: *mut *const c_char) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        unsafe { result.write(environment.module_file_url.as_ptr()) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_is_exception_pending(env: NapiEnv, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        unsafe { result.write(environment.pending_exception.borrow().is_some()) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_and_clear_last_exception(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let exception = environment
            .pending_exception
            .borrow_mut()
            .take()
            .unwrap_or(Value::Undefined);
        let handle = environment.handles.borrow_mut().create(exception)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_is_error(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        unsafe { result.write(matches!(value, Value::Error(_))) };
        Ok(())
    })
}

pub(super) fn napi_reference_uses_weak_semantics(environment: &NapiEnvironment, value: &Value) -> bool {
    napi_is_external_value(environment, value)
        || matches!(
            napi_value_type(value),
            NAPI_SYMBOL_TYPE | NAPI_OBJECT_TYPE | NAPI_FUNCTION_TYPE
        )
}

pub(super) unsafe extern "C" fn api_create_reference(
    env: NapiEnv,
    value: NapiValue,
    initial_ref_count: u32,
    result: *mut NapiRef,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let uses_weak_semantics = napi_reference_uses_weak_semantics(&environment, &value);
        if environment.api_version < 10 && !uses_weak_semantics {
            return Err(NAPI_INVALID_ARG);
        }
        let reference = new_opaque_handle()?;
        environment.references.borrow_mut().insert(
            reference as usize,
            NapiReference {
                value: (initial_ref_count > 0 || uses_weak_semantics).then_some(value),
                ref_count: initial_ref_count,
            },
        );
        unsafe { result.write(reference) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_delete_reference(env: NapiEnv, reference: NapiRef) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        environment
            .references
            .borrow_mut()
            .remove(&(reference as usize))
            .map(|_| ())
            .ok_or(NAPI_INVALID_ARG)
    })
}

pub(super) unsafe extern "C" fn api_reference_ref(env: NapiEnv, reference: NapiRef, result: *mut u32) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let mut references = environment.references.borrow_mut();
        napi_collect_weak_reference(&environment, &mut references, reference as usize);
        let reference = references
            .get_mut(&(reference as usize))
            .ok_or(NAPI_INVALID_ARG)?;
        reference.ref_count = reference
            .ref_count
            .checked_add(1)
            .ok_or(NAPI_GENERIC_FAILURE)?;
        if !result.is_null() {
            unsafe { result.write(reference.ref_count) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_reference_unref(
    env: NapiEnv,
    reference: NapiRef,
    result: *mut u32,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let mut references = environment.references.borrow_mut();
        let reference = references
            .get_mut(&(reference as usize))
            .ok_or(NAPI_INVALID_ARG)?;
        reference.ref_count = reference.ref_count.saturating_sub(1);
        if reference.ref_count == 0
            && reference
                .value
                .as_ref()
                .is_some_and(|value| !napi_reference_uses_weak_semantics(&environment, value))
        {
            reference.value = None;
        }
        if !result.is_null() {
            unsafe { result.write(reference.ref_count) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_reference_value(
    env: NapiEnv,
    reference: NapiRef,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = {
            let mut references = environment.references.borrow_mut();
            napi_collect_weak_reference(&environment, &mut references, reference as usize);
            references
                .get(&(reference as usize))
                .map(|reference| reference.value.clone())
                .ok_or(NAPI_INVALID_ARG)?
        };
        let Some(value) = value else {
            unsafe { result.write(std::ptr::null_mut()) };
            return Ok(());
        };
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_wrap(
    env: NapiEnv,
    object: NapiValue,
    native_object: *mut c_void,
    finalize: Option<NapiFinalize>,
    finalize_hint: *mut c_void,
    result: *mut NapiRef,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        if environment.finalizing.get() {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let value = environment.handles.borrow().get(object)?;
        if napi_is_external_value(&environment, &value) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let identity = napi_object_identity(&value)?;
        if environment.wraps.borrow().contains_key(&identity) {
            return Err(NAPI_INVALID_ARG);
        }

        let reference = if result.is_null() {
            None
        } else {
            let reference = new_opaque_handle()?;
            environment.references.borrow_mut().insert(
                reference as usize,
                NapiReference {
                    value: Some(value.clone()),
                    ref_count: 0,
                },
            );
            Some(reference)
        };
        environment.wraps.borrow_mut().insert(
            identity,
            NapiWrap {
                _value: value,
                data: native_object,
                finalize,
                hint: finalize_hint,
            },
        );
        if let Some(reference) = reference {
            unsafe { result.write(reference) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_add_finalizer(
    env: NapiEnv,
    object: NapiValue,
    finalize_data: *mut c_void,
    finalize: Option<NapiFinalize>,
    finalize_hint: *mut c_void,
    result: *mut NapiRef,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        if environment.finalizing.get() {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let finalize = finalize.ok_or(NAPI_INVALID_ARG)?;
        let value = environment.handles.borrow().get(object)?;
        if napi_is_external_value(&environment, &value) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        napi_object_identity(&value)?;

        let reference = if result.is_null() {
            None
        } else {
            let reference = new_opaque_handle()?;
            environment.references.borrow_mut().insert(
                reference as usize,
                NapiReference {
                    value: Some(value.clone()),
                    ref_count: 0,
                },
            );
            Some(reference)
        };
        environment
            .added_finalizers
            .borrow_mut()
            .push(NapiAddedFinalizer {
                _value: value,
                data: finalize_data,
                finalize,
                hint: finalize_hint,
                reference: reference.map(|reference| reference as usize),
            });
        if let Some(reference) = reference {
            unsafe { result.write(reference) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_set_instance_data(
    env: NapiEnv,
    data: *mut c_void,
    finalize: Option<NapiFinalize>,
    hint: *mut c_void,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        if environment.finalizing.get() {
            return Err(NAPI_GENERIC_FAILURE);
        }
        // Node-API replaces the prior slot without invoking its finalizer.
        *environment.instance_data.borrow_mut() = Some(NapiInstanceData {
            data,
            finalize,
            hint,
        });
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_instance_data(env: NapiEnv, data: *mut *mut c_void) -> i32 {
    with_ffi_status(env, || {
        if data.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment
            .instance_data
            .borrow()
            .as_ref()
            .map_or(std::ptr::null_mut(), |instance| instance.data);
        unsafe { data.write(value) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_unwrap(env: NapiEnv, object: NapiValue, result: *mut *mut c_void) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(object)?;
        if napi_is_external_value(&environment, &value) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let identity = napi_object_identity(&value)?;
        let data = environment
            .wraps
            .borrow()
            .get(&identity)
            .map(|wrap| wrap.data)
            .ok_or(NAPI_INVALID_ARG)?;
        unsafe { result.write(data) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_remove_wrap(
    env: NapiEnv,
    object: NapiValue,
    result: *mut *mut c_void,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(object)?;
        if napi_is_external_value(&environment, &value) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let identity = napi_object_identity(&value)?;
        let wrap = environment
            .wraps
            .borrow_mut()
            .remove(&identity)
            .ok_or(NAPI_INVALID_ARG)?;
        unsafe { result.write(wrap.data) };
        // Removing a wrap deliberately drops its finalizer without calling it.
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_object(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::object(Vec::new()))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}
