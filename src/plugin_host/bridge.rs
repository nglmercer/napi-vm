//! Host bridge that dispatches guest calls to registered Rust functions.

use super::*;
use crate::host::HostCallback;

pub(super) type HostFunction = Rc<dyn Fn(Vec<Value>) -> Result<Value, VmErr>>;

/// A plugin host callback with interpreter access, for capability exports
/// that convert values or call back into the guest.
pub(super) type InterpHostFunction =
    Rc<dyn Fn(&mut Interpreter, Vec<Value>) -> Result<Value, VmErr>>;

#[derive(Default)]
pub(super) struct PluginHostBridge {
    next_id: Cell<usize>,
    pub(super) functions: RefCell<HashMap<usize, HostFunction>>,
    pub(super) interp_functions: RefCell<HashMap<usize, InterpHostFunction>>,
}

impl PluginHostBridge {
    fn register(&self, function: HostFunction) -> Result<usize, PluginHostError> {
        let next = self
            .next_id
            .get()
            .checked_add(1)
            .ok_or_else(|| PluginHostError::Load("too many plugin host functions".into()))?;
        if next >= PLUGIN_FUNCTION_TAG {
            return Err(PluginHostError::Load(
                "too many plugin host functions".into(),
            ));
        }
        let id = PLUGIN_FUNCTION_TAG | next;
        self.next_id.set(next);
        self.functions.borrow_mut().insert(id, function);
        Ok(id)
    }

    fn register_interp(&self, function: InterpHostFunction) -> Result<usize, PluginHostError> {
        let next = self
            .next_id
            .get()
            .checked_add(1)
            .ok_or_else(|| PluginHostError::Load("too many plugin host functions".into()))?;
        if next >= PLUGIN_FUNCTION_TAG {
            return Err(PluginHostError::Load(
                "too many plugin host functions".into(),
            ));
        }
        let id = PLUGIN_FUNCTION_TAG | next;
        self.next_id.set(next);
        self.interp_functions.borrow_mut().insert(id, function);
        Ok(id)
    }

    #[cfg(all(
        feature = "node-api-host",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    fn has_tag(id: usize) -> bool {
        id & PLUGIN_FUNCTION_TAG != 0
    }
}

impl HostBridge for PluginHostBridge {
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        let function = self
            .functions
            .borrow()
            .get(&id)
            .cloned()
            .ok_or_else(|| VmErr::Msg(format!("unknown plugin host function id {id}")))?;
        function(args)
    }

    fn call_host_with_interp(
        &self,
        id: usize,
        _this_value: Value,
        args: Vec<Value>,
        _handler: &mut dyn FnMut(&mut Interpreter, HostCallback) -> Result<Value, VmErr>,
        interp: &mut Interpreter,
    ) -> Result<Value, VmErr> {
        if let Some(function) = self.interp_functions.borrow().get(&id).cloned() {
            return function(interp, args);
        }
        let function = self
            .functions
            .borrow()
            .get(&id)
            .cloned()
            .ok_or_else(|| VmErr::Msg(format!("unknown plugin host function id {id}")))?;
        function(args)
    }
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub(super) struct CompositeHostBridge {
    pub(super) plugin: Rc<PluginHostBridge>,
    pub(super) native: Rc<dyn HostBridge>,
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
impl HostBridge for CompositeHostBridge {
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.call_host(id, args)
        } else {
            self.native.call_host(id, args)
        }
    }

    fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        self.native.poll_host_events(timeout)
    }

    fn set_wake_notifier(&self, notifier: WakeNotifier) {
        self.plugin.set_wake_notifier(notifier.clone());
        self.native.set_wake_notifier(notifier);
    }

    fn call_host_with_interp(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        handler: &mut dyn FnMut(&mut Interpreter, HostCallback) -> Result<Value, VmErr>,
        interp: &mut Interpreter,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin
                .call_host_with_interp(id, this_value, args, handler, interp)
        } else {
            self.native
                .call_host_with_interp(id, this_value, args, handler, interp)
        }
    }

    fn has_pending_host_work(&self, promise: &Rc<RefCell<PromiseInner>>) -> bool {
        self.native.has_pending_host_work(promise)
    }

    fn call_host_with_this(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.call_host_with_this(id, this_value, args)
        } else {
            self.native.call_host_with_this(id, this_value, args)
        }
    }

    fn call_host_with_callback_handler(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin
                .call_host_with_callback_handler(id, this_value, args, callback_handler)
        } else {
            self.native
                .call_host_with_callback_handler(id, this_value, args, callback_handler)
        }
    }

    fn construct_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.construct_host(id, args)
        } else {
            self.native.construct_host(id, args)
        }
    }

    fn construct_host_with_callback_handler(
        &self,
        id: usize,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin
                .construct_host_with_callback_handler(id, args, callback_handler)
        } else {
            self.native
                .construct_host_with_callback_handler(id, args, callback_handler)
        }
    }

    fn construct_host_with_callback_handler_and_target(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        new_target: Value,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.construct_host_with_callback_handler_and_target(
                id,
                this_value,
                args,
                new_target,
                callback_handler,
            )
        } else {
            self.native.construct_host_with_callback_handler_and_target(
                id,
                this_value,
                args,
                new_target,
                callback_handler,
            )
        }
    }

    fn call_host_constructor_with_callback_handler_and_target(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        new_target: Value,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin
                .call_host_constructor_with_callback_handler_and_target(
                    id,
                    this_value,
                    args,
                    new_target,
                    callback_handler,
                )
        } else {
            self.native
                .call_host_constructor_with_callback_handler_and_target(
                    id,
                    this_value,
                    args,
                    new_target,
                    callback_handler,
                )
        }
    }

    fn is_async_fn(&self, id: usize) -> bool {
        !PluginHostBridge::has_tag(id) && self.native.is_async_fn(id)
    }

    fn call_host_async(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.call_host_async(id, args)
        } else {
            self.native.call_host_async(id, args)
        }
    }

    fn call_host_async_with_this(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        if PluginHostBridge::has_tag(id) {
            self.plugin.call_host_async_with_this(id, this_value, args)
        } else {
            self.native.call_host_async_with_this(id, this_value, args)
        }
    }

    fn await_host(&self, pending_id: usize) -> Result<Value, VmErr> {
        self.native.await_host(pending_id)
    }
}

#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub(super) fn native_host_bridge(runtime: &NativeAddonRuntime) -> Rc<dyn HostBridge> {
    match runtime {
        NativeAddonRuntime::RustNodeApi(host) => host.clone(),
        NativeAddonRuntime::NodeSidecar(host) => host.clone(),
    }
}

pub(super) fn expose_plugin_function(
    interpreter: &mut Interpreter,
    bridge: &PluginHostBridge,
    name: &str,
    function: impl Fn(Vec<Value>) -> Result<Value, VmErr> + 'static,
) -> Result<(), PluginHostError> {
    let id = bridge.register(Rc::new(function))?;
    interpreter
        .global
        .borrow_mut()
        .set(name, Value::host_function(name, id));
    Ok(())
}

pub(super) fn expose_interp_plugin_function(
    interpreter: &mut Interpreter,
    bridge: &PluginHostBridge,
    name: &str,
    function: impl Fn(&mut Interpreter, Vec<Value>) -> Result<Value, VmErr> + 'static,
) -> Result<(), PluginHostError> {
    let id = bridge.register_interp(Rc::new(function))?;
    interpreter
        .global
        .borrow_mut()
        .set(name, Value::host_function(name, id));
    Ok(())
}
