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

/// Level-triggered wake latch. Events live in their own queues; only wake
/// notifications coalesce. Signalling between an owner's scan and wait cannot
/// be lost because the latch is tested under the same mutex as the wait.
#[derive(Default)]
pub struct WakeSignal {
    pending: Mutex<bool>,
    #[cfg(test)]
    before_wait: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    ready: std::sync::Condvar,
    wakeups: std::sync::atomic::AtomicU64,
}
impl WakeSignal {
    pub fn fire(&self) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if !*pending {
            *pending = true;
            self.wakeups
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.ready.notify_one();
        }
    }
    pub fn wait(&self, timeout: Option<Duration>) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if !*pending {
            #[cfg(test)]
            if let Some(ready) = self.before_wait.lock().unwrap().take() {
                ready.send(()).unwrap();
            }
            pending = match timeout {
                Some(d) => {
                    self.ready
                        .wait_timeout_while(pending, d, |p| !*p)
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                }
                None => self
                    .ready
                    .wait_while(pending, |p| !*p)
                    .unwrap_or_else(|e| e.into_inner()),
            };
        }
        *pending = false;
    }
    pub fn wakeups(&self) -> u64 {
        self.wakeups.load(std::sync::atomic::Ordering::Relaxed)
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

/// How a host bridge makes external events available to the scheduler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostWaitMode {
    /// Function-only or nonblocking bridge; use the VM notification signal.
    Nonblocking,
    /// Legacy blocking polls without a notifier; use bounded timeout slices.
    BlockingPoll,
    /// Ingress fires the registered, latched wake notifier.
    Notifications,
}

/// Bridge that lets the VM call functions owned by its host runtime.
///
/// The interpreter is single-threaded (`Rc`/`RefCell`, not `Send`/`Sync`), so
/// the bridge is stored as a plain `Rc<dyn HostBridge>` and invoked on the same
/// thread that drives the VM. Implementations marshal `Value`s into their
/// host representation and invoke the registered function synchronously.
pub trait HostBridge {
    /// Values retained by a bridge outside interpreter/module/job roots.
    /// Custom callbacks that capture opaque guest values must pin them with
    /// `heap::RootPin`, or report them here before enabling collection.
    fn trace_roots(&self, _values: &mut Vec<Value>, _envs: &mut Vec<crate::interpreter::Env>) {}

    /// Invoke the host function registered under `id` with `args`, returning
    /// the marshalled result back into the VM.
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr>;

    /// Configure interruption of blocking host calls. Existing bridges may
    /// retain the default when they have no blocking operations.
    fn set_execution_context(&self, _token: crate::CancellationToken, _timeout: Option<Duration>) {}

    /// Poll host-originated events. Implementations must enqueue work here
    /// instead of entering guest code from a host or native thread.
    fn poll_host_events(&self, _timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        Ok(Vec::new())
    }

    /// True when poll_host_events honors blocking timeouts or wake signals.
    /// Function-only bridges keep the default; real-time timers use VM waits.
    fn supports_blocking_event_wait(&self) -> bool {
        false
    }

    /// Explicit wait capability. Older blocking bridges remain compatible.
    fn event_wait_mode(&self) -> HostWaitMode {
        if self.supports_blocking_event_wait() {
            HostWaitMode::BlockingPoll
        } else {
            HostWaitMode::Nonblocking
        }
    }

    /// Bounded ingress contract. Implementations should return at most `limit`
    /// events and retain excess work at the source, applying producer backpressure.
    /// The compatibility implementation delegates to the older API; the VM retains
    /// any oversized legacy batch and reports Backpressure rather than dropping it.
    fn poll_host_events_bounded(
        &self,
        timeout: Duration,
        _limit: usize,
    ) -> Result<Vec<HostEvent>, VmErr> {
        self.poll_host_events(timeout)
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

#[cfg(test)]
mod wake_tests {
    use super::*;
    #[test]
    fn notifications_coalesce_but_latched_wakeup_is_not_lost() {
        let signal = Arc::new(WakeSignal::default());
        signal.fire();
        signal.fire();
        assert_eq!(signal.wakeups(), 1);
        signal.wait(None);
        let s = signal.clone();
        let producer = std::thread::spawn(move || {
            s.fire();
        });
        producer.join().unwrap();
        signal.wait(None);
        assert_eq!(signal.wakeups(), 2);
    }
    #[test]
    fn cancellation_wakes_a_waiter_at_the_condvar_sleep_boundary() {
        let signal = Arc::new(WakeSignal::default());
        let token = crate::CancellationToken::default();
        token.register_wake(&signal);
        let (ready, observed) = std::sync::mpsc::channel();
        *signal.before_wait.lock().unwrap() = Some(ready);
        let waiter = signal.clone();
        let worker = std::thread::spawn(move || waiter.wait(None));
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        // The notification is sent with the condition mutex held. fire()
        // acquires it only once wait() atomically releases it for sleeping.
        token.cancel();
        worker.join().unwrap();
    }

    #[test]
    fn racing_producer_preserves_every_event() {
        let signal = Arc::new(WakeSignal::default());
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let s = signal.clone();
        let p = std::thread::spawn(move || {
            for i in 0..10000 {
                tx.send(i).unwrap();
                s.fire();
            }
        });
        let mut received = 0;
        while received < 10000 {
            while let Ok(i) = rx.try_recv() {
                assert_eq!(i, received);
                received += 1;
            }
            if received < 10000 {
                signal.wait(Some(Duration::from_secs(1)));
            }
        }
        p.join().unwrap();
    }
}
