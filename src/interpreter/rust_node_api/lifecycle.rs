//! Environment registration, weak references, cleanup hooks, and finalizers.

use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;

use crate::value::Value;

use super::api::napi_reference_uses_weak_semantics;
use super::state::{
    AsyncCleanupHookPhase, NAPI_ENVIRONMENTS, NapiAsyncCleanupHookRecord, NapiCleanupHookRecord,
    NapiEnvironment, NapiExternalBufferFinalizer, NapiObjectIdentity, NapiReference,
    NapiReferenceIdentity, post_finalizer_senders,
};
use super::{NAPI_GENERIC_FAILURE, NAPI_OBJECT_EXPECTED, NapiAsyncCleanupHookHandle};

pub(super) fn napi_object_identity(value: &Value) -> Result<NapiObjectIdentity, i32> {
    match value {
        Value::GlobalObject => Ok(NapiObjectIdentity::Global),
        Value::Object { props } => Ok(NapiObjectIdentity::Object(Rc::as_ptr(props) as usize)),
        Value::Array(array) => Ok(NapiObjectIdentity::Array(Rc::as_ptr(array) as usize)),
        // FunctionData carries a shared identity token so Value clones remain
        // the same function object without conflating separate closures.
        Value::Function(function) => Ok(NapiObjectIdentity::Function(
            Rc::as_ptr(&function.identity) as usize,
        )),
        Value::NativeFunction { name, .. } => Ok(NapiObjectIdentity::NativeFunction(
            Rc::as_ptr(name) as *const () as usize,
        )),
        Value::HostFunction { properties, .. } => properties
            .meta
            .borrow()
            .host_function_id
            .map(NapiObjectIdentity::HostFunction)
            .ok_or(NAPI_GENERIC_FAILURE),
        Value::Class(class) => Ok(NapiObjectIdentity::Class(
            Rc::as_ptr(&class.prototype) as usize
        )),
        Value::Promise(promise) => Ok(NapiObjectIdentity::Promise(Rc::as_ptr(promise) as usize)),
        Value::Generator { inner } => Ok(NapiObjectIdentity::Generator(Rc::as_ptr(inner) as usize)),
        Value::StringIterator { inner } => Ok(NapiObjectIdentity::StringIterator(
            Rc::as_ptr(inner) as usize,
        )),
        Value::Date(date) => Ok(NapiObjectIdentity::Date(Rc::as_ptr(date) as usize)),
        Value::Proxy(proxy) => Ok(NapiObjectIdentity::Proxy(Rc::as_ptr(proxy) as usize)),
        Value::ArrayBuffer(buffer) => Ok(NapiObjectIdentity::ArrayBuffer(buffer.identity())),
        Value::SharedArrayBuffer(buffer) => {
            Ok(NapiObjectIdentity::SharedArrayBuffer(buffer.identity()))
        }
        Value::TypedArray(view) => Ok(NapiObjectIdentity::TypedArray(Rc::as_ptr(view) as usize)),
        Value::DataView(view) => Ok(NapiObjectIdentity::DataView(Rc::as_ptr(view) as usize)),
        Value::RegExp(regexp) => Ok(NapiObjectIdentity::RegExp(Rc::as_ptr(regexp) as usize)),
        Value::Error(error) => Ok(NapiObjectIdentity::Error(
            Rc::as_ptr(&error.identity) as usize
        )),
        Value::Undefined
        | Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::HostPending { .. }
        | Value::Symbol(_)
        | Value::BigInt(_)
        | Value::Binding(_) => Err(NAPI_OBJECT_EXPECTED),
        #[cfg(stackful_coroutines)]
        Value::AsyncTask(_) => Err(NAPI_OBJECT_EXPECTED),
    }
}

pub(super) fn napi_is_external_value(environment: &NapiEnvironment, value: &Value) -> bool {
    napi_object_identity(value)
        .ok()
        .is_some_and(|identity| environment.externals.borrow().contains_key(&identity))
}

pub(super) fn napi_reference_identity(
    value: &Value,
    environment: &NapiEnvironment,
) -> Option<NapiReferenceIdentity> {
    if let Value::Symbol(symbol) = value {
        return Some(NapiReferenceIdentity::Symbol(symbol.id));
    }
    napi_object_identity(value)
        .ok()
        .map(NapiReferenceIdentity::Object)
        .filter(|_| napi_reference_uses_weak_semantics(environment, value))
}

pub(super) fn napi_reference_value_strong_count(value: &Value) -> Option<usize> {
    match value {
        Value::Object { props } => Some(Rc::strong_count(props)),
        Value::Array(array) => Some(Rc::strong_count(array)),
        Value::Function(function) => Some(Rc::strong_count(&function.identity)),
        Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
            Some(Rc::strong_count(name))
        }
        Value::Class(class) => Some(Rc::strong_count(&class.statics)),
        Value::Promise(promise) => Some(Rc::strong_count(promise)),
        Value::Generator { inner } => Some(Rc::strong_count(inner)),
        Value::StringIterator { inner } => Some(Rc::strong_count(inner)),
        Value::Symbol(symbol) if symbol.id >= crate::value::FIRST_USER_SYMBOL => {
            Some(Rc::strong_count(symbol))
        }
        Value::Date(date) => Some(Rc::strong_count(date)),
        Value::Proxy(proxy) => Some(Rc::strong_count(proxy)),
        Value::ArrayBuffer(buffer) => Some(buffer.strong_count()),
        // Typed views keep the shared byte store, not the SharedArrayBuffer
        // wrapper allocation, alive in the current value model. Without a
        // tracing heap that wrapper cannot be collected safely here.
        Value::SharedArrayBuffer(_) => None,
        Value::TypedArray(view) | Value::DataView(view) => Some(Rc::strong_count(view)),
        Value::RegExp(regexp) => Some(Rc::strong_count(regexp)),
        Value::Error(error) => Some(Rc::strong_count(&error.identity)),
        // GlobalObject is a permanent runtime root. Well-known symbols are
        // immortal, and all remaining variants either use v10 primitive
        // lifetime rules or are internal sentinels rather than guest objects.
        Value::GlobalObject
        | Value::Symbol(_)
        | Value::Undefined
        | Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::HostPending { .. }
        | Value::BigInt(_)
        | Value::Binding(_) => None,
        #[cfg(stackful_coroutines)]
        Value::AsyncTask(_) => None,
    }
}

pub(super) fn napi_collect_weak_reference(
    environment: &NapiEnvironment,
    references: &mut HashMap<usize, NapiReference>,
    reference_id: usize,
) {
    let Some(reference) = references.get(&reference_id) else {
        return;
    };
    if reference.ref_count != 0 {
        return;
    }
    let Some(value) = reference.value.as_ref() else {
        return;
    };
    let Some(identity) = napi_reference_identity(value, environment) else {
        return;
    };
    let Some(strong_count) = napi_reference_value_strong_count(value) else {
        return;
    };
    let weak_references = references
        .values()
        .filter(|candidate| candidate.ref_count == 0)
        .filter_map(|candidate| candidate.value.as_ref())
        .filter(|candidate| napi_reference_identity(candidate, environment) == Some(identity))
        .count();
    if strong_count <= weak_references {
        for candidate in references
            .values_mut()
            .filter(|candidate| candidate.ref_count == 0)
        {
            if candidate.value.as_ref().is_some_and(|candidate| {
                napi_reference_identity(candidate, environment) == Some(identity)
            }) {
                candidate.value = None;
            }
        }
    }
}

pub(super) fn napi_collect_weak_references(environment: &NapiEnvironment) {
    let reference_ids = environment
        .references
        .borrow()
        .keys()
        .copied()
        .collect::<Vec<_>>();
    let mut references = environment.references.borrow_mut();
    for reference_id in reference_ids {
        napi_collect_weak_reference(environment, &mut references, reference_id);
    }
}

pub(super) fn finalize_environment_wraps(environment: &Rc<NapiEnvironment>) {
    if let Some(instance_data) = *environment.instance_data.borrow()
        && let Some(finalize) = instance_data.finalize
    {
        let scope = environment.handles.borrow_mut().open_scope().ok();
        unsafe { finalize(environment.raw(), instance_data.data, instance_data.hint) };
        environment.pending_exception.borrow_mut().take();
        if let Some(scope) = scope {
            let _ = environment.handles.borrow_mut().close_scope(scope);
        }
    }
    environment.instance_data.borrow_mut().take();

    let finalizers = std::mem::take(&mut *environment.added_finalizers.borrow_mut());
    for finalizer in finalizers {
        let scope = environment.handles.borrow_mut().open_scope().ok();
        unsafe { (finalizer.finalize)(environment.raw(), finalizer.data, finalizer.hint) };
        environment.pending_exception.borrow_mut().take();
        if let Some(scope) = scope {
            let _ = environment.handles.borrow_mut().close_scope(scope);
        }
        if let Some(reference) = finalizer.reference {
            environment.references.borrow_mut().remove(&reference);
        }
    }

    let wraps = std::mem::take(&mut *environment.wraps.borrow_mut());
    for wrap in wraps.into_values() {
        let Some(finalize) = wrap.finalize else {
            continue;
        };
        let scope = environment.handles.borrow_mut().open_scope().ok();
        unsafe { finalize(environment.raw(), wrap.data, wrap.hint) };
        environment.pending_exception.borrow_mut().take();
        if let Some(scope) = scope {
            let _ = environment.handles.borrow_mut().close_scope(scope);
        }
    }

    let externals = std::mem::take(&mut *environment.externals.borrow_mut());
    for external in externals.into_values() {
        let Some(finalize) = external.finalize else {
            continue;
        };
        let scope = environment.handles.borrow_mut().open_scope().ok();
        unsafe { finalize(environment.raw(), external.data, external.hint) };
        environment.pending_exception.borrow_mut().take();
        if let Some(scope) = scope {
            let _ = environment.handles.borrow_mut().close_scope(scope);
        }
    }

    let external_buffers = std::mem::take(&mut *environment.external_buffers.borrow_mut());
    for external in external_buffers.into_values() {
        match external.finalize {
            NapiExternalBufferFinalizer::Napi(Some(finalize)) => {
                let scope = environment.handles.borrow_mut().open_scope().ok();
                unsafe { finalize(environment.raw(), external.data, external.hint) };
                environment.pending_exception.borrow_mut().take();
                if let Some(scope) = scope {
                    let _ = environment.handles.borrow_mut().close_scope(scope);
                }
            }
            NapiExternalBufferFinalizer::NoEnv(Some(finalize)) => unsafe {
                finalize(external.data, external.hint)
            },
            NapiExternalBufferFinalizer::Napi(None) | NapiExternalBufferFinalizer::NoEnv(None) => {}
        }
    }
}

pub(super) enum NapiEnvironmentCleanupHook {
    Sync(NapiCleanupHookRecord),
    Async(NapiAsyncCleanupHookRecord),
}

pub(super) fn take_next_environment_cleanup_hook(
    environment: &NapiEnvironment,
) -> Option<NapiEnvironmentCleanupHook> {
    let sync_order = environment
        .cleanup_hooks
        .borrow()
        .last()
        .map(|hook| hook.order);
    let async_order = environment
        .async_cleanup_hooks
        .borrow()
        .last()
        .map(|hook| hook.order);
    match (sync_order, async_order) {
        (Some(sync), Some(asynchronous)) if sync > asynchronous => environment
            .cleanup_hooks
            .borrow_mut()
            .pop()
            .map(NapiEnvironmentCleanupHook::Sync),
        (Some(_), None) => environment
            .cleanup_hooks
            .borrow_mut()
            .pop()
            .map(NapiEnvironmentCleanupHook::Sync),
        (_, Some(_)) => environment
            .async_cleanup_hooks
            .borrow_mut()
            .pop()
            .map(NapiEnvironmentCleanupHook::Async),
        (None, None) => None,
    }
}

pub(super) fn run_environment_cleanup_hooks(environment: &Rc<NapiEnvironment>) {
    let mut pending_async_hooks = Vec::new();
    while let Some(hook) = take_next_environment_cleanup_hook(environment) {
        match hook {
            NapiEnvironmentCleanupHook::Sync(hook) => {
                let scope = environment.handles.borrow_mut().open_scope().ok();
                unsafe { (hook.function)(hook.argument as *mut c_void) };
                environment.pending_exception.borrow_mut().take();
                if let Some(scope) = scope {
                    let _ = environment.handles.borrow_mut().close_scope(scope);
                }
            }
            NapiEnvironmentCleanupHook::Async(hook) => {
                let should_run = if let Ok(mut phase) = hook.control.phase.lock() {
                    if *phase == AsyncCleanupHookPhase::Registered {
                        *phase = AsyncCleanupHookPhase::Running;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if should_run {
                    // Start every registered async hook in LIFO order before
                    // waiting. Like Node, synchronous hooks later in the
                    // sequence can therefore run while an earlier async hook
                    // is still cleaning up.
                    unsafe {
                        (hook.function)(
                            hook.handle as NapiAsyncCleanupHookHandle,
                            hook.argument as *mut c_void,
                        )
                    };
                    pending_async_hooks.push(hook.control);
                }
            }
        }
    }

    for control in pending_async_hooks {
        if let Ok(mut phase) = control.phase.lock() {
            while *phase == AsyncCleanupHookPhase::Running {
                match control.completed.wait(phase) {
                    Ok(next) => phase = next,
                    Err(_) => break,
                }
            }
        }
    }
}

pub(super) fn register_environment(environment: &Rc<NapiEnvironment>) {
    let _ = NAPI_ENVIRONMENTS.try_with(|environments| {
        environments
            .borrow_mut()
            .insert(environment.raw() as usize, Rc::downgrade(environment));
    });
    if let Some(owner) = environment.owner.upgrade()
        && let Ok(mut senders) = post_finalizer_senders().lock()
    {
        senders.insert(
            environment.raw() as usize,
            owner.borrow().runtime_notification_sender.clone(),
        );
    }
}

pub(super) fn close_post_finalizer_senders(environments: &[Rc<NapiEnvironment>]) {
    if let Ok(mut senders) = post_finalizer_senders().lock() {
        for environment in environments {
            senders.remove(&(environment.raw() as usize));
        }
    }
}
