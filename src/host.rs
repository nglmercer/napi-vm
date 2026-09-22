use crate::error::VmErr;
use crate::value::{PromiseInner, PromiseState, Value};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// A guest callback requested by the host runtime, ready for an event-loop
/// checkpoint on the interpreter thread.
pub struct HostCallback {
    pub callback: Value,
    pub this_value: Value,
    pub args: Vec<Value>,
}

/// An event delivered from the host into the VM's shared event loop.
pub enum HostEvent {
    Callback(HostCallback),
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

    /// Construct a host function with `new`. The default preserves legacy
    /// bridges; runtimes that expose constructors can implement actual host
    /// construction semantics.
    fn construct_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        self.call_host(id, args)
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
