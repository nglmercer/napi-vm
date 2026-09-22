use crate::error::VmErr;
use crate::value::Value;

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

    /// Block the current (VM) thread until the async host call identified by
    /// `pending_id` resolves. Called by the interpreter when `await`
    /// encounters a `Value::HostPending`. The default implementation is
    /// unreachable (only called when `is_async_fn` returned true).
    fn await_host(&self, _pending_id: usize) -> Result<Value, VmErr> {
        Err(VmErr::Msg("async host call not supported".to_string()))
    }
}
