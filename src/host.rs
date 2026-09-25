use crate::error::VmErr;
use crate::interpreter::Interpreter;
use crate::value::{PromiseInner, PromiseState, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A guest callback requested by the host runtime, ready for an event-loop
/// checkpoint on the interpreter thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostCallbackKind {
    /// Invoke the callback with `this_value` and `args`.
    #[default]
    Call,
    /// Invoke via Node-API `napi_make_callback`, then drain the VM's
    /// microtasks before returning to native code.
    MakeCallback,
    /// Construct the callback using `args`, ignoring `this_value`.
    Construct,
}

pub struct HostCallback {
    pub callback: Value,
    pub this_value: Value,
    pub args: Vec<Value>,
    pub kind: HostCallbackKind,
}

/// Thread-safe wake signal a bridge fires whenever host-originated work
/// arrives from another thread (native callbacks, async completions,
/// finalizers). The VM owner registers a notifier that wakes its command
/// wait, so idle owners sleep instead of polling; the owner still drains
/// through [`HostBridge::poll_host_events`] on the interpreter thread.
pub type WakeNotifier = Arc<dyn Fn() + Send + Sync>;

/// Shared slot holding one owner's wake notifier. Bridges hand an `Arc`
/// of this to every native-thread ingress path (thread-safe functions,
/// async-work completions, finalizer posts, sidecar readers) so a
/// notifier registered after those paths were created still takes effect.
#[derive(Default)]
pub struct WakeSlot {
    notifier: Mutex<Option<WakeNotifier>>,
}

impl WakeSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the registered notifier. Best-effort under lock poisoning:
    /// a poisoned slot keeps waking (or not) as before rather than
    /// panicking a native thread.
    pub fn set(&self, notifier: WakeNotifier) {
        if let Ok(mut slot) = self.notifier.lock() {
            *slot = Some(notifier);
        }
    }

    /// Invoke the registered notifier, if any. The notifier runs outside
    /// the slot lock and must never enter guest code; it only wakes the
    /// interpreter thread, which drains through the normal event loop.
    pub fn fire(&self) {
        let notifier = self.notifier.lock().ok().and_then(|slot| slot.clone());
        if let Some(notifier) = notifier {
            notifier();
        }
    }
}

/// An event delivered from the host into the VM's shared event loop.
pub enum HostEvent {
    Callback(HostCallback),
    /// An exception raised by a host runtime with no synchronous caller to
    /// receive it. The interpreter delivers it as an uncaught event.
    UncaughtException(Value),
    PromiseSettled {
        promise: Rc<RefCell<PromiseInner>>,
        state: PromiseState,
        value: Value,
    },
}

/// Bridge that lets the VM call functions owned by its host runtime.
///
/// The interpreter is single-threaded (`Rc`/`RefCell`, not `Send`/`Sync`), so
/// the bridge is stored as a plain `Rc<dyn HostBridge>` and invoked on the same
/// thread that drives the VM. Implementations marshal `Value`s into their
/// host representation and invoke the registered function synchronously.
pub trait HostBridge {
    /// Invoke the host function registered under `id` with `args`, returning
    /// the marshalled result back into the VM.
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr>;

    /// Poll host-originated events. Implementations must enqueue work here
    /// instead of entering guest code from a host or native thread.
    fn poll_host_events(&self, _timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        Ok(Vec::new())
    }

    /// Register a wake notifier the bridge fires (from any thread) when
    /// host-originated work arrives, so the VM owner can sleep instead of
    /// polling. Bridges without threaded ingress keep the default no-op.
    fn set_wake_notifier(&self, _notifier: WakeNotifier) {}

    /// Whether an awaited promise still depends on an external host event.
    /// Synchronous top-level `await` uses this to pump only the work needed
    /// for that promise chain.
    fn has_pending_host_work(&self, _promise: &Rc<RefCell<PromiseInner>>) -> bool {
        false
    }

    /// Invoke a host function with the guest receiver from a property call.
    /// Bridges that do not model `this` retain the older `call_host` behavior.
    fn call_host_with_this(
        &self,
        id: usize,
        _this_value: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        self.call_host(id, args)
    }

    /// Invoke a host function while allowing it to synchronously request a
    /// guest callback. The callback handler runs on the interpreter thread,
    /// on the paused host-call stack; it must never be called from a native
    /// or transport thread.
    fn call_host_with_callback_handler(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        _callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.call_host_with_this(id, this_value, args)
    }

    /// Invoke a host function with interpreter access for callbacks that
    /// need it (value conversion, guest calls). The handler takes the
    /// interpreter as a parameter instead of capturing it, so the call
    /// site lends `&mut` exactly once. The default ignores the
    /// interpreter and delegates to
    /// [`Self::call_host_with_callback_handler`]; bridges whose callbacks
    /// need the interpreter override this.
    fn call_host_with_interp(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        handler: &mut dyn FnMut(&mut Interpreter, HostCallback) -> Result<Value, VmErr>,
        interp: &mut Interpreter,
    ) -> Result<Value, VmErr> {
        self.call_host_with_callback_handler(id, this_value, args, &mut |callback| {
            handler(&mut *interp, callback)
        })
    }

    /// Construct a host function with `new`. The default preserves legacy
    /// bridges; runtimes that expose constructors can implement actual host
    /// construction semantics.
    fn construct_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        self.call_host(id, args)
    }

    /// Constructor counterpart to [`HostBridge::call_host_with_callback_handler`].
    fn construct_host_with_callback_handler(
        &self,
        id: usize,
        args: Vec<Value>,
        _callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.construct_host(id, args)
    }

    /// Construct a host function with an explicit receiver and `new.target`.
    /// Bridges that do not expose constructor callback metadata can retain
    /// their existing construction behavior through the default.
    fn construct_host_with_callback_handler_and_target(
        &self,
        id: usize,
        _this_value: Value,
        args: Vec<Value>,
        _new_target: Value,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.construct_host_with_callback_handler(id, args, callback_handler)
    }

    /// Invoke a host-backed superclass constructor against an already-created
    /// guest receiver. This differs from constructing a bare host function:
    /// bridges that model imported classes as host functions can preserve
    /// their existing call behavior, while native ABI bridges can attach the
    /// inherited `new.target` to the callback frame.
    fn call_host_constructor_with_callback_handler_and_target(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        _new_target: Value,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.call_host_with_callback_handler(id, this_value, args, callback_handler)
    }

    /// Whether the function registered under `id` is async (registered via
    /// `exposeAsyncFunction`). Async functions return `HostPending` when
    /// called, and the interpreter parks at `await` until resolved.
    fn is_async_fn(&self, _id: usize) -> bool {
        false
    }

    /// Dispatch an async host function call. Returns `Value::HostPending`
    /// with a unique pending-id that the interpreter uses at `await` to
    /// block until the host resolves the operation.
    fn call_host_async(&self, _id: usize, _args: Vec<Value>) -> Result<Value, VmErr> {
        Err(VmErr::Msg("async host calls not supported".to_string()))
    }

    /// Async counterpart to `call_host_with_this`.
    fn call_host_async_with_this(
        &self,
        id: usize,
        _this_value: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        self.call_host_async(id, args)
    }

    /// Block the current (VM) thread until the async host call identified by
    /// `pending_id` resolves. Called by the interpreter when `await`
    /// encounters a `Value::HostPending`. The default implementation is
    /// unreachable (only called when `is_async_fn` returned true).
    fn await_host(&self, _pending_id: usize) -> Result<Value, VmErr> {
        Err(VmErr::Msg("async host call not supported".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn wake_slot_fires_registered_notifier() {
        let slot = WakeSlot::new();
        slot.fire(); // No notifier: silent no-op, never panics.
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&calls);
        slot.set(Arc::new(move || {
            probe.fetch_add(1, Ordering::SeqCst);
        }));
        slot.fire();
        slot.fire();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // Replacing the notifier swaps delivery to the new target.
        let second = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&second);
        slot.set(Arc::new(move || {
            probe.fetch_add(1, Ordering::SeqCst);
        }));
        slot.fire();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(second.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn wake_notifier_is_thread_safe() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WakeNotifier>();
        assert_send_sync::<Arc<WakeSlot>>();
    }
}
