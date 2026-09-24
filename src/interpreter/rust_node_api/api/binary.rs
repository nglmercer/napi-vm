pub(super) fn create_napi_buffer(bytes: Vec<u8>) -> Result<(Value, *mut c_void), i32> {
    if bytes.len() > MAX_NAPI_BUFFER_BYTES {
        return Err(NAPI_GENERIC_FAILURE);
    }
    let length = bytes.len();
    let value = Value::TypedArray(Rc::new(TypedArrayData {
        kind: TypedKind::Uint8,
        buffer: Buffer::owned(bytes).into(),
        byte_offset: 0,
        length,
        is_buffer: true,
    }));
    let (data, _) = napi_buffer_data(&value)?;
    Ok((value, data))
}

pub(super) fn napi_buffer_data(value: &Value) -> Result<(*mut c_void, usize), i32> {
    let Value::TypedArray(view) = value else {
        return Err(NAPI_INVALID_ARG);
    };
    if view.kind != TypedKind::Uint8 {
        return Err(NAPI_INVALID_ARG);
    }
    let (data, length) = napi_typedarray_data(view)?;
    if length > MAX_NAPI_BUFFER_BYTES {
        return Err(NAPI_GENERIC_FAILURE);
    }
    Ok((data, length))
}

pub(super) fn create_napi_external_buffer_value(
    environment: &NapiEnvironment,
    value: Value,
    data: *mut c_void,
    finalize: NapiExternalBufferFinalizer,
    hint: *mut c_void,
) -> Result<NapiValue, i32> {
    let identity = napi_object_identity(&value)?;
    let handle = environment.handles.borrow_mut().create(value.clone())?;
    environment.external_buffers.borrow_mut().insert(
        identity,
        NapiExternalBuffer {
            _value: value,
            data,
            finalize,
            hint,
        },
    );
    Ok(handle)
}

pub(super) fn is_napi_buffer(_: &NapiEnvironment, value: &Value) -> bool {
    matches!(value, Value::TypedArray(view) if view.is_buffer)
}

pub(super) unsafe extern "C" fn api_create_buffer(
    env: NapiEnv,
    length: usize,
    data: *mut *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() || length > MAX_NAPI_BUFFER_BYTES {
            return Err(if result.is_null() {
                NAPI_INVALID_ARG
            } else {
                NAPI_GENERIC_FAILURE
            });
        }
        let environment = environment(env)?;
        let (value, data_pointer) = create_napi_buffer(vec![0; length])?;
        let handle = environment.handles.borrow_mut().create(value.clone())?;
        if !data.is_null() {
            unsafe { data.write(data_pointer) };
        }
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_buffer_copy(
    env: NapiEnv,
    length: usize,
    data: *const c_void,
    result_data: *mut *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() || length > MAX_NAPI_BUFFER_BYTES || (data.is_null() && length != 0) {
            return Err(if result.is_null() || data.is_null() && length != 0 {
                NAPI_INVALID_ARG
            } else {
                NAPI_GENERIC_FAILURE
            });
        }
        let bytes = if length == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), length) }.to_vec()
        };
        let environment = environment(env)?;
        let (value, data_pointer) = create_napi_buffer(bytes)?;
        let handle = environment.handles.borrow_mut().create(value.clone())?;
        if !result_data.is_null() {
            unsafe { result_data.write(data_pointer) };
        }
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_external_buffer(
    env: NapiEnv,
    length: usize,
    data: *mut c_void,
    finalize: Option<NapiFinalize>,
    hint: *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() || (data.is_null() && length != 0) {
            return Err(NAPI_INVALID_ARG);
        }
        if length > MAX_NAPI_BUFFER_BYTES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let environment = environment(env)?;
        // SAFETY: Node-API requires the addon to keep this allocation live
        // until its supplied finalizer is called. The environment retains the
        // guest value and runs that finalizer only during host shutdown.
        let backing =
            unsafe { Buffer::external(data.cast::<u8>(), length) }.ok_or(NAPI_INVALID_ARG)?;
        let value = Value::TypedArray(Rc::new(TypedArrayData {
            kind: TypedKind::Uint8,
            buffer: backing.into(),
            byte_offset: 0,
            length,
            is_buffer: true,
        }));
        let handle = create_napi_external_buffer_value(
            &environment,
            value,
            data,
            NapiExternalBufferFinalizer::Napi(finalize),
            hint,
        )?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_buffer_from_arraybuffer(
    env: NapiEnv,
    arraybuffer: NapiValue,
    byte_offset: usize,
    byte_length: usize,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let arraybuffer = environment.handles.borrow().get(arraybuffer)?;
        let Value::ArrayBuffer(buffer) = &arraybuffer else {
            return Err(NAPI_ARRAYBUFFER_EXPECTED);
        };
        if buffer.is_detached() {
            set_pending_exception(
                &environment,
                Value::Error(ErrorData::new(
                    "TypeError",
                    "Cannot create a Buffer from a detached ArrayBuffer",
                )),
            )?;
            return Err(NAPI_PENDING_EXCEPTION);
        }
        let backing_length = buffer.borrow().len();
        let end = byte_offset.checked_add(byte_length);
        if end.is_none_or(|end| end > backing_length) {
            set_pending_exception(
                &environment,
                Value::Error(ErrorData::new(
                    "RangeError",
                    "Buffer byte range is outside the ArrayBuffer",
                )),
            )?;
            return Err(NAPI_PENDING_EXCEPTION);
        }
        if byte_length > MAX_NAPI_BUFFER_BYTES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let value = Value::TypedArray(Rc::new(TypedArrayData {
            kind: TypedKind::Uint8,
            buffer: buffer.clone().into(),
            byte_offset,
            length: byte_length,
            is_buffer: true,
        }));
        let handle = environment.handles.borrow_mut().create(value.clone())?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_buffer_info(
    env: NapiEnv,
    value: NapiValue,
    data: *mut *mut c_void,
    length: *mut usize,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        if !is_napi_buffer(&environment, &value) {
            return Err(NAPI_INVALID_ARG);
        }
        let (data_pointer, byte_length) = napi_buffer_data(&value)?;
        if !data.is_null() {
            unsafe { data.write(data_pointer) };
        }
        if !length.is_null() {
            unsafe { length.write(byte_length) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_is_buffer(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let is_buffer = is_napi_buffer(&environment, &value);
        unsafe { result.write(is_buffer) };
        Ok(())
    })
}

pub(super) fn napi_arraybuffer_data(buffer: &Buffer) -> (*mut c_void, usize) {
    if buffer.is_detached() {
        return (std::ptr::null_mut(), 0);
    }
    let mut bytes = buffer.borrow_mut();
    (bytes.as_mut_ptr().cast::<c_void>(), bytes.len())
}

pub(super) fn napi_typedarray_data(view: &TypedArrayData) -> Result<(*mut c_void, usize), i32> {
    if view.buffer.is_detached() {
        return Ok((std::ptr::null_mut(), 0));
    }
    let byte_length = view
        .length
        .checked_mul(view.kind.size())
        .ok_or(NAPI_INVALID_ARG)?;
    let end = view
        .byte_offset
        .checked_add(byte_length)
        .ok_or(NAPI_INVALID_ARG)?;
    if end > view.buffer.len() {
        return Err(NAPI_INVALID_ARG);
    }
    let data = unsafe {
        view.buffer
            .data_ptr()
            .add(view.byte_offset)
            .cast::<c_void>()
    };
    Ok((data, byte_length))
}

pub(super) fn napi_typed_kind(kind: i32) -> Option<TypedKind> {
    match kind {
        0 => Some(TypedKind::Int8),
        1 => Some(TypedKind::Uint8),
        2 => Some(TypedKind::Uint8Clamped),
        3 => Some(TypedKind::Int16),
        4 => Some(TypedKind::Uint16),
        5 => Some(TypedKind::Int32),
        6 => Some(TypedKind::Uint32),
        7 => Some(TypedKind::Float32),
        8 => Some(TypedKind::Float64),
        9 => Some(TypedKind::BigInt64),
        10 => Some(TypedKind::BigUint64),
        _ => None,
    }
}

pub(super) fn napi_typed_kind_id(kind: TypedKind) -> i32 {
    match kind {
        TypedKind::Int8 => 0,
        TypedKind::Uint8 => 1,
        TypedKind::Uint8Clamped => 2,
        TypedKind::Int16 => 3,
        TypedKind::Uint16 => 4,
        TypedKind::Int32 => 5,
        TypedKind::Uint32 => 6,
        TypedKind::Float32 => 7,
        TypedKind::Float64 => 8,
        TypedKind::BigInt64 => 9,
        TypedKind::BigUint64 => 10,
    }
}

pub(super) fn validate_arraybuffer_window(
    buffer: &Buffer,
    byte_offset: usize,
    byte_length: usize,
    alignment: usize,
) -> Result<(), i32> {
    if byte_length > MAX_NAPI_BUFFER_BYTES
        || alignment == 0
        || !byte_offset.is_multiple_of(alignment)
    {
        return Err(NAPI_INVALID_ARG);
    }
    let end = byte_offset
        .checked_add(byte_length)
        .ok_or(NAPI_INVALID_ARG)?;
    if end > buffer.borrow().len() {
        return Err(NAPI_INVALID_ARG);
    }
    Ok(())
}

pub(super) unsafe extern "C" fn api_is_arraybuffer(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        unsafe { result.write(matches!(value, Value::ArrayBuffer(_))) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_arraybuffer(
    env: NapiEnv,
    byte_length: usize,
    data: *mut *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        if byte_length > MAX_NAPI_BUFFER_BYTES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let environment = environment(env)?;
        let buffer = Buffer::zeroed(byte_length);
        let (data_pointer, _) = napi_arraybuffer_data(&buffer);
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::ArrayBuffer(buffer))?;
        if !data.is_null() {
            unsafe { data.write(data_pointer) };
        }
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_sharedarraybuffer(
    env: NapiEnv,
    byte_length: usize,
    data: *mut *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        if byte_length > MAX_NAPI_BUFFER_BYTES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let environment = environment(env)?;
        let buffer = SharedBuffer::zeroed(byte_length).ok_or(NAPI_GENERIC_FAILURE)?;
        let data_pointer = buffer.data_ptr().cast::<c_void>();
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::SharedArrayBuffer(buffer))?;
        if !data.is_null() {
            unsafe { data.write(data_pointer) };
        }
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_external_sharedarraybuffer(
    env: NapiEnv,
    external_data: *mut c_void,
    byte_length: usize,
    finalize: Option<NodeApiNoEnvFinalize>,
    hint: *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() || (external_data.is_null() && byte_length != 0) {
            return Err(NAPI_INVALID_ARG);
        }
        if byte_length > MAX_NAPI_BUFFER_BYTES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let environment = environment(env)?;
        // SAFETY: Node-API transfers the external byte range's lifetime to the
        // addon finalizer. The host retains this value until that finalizer
        // runs during environment shutdown.
        let buffer = unsafe { SharedBuffer::external(external_data.cast::<u8>(), byte_length) }
            .ok_or(NAPI_INVALID_ARG)?;
        let value = Value::SharedArrayBuffer(buffer);
        let handle = create_napi_external_buffer_value(
            &environment,
            value,
            external_data,
            NapiExternalBufferFinalizer::NoEnv(finalize),
            hint,
        )?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_is_sharedarraybuffer(
    env: NapiEnv,
    value: NapiValue,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        unsafe { result.write(matches!(value, Value::SharedArrayBuffer(_))) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_external_arraybuffer(
    env: NapiEnv,
    external_data: *mut c_void,
    byte_length: usize,
    finalize: Option<NapiFinalize>,
    hint: *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() || (external_data.is_null() && byte_length != 0) {
            return Err(NAPI_INVALID_ARG);
        }
        if byte_length > MAX_NAPI_BUFFER_BYTES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let environment = environment(env)?;
        // SAFETY: The native addon owns this memory and promises to keep it
        // alive until its Node-API finalizer runs. We retain the ArrayBuffer
        // in the environment so its backing memory remains reachable.
        let buffer = unsafe { Buffer::external(external_data.cast::<u8>(), byte_length) }
            .ok_or(NAPI_INVALID_ARG)?;
        let value = Value::ArrayBuffer(buffer);
        let handle = create_napi_external_buffer_value(
            &environment,
            value,
            external_data,
            NapiExternalBufferFinalizer::Napi(finalize),
            hint,
        )?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_arraybuffer_info(
    env: NapiEnv,
    value: NapiValue,
    data: *mut *mut c_void,
    byte_length: *mut usize,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::ArrayBuffer(buffer) = &value else {
            return Err(NAPI_ARRAYBUFFER_EXPECTED);
        };
        let (data_pointer, length) = napi_arraybuffer_data(buffer);
        if !data.is_null() {
            unsafe { data.write(data_pointer) };
        }
        if !byte_length.is_null() {
            unsafe { byte_length.write(length) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_detach_arraybuffer(env: NapiEnv, value: NapiValue) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::ArrayBuffer(buffer) = &value else {
            return Err(NAPI_ARRAYBUFFER_EXPECTED);
        };
        buffer.detach();
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_is_detached_arraybuffer(
    env: NapiEnv,
    value: NapiValue,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let detached = match &value {
            Value::ArrayBuffer(buffer) => buffer.is_detached(),
            // Node reports false for non-ArrayBuffer values rather than
            // returning napi_arraybuffer_expected from this predicate.
            _ => false,
        };
        unsafe { result.write(detached) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_type_tag_object(
    env: NapiEnv,
    object: NapiValue,
    type_tag: *const NapiTypeTag,
) -> i32 {
    with_ffi_status(env, || {
        if type_tag.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(object)?;
        let identity = napi_object_identity(&value)?;
        let tag = unsafe { type_tag.read() };
        let owner = environment.owner.upgrade().ok_or(NAPI_INVALID_ARG)?;
        let mut state = owner.borrow_mut();
        if state.type_tags.contains_key(&identity) {
            return Err(NAPI_INVALID_ARG);
        }
        if state.type_tags.len() >= MAX_LOCAL_HANDLES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        state.type_tags.insert(identity, (tag, value));
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_check_object_type_tag(
    env: NapiEnv,
    object: NapiValue,
    type_tag: *const NapiTypeTag,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if type_tag.is_null() || result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(object)?;
        let identity = napi_object_identity(&value)?;
        let tag = unsafe { type_tag.read() };
        let owner = environment.owner.upgrade().ok_or(NAPI_INVALID_ARG)?;
        let state = owner.borrow();
        let matches = state
            .type_tags
            .get(&identity)
            .is_some_and(|(existing, _)| *existing == tag);
        unsafe { result.write(matches) };
        Ok(())
    })
}

pub(super) fn napi_set_object_integrity(value: &Value, freeze: bool) -> Result<(), i32> {
    if let Value::Array(array) = value {
        array.set_integrity(freeze);
        return Ok(());
    }
    if let Value::Function(function) = value {
        function.ensure_name_length_properties();
        function.prototype_value(value);
    }
    let cell = match value {
        Value::Object { props } => props,
        Value::Function(function) => &function.properties,
        Value::HostFunction { properties, .. } => properties,
        Value::Class(class) => &class.statics,
        _ => {
            return Err(if napi_object_identity(value).is_ok() {
                NAPI_GENERIC_FAILURE
            } else {
                NAPI_OBJECT_EXPECTED
            });
        }
    };
    let names = cell
        .borrow()
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    let mut meta = cell.meta.borrow_mut();
    meta.non_extensible = true;
    for name in names {
        let mut attributes = meta.attrs_of(&name);
        attributes.configurable = false;
        if freeze {
            attributes.writable = false;
        }
        meta.set_attrs(&name, attributes);
    }
    Ok(())
}

pub(super) unsafe extern "C" fn api_object_freeze(env: NapiEnv, object: NapiValue) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(object)?;
        napi_set_object_integrity(&value, true)
    })
}

pub(super) unsafe extern "C" fn api_object_seal(env: NapiEnv, object: NapiValue) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(object)?;
        napi_set_object_integrity(&value, false)
    })
}

pub(super) unsafe extern "C" fn api_add_async_cleanup_hook(
    env: NapiEnv,
    function: Option<NapiAsyncCleanupHook>,
    argument: *mut c_void,
    remove_handle: *mut NapiAsyncCleanupHookHandle,
) -> i32 {
    with_ffi_status(env, || {
        let function = function.ok_or(NAPI_INVALID_ARG)?;
        let environment = environment(env)?;
        if environment.finalizing.get() {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let function_address = function as usize;
        let argument = argument as usize;
        let mut hooks = environment.async_cleanup_hooks.borrow_mut();
        if hooks.iter().any(|hook| {
            hook.function_address == function_address
                && hook.argument == argument
                && hook
                    .control
                    .phase
                    .lock()
                    .is_ok_and(|phase| *phase != AsyncCleanupHookPhase::Removed)
        }) {
            return Err(NAPI_INVALID_ARG);
        }
        if hooks.len() >= MAX_LOCAL_HANDLES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let order = next_environment_cleanup_hook_order(&environment)?;
        let handle = new_opaque_handle()? as usize;
        let control = Arc::new(AsyncCleanupHookControl {
            phase: Mutex::new(AsyncCleanupHookPhase::Registered),
            completed: Condvar::new(),
        });
        async_cleanup_hook_registry()
            .lock()
            .map_err(|_| NAPI_GENERIC_FAILURE)?
            .insert(handle, control.clone());
        hooks.push(NapiAsyncCleanupHookRecord {
            function,
            function_address,
            argument,
            handle,
            order,
            control,
        });
        if !remove_handle.is_null() {
            unsafe { remove_handle.write(handle as NapiAsyncCleanupHookHandle) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_remove_async_cleanup_hook(handle: NapiAsyncCleanupHookHandle) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        remove_async_cleanup_hook_handle(handle);
    }));
}

pub(super) unsafe extern "C" fn api_is_typedarray(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        unsafe { result.write(matches!(value, Value::TypedArray(_))) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_typedarray(
    env: NapiEnv,
    kind: i32,
    length: usize,
    arraybuffer: NapiValue,
    byte_offset: usize,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let kind = napi_typed_kind(kind).ok_or(NAPI_INVALID_ARG)?;
        let byte_length = length.checked_mul(kind.size()).ok_or(NAPI_INVALID_ARG)?;
        if byte_length > MAX_NAPI_BUFFER_BYTES {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let arraybuffer = environment.handles.borrow().get(arraybuffer)?;
        let Value::ArrayBuffer(buffer) = &arraybuffer else {
            return Err(NAPI_ARRAYBUFFER_EXPECTED);
        };
        if buffer.is_detached() {
            return Err(NAPI_INVALID_ARG);
        }
        if kind.size() > 1 && !byte_offset.is_multiple_of(kind.size()) {
            let message = format!(
                "start offset of {} should be a multiple of {}",
                kind.name(),
                kind.size()
            );
            set_pending_exception(
                &environment,
                Value::Error(ErrorData::with_code(
                    "RangeError",
                    message,
                    "ERR_NAPI_INVALID_TYPEDARRAY_ALIGNMENT",
                )),
            )?;
            return Err(NAPI_INVALID_ARG);
        }
        let end = byte_offset
            .checked_add(byte_length)
            .ok_or(NAPI_INVALID_ARG)?;
        if end > buffer.borrow().len() {
            set_pending_exception(
                &environment,
                Value::Error(ErrorData::with_code(
                    "RangeError",
                    "Invalid typed array length",
                    "ERR_NAPI_INVALID_TYPEDARRAY_LENGTH",
                )),
            )?;
            return Err(NAPI_INVALID_ARG);
        }
        let value = Value::TypedArray(Rc::new(TypedArrayData {
            kind,
            buffer: buffer.clone().into(),
            byte_offset,
            length,
            is_buffer: false,
        }));
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_typedarray_info(
    env: NapiEnv,
    typedarray: NapiValue,
    kind: *mut i32,
    length: *mut usize,
    data: *mut *mut c_void,
    arraybuffer: *mut NapiValue,
    byte_offset: *mut usize,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(typedarray)?;
        let Value::TypedArray(view) = &value else {
            return Err(NAPI_INVALID_ARG);
        };
        let (data_pointer, _) = napi_typedarray_data(view)?;
        let arraybuffer_handle = if arraybuffer.is_null() {
            None
        } else {
            Some(
                environment
                    .handles
                    .borrow_mut()
                    .create(view.buffer.to_value())?,
            )
        };
        if !kind.is_null() {
            unsafe { kind.write(napi_typed_kind_id(view.kind)) };
        }
        if !length.is_null() {
            unsafe { length.write(view.effective_length()) };
        }
        if !data.is_null() {
            unsafe { data.write(data_pointer) };
        }
        if let Some(handle) = arraybuffer_handle {
            unsafe { arraybuffer.write(handle) };
        }
        if !byte_offset.is_null() {
            unsafe { byte_offset.write(view.effective_byte_offset()) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_dataview(
    env: NapiEnv,
    byte_length: usize,
    arraybuffer: NapiValue,
    byte_offset: usize,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let arraybuffer = environment.handles.borrow().get(arraybuffer)?;
        let Value::ArrayBuffer(buffer) = &arraybuffer else {
            return Err(NAPI_ARRAYBUFFER_EXPECTED);
        };
        if buffer.is_detached() {
            return Err(NAPI_INVALID_ARG);
        }
        validate_arraybuffer_window(buffer, byte_offset, byte_length, 1)?;
        let value = Value::DataView(Rc::new(TypedArrayData {
            kind: TypedKind::Uint8,
            buffer: buffer.clone().into(),
            byte_offset,
            length: byte_length,
            is_buffer: false,
        }));
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_is_dataview(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        unsafe { result.write(matches!(value, Value::DataView(_))) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_dataview_info(
    env: NapiEnv,
    dataview: NapiValue,
    byte_length: *mut usize,
    data: *mut *mut c_void,
    arraybuffer: *mut NapiValue,
    byte_offset: *mut usize,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(dataview)?;
        let Value::DataView(view) = &value else {
            return Err(NAPI_INVALID_ARG);
        };
        let (data_pointer, _) = napi_typedarray_data(view)?;
        let arraybuffer_handle = if arraybuffer.is_null() {
            None
        } else {
            Some(
                environment
                    .handles
                    .borrow_mut()
                    .create(view.buffer.to_value())?,
            )
        };
        if !byte_length.is_null() {
            unsafe { byte_length.write(view.effective_length()) };
        }
        if !data.is_null() {
            unsafe { data.write(data_pointer) };
        }
        if let Some(handle) = arraybuffer_handle {
            unsafe { arraybuffer.write(handle) };
        }
        if !byte_offset.is_null() {
            unsafe { byte_offset.write(view.effective_byte_offset()) };
        }
        Ok(())
    })
}

unsafe fn api_create_error_with_name(
    env: NapiEnv,
    code: NapiValue,
    message: NapiValue,
    result: *mut NapiValue,
    name: &'static str,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let message = environment.handles.borrow().get(message)?;
        let Value::String(message) = &message else {
            return Err(NAPI_STRING_EXPECTED);
        };
        let code = if code.is_null() {
            None
        } else {
            let code_value = environment.handles.borrow().get(code)?;
            match &code_value {
                Value::String(code) => Some(code.clone()),
                Value::Undefined | Value::Null => None,
                _ => return Err(NAPI_STRING_EXPECTED),
            }
        };
        let mut error = ErrorData::new(name, message.clone());
        error.code = code;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Error(error))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}
