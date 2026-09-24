pub(super) unsafe extern "C" fn api_create_promise(
    env: NapiEnv,
    deferred_result: *mut NapiDeferred,
    promise_result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if deferred_result.is_null() || promise_result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        if environment.deferreds.borrow().len() >= MAX_LOCAL_HANDLES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let promise = Value::pending_promise();
        promise.borrow_mut().external_pending = true;
        let deferred = new_opaque_handle()?;
        environment.deferreds.borrow_mut().insert(
            deferred as usize,
            NapiDeferredState {
                promise: promise.clone(),
                settling: false,
            },
        );
        let handle = match environment
            .handles
            .borrow_mut()
            .create(Value::Promise(promise))
        {
            Ok(handle) => handle,
            Err(status) => {
                environment
                    .deferreds
                    .borrow_mut()
                    .remove(&(deferred as usize));
                return Err(status);
            }
        };
        unsafe {
            deferred_result.write(deferred);
            promise_result.write(handle);
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_resolve_deferred(
    env: NapiEnv,
    deferred: NapiDeferred,
    resolution: NapiValue,
) -> i32 {
    settle_deferred(env, deferred, resolution, false)
}

pub(super) unsafe extern "C" fn api_reject_deferred(
    env: NapiEnv,
    deferred: NapiDeferred,
    rejection: NapiValue,
) -> i32 {
    settle_deferred(env, deferred, rejection, true)
}

pub(super) unsafe extern "C" fn api_is_promise(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        unsafe { result.write(matches!(value, Value::Promise(_))) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_create_async_work(
    env: NapiEnv,
    async_resource: NapiValue,
    async_resource_name: NapiValue,
    execute: Option<NapiAsyncExecuteCallback>,
    complete: Option<NapiAsyncCompleteCallback>,
    data: *mut c_void,
    result: *mut NapiAsyncWork,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let execute = execute.ok_or(NAPI_INVALID_ARG)?;
        let complete = complete.ok_or(NAPI_INVALID_ARG)?;
        let environment = environment(env)?;
        if !async_resource.is_null() {
            environment.handles.borrow().get(async_resource)?;
        }
        let resource_name = environment.handles.borrow().get(async_resource_name)?;
        if !matches!(resource_name, Value::String(_)) {
            return Err(NAPI_STRING_EXPECTED);
        }
        let mut works = environment.async_works.borrow_mut();
        if works.len() >= MAX_LOCAL_HANDLES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let work = new_opaque_handle()?;
        works.insert(
            work as usize,
            NapiAsyncWorkState {
                execute,
                complete,
                data,
                state: Arc::new(AtomicU8::new(ASYNC_WORK_CREATED)),
                completion_status: Arc::new(AtomicU8::new(u8::MAX)),
                completion_callback_active: false,
                callback_run: false,
            },
        );
        unsafe { result.write(work) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_delete_async_work(env: NapiEnv, work: NapiAsyncWork) -> i32 {
    with_ffi_status(env, || {
        if work.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let mut works = environment.async_works.borrow_mut();
        let state = works.get(&(work as usize)).ok_or(NAPI_INVALID_ARG)?;
        if !matches!(
            state.state.load(Ordering::Acquire),
            ASYNC_WORK_CREATED | ASYNC_WORK_FINISHED
        ) || (state.state.load(Ordering::Acquire) == ASYNC_WORK_FINISHED
            && !state.completion_callback_active
            && !state.callback_run)
        {
            return Err(NAPI_GENERIC_FAILURE);
        }
        works.remove(&(work as usize));
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_queue_async_work(env: NapiEnv, work: NapiAsyncWork) -> i32 {
    with_ffi_status(env, || {
        if work.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let work_id = work as usize;
        let (execute, data, state, completion_status) = {
            let works = environment.async_works.borrow();
            let work = works.get(&work_id).ok_or(NAPI_INVALID_ARG)?;
            if work
                .state
                .compare_exchange(
                    ASYNC_WORK_CREATED,
                    ASYNC_WORK_QUEUED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
            {
                return Err(NAPI_GENERIC_FAILURE);
            }
            (
                work.execute,
                work.data,
                work.state.clone(),
                work.completion_status.clone(),
            )
        };
        let owner = environment.owner.upgrade().ok_or(NAPI_GENERIC_FAILURE)?;
        let sender = owner.borrow().async_work_sender.clone();
        let task = AsyncWorkTask {
            work_id,
            environment: environment.raw() as usize,
            execute,
            data: data as usize,
            state: state.clone(),
            completion_status,
        };
        match sender.try_send(AsyncWorkTaskMessage::Run(task)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(AsyncWorkTaskMessage::Run(task))) => {
                task.state.store(ASYNC_WORK_CREATED, Ordering::Release);
                Err(NAPI_GENERIC_FAILURE)
            }
            Err(TrySendError::Disconnected(AsyncWorkTaskMessage::Run(task))) => {
                task.state.store(ASYNC_WORK_CREATED, Ordering::Release);
                Err(NAPI_GENERIC_FAILURE)
            }
            Err(
                TrySendError::Full(AsyncWorkTaskMessage::Stop)
                | TrySendError::Disconnected(AsyncWorkTaskMessage::Stop),
            ) => Err(NAPI_GENERIC_FAILURE),
        }
    })
}

pub(super) unsafe extern "C" fn api_cancel_async_work(env: NapiEnv, work: NapiAsyncWork) -> i32 {
    with_ffi_status(env, || {
        if work.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let works = environment.async_works.borrow();
        let work = works.get(&(work as usize)).ok_or(NAPI_INVALID_ARG)?;
        work.state
            .compare_exchange(
                ASYNC_WORK_QUEUED,
                ASYNC_WORK_CANCELLED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| NAPI_GENERIC_FAILURE)
    })
}

pub(super) fn threadsafe_function_registry()
-> &'static Mutex<HashMap<usize, Arc<NapiThreadsafeFunctionShared>>> {
    THREADSAFE_FUNCTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn get_threadsafe_function(
    function: NapiThreadsafeFunction,
) -> Result<Arc<NapiThreadsafeFunctionShared>, i32> {
    if function.is_null() {
        return Err(NAPI_INVALID_ARG);
    }
    threadsafe_function_registry()
        .lock()
        .map_err(|_| NAPI_GENERIC_FAILURE)?
        .get(&(function as usize))
        .cloned()
        .ok_or(NAPI_CLOSING)
}

pub(super) unsafe extern "C" fn api_create_threadsafe_function(
    env: NapiEnv,
    function: NapiValue,
    async_resource: NapiValue,
    async_resource_name: NapiValue,
    max_queue_size: usize,
    initial_thread_count: usize,
    thread_finalize_data: *mut c_void,
    thread_finalize_callback: Option<NapiFinalize>,
    context: *mut c_void,
    call_js: Option<NapiThreadsafeFunctionCallJs>,
    result: *mut NapiThreadsafeFunction,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() || initial_thread_count == 0 {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        if !async_resource.is_null() {
            let resource = environment.handles.borrow().get(async_resource)?;
            if !is_napi_property_object(&resource) {
                return Err(NAPI_OBJECT_EXPECTED);
            }
        }
        let resource_name = environment.handles.borrow().get(async_resource_name)?;
        if !matches!(resource_name, Value::String(_)) {
            return Err(NAPI_STRING_EXPECTED);
        }
        let callback = if function.is_null() {
            None
        } else {
            let callback = environment.handles.borrow().get(function)?;
            if !is_napi_function(&callback) {
                return Err(NAPI_FUNCTION_EXPECTED);
            }
            Some(callback)
        };
        if callback.is_none() && call_js.is_none() {
            return Err(NAPI_INVALID_ARG);
        }
        let mut functions = environment.threadsafe_functions.borrow_mut();
        if functions.len() >= MAX_LOCAL_HANDLES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let owner = environment.owner.upgrade().ok_or(NAPI_GENERIC_FAILURE)?;
        let notifications = owner.borrow().runtime_notification_sender.clone();
        let id = new_opaque_handle()? as usize;
        let shared = Arc::new(NapiThreadsafeFunctionShared {
            id,
            environment: env as usize,
            context: context as usize,
            max_queue_size,
            owner_thread: thread::current().id(),
            notifications,
            state: Mutex::new(NapiThreadsafeFunctionQueue {
                values: VecDeque::new(),
                thread_count: initial_thread_count,
                in_flight: 0,
                closing: false,
                orphaned: false,
                finalized: false,
            }),
            queue_space: Condvar::new(),
        });
        threadsafe_function_registry()
            .lock()
            .map_err(|_| NAPI_GENERIC_FAILURE)?
            .insert(id, shared.clone());
        functions.insert(
            id,
            NapiThreadsafeFunctionState {
                shared: shared.clone(),
                callback,
                call_js,
                context,
                finalize_data: thread_finalize_data,
                finalize: thread_finalize_callback,
                referenced: true,
            },
        );
        unsafe { result.write(id as NapiThreadsafeFunction) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_get_threadsafe_function_context(
    function: NapiThreadsafeFunction,
    result: *mut *mut c_void,
) -> i32 {
    with_threadsafe_ffi_status(function, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let shared = get_threadsafe_function(function)?;
        let state = shared.state.lock().map_err(|_| NAPI_GENERIC_FAILURE)?;
        if state.closing || state.finalized {
            return Err(NAPI_CLOSING);
        }
        unsafe { result.write(shared.context as *mut c_void) };
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_call_threadsafe_function(
    function: NapiThreadsafeFunction,
    data: *mut c_void,
    call_mode: i32,
) -> i32 {
    with_threadsafe_ffi_status(function, || {
        if !matches!(call_mode, TSFN_BLOCKING | TSFN_NONBLOCKING) {
            return Err(NAPI_INVALID_ARG);
        }
        let shared = get_threadsafe_function(function)?;
        let mut state = shared.state.lock().map_err(|_| NAPI_GENERIC_FAILURE)?;
        loop {
            if state.closing || state.thread_count == 0 || state.finalized {
                return Err(NAPI_CLOSING);
            }
            if shared.max_queue_size == 0 || state.values.len() < shared.max_queue_size {
                break;
            }
            if call_mode == TSFN_NONBLOCKING {
                return Err(NAPI_QUEUE_FULL);
            }
            // A blocking call from the VM owner thread would prevent the
            // event loop from draining the queue that this call is waiting on.
            if thread::current().id() == shared.owner_thread {
                return Err(NAPI_QUEUE_FULL);
            }
            state = shared
                .queue_space
                .wait(state)
                .map_err(|_| NAPI_GENERIC_FAILURE)?;
        }
        state.values.push_back(data as usize);
        if shared
            .notifications
            .send(HostRuntimeNotification::ThreadsafeFunction(shared.id))
            .is_err()
        {
            state.values.pop_back();
            shared.queue_space.notify_all();
            return Err(NAPI_GENERIC_FAILURE);
        }
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_acquire_threadsafe_function(function: NapiThreadsafeFunction) -> i32 {
    with_threadsafe_ffi_status(function, || {
        let shared = get_threadsafe_function(function)?;
        let mut state = shared.state.lock().map_err(|_| NAPI_GENERIC_FAILURE)?;
        if state.closing || state.thread_count == 0 || state.finalized {
            return Err(NAPI_CLOSING);
        }
        state.thread_count = state
            .thread_count
            .checked_add(1)
            .ok_or(NAPI_GENERIC_FAILURE)?;
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_release_threadsafe_function(
    function: NapiThreadsafeFunction,
    release_mode: i32,
) -> i32 {
    with_threadsafe_ffi_status(function, || {
        if !matches!(release_mode, TSFN_RELEASE | TSFN_ABORT) {
            return Err(NAPI_INVALID_ARG);
        }
        let shared = get_threadsafe_function(function)?;
        let mut state = shared.state.lock().map_err(|_| NAPI_GENERIC_FAILURE)?;
        if state.thread_count == 0 || state.finalized {
            return Err(NAPI_CLOSING);
        }
        if release_mode == TSFN_ABORT {
            state.closing = true;
        }
        state.thread_count -= 1;
        if state.thread_count == 0 {
            state.closing = true;
        }
        let remove_orphaned = state.orphaned && state.thread_count == 0;
        shared.queue_space.notify_all();
        let _ = shared
            .notifications
            .send(HostRuntimeNotification::ThreadsafeFunction(shared.id));
        drop(state);
        if remove_orphaned && let Ok(mut registry) = threadsafe_function_registry().lock() {
            registry.remove(&shared.id);
        }
        Ok(())
    })
}

pub(super) fn set_threadsafe_function_referenced(
    env: NapiEnv,
    function: NapiThreadsafeFunction,
    referenced: bool,
) -> Result<(), i32> {
    let environment = environment(env)?;
    let shared = get_threadsafe_function(function)?;
    if shared.environment != env as usize {
        return Err(NAPI_INVALID_ARG);
    }
    let state = shared.state.lock().map_err(|_| NAPI_GENERIC_FAILURE)?;
    if state.closing || state.finalized {
        return Err(NAPI_CLOSING);
    }
    drop(state);
    let mut functions = environment.threadsafe_functions.borrow_mut();
    let function = functions.get_mut(&shared.id).ok_or(NAPI_CLOSING)?;
    function.referenced = referenced;
    Ok(())
}

pub(super) unsafe extern "C" fn api_ref_threadsafe_function(
    env: NapiEnv,
    function: NapiThreadsafeFunction,
) -> i32 {
    with_ffi_status(env, || {
        set_threadsafe_function_referenced(env, function, true)
    })
}

pub(super) unsafe extern "C" fn api_unref_threadsafe_function(
    env: NapiEnv,
    function: NapiThreadsafeFunction,
) -> i32 {
    with_ffi_status(env, || {
        set_threadsafe_function_referenced(env, function, false)
    })
}

pub(super) unsafe extern "C" fn api_add_env_cleanup_hook(
    env: NapiEnv,
    function: Option<NapiCleanupHook>,
    argument: *mut c_void,
) -> i32 {
    with_ffi_status(env, || {
        let function = function.ok_or(NAPI_INVALID_ARG)?;
        let environment = environment(env)?;
        if environment.finalizing.get() {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let function_address = function as usize;
        let argument = argument as usize;
        let mut hooks = environment.cleanup_hooks.borrow_mut();
        if hooks
            .iter()
            .any(|hook| hook.function_address == function_address && hook.argument == argument)
        {
            // Node aborts for duplicate pairs. Return an error instead so a
            // malformed addon cannot terminate the embedding desktop app.
            return Err(NAPI_INVALID_ARG);
        }
        if hooks.len() >= MAX_LOCAL_HANDLES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let order = next_environment_cleanup_hook_order(&environment)?;
        hooks.push(NapiCleanupHookRecord {
            function,
            function_address,
            argument,
            order,
        });
        Ok(())
    })
}

pub(super) unsafe extern "C" fn api_remove_env_cleanup_hook(
    env: NapiEnv,
    function: Option<NapiCleanupHook>,
    argument: *mut c_void,
) -> i32 {
    with_ffi_status(env, || {
        let function = function.ok_or(NAPI_INVALID_ARG)?;
        let environment = environment(env)?;
        let function_address = function as usize;
        let argument = argument as usize;
        let mut hooks = environment.cleanup_hooks.borrow_mut();
        let Some(index) = hooks.iter().position(|hook| {
            hook.function_address == function_address && hook.argument == argument
        }) else {
            // Node aborts for an unknown pair. Keep the same exact-match
            // requirement while reporting a recoverable argument error.
            return Err(NAPI_INVALID_ARG);
        };
        hooks.remove(index);
        Ok(())
    })
}

pub(super) fn settle_deferred(
    env: NapiEnv,
    deferred: NapiDeferred,
    value_handle: NapiValue,
    rejected: bool,
) -> i32 {
    with_ffi_status(env, || {
        if deferred.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value_handle)?;
        let key = deferred as usize;
        let promise = {
            let mut deferreds = environment.deferreds.borrow_mut();
            let deferred = deferreds.get_mut(&key).ok_or(NAPI_INVALID_ARG)?;
            if deferred.settling {
                return Err(NAPI_GENERIC_FAILURE);
            }
            deferred.settling = true;
            deferred.promise.clone()
        };

        let result = if has_guest_callback_dispatcher(&environment) {
            let (name, operation): (&'static str, NapiGuestOperation) = if rejected {
                ("napi_reject_deferred", napi_guest_reject_deferred)
            } else {
                ("napi_resolve_deferred", napi_guest_resolve_deferred)
            };
            run_napi_guest_operation(
                &environment,
                name,
                operation,
                Value::Promise(promise),
                vec![value.clone()],
            )
            .map(|_| ())
        } else {
            settle_deferred_without_interpreter(&promise, value, rejected)
        };

        if result.is_ok() {
            environment.deferreds.borrow_mut().remove(&key);
        } else if let Some(deferred) = environment.deferreds.borrow_mut().get_mut(&key) {
            deferred.settling = false;
        }
        result
    })
}

pub(super) fn settle_deferred_without_interpreter(
    promise: &Rc<RefCell<PromiseInner>>,
    value: Value,
    rejected: bool,
) -> Result<(), i32> {
    let mut promise = promise.borrow_mut();
    if !promise.reactions.is_empty() {
        return Err(NAPI_GENERIC_FAILURE);
    }
    if !rejected
        && !matches!(
            value,
            Value::Undefined
                | Value::Null
                | Value::Bool(_)
                | Value::Number(_)
                | Value::BigInt(_)
                | Value::String(_)
                | Value::Symbol(_)
        )
    {
        // Assimilating an object or another promise can execute guest code,
        // which is allowed only through the active interpreter dispatcher.
        return Err(NAPI_GENERIC_FAILURE);
    }
    if promise.state == PromiseState::Pending {
        promise.resolution_locked = true;
        promise.state = if rejected {
            PromiseState::Rejected
        } else {
            PromiseState::Fulfilled
        };
        promise.external_pending = false;
        promise.value = value;
    }
    Ok(())
}
