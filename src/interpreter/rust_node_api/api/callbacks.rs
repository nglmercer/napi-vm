pub(super) unsafe extern "C" fn api_async_init(
    env: NapiEnv,
    async_resource: NapiValue,
    async_resource_name: NapiValue,
    result: *mut NapiAsyncContext,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let resource = if async_resource.is_null() {
            Value::Null
        } else {
            environment.handles.borrow().get(async_resource)?
        };
        if !matches!(resource, Value::Null) && !is_napi_property_object(&resource) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let resource_name = environment.handles.borrow().get(async_resource_name)?;
        if !matches!(resource_name, Value::String(_)) {
            return Err(NAPI_STRING_EXPECTED);
        }
        let context = new_opaque_handle()?;
        environment.async_contexts.borrow_mut().insert(
            context as usize,
            NapiAsyncContextState {
                _resource: resource,
                _resource_name: resource_name,
            },
        );
        unsafe { result.write(context) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_async_destroy(env: NapiEnv, context: NapiAsyncContext) -> i32 {
    with_ffi_status(env, || {
        if context.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let context_id = context as usize;
        if environment
            .callback_scopes
            .borrow()
            .iter()
            .any(|scope| scope.async_context == context_id)
        {
            return Err(NAPI_INVALID_ARG);
        }
        environment
            .async_contexts
            .borrow_mut()
            .remove(&context_id)
            .map(|_| ())
            .ok_or(NAPI_INVALID_ARG)
    })
}

pub(super) fn call_napi_guest_function(
    environment: &NapiEnvironment,
    receiver: NapiValue,
    function: NapiValue,
    argc: usize,
    argv: *const NapiValue,
    kind: HostCallbackKind,
) -> Result<Value, i32> {
    let receiver = environment.handles.borrow().get(receiver)?;
    let function = environment.handles.borrow().get(function)?;
    if !is_napi_function(&function) {
        return Err(NAPI_FUNCTION_EXPECTED);
    }
    let args = callback_arguments(environment, argc, argv)?;
    call_guest_callback(
        environment,
        HostCallback {
            callback: function,
            this_value: receiver,
            args,
            kind,
        },
    )
}

pub(super) unsafe extern "C" fn api_make_callback(
    env: NapiEnv,
    context: NapiAsyncContext,
    recv: NapiValue,
    function: NapiValue,
    argc: usize,
    argv: *const NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        if !context.is_null()
            && !environment
                .async_contexts
                .borrow()
                .contains_key(&(context as usize))
        {
            return Err(NAPI_INVALID_ARG);
        }
        let value = call_napi_guest_function(
            &environment,
            recv,
            function,
            argc,
            argv,
            HostCallbackKind::MakeCallback,
        )?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_open_callback_scope(
    env: NapiEnv,
    resource_object: NapiValue,
    context: NapiAsyncContext,
    result: *mut NapiCallbackScope,
) -> i32 {
    with_ffi_status(env, || {
        if context.is_null() || result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        if !resource_object.is_null() {
            let resource = environment.handles.borrow().get(resource_object)?;
            if !matches!(resource, Value::Null) && !is_napi_property_object(&resource) {
                return Err(NAPI_OBJECT_EXPECTED);
            }
        }
        let context_id = context as usize;
        if !environment
            .async_contexts
            .borrow()
            .contains_key(&context_id)
        {
            return Err(NAPI_INVALID_ARG);
        }
        let token = new_opaque_handle()?;
        environment
            .callback_scopes
            .borrow_mut()
            .push(NapiCallbackScopeState {
                token: token as usize,
                async_context: context_id,
            });
        unsafe { result.write(token) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_close_callback_scope(env: NapiEnv, scope: NapiCallbackScope) -> i32 {
    with_ffi_status(env, || {
        if scope.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let mut scopes = environment.callback_scopes.borrow_mut();
        if scopes.last().map(|active| active.token) != Some(scope as usize) {
            return Err(NAPI_CALLBACK_SCOPE_MISMATCH);
        }
        scopes.pop();
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_call_function(
    env: NapiEnv,
    recv: NapiValue,
    function: NapiValue,
    argc: usize,
    argv: *const NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = call_napi_guest_function(
            &environment,
            recv,
            function,
            argc,
            argv,
            HostCallbackKind::Call,
        )?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_new_instance(
    env: NapiEnv,
    constructor: NapiValue,
    argc: usize,
    argv: *const NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let constructor = environment.handles.borrow().get(constructor)?;
        if !is_napi_function(&constructor) {
            return Err(NAPI_FUNCTION_EXPECTED);
        }
        let args = callback_arguments(&environment, argc, argv)?;
        let value = call_guest_callback(
            &environment,
            HostCallback {
                callback: constructor,
                this_value: Value::Undefined,
                args,
                kind: HostCallbackKind::Construct,
            },
        )?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_instanceof(
    env: NapiEnv,
    object: NapiValue,
    constructor: NapiValue,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handles = environment.handles.borrow();
        let object = handles.get(object)?;
        let constructor = handles.get(constructor)?;
        if matches!(&constructor, Value::Object { props }
            if props.meta.borrow().builtin_constructor == Some(crate::value::BuiltinConstructor::Date))
        {
            unsafe { result.write(matches!(object, Value::Date(_))) };
            return Ok(());
        }
        if !is_napi_function(&constructor) {
            return Err(NAPI_FUNCTION_EXPECTED);
        }
        let proxy_in_prototype_chain = napi_prototype_chain_contains_proxy(&object)
            || napi_prototype_chain_contains_proxy(&constructor);
        if proxy_in_prototype_chain || napi_constructor_has_custom_has_instance(&constructor)? {
            if !has_guest_callback_dispatcher(&environment) {
                // A custom @@hasInstance method or Proxy trap can run guest
                // code. Keep it on the interpreter's paused callback path;
                // addon initialization and shutdown do not have that
                // dispatcher.
                return Err(NAPI_GENERIC_FAILURE);
            }
            let value = run_napi_guest_operation(
                &environment,
                "napi_instanceof",
                napi_guest_instanceof,
                constructor,
                vec![object],
            )?;
            let Value::Bool(is_instance) = value else {
                return Err(NAPI_GENERIC_FAILURE);
            };
            unsafe { result.write(is_instance) };
            return Ok(());
        }
        let is_instance = match &constructor {
            Value::Class(class) => napi_class_instanceof(&object, class)?,
            Value::Function(function) => napi_function_instanceof(&object, &constructor, function)?,
            Value::HostFunction { .. } => napi_host_function_instanceof(&object, &constructor)?,
            _ => return Err(NAPI_GENERIC_FAILURE),
        };
        unsafe { result.write(is_instance) };
        Ok(())
    })
}

pub(super) fn napi_prototype_chain_contains_proxy(value: &Value) -> bool {
    let mut current = value.clone();
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        if matches!(current, Value::Proxy(_)) {
            return true;
        }
        let Some(prototype) = current.proto_of() else {
            return false;
        };
        current = prototype.as_ref().clone();
    }
    // Let the interpreter's ordinary instanceof path report a prototype
    // depth or cycle error instead of silently using the direct fast path.
    true
}

pub(super) fn napi_constructor_has_custom_has_instance(constructor: &Value) -> Result<bool, i32> {
    let mut current = constructor.clone();
    let mut visited = HashSet::new();
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        let properties = match &current {
            Value::Class(class) => class.statics.clone(),
            Value::Function(function) => function.properties.clone(),
            Value::HostFunction { properties, .. } => properties.clone(),
            Value::Object { props } => props.clone(),
            Value::Proxy(_) => return Err(NAPI_GENERIC_FAILURE),
            _ => return Ok(false),
        };
        let identity = Rc::as_ptr(&properties) as usize;
        if !visited.insert(identity) {
            return Err(NAPI_GENERIC_FAILURE);
        }
        if let Some((_, value)) = properties
            .borrow()
            .iter()
            .find(|(key, _)| key == "__symbol:4__")
        {
            // An explicit nullish value shadows any inherited method and
            // restores ordinary class prototype checking.
            let value = value.deref_binding();
            if matches!(value, Value::Undefined | Value::Null) {
                return Ok(false);
            }
            if !crate::builtins::is_default_has_instance_method(&value) {
                return Ok(true);
            }
        }
        if let Value::Function(function) = &current
            && let Some(bound) = &function.bound
        {
            current = bound.target.clone();
            continue;
        }
        let Some(prototype) = properties.proto() else {
            return Ok(false);
        };
        current = prototype.as_ref().clone();
    }
    Err(NAPI_GENERIC_FAILURE)
}

pub(super) fn napi_guest_instanceof(
    interpreter: &mut Interpreter,
    constructor: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let value = args.first().cloned().unwrap_or(Value::Undefined);
    let result = interpreter.instance_of(&value, &constructor)?;
    Ok(Value::Bool(interpreter.truthy(&result)))
}

pub(super) fn napi_function_instanceof(
    object: &Value,
    constructor: &Value,
    function: &crate::value::FunctionData,
) -> Result<bool, i32> {
    if matches!(object, Value::Proxy(_)) {
        return Err(NAPI_GENERIC_FAILURE);
    }
    if !is_napi_property_object(object) {
        return Ok(false);
    }
    if let Some(bound) = &function.bound {
        return match &bound.target {
            Value::Class(class) => napi_class_instanceof(object, class),
            Value::Function(target) => napi_function_instanceof(object, &bound.target, target),
            _ => Err(NAPI_GENERIC_FAILURE),
        };
    }
    let prototype = function.prototype_value(constructor);
    if !is_napi_property_object(&prototype) {
        return Err(NAPI_GENERIC_FAILURE);
    }
    let mut current = object.proto_of();
    let mut visited = HashSet::new();
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        let Some(prototype_link) = current else {
            return Ok(false);
        };
        if crate::interpreter::strict_equals(prototype_link.as_ref(), &prototype) {
            return Ok(true);
        }
        let identity = match prototype_link.as_ref() {
            Value::Object { props } => Rc::as_ptr(props) as usize,
            Value::Class(class) => Rc::as_ptr(&class.statics) as usize,
            Value::Function(function) => Rc::as_ptr(&function.properties) as usize,
            Value::HostFunction { properties, .. } => Rc::as_ptr(properties) as usize,
            Value::Proxy(_) => return Err(NAPI_GENERIC_FAILURE),
            _ => return Ok(false),
        };
        if !visited.insert(identity) {
            return Err(NAPI_GENERIC_FAILURE);
        }
        current = prototype_link.proto_of();
    }
    Err(NAPI_GENERIC_FAILURE)
}

pub(super) fn napi_host_function_instanceof(object: &Value, constructor: &Value) -> Result<bool, i32> {
    if matches!(object, Value::Proxy(_)) {
        return Err(NAPI_GENERIC_FAILURE);
    }
    if !is_napi_property_object(object) {
        return Ok(false);
    }
    let prototype = constructor
        .get_prop("prototype")
        .unwrap_or(Value::Undefined);
    if !is_napi_property_object(&prototype) {
        return Err(NAPI_GENERIC_FAILURE);
    }
    let mut current = object.proto_of();
    let mut visited = HashSet::new();
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        let Some(prototype_link) = current else {
            return Ok(false);
        };
        if crate::interpreter::strict_equals(prototype_link.as_ref(), &prototype) {
            return Ok(true);
        }
        let identity = match prototype_link.as_ref() {
            Value::Object { props } => Rc::as_ptr(props) as usize,
            Value::Class(class) => Rc::as_ptr(&class.statics) as usize,
            Value::Function(function) => Rc::as_ptr(&function.properties) as usize,
            Value::HostFunction { properties, .. } => Rc::as_ptr(properties) as usize,
            Value::Proxy(_) => return Err(NAPI_GENERIC_FAILURE),
            _ => return Ok(false),
        };
        if !visited.insert(identity) {
            return Err(NAPI_GENERIC_FAILURE);
        }
        current = prototype_link.proto_of();
    }
    Err(NAPI_GENERIC_FAILURE)
}

pub(super) fn napi_class_instanceof(object: &Value, class: &ClassData) -> Result<bool, i32> {
    if let Value::Error(error) = object {
        return Ok(class.name == "Error" || class.name == error.name);
    }
    if matches!(object, Value::Proxy(_)) {
        return Err(NAPI_GENERIC_FAILURE);
    }
    let mut prototype = object.proto_of();
    let mut visited = HashSet::new();
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        let Some(current) = prototype else {
            return Ok(false);
        };
        if crate::interpreter::strict_equals(current.as_ref(), class.prototype.as_ref()) {
            return Ok(true);
        }
        let identity = match current.as_ref() {
            Value::Object { props } => Rc::as_ptr(props) as usize,
            Value::Class(class) => Rc::as_ptr(&class.prototype) as usize,
            Value::Proxy(_) => return Err(NAPI_GENERIC_FAILURE),
            _ => return Ok(false),
        };
        if !visited.insert(identity) {
            return Err(NAPI_GENERIC_FAILURE);
        }
        prototype = current.proto_of();
    }
    Err(NAPI_GENERIC_FAILURE)
}

pub(super) unsafe extern "C" fn api_get_cb_info(
    env: NapiEnv,
    info: NapiCallbackInfo,
    argc: *mut usize,
    argv: *mut NapiValue,
    this_arg: *mut NapiValue,
    data: *mut *mut c_void,
) -> i32 {
    with_ffi_status(env, || {
        if argc.is_null() || info.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let frame = environment
            .active_callbacks
            .borrow()
            .get(&(info as usize))
            .cloned()
            .ok_or(NAPI_INVALID_ARG)?;
        let capacity = unsafe { argc.read() };
        if argv.is_null() {
            unsafe { argc.write(frame.args.len()) };
        } else {
            let count = capacity.min(frame.args.len());
            for (index, handle) in frame.args.iter().take(count).enumerate() {
                unsafe { argv.add(index).write(*handle) };
            }
            unsafe { argc.write(count) };
        }
        if !this_arg.is_null() {
            unsafe { this_arg.write(frame.this_arg) };
        }
        if !data.is_null() {
            unsafe { data.write(frame.data) };
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_new_target(
    env: NapiEnv,
    info: NapiCallbackInfo,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if info.is_null() || result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let frame = environment
            .active_callbacks
            .borrow()
            .get(&(info as usize))
            .cloned()
            .ok_or(NAPI_INVALID_ARG)?;
        unsafe { result.write(frame.new_target) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_adjust_external_memory(
    env: NapiEnv,
    change_in_bytes: i64,
    result: *mut i64,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let adjusted = environment
            .external_memory
            .get()
            .checked_add(change_in_bytes)
            .ok_or(NAPI_GENERIC_FAILURE)?;
        environment.external_memory.set(adjusted);
        unsafe { result.write(adjusted) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_version(env: NapiEnv, result: *mut u32) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let owner = environment.owner.upgrade().ok_or(NAPI_INVALID_ARG)?;
        let max_napi_version = owner.borrow().max_napi_version;
        unsafe { result.write(max_napi_version) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_node_version(
    env: NapiEnv,
    result: *mut *const NapiNodeVersion,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        unsafe { result.write(&environment.node_version) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_uv_event_loop(env: NapiEnv, loop_result: *mut *mut c_void) -> i32 {
    let status = with_ffi_status(env, || {
        if loop_result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let _environment = environment(env)?;
        // This backend has no libuv loop. Return an explicit failure and a
        // null output instead of fabricating an ABI-compatible-looking ptr.
        unsafe { loop_result.write(std::ptr::null_mut()) };
        Err(NAPI_GENERIC_FAILURE)
    });
    if status == NAPI_GENERIC_FAILURE
        && let Ok(environment) = environment(env)
    {
        environment.last_error.set(napi_extended_error_info_with_message(
            status,
            b"napi_get_uv_event_loop is unsupported by the Rust Node-API backend: libuv is not embedded\0",
        ));
    }
    status
}

pub(super) unsafe extern "C" fn api_module_register(module: *mut c_void) {
    if module.is_null() {
        return;
    }
    let _ = NAPI_MODULE_REGISTRATIONS.try_with(|registrations| {
        if let Ok(mut registrations) = registrations.try_borrow_mut()
            && let Some(active) = registrations.last_mut()
            && active.len() < 1024
        {
            active.push(module as usize);
        }
    });
}

unsafe fn fatal_error_message(pointer: *const c_char, length: usize) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let bytes = if length == usize::MAX {
        // SAFETY: NAPI_AUTO_LENGTH requires a NUL-terminated input string.
        unsafe { CStr::from_ptr(pointer) }.to_bytes()
    } else {
        // Fatal diagnostics should remain bounded even if an addon reports an
        // unreasonable explicit length. The API contract requires valid data.
        let length = length.min(64 * 1024);
        // SAFETY: Node-API callers must provide `length` readable bytes.
        unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) }
    };
    String::from_utf8_lossy(bytes).into_owned()
}

pub(super) unsafe extern "C" fn api_fatal_error(
    location: *const c_char,
    location_length: usize,
    message: *const c_char,
    message_length: usize,
) -> ! {
    let location = unsafe { fatal_error_message(location, location_length) };
    let message = unsafe { fatal_error_message(message, message_length) };
    let mut stderr = std::io::stderr().lock();
    if location.is_empty() {
        let _ = writeln!(stderr, "FATAL ERROR: {message}");
    } else {
        let _ = writeln!(stderr, "FATAL ERROR: {location}: {message}");
    }
    std::process::abort()
}

pub(super) unsafe extern "C" fn api_fatal_exception(env: NapiEnv, exception: NapiValue) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let exception = environment.handles.borrow().get(exception)?;
        let mut exceptions = environment.fatal_exceptions.borrow_mut();
        if exceptions.len() >= MAX_PENDING_FATAL_EXCEPTIONS {
            return Err(NAPI_QUEUE_FULL);
        }
        exceptions.push_back(exception);
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_strict_equals(
    env: NapiEnv,
    left: NapiValue,
    right: NapiValue,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handles = environment.handles.borrow();
        let left = handles.get(left)?;
        let right = handles.get(right)?;
        unsafe { result.write(crate::interpreter::strict_equals(&left, &right)) };
        Ok(())
    })
}

pub(super) fn napi_guest_run_script(
    interpreter: &mut Interpreter,
    _receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let Some(Value::String(source)) = args.first() else {
        return Err(VmErr::Msg("TypeError: script must be a string".into()));
    };
    interpreter.run_script_source(source)
}

pub(super) unsafe extern "C" fn api_run_script(
    env: NapiEnv,
    script: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let script = environment.handles.borrow().get(script)?;
        let Value::String(ref script) = script else {
            return Err(NAPI_STRING_EXPECTED);
        };
        let value = run_napi_guest_operation(
            &environment,
            "napi_run_script",
            napi_guest_run_script,
            Value::Undefined,
            vec![Value::String(script.clone())],
        )?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_open_handle_scope(env: NapiEnv, result: *mut NapiHandleScope) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let mut handles = environment.handles.borrow_mut();
        let scope = handles.open_scope()?;
        let handle = match handles.create_scope_handle(scope, false) {
            Ok(handle) => handle,
            Err(status) => {
                let _ = handles.close_scope(scope);
                return Err(status);
            }
        };
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_close_handle_scope(env: NapiEnv, scope: NapiHandleScope) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        environment.handles.borrow_mut().close_scope_handle(scope)
    })
}

pub(super) unsafe extern "C" fn api_open_escapable_handle_scope(
    env: NapiEnv,
    result: *mut NapiHandleScope,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let mut handles = environment.handles.borrow_mut();
        let scope = handles.open_escapable_scope()?;
        let handle = match handles.create_scope_handle(scope, true) {
            Ok(handle) => handle,
            Err(status) => {
                let _ = handles.close_scope(scope);
                return Err(status);
            }
        };
        unsafe { result.write(handle) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_close_escapable_handle_scope(env: NapiEnv, scope: NapiHandleScope) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        environment
            .handles
            .borrow_mut()
            .close_escapable_scope_handle(scope)
    })
}

pub(super) unsafe extern "C" fn api_escape_handle(
    env: NapiEnv,
    scope: NapiHandleScope,
    escapee: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let escaped = environment
            .handles
            .borrow_mut()
            .escape_handle(scope, escapee)?;
        unsafe { result.write(escaped) };
        Ok(())
    })
}

pub(super) fn to_int32(number: f64) -> i32 {
    if !number.is_finite() || number == 0.0 {
        return 0;
    }
    let modulo = number.trunc().rem_euclid(4_294_967_296.0) as u32;
    modulo as i32
}
