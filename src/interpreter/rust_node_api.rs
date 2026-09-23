//! Experimental in-process host for the Node-API C ABI.
//!
//! The in-process host intentionally implements a selected Node-API surface.
//! Unimplemented imports fail during dynamic loading; this backend does not
//! emulate Node, V8, NAN, or libuv.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{CStr, c_char, c_void};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};

use crate::error::VmErr;
use crate::host::{HostBridge, HostCallback, HostCallbackKind, HostEvent};
use crate::interpreter::commonjs::NativeAddonLoader;
use crate::interpreter::{Env, FileCommonJsLoader, Interpreter};
use crate::value::{
    Buffer, ClassData, ErrorData, PromiseInner, PromiseState, PropAttrs, TypedArrayData, TypedKind,
    Value,
};

const NAPI_OK: i32 = 0;
const NAPI_INVALID_ARG: i32 = 1;
const NAPI_OBJECT_EXPECTED: i32 = 2;
const NAPI_STRING_EXPECTED: i32 = 3;
const NAPI_FUNCTION_EXPECTED: i32 = 5;
const NAPI_NUMBER_EXPECTED: i32 = 6;
const NAPI_BOOLEAN_EXPECTED: i32 = 7;
const NAPI_ARRAY_EXPECTED: i32 = 8;
const NAPI_GENERIC_FAILURE: i32 = 9;
const NAPI_PENDING_EXCEPTION: i32 = 10;
const NAPI_CANCELLED: i32 = 11;
const NAPI_ESCAPE_CALLED_TWICE: i32 = 12;
const NAPI_HANDLE_SCOPE_MISMATCH: i32 = 13;
const NAPI_QUEUE_FULL: i32 = 15;
const NAPI_CLOSING: i32 = 16;
const NAPI_ARRAYBUFFER_EXPECTED: i32 = 19;
const NAPI_PROPERTY_STATIC: i32 = 1 << 10;
const MAX_LOCAL_HANDLES: usize = 1_048_576;
const MAX_NAPI_BUFFER_BYTES: usize = crate::value::MAX_STRING_LEN;
const ASYNC_WORKER_COUNT: usize = 4;
const ASYNC_WORK_QUEUE_CAPACITY: usize = 128;
const ASYNC_WORK_CREATED: u8 = 0;
const ASYNC_WORK_QUEUED: u8 = 1;
const ASYNC_WORK_RUNNING: u8 = 2;
const ASYNC_WORK_FINISHED: u8 = 3;
const ASYNC_WORK_CANCELLED: u8 = 4;
const TSFN_RELEASE: i32 = 0;
const TSFN_ABORT: i32 = 1;
const TSFN_NONBLOCKING: i32 = 0;
const TSFN_BLOCKING: i32 = 1;
const MAX_NODE_API_VERSION: i32 = 4;
const UTF16_INPUT_ERROR_MESSAGE: &[u8] =
    b"UTF-16 input is malformed or exceeds napi-vm string limits\0";
static NEXT_OPAQUE_HANDLE_ID: AtomicUsize = AtomicUsize::new(1);

type NapiEnv = *mut c_void;
type NapiValue = *mut c_void;
type NapiCallbackInfo = *mut c_void;
type NapiHandleScope = *mut c_void;
type NapiRef = *mut c_void;
type NapiDeferred = *mut c_void;
type NapiAsyncWork = *mut c_void;
type NapiCallback = unsafe extern "C" fn(NapiEnv, NapiCallbackInfo) -> NapiValue;
type NapiFinalize = unsafe extern "C" fn(NapiEnv, *mut c_void, *mut c_void);
type NapiCleanupHook = unsafe extern "C" fn(*mut c_void);
type NapiAsyncExecuteCallback = unsafe extern "C" fn(NapiEnv, *mut c_void);
type NapiAsyncCompleteCallback = unsafe extern "C" fn(NapiEnv, i32, *mut c_void);
type NapiThreadsafeFunction = *mut c_void;
type NapiThreadsafeFunctionCallJs =
    unsafe extern "C" fn(NapiEnv, NapiValue, *mut c_void, *mut c_void);
type NapiGuestOperation = fn(&mut Interpreter, Value, Vec<Value>) -> Result<Value, VmErr>;

/// Filesystem and integrity policy for the experimental in-process backend.
///
/// Addons execute with the desktop process's native privileges and are not
/// sandboxed by guest capability policy. Every `.node` file must be explicitly
/// allowlisted, preferably with a digest from trusted application metadata.
#[derive(Clone, Debug)]
pub struct RustNodeApiOptions {
    roots: Vec<PathBuf>,
    allowed_addons: Vec<(PathBuf, Option<[u8; 32]>)>,
    entry: Option<PathBuf>,
}

impl RustNodeApiOptions {
    pub fn new<I, P>(roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        Self {
            roots: roots.into_iter().map(Into::into).collect(),
            allowed_addons: Vec::new(),
            entry: None,
        }
    }

    pub fn allow_native_addon(mut self, path: impl Into<PathBuf>) -> Self {
        self.allowed_addons.push((path.into(), None));
        self
    }

    pub fn allow_native_addon_with_sha256(
        mut self,
        path: impl Into<PathBuf>,
        expected_sha256: [u8; 32],
    ) -> Self {
        self.allowed_addons
            .push((path.into(), Some(expected_sha256)));
        self
    }

    pub fn entry(mut self, path: impl Into<PathBuf>) -> Self {
        self.entry = Some(path.into());
        self
    }
}

/// In-process Node-API addon host. This is opt-in and implements Linux ELF and
/// macOS Mach-O loading for the Node-API v1-v4 calls below. Linux is runtime
/// tested; macOS still needs native CI verification.
pub struct RustNodeApiHost {
    state: Rc<RefCell<HostState>>,
    _shim: Rc<NodeApiShim>,
}

impl Drop for RustNodeApiHost {
    fn drop(&mut self) {
        let (async_work_sender, workers, environments) = {
            let mut state = self.state.borrow_mut();
            let environments = state.environments.clone();
            for environment in &environments {
                for function in environment.threadsafe_functions.borrow().values() {
                    if let Ok(mut queue) = function.shared.state.lock() {
                        queue.closing = true;
                        function.shared.queue_space.notify_all();
                    }
                }
                for work in environment.async_works.borrow().values() {
                    let _ = work.state.compare_exchange(
                        ASYNC_WORK_QUEUED,
                        ASYNC_WORK_CANCELLED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
            }
            (
                state.async_work_sender.clone(),
                std::mem::take(&mut state.async_workers),
                environments,
            )
        };
        for _ in 0..workers.len() {
            let _ = async_work_sender.send(AsyncWorkTaskMessage::Stop);
        }
        for worker in workers {
            let _ = worker.join();
        }

        // If the host is dropped before the VM drains its event queue, deliver
        // completed callbacks once on the owner thread so addon work data can
        // still be released while its library is loaded. Guest re-entry is
        // unavailable during shutdown.
        for environment in &environments {
            let pending = environment
                .async_works
                .borrow()
                .iter()
                .filter_map(|(work_id, work)| {
                    let status = work.completion_status.load(Ordering::Acquire);
                    (status != u8::MAX && !work.callback_run).then_some((
                        *work_id,
                        work.clone(),
                        status as i32,
                    ))
                })
                .collect::<Vec<_>>();
            for (work_id, work, status) in pending {
                self.state.borrow_mut().callbacks.retain(|_, callback| {
                    !matches!(
                        &callback.callback,
                        NativeCallback::AsyncComplete {
                            work_id: callback_work_id,
                            ..
                        } if *callback_work_id == work_id
                    )
                });
                let Ok(Value::HostFunction { id, .. }) = create_native_async_complete_value(
                    environment,
                    work.complete,
                    status,
                    work.data,
                    work_id,
                ) else {
                    continue;
                };
                let _ = self.invoke_native(
                    id,
                    Value::Undefined,
                    Vec::new(),
                    None,
                    &mut reject_guest_callback,
                );
            }
        }

        // Node runs synchronous environment cleanup hooks before N-API
        // finalizers. Keep addon libraries loaded and the environment usable
        // while hooks release native resources or stop addon-owned threads.
        for environment in environments.iter().rev() {
            run_environment_cleanup_hooks(environment);
        }

        // Queue callbacks are reclaimed with a null env at shutdown. If a
        // native producer has not released its TSFN yet, keep the addon and
        // ABI shim mapped so its eventual closing/release calls stay valid.
        let active_threadsafe_workers = shutdown_threadsafe_functions(&self.state, &environments);
        if active_threadsafe_workers {
            let libraries = std::mem::take(&mut self.state.borrow_mut().libraries);
            std::mem::forget(libraries);
            std::mem::forget(self._shim.clone());
        }

        // Keep HostState strongly reachable while finalizers run so ordinary
        // Node-API calls made by a finalizer can still access the host. The
        // addon libraries and symbol shim remain loaded in HostState until
        // this callback pass is complete.
        for environment in &environments {
            environment.finalizing.set(true);
        }
        for environment in &environments {
            finalize_environment_wraps(environment);
        }
    }
}

struct HostState {
    global: Env,
    object_prototype: Option<Value>,
    next_callback_id: usize,
    callbacks: HashMap<usize, NativeCallbackRecord>,
    environments: Vec<Rc<NapiEnvironment>>,
    libraries: Vec<Library>,
    async_work_sender: SyncSender<AsyncWorkTaskMessage>,
    runtime_notifications: Receiver<HostRuntimeNotification>,
    runtime_notification_sender: Sender<HostRuntimeNotification>,
    async_workers: Vec<JoinHandle<()>>,
    // Keep the process-global ABI shim loaded until every addon library closes.
    _shim: Rc<NodeApiShim>,
}

#[derive(Clone)]
struct NativeCallbackRecord {
    env: Rc<NapiEnvironment>,
    callback: NativeCallback,
    data: *mut c_void,
    one_shot: bool,
}

#[derive(Clone)]
enum NativeCallback {
    Function(NapiCallback),
    AsyncComplete {
        callback: NapiAsyncCompleteCallback,
        status: i32,
        work_id: usize,
    },
    ThreadsafeFunctionCall {
        callback: Option<Value>,
        call_js: Option<NapiThreadsafeFunctionCallJs>,
        context: *mut c_void,
        shared: Arc<NapiThreadsafeFunctionShared>,
    },
}

struct NapiEnvironment {
    module_path: String,
    owner: Weak<RefCell<HostState>>,
    handles: RefCell<NapiHandleArena>,
    references: RefCell<HashMap<usize, NapiReference>>,
    deferreds: RefCell<HashMap<usize, NapiDeferredState>>,
    async_works: RefCell<HashMap<usize, NapiAsyncWorkState>>,
    threadsafe_functions: RefCell<HashMap<usize, NapiThreadsafeFunctionState>>,
    cleanup_hooks: RefCell<Vec<NapiCleanupHookRecord>>,
    last_error: Cell<NapiExtendedErrorInfo>,
    wraps: RefCell<HashMap<NapiObjectIdentity, NapiWrap>>,
    externals: RefCell<HashMap<NapiObjectIdentity, NapiExternal>>,
    external_memory: Cell<i64>,
    buffer_values: RefCell<HashMap<NapiObjectIdentity, Weak<TypedArrayData>>>,
    finalizing: Cell<bool>,
    active_callbacks: RefCell<HashMap<usize, CallbackFrame>>,
    guest_callback_dispatchers: RefCell<Vec<GuestCallbackDispatcher>>,
    pending_exception: RefCell<Option<Value>>,
}

#[derive(Clone, Copy)]
struct NapiCleanupHookRecord {
    function: NapiCleanupHook,
    function_address: usize,
    argument: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NapiExtendedErrorInfo {
    error_message: *const c_char,
    engine_reserved: *mut c_void,
    engine_error_code: u32,
    error_code: i32,
}

#[derive(Clone, Copy)]
struct GuestCallbackDispatcher {
    context: *mut c_void,
    invoke: unsafe fn(*mut c_void, HostCallback) -> Result<Value, VmErr>,
}

/// The dispatcher is pushed only while a native callback is running. Its
/// context points at that callback's borrowed interpreter handler and is
/// removed before the handler's stack frame returns.
struct GuestCallbackDispatcherScope {
    environment: Rc<NapiEnvironment>,
}

impl GuestCallbackDispatcherScope {
    fn push(environment: Rc<NapiEnvironment>, dispatcher: GuestCallbackDispatcher) -> Self {
        environment
            .guest_callback_dispatchers
            .borrow_mut()
            .push(dispatcher);
        Self { environment }
    }
}

impl Drop for GuestCallbackDispatcherScope {
    fn drop(&mut self) {
        self.environment
            .guest_callback_dispatchers
            .borrow_mut()
            .pop();
    }
}

struct NapiReference {
    value: Value,
    ref_count: u32,
}

struct NapiDeferredState {
    promise: Rc<RefCell<PromiseInner>>,
    settling: bool,
}

#[derive(Clone)]
struct NapiAsyncWorkState {
    execute: NapiAsyncExecuteCallback,
    complete: NapiAsyncCompleteCallback,
    data: *mut c_void,
    state: Arc<AtomicU8>,
    completion_status: Arc<AtomicU8>,
    completion_callback_active: bool,
    callback_run: bool,
}

struct NapiThreadsafeFunctionState {
    shared: Arc<NapiThreadsafeFunctionShared>,
    callback: Option<Value>,
    call_js: Option<NapiThreadsafeFunctionCallJs>,
    context: *mut c_void,
    finalize_data: *mut c_void,
    finalize: Option<NapiFinalize>,
    referenced: bool,
}

struct NapiThreadsafeFunctionShared {
    id: usize,
    environment: usize,
    context: usize,
    max_queue_size: usize,
    owner_thread: thread::ThreadId,
    notifications: Sender<HostRuntimeNotification>,
    state: Mutex<NapiThreadsafeFunctionQueue>,
    queue_space: Condvar,
}

struct NapiThreadsafeFunctionQueue {
    values: VecDeque<usize>,
    thread_count: usize,
    in_flight: usize,
    closing: bool,
    orphaned: bool,
    finalized: bool,
}

static THREADSAFE_FUNCTIONS: OnceLock<Mutex<HashMap<usize, Arc<NapiThreadsafeFunctionShared>>>> =
    OnceLock::new();

enum HostRuntimeNotification {
    AsyncWorkCompletion(AsyncWorkCompletion),
    ThreadsafeFunction(usize),
}

struct AsyncWorkTask {
    work_id: usize,
    environment: usize,
    execute: NapiAsyncExecuteCallback,
    data: usize,
    state: Arc<AtomicU8>,
    completion_status: Arc<AtomicU8>,
}

enum AsyncWorkTaskMessage {
    Run(AsyncWorkTask),
    Stop,
}

#[derive(Clone, Copy)]
struct AsyncWorkCompletion {
    work_id: usize,
    status: i32,
}

type AsyncWorkPool = (SyncSender<AsyncWorkTaskMessage>, Vec<JoinHandle<()>>);

struct NapiWrap {
    // Keeping the guest value alive prevents its identity pointer from being
    // reused while native data is still attached to it.
    _value: Value,
    data: *mut c_void,
    finalize: Option<NapiFinalize>,
    hint: *mut c_void,
}

struct NapiExternal {
    // The Node-API external has no own properties or prototype, but it must
    // stay alive until its finalizer runs during environment shutdown.
    _value: Value,
    data: *mut c_void,
    finalize: Option<NapiFinalize>,
    hint: *mut c_void,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum NapiObjectIdentity {
    Global,
    Object(usize),
    Array(usize),
    Function(usize),
    NativeFunction(usize),
    HostFunction(usize),
    Class(usize),
    Promise(usize),
    Generator(usize),
    StringIterator(usize),
    Date(usize),
    Proxy(usize),
    ArrayBuffer(usize),
    TypedArray(usize),
    DataView(usize),
    RegExp(usize),
}

thread_local! {
    /// Only environments created on this thread may enter the synchronous
    /// Node-API surface. Looking up the opaque pointer before dereferencing it
    /// makes invalid `napi_env` values fail with `napi_invalid_arg` instead of
    /// causing undefined behavior in the host.
    static NAPI_ENVIRONMENTS: RefCell<HashMap<usize, Weak<NapiEnvironment>>> =
        RefCell::new(HashMap::new());
}

#[derive(Clone)]
struct CallbackFrame {
    args: Vec<NapiValue>,
    this_arg: NapiValue,
    new_target: NapiValue,
    data: *mut c_void,
}

#[derive(Clone, Copy)]
struct HandleRef {
    slot: usize,
    generation: u64,
}

struct HandleSlot {
    generation: u64,
    value: Option<Value>,
}

struct HandleScope {
    id: u64,
    slots: Vec<usize>,
    escapable: bool,
    escape_used: bool,
}

struct NapiHandleArena {
    slots: Vec<HandleSlot>,
    free_slots: Vec<usize>,
    scopes: Vec<HandleScope>,
    next_scope_id: u64,
    handles: HashMap<usize, HandleRef>,
    scope_handles: HashMap<usize, (u64, bool)>,
}

impl Default for NapiHandleArena {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free_slots: Vec::new(),
            scopes: vec![HandleScope {
                id: 0,
                slots: Vec::new(),
                escapable: false,
                escape_used: false,
            }],
            next_scope_id: 1,
            handles: HashMap::new(),
            scope_handles: HashMap::new(),
        }
    }
}

impl NapiHandleArena {
    fn create(&mut self, value: Value) -> Result<NapiValue, i32> {
        self.create_in_scope(value, self.scopes.len() - 1)
    }

    fn create_in_scope(&mut self, value: Value, scope_index: usize) -> Result<NapiValue, i32> {
        if self.handles.len() >= MAX_LOCAL_HANDLES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        if scope_index >= self.scopes.len() {
            return Err(NAPI_HANDLE_SCOPE_MISMATCH);
        }
        let pointer = new_opaque_handle()?;
        let slot_index = if let Some(slot_index) = self.free_slots.pop() {
            slot_index
        } else {
            self.slots.push(HandleSlot {
                generation: 1,
                value: None,
            });
            self.slots.len() - 1
        };
        let slot = &mut self.slots[slot_index];
        debug_assert!(slot.value.is_none());
        slot.value = Some(value);
        let generation = slot.generation;
        self.scopes[scope_index].slots.push(slot_index);

        self.handles.insert(
            pointer as usize,
            HandleRef {
                slot: slot_index,
                generation,
            },
        );
        Ok(pointer)
    }

    fn get(&self, pointer: NapiValue) -> Result<Value, i32> {
        let handle = self
            .handles
            .get(&(pointer as usize))
            .ok_or(NAPI_INVALID_ARG)?;
        let slot = self.slots.get(handle.slot).ok_or(NAPI_INVALID_ARG)?;
        if slot.generation != handle.generation {
            return Err(NAPI_INVALID_ARG);
        }
        slot.value.clone().ok_or(NAPI_INVALID_ARG)
    }

    fn open_scope(&mut self) -> Result<u64, i32> {
        let id = self.next_scope_id;
        self.next_scope_id = self
            .next_scope_id
            .checked_add(1)
            .ok_or(NAPI_GENERIC_FAILURE)?;
        self.scopes.push(HandleScope {
            id,
            slots: Vec::new(),
            escapable: false,
            escape_used: false,
        });
        Ok(id)
    }

    fn open_escapable_scope(&mut self) -> Result<u64, i32> {
        let id = self.open_scope()?;
        self.scopes
            .last_mut()
            .expect("scope was just opened")
            .escapable = true;
        Ok(id)
    }

    fn create_scope_handle(&mut self, id: u64, escapable: bool) -> Result<NapiHandleScope, i32> {
        if self.scope_handles.len() >= MAX_LOCAL_HANDLES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let pointer = new_opaque_handle()?;
        self.scope_handles.insert(pointer as usize, (id, escapable));
        Ok(pointer)
    }

    fn close_scope_handle(&mut self, pointer: NapiHandleScope) -> Result<(), i32> {
        self.close_scope_handle_with_kind(pointer, false)
    }

    fn close_escapable_scope_handle(&mut self, pointer: NapiHandleScope) -> Result<(), i32> {
        self.close_scope_handle_with_kind(pointer, true)
    }

    fn close_scope_handle_with_kind(
        &mut self,
        pointer: NapiHandleScope,
        escapable: bool,
    ) -> Result<(), i32> {
        let (id, actual_escapable) = *self
            .scope_handles
            .get(&(pointer as usize))
            .ok_or(NAPI_INVALID_ARG)?;
        if actual_escapable != escapable {
            return Err(NAPI_HANDLE_SCOPE_MISMATCH);
        }
        self.close_scope(id)?;
        self.scope_handles.remove(&(pointer as usize));
        Ok(())
    }

    fn escape_handle(
        &mut self,
        scope_pointer: NapiHandleScope,
        escapee: NapiValue,
    ) -> Result<NapiValue, i32> {
        let (id, escapable) = *self
            .scope_handles
            .get(&(scope_pointer as usize))
            .ok_or(NAPI_INVALID_ARG)?;
        if !escapable || self.scopes.len() <= 1 {
            return Err(NAPI_HANDLE_SCOPE_MISMATCH);
        }
        let top_index = self.scopes.len() - 1;
        let scope = &self.scopes[top_index];
        if scope.id != id || !scope.escapable {
            return Err(NAPI_HANDLE_SCOPE_MISMATCH);
        }
        if scope.escape_used {
            return Err(NAPI_ESCAPE_CALLED_TWICE);
        }
        let handle = self
            .handles
            .get(&(escapee as usize))
            .ok_or(NAPI_INVALID_ARG)?;
        if !scope.slots.contains(&handle.slot) {
            return Err(NAPI_HANDLE_SCOPE_MISMATCH);
        }
        let value = self.get(escapee)?;
        let escaped = self.create_in_scope(value, top_index - 1)?;
        self.scopes[top_index].escape_used = true;
        Ok(escaped)
    }

    fn close_scope(&mut self, id: u64) -> Result<(), i32> {
        if self.scopes.len() <= 1 || self.scopes.last().map(|scope| scope.id) != Some(id) {
            return Err(NAPI_INVALID_ARG);
        }
        let scope = self.scopes.pop().expect("validated handle scope");
        for slot_index in scope.slots {
            let generation = self.slots[slot_index].generation;
            self.handles
                .retain(|_, handle| handle.slot != slot_index || handle.generation != generation);
            let slot = &mut self.slots[slot_index];
            slot.value = None;
            if let Some(next_generation) = slot.generation.checked_add(1) {
                slot.generation = next_generation;
                self.free_slots.push(slot_index);
            }
        }
        Ok(())
    }
}

/// Node-API values and scope tokens are opaque. Integer-backed, process-wide
/// tokens avoid retaining a heap allocation for every short-lived callback
/// handle while remaining unique across environments and after scope closure.
fn new_opaque_handle() -> Result<*mut c_void, i32> {
    NEXT_OPAQUE_HANDLE_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map(|id| id as *mut c_void)
        .map_err(|_| NAPI_GENERIC_FAILURE)
}

impl NapiEnvironment {
    fn raw(&self) -> NapiEnv {
        (self as *const Self).cast_mut().cast()
    }
}

#[repr(C)]
struct NapiVmApiTable {
    get_undefined: unsafe extern "C" fn(NapiEnv, *mut NapiValue) -> i32,
    get_global: unsafe extern "C" fn(NapiEnv, *mut NapiValue) -> i32,
    get_null: unsafe extern "C" fn(NapiEnv, *mut NapiValue) -> i32,
    get_boolean: unsafe extern "C" fn(NapiEnv, bool, *mut NapiValue) -> i32,
    coerce_to_bool: unsafe extern "C" fn(NapiEnv, NapiValue, *mut NapiValue) -> i32,
    coerce_to_number: unsafe extern "C" fn(NapiEnv, NapiValue, *mut NapiValue) -> i32,
    coerce_to_string: unsafe extern "C" fn(NapiEnv, NapiValue, *mut NapiValue) -> i32,
    create_double: unsafe extern "C" fn(NapiEnv, f64, *mut NapiValue) -> i32,
    create_int32: unsafe extern "C" fn(NapiEnv, i32, *mut NapiValue) -> i32,
    create_uint32: unsafe extern "C" fn(NapiEnv, u32, *mut NapiValue) -> i32,
    create_int64: unsafe extern "C" fn(NapiEnv, i64, *mut NapiValue) -> i32,
    create_string_latin1:
        unsafe extern "C" fn(NapiEnv, *const c_char, usize, *mut NapiValue) -> i32,
    create_string_utf8: unsafe extern "C" fn(NapiEnv, *const c_char, usize, *mut NapiValue) -> i32,
    create_string_utf16: unsafe extern "C" fn(NapiEnv, *const u16, usize, *mut NapiValue) -> i32,
    create_symbol: unsafe extern "C" fn(NapiEnv, NapiValue, *mut NapiValue) -> i32,
    create_external: unsafe extern "C" fn(
        NapiEnv,
        *mut c_void,
        Option<NapiFinalize>,
        *mut c_void,
        *mut NapiValue,
    ) -> i32,
    typeof_value: unsafe extern "C" fn(NapiEnv, NapiValue, *mut i32) -> i32,
    get_value_external: unsafe extern "C" fn(NapiEnv, NapiValue, *mut *mut c_void) -> i32,
    get_value_double: unsafe extern "C" fn(NapiEnv, NapiValue, *mut f64) -> i32,
    get_value_int32: unsafe extern "C" fn(NapiEnv, NapiValue, *mut i32) -> i32,
    get_value_uint32: unsafe extern "C" fn(NapiEnv, NapiValue, *mut u32) -> i32,
    get_value_int64: unsafe extern "C" fn(NapiEnv, NapiValue, *mut i64) -> i32,
    get_value_bool: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    get_value_string_latin1:
        unsafe extern "C" fn(NapiEnv, NapiValue, *mut c_char, usize, *mut usize) -> i32,
    get_value_string_utf8:
        unsafe extern "C" fn(NapiEnv, NapiValue, *mut c_char, usize, *mut usize) -> i32,
    get_value_string_utf16:
        unsafe extern "C" fn(NapiEnv, NapiValue, *mut u16, usize, *mut usize) -> i32,
    create_array: unsafe extern "C" fn(NapiEnv, *mut NapiValue) -> i32,
    create_array_with_length: unsafe extern "C" fn(NapiEnv, usize, *mut NapiValue) -> i32,
    is_array: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    get_array_length: unsafe extern "C" fn(NapiEnv, NapiValue, *mut u32) -> i32,
    get_prototype: unsafe extern "C" fn(NapiEnv, NapiValue, *mut NapiValue) -> i32,
    get_element: unsafe extern "C" fn(NapiEnv, NapiValue, u32, *mut NapiValue) -> i32,
    set_element: unsafe extern "C" fn(NapiEnv, NapiValue, u32, NapiValue) -> i32,
    has_element: unsafe extern "C" fn(NapiEnv, NapiValue, u32, *mut bool) -> i32,
    create_buffer: unsafe extern "C" fn(NapiEnv, usize, *mut *mut c_void, *mut NapiValue) -> i32,
    create_buffer_copy: unsafe extern "C" fn(
        NapiEnv,
        usize,
        *const c_void,
        *mut *mut c_void,
        *mut NapiValue,
    ) -> i32,
    get_buffer_info: unsafe extern "C" fn(NapiEnv, NapiValue, *mut *mut c_void, *mut usize) -> i32,
    is_buffer: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    is_arraybuffer: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    create_arraybuffer:
        unsafe extern "C" fn(NapiEnv, usize, *mut *mut c_void, *mut NapiValue) -> i32,
    get_arraybuffer_info:
        unsafe extern "C" fn(NapiEnv, NapiValue, *mut *mut c_void, *mut usize) -> i32,
    is_typedarray: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    create_typedarray:
        unsafe extern "C" fn(NapiEnv, i32, usize, NapiValue, usize, *mut NapiValue) -> i32,
    get_typedarray_info: unsafe extern "C" fn(
        NapiEnv,
        NapiValue,
        *mut i32,
        *mut usize,
        *mut *mut c_void,
        *mut NapiValue,
        *mut usize,
    ) -> i32,
    create_dataview: unsafe extern "C" fn(NapiEnv, usize, NapiValue, usize, *mut NapiValue) -> i32,
    is_dataview: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    get_dataview_info: unsafe extern "C" fn(
        NapiEnv,
        NapiValue,
        *mut usize,
        *mut *mut c_void,
        *mut NapiValue,
        *mut usize,
    ) -> i32,
    create_error: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, *mut NapiValue) -> i32,
    create_type_error: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, *mut NapiValue) -> i32,
    create_range_error: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, *mut NapiValue) -> i32,
    throw: unsafe extern "C" fn(NapiEnv, NapiValue) -> i32,
    throw_error: unsafe extern "C" fn(NapiEnv, *const c_char, *const c_char) -> i32,
    throw_type_error: unsafe extern "C" fn(NapiEnv, *const c_char, *const c_char) -> i32,
    throw_range_error: unsafe extern "C" fn(NapiEnv, *const c_char, *const c_char) -> i32,
    is_exception_pending: unsafe extern "C" fn(NapiEnv, *mut bool) -> i32,
    get_and_clear_last_exception: unsafe extern "C" fn(NapiEnv, *mut NapiValue) -> i32,
    is_error: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    create_reference: unsafe extern "C" fn(NapiEnv, NapiValue, u32, *mut NapiRef) -> i32,
    delete_reference: unsafe extern "C" fn(NapiEnv, NapiRef) -> i32,
    reference_ref: unsafe extern "C" fn(NapiEnv, NapiRef, *mut u32) -> i32,
    reference_unref: unsafe extern "C" fn(NapiEnv, NapiRef, *mut u32) -> i32,
    get_reference_value: unsafe extern "C" fn(NapiEnv, NapiRef, *mut NapiValue) -> i32,
    wrap: unsafe extern "C" fn(
        NapiEnv,
        NapiValue,
        *mut c_void,
        Option<NapiFinalize>,
        *mut c_void,
        *mut NapiRef,
    ) -> i32,
    unwrap: unsafe extern "C" fn(NapiEnv, NapiValue, *mut *mut c_void) -> i32,
    remove_wrap: unsafe extern "C" fn(NapiEnv, NapiValue, *mut *mut c_void) -> i32,
    create_object: unsafe extern "C" fn(NapiEnv, *mut NapiValue) -> i32,
    define_properties:
        unsafe extern "C" fn(NapiEnv, NapiValue, usize, *const NapiPropertyDescriptor) -> i32,
    define_class: unsafe extern "C" fn(
        NapiEnv,
        *const c_char,
        usize,
        Option<NapiCallback>,
        *mut c_void,
        usize,
        *const NapiPropertyDescriptor,
        *mut NapiValue,
    ) -> i32,
    create_function: unsafe extern "C" fn(
        NapiEnv,
        *const c_char,
        usize,
        Option<NapiCallback>,
        *mut c_void,
        *mut NapiValue,
    ) -> i32,
    set_named_property: unsafe extern "C" fn(NapiEnv, NapiValue, *const c_char, NapiValue) -> i32,
    get_named_property:
        unsafe extern "C" fn(NapiEnv, NapiValue, *const c_char, *mut NapiValue) -> i32,
    get_property: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, *mut NapiValue) -> i32,
    set_property: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, NapiValue) -> i32,
    has_property: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, *mut bool) -> i32,
    delete_property: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, *mut bool) -> i32,
    delete_element: unsafe extern "C" fn(NapiEnv, NapiValue, u32, *mut bool) -> i32,
    has_own_property: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, *mut bool) -> i32,
    has_named_property: unsafe extern "C" fn(NapiEnv, NapiValue, *const c_char, *mut bool) -> i32,
    get_property_names: unsafe extern "C" fn(NapiEnv, NapiValue, *mut NapiValue) -> i32,
    call_function: unsafe extern "C" fn(
        NapiEnv,
        NapiValue,
        NapiValue,
        usize,
        *const NapiValue,
        *mut NapiValue,
    ) -> i32,
    new_instance:
        unsafe extern "C" fn(NapiEnv, NapiValue, usize, *const NapiValue, *mut NapiValue) -> i32,
    instanceof: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, *mut bool) -> i32,
    get_cb_info: unsafe extern "C" fn(
        NapiEnv,
        NapiCallbackInfo,
        *mut usize,
        *mut NapiValue,
        *mut NapiValue,
        *mut *mut c_void,
    ) -> i32,
    open_handle_scope: unsafe extern "C" fn(NapiEnv, *mut NapiHandleScope) -> i32,
    close_handle_scope: unsafe extern "C" fn(NapiEnv, NapiHandleScope) -> i32,
    open_escapable_handle_scope: unsafe extern "C" fn(NapiEnv, *mut NapiHandleScope) -> i32,
    close_escapable_handle_scope: unsafe extern "C" fn(NapiEnv, NapiHandleScope) -> i32,
    escape_handle: unsafe extern "C" fn(NapiEnv, NapiHandleScope, NapiValue, *mut NapiValue) -> i32,
    create_promise: unsafe extern "C" fn(NapiEnv, *mut NapiDeferred, *mut NapiValue) -> i32,
    resolve_deferred: unsafe extern "C" fn(NapiEnv, NapiDeferred, NapiValue) -> i32,
    reject_deferred: unsafe extern "C" fn(NapiEnv, NapiDeferred, NapiValue) -> i32,
    is_promise: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    create_async_work: unsafe extern "C" fn(
        NapiEnv,
        NapiValue,
        NapiValue,
        Option<NapiAsyncExecuteCallback>,
        Option<NapiAsyncCompleteCallback>,
        *mut c_void,
        *mut NapiAsyncWork,
    ) -> i32,
    delete_async_work: unsafe extern "C" fn(NapiEnv, NapiAsyncWork) -> i32,
    queue_async_work: unsafe extern "C" fn(NapiEnv, NapiAsyncWork) -> i32,
    cancel_async_work: unsafe extern "C" fn(NapiEnv, NapiAsyncWork) -> i32,
    create_threadsafe_function: unsafe extern "C" fn(
        NapiEnv,
        NapiValue,
        NapiValue,
        NapiValue,
        usize,
        usize,
        *mut c_void,
        Option<NapiFinalize>,
        *mut c_void,
        Option<NapiThreadsafeFunctionCallJs>,
        *mut NapiThreadsafeFunction,
    ) -> i32,
    get_threadsafe_function_context:
        unsafe extern "C" fn(NapiThreadsafeFunction, *mut *mut c_void) -> i32,
    call_threadsafe_function: unsafe extern "C" fn(NapiThreadsafeFunction, *mut c_void, i32) -> i32,
    acquire_threadsafe_function: unsafe extern "C" fn(NapiThreadsafeFunction) -> i32,
    release_threadsafe_function: unsafe extern "C" fn(NapiThreadsafeFunction, i32) -> i32,
    ref_threadsafe_function: unsafe extern "C" fn(NapiEnv, NapiThreadsafeFunction) -> i32,
    unref_threadsafe_function: unsafe extern "C" fn(NapiEnv, NapiThreadsafeFunction) -> i32,
    add_env_cleanup_hook:
        unsafe extern "C" fn(NapiEnv, Option<NapiCleanupHook>, *mut c_void) -> i32,
    remove_env_cleanup_hook:
        unsafe extern "C" fn(NapiEnv, Option<NapiCleanupHook>, *mut c_void) -> i32,
    get_last_error_info: unsafe extern "C" fn(NapiEnv, *mut *const NapiExtendedErrorInfo) -> i32,
    get_new_target: unsafe extern "C" fn(NapiEnv, NapiCallbackInfo, *mut NapiValue) -> i32,
    get_version: unsafe extern "C" fn(NapiEnv, *mut u32) -> i32,
    strict_equals: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, *mut bool) -> i32,
    run_script: unsafe extern "C" fn(NapiEnv, NapiValue, *mut NapiValue) -> i32,
    adjust_external_memory: unsafe extern "C" fn(NapiEnv, i64, *mut i64) -> i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NapiPropertyDescriptor {
    utf8name: *const c_char,
    name: NapiValue,
    method: Option<NapiCallback>,
    getter: Option<NapiCallback>,
    setter: Option<NapiCallback>,
    value: NapiValue,
    attributes: i32,
    data: *mut c_void,
}

static NAPI_VM_API_TABLE: NapiVmApiTable = NapiVmApiTable {
    get_undefined: api_get_undefined,
    get_global: api_get_global,
    get_null: api_get_null,
    get_boolean: api_get_boolean,
    coerce_to_bool: api_coerce_to_bool,
    coerce_to_number: api_coerce_to_number,
    coerce_to_string: api_coerce_to_string,
    create_double: api_create_double,
    create_int32: api_create_int32,
    create_uint32: api_create_uint32,
    create_int64: api_create_int64,
    create_string_latin1: api_create_string_latin1,
    create_string_utf8: api_create_string_utf8,
    create_string_utf16: api_create_string_utf16,
    create_symbol: api_create_symbol,
    create_external: api_create_external,
    typeof_value: api_typeof,
    get_value_external: api_get_value_external,
    get_value_double: api_get_value_double,
    get_value_int32: api_get_value_int32,
    get_value_uint32: api_get_value_uint32,
    get_value_int64: api_get_value_int64,
    get_value_bool: api_get_value_bool,
    get_value_string_latin1: api_get_value_string_latin1,
    get_value_string_utf8: api_get_value_string_utf8,
    get_value_string_utf16: api_get_value_string_utf16,
    create_array: api_create_array,
    create_array_with_length: api_create_array_with_length,
    is_array: api_is_array,
    get_array_length: api_get_array_length,
    get_prototype: api_get_prototype,
    get_element: api_get_element,
    set_element: api_set_element,
    has_element: api_has_element,
    create_buffer: api_create_buffer,
    create_buffer_copy: api_create_buffer_copy,
    get_buffer_info: api_get_buffer_info,
    is_buffer: api_is_buffer,
    is_arraybuffer: api_is_arraybuffer,
    create_arraybuffer: api_create_arraybuffer,
    get_arraybuffer_info: api_get_arraybuffer_info,
    is_typedarray: api_is_typedarray,
    create_typedarray: api_create_typedarray,
    get_typedarray_info: api_get_typedarray_info,
    create_dataview: api_create_dataview,
    is_dataview: api_is_dataview,
    get_dataview_info: api_get_dataview_info,
    create_error: api_create_error,
    create_type_error: api_create_type_error,
    create_range_error: api_create_range_error,
    throw: api_throw,
    throw_error: api_throw_error,
    throw_type_error: api_throw_type_error,
    throw_range_error: api_throw_range_error,
    is_exception_pending: api_is_exception_pending,
    get_and_clear_last_exception: api_get_and_clear_last_exception,
    is_error: api_is_error,
    create_reference: api_create_reference,
    delete_reference: api_delete_reference,
    reference_ref: api_reference_ref,
    reference_unref: api_reference_unref,
    get_reference_value: api_get_reference_value,
    wrap: api_wrap,
    unwrap: api_unwrap,
    remove_wrap: api_remove_wrap,
    create_object: api_create_object,
    define_properties: api_define_properties,
    define_class: api_define_class,
    create_function: api_create_function,
    set_named_property: api_set_named_property,
    get_named_property: api_get_named_property,
    get_property: api_get_property,
    set_property: api_set_property,
    has_property: api_has_property,
    delete_property: api_delete_property,
    delete_element: api_delete_element,
    has_own_property: api_has_own_property,
    has_named_property: api_has_named_property,
    get_property_names: api_get_property_names,
    call_function: api_call_function,
    new_instance: api_new_instance,
    instanceof: api_instanceof,
    get_cb_info: api_get_cb_info,
    open_handle_scope: api_open_handle_scope,
    close_handle_scope: api_close_handle_scope,
    open_escapable_handle_scope: api_open_escapable_handle_scope,
    close_escapable_handle_scope: api_close_escapable_handle_scope,
    escape_handle: api_escape_handle,
    create_promise: api_create_promise,
    resolve_deferred: api_resolve_deferred,
    reject_deferred: api_reject_deferred,
    is_promise: api_is_promise,
    create_async_work: api_create_async_work,
    delete_async_work: api_delete_async_work,
    queue_async_work: api_queue_async_work,
    cancel_async_work: api_cancel_async_work,
    create_threadsafe_function: api_create_threadsafe_function,
    get_threadsafe_function_context: api_get_threadsafe_function_context,
    call_threadsafe_function: api_call_threadsafe_function,
    acquire_threadsafe_function: api_acquire_threadsafe_function,
    release_threadsafe_function: api_release_threadsafe_function,
    ref_threadsafe_function: api_ref_threadsafe_function,
    unref_threadsafe_function: api_unref_threadsafe_function,
    add_env_cleanup_hook: api_add_env_cleanup_hook,
    remove_env_cleanup_hook: api_remove_env_cleanup_hook,
    get_last_error_info: api_get_last_error_info,
    get_new_target: api_get_new_target,
    get_version: api_get_version,
    strict_equals: api_strict_equals,
    run_script: api_run_script,
    adjust_external_memory: api_adjust_external_memory,
};

fn napi_extended_error_info(status: i32) -> NapiExtendedErrorInfo {
    let message: &'static [u8] = match status {
        NAPI_OK => b"success\0",
        NAPI_INVALID_ARG => b"invalid argument\0",
        NAPI_OBJECT_EXPECTED => b"object expected\0",
        NAPI_STRING_EXPECTED => b"string expected\0",
        NAPI_FUNCTION_EXPECTED => b"function expected\0",
        NAPI_NUMBER_EXPECTED => b"number expected\0",
        NAPI_BOOLEAN_EXPECTED => b"boolean expected\0",
        NAPI_ARRAY_EXPECTED => b"array expected\0",
        NAPI_PENDING_EXCEPTION => b"pending exception\0",
        NAPI_CANCELLED => b"cancelled\0",
        NAPI_ESCAPE_CALLED_TWICE => b"escape called twice\0",
        NAPI_HANDLE_SCOPE_MISMATCH => b"handle scope mismatch\0",
        NAPI_QUEUE_FULL => b"thread-safe function queue is full\0",
        NAPI_CLOSING => b"thread-safe function is closing\0",
        NAPI_ARRAYBUFFER_EXPECTED => b"ArrayBuffer expected\0",
        _ => b"generic Node-API failure\0",
    };
    NapiExtendedErrorInfo {
        error_message: message.as_ptr().cast(),
        engine_reserved: std::ptr::null_mut(),
        engine_error_code: 0,
        error_code: status,
    }
}

fn napi_extended_error_info_with_message(
    status: i32,
    message: &'static [u8],
) -> NapiExtendedErrorInfo {
    let mut info = napi_extended_error_info(status);
    info.error_message = message.as_ptr().cast();
    info
}

fn with_ffi_status(env: NapiEnv, callback: impl FnOnce() -> Result<(), i32>) -> i32 {
    let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback))
        .unwrap_or(Err(NAPI_GENERIC_FAILURE))
        .map_or_else(|status| status, |_| NAPI_OK);
    if let Ok(environment) = environment(env) {
        environment.last_error.set(napi_extended_error_info(status));
    }
    status
}

fn with_threadsafe_ffi_status(
    function: NapiThreadsafeFunction,
    callback: impl FnOnce() -> Result<(), i32>,
) -> i32 {
    let env = get_threadsafe_function(function)
        .map(|shared| shared.environment as NapiEnv)
        .unwrap_or(std::ptr::null_mut());
    with_ffi_status(env, callback)
}

fn environment(env: NapiEnv) -> Result<Rc<NapiEnvironment>, i32> {
    if env.is_null() {
        return Err(NAPI_INVALID_ARG);
    }
    let key = env as usize;
    NAPI_ENVIRONMENTS
        .try_with(|environments| {
            let mut environments = environments.borrow_mut();
            match environments.get(&key).and_then(Weak::upgrade) {
                Some(environment) => Ok(environment),
                None => {
                    environments.remove(&key);
                    Err(NAPI_INVALID_ARG)
                }
            }
        })
        .unwrap_or(Err(NAPI_INVALID_ARG))
}

unsafe extern "C" fn api_get_last_error_info(
    env: NapiEnv,
    result: *mut *const NapiExtendedErrorInfo,
) -> i32 {
    let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let environment = environment(env)?;
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        unsafe { result.write(environment.last_error.as_ptr()) };
        Ok(())
    }))
    .unwrap_or(Err(NAPI_GENERIC_FAILURE))
    .map_or_else(|status| status, |_| NAPI_OK);
    if status != NAPI_OK
        && let Ok(environment) = environment(env)
    {
        environment.last_error.set(napi_extended_error_info(status));
    }
    status
}

unsafe fn dispatch_guest_callback(
    context: *mut c_void,
    callback: HostCallback,
) -> Result<Value, VmErr> {
    // SAFETY: invoke_native points context at its live callback-handler
    // reference and keeps the dispatcher on the environment stack only until
    // the corresponding C callback returns.
    let handler = unsafe {
        &mut *context.cast::<&mut (dyn FnMut(HostCallback) -> Result<Value, VmErr> + '_)>()
    };
    (*handler)(callback)
}

fn exception_from_callback_error(error: VmErr) -> Value {
    let message = match error {
        VmErr::Throw(value) => return value,
        VmErr::RuntimeError(error) => error.message.clone(),
        other => other.to_string(),
    };
    for name in ["TypeError", "RangeError", "ReferenceError", "SyntaxError"] {
        if let Some(message) = message.strip_prefix(&format!("{name}: ")) {
            return Value::Error(ErrorData::new(name, message));
        }
    }
    Value::Error(ErrorData::new("Error", message))
}

fn call_guest_callback(
    environment: &NapiEnvironment,
    callback: HostCallback,
) -> Result<Value, i32> {
    if environment.pending_exception.borrow().is_some() {
        return Err(NAPI_PENDING_EXCEPTION);
    }
    let dispatcher = environment
        .guest_callback_dispatchers
        .borrow()
        .last()
        .copied()
        .ok_or(NAPI_GENERIC_FAILURE)?;
    match unsafe { (dispatcher.invoke)(dispatcher.context, callback) } {
        Ok(value) => Ok(value),
        Err(error) => {
            set_pending_exception(environment, exception_from_callback_error(error))?;
            Err(NAPI_PENDING_EXCEPTION)
        }
    }
}

fn has_guest_callback_dispatcher(environment: &NapiEnvironment) -> bool {
    !environment.guest_callback_dispatchers.borrow().is_empty()
}

fn napi_global_scope(environment: &NapiEnvironment) -> Result<Env, i32> {
    environment
        .owner
        .upgrade()
        .map(|owner| owner.borrow().global.clone())
        .ok_or(NAPI_INVALID_ARG)
}

fn napi_global_get(environment: &NapiEnvironment, key: &str) -> Result<Value, i32> {
    Ok(napi_global_scope(environment)?
        .borrow()
        .get(key)
        .unwrap_or(Value::Undefined))
}

fn napi_global_has(environment: &NapiEnvironment, key: &str) -> Result<bool, i32> {
    Ok(napi_global_scope(environment)?.borrow().get(key).is_some())
}

fn napi_global_has_own(environment: &NapiEnvironment, key: &str) -> Result<bool, i32> {
    Ok(napi_global_scope(environment)?
        .borrow()
        .all_keys()
        .iter()
        .any(|name| name == key))
}

fn napi_global_set(environment: &NapiEnvironment, key: &str, value: Value) -> Result<(), i32> {
    napi_global_scope(environment)?
        .borrow_mut()
        .try_set(key, value)
        .map_err(|_| NAPI_GENERIC_FAILURE)
}

fn napi_global_delete(environment: &NapiEnvironment, key: &str) -> Result<bool, i32> {
    let global = napi_global_scope(environment)?;
    if global.borrow().has(key) {
        return Ok(global.borrow_mut().remove(key));
    }
    Ok(true)
}

fn run_napi_guest_operation(
    environment: &NapiEnvironment,
    name: &'static str,
    operation: fn(&mut Interpreter, Value, Vec<Value>) -> Result<Value, VmErr>,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, i32> {
    call_guest_callback(
        environment,
        HostCallback {
            callback: Value::NativeFunction {
                name: Rc::from(name),
                callable: operation,
            },
            this_value: receiver,
            args,
            kind: HostCallbackKind::Call,
        },
    )
}

fn is_napi_property_object(value: &Value) -> bool {
    matches!(
        value,
        Value::Object { .. }
            | Value::Array(_)
            | Value::Function(_)
            | Value::NativeFunction { .. }
            | Value::HostFunction { .. }
            | Value::GlobalObject
            | Value::Class(_)
            | Value::Promise(_)
            | Value::Generator { .. }
            | Value::StringIterator { .. }
            | Value::Date(_)
            | Value::Proxy(_)
            | Value::ArrayBuffer(_)
            | Value::TypedArray(_)
            | Value::DataView(_)
            | Value::RegExp(_)
            | Value::Error(_)
    )
}

fn callback_arguments(
    environment: &NapiEnvironment,
    argc: usize,
    argv: *const NapiValue,
) -> Result<Vec<Value>, i32> {
    if argc > 0 && argv.is_null() {
        return Err(NAPI_INVALID_ARG);
    }
    if argc == 0 {
        return Ok(Vec::new());
    }
    let handles = unsafe { std::slice::from_raw_parts(argv, argc) };
    let arena = environment.handles.borrow();
    handles.iter().map(|handle| arena.get(*handle)).collect()
}

fn is_napi_function(value: &Value) -> bool {
    match value {
        Value::Function(_)
        | Value::NativeFunction { .. }
        | Value::HostFunction { .. }
        | Value::Class(_) => true,
        Value::Proxy(proxy) => is_napi_function(&proxy.target),
        Value::Object { .. } => value.get_prop("__symbol_call__").is_some_and(|target| {
            matches!(
                target,
                Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
            )
        }),
        _ => false,
    }
}

fn create_native_callback_value(
    environment: &Rc<NapiEnvironment>,
    function_name: &str,
    callback: NapiCallback,
    data: *mut c_void,
) -> Result<Value, i32> {
    create_native_callback_value_with_kind(
        environment,
        function_name,
        NativeCallback::Function(callback),
        data,
        false,
    )
}

fn create_native_async_complete_value(
    environment: &Rc<NapiEnvironment>,
    callback: NapiAsyncCompleteCallback,
    status: i32,
    data: *mut c_void,
    work_id: usize,
) -> Result<Value, i32> {
    create_native_callback_value_with_kind(
        environment,
        "napi_async_complete",
        NativeCallback::AsyncComplete {
            callback,
            status,
            work_id,
        },
        data,
        true,
    )
}

fn create_native_callback_value_with_kind(
    environment: &Rc<NapiEnvironment>,
    function_name: &str,
    callback: NativeCallback,
    data: *mut c_void,
    one_shot: bool,
) -> Result<Value, i32> {
    let owner = environment.owner.upgrade().ok_or(NAPI_GENERIC_FAILURE)?;
    let id = {
        let mut state = owner.borrow_mut();
        let id = state.next_callback_id;
        state.next_callback_id = id.checked_add(1).ok_or(NAPI_GENERIC_FAILURE)?;
        state.callbacks.insert(
            id,
            NativeCallbackRecord {
                env: environment.clone(),
                callback,
                data,
                one_shot,
            },
        );
        id
    };
    Ok(Value::HostFunction {
        name: Rc::from(function_name),
        id,
    })
}

fn napi_property_key(value: &Value) -> Result<String, i32> {
    match value {
        Value::String(key) => Ok(key.clone()),
        Value::Symbol(symbol) => Ok(crate::interpreter::symbol_slot_key(symbol)),
        _ => Err(NAPI_INVALID_ARG),
    }
}

fn napi_direct_get_property(object: &Value, key: &Value) -> Result<Value, i32> {
    let key = napi_property_key(key)?;
    Ok(object.get_prop(&key).unwrap_or(Value::Undefined))
}

fn napi_direct_set_property(object: &Value, key: &Value, value: Value) -> Result<(), i32> {
    let symbol = match key {
        Value::Symbol(symbol) => Some(symbol.clone()),
        _ => None,
    };
    let key = napi_property_key(key)?;
    match object {
        Value::Object { props } => {
            object
                .set_prop(key.clone(), value)
                .map_err(|_| NAPI_GENERIC_FAILURE)?;
            if let Some(symbol) = symbol {
                props.meta.borrow_mut().set_symbol_key(&key, symbol);
            }
            Ok(())
        }
        Value::Class(class) => {
            if class.statics.meta.borrow().has_accessors {
                let is_setter = |value: &Value| match value {
                    Value::Function(function) => function
                        .name
                        .as_ref()
                        .is_some_and(|name| name.starts_with("set ")),
                    Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
                        name.starts_with("set ")
                    }
                    _ => false,
                };
                if class
                    .statics
                    .borrow()
                    .iter()
                    .any(|(name, value)| name == &key && is_setter(value))
                {
                    return Err(NAPI_GENERIC_FAILURE);
                }
            }
            object
                .set_prop(key.clone(), value)
                .map_err(|_| NAPI_GENERIC_FAILURE)?;
            if let Some(symbol) = symbol {
                class.statics.meta.borrow_mut().set_symbol_key(&key, symbol);
            }
            Ok(())
        }
        Value::Array(array) => {
            if key == "length" {
                let Value::Number(length) = value else {
                    return Err(NAPI_INVALID_ARG);
                };
                if !length.is_finite()
                    || length < 0.0
                    || length.fract() != 0.0
                    || length > crate::value::MAX_ARRAY_LEN as f64
                {
                    return Err(NAPI_INVALID_ARG);
                }
                let length = length as usize;
                let old_length = array.borrow().len();
                array.borrow_mut().resize(length, Value::Undefined);
                array.resize_presence(old_length, length, false);
                return Ok(());
            }
            if let Some(index) = crate::value::array_index(&key) {
                if index >= crate::value::MAX_ARRAY_LEN {
                    return Err(NAPI_GENERIC_FAILURE);
                }
                let old_length = array.borrow().len();
                let mut elements = array.borrow_mut();
                if index < elements.len() {
                    elements[index] = value;
                } else {
                    elements.resize(index, Value::Undefined);
                    elements.push(value);
                }
                let new_length = elements.len();
                drop(elements);
                if index >= old_length {
                    array.resize_presence(old_length, new_length, false);
                }
                array.set_index_presence(index, true);
                return Ok(());
            }
            array.set_named(key, value);
            Ok(())
        }
        _ => Err(NAPI_OBJECT_EXPECTED),
    }
}

fn napi_direct_has_own_property(object: &Value, key: &Value) -> Result<bool, i32> {
    let key = napi_property_key(key)?;
    Ok(match object {
        Value::Object { props } => props.borrow().iter().any(|(name, _)| name == &key),
        Value::Class(class) => class.statics.borrow().iter().any(|(name, _)| name == &key),
        Value::Array(array) => {
            key == "length"
                || crate::value::array_index(&key).is_some_and(|index| array.has_index(index))
                || array.named_prop(&key).is_some()
        }
        Value::Error(error) => {
            matches!(key.as_str(), "name" | "message" | "stack")
                || (key == "code" && error.code.is_some())
        }
        Value::String(string) => {
            key == "length"
                || key
                    .parse::<usize>()
                    .is_ok_and(|index| index < string.chars().count())
        }
        _ => false,
    })
}

fn napi_direct_delete_property(object: &Value, key: &Value) -> Result<bool, i32> {
    let key = napi_property_key(key)?;
    match object {
        Value::Object { props } => {
            if !props.meta.borrow().attrs_of(&key).configurable
                && props.borrow().iter().any(|(name, _)| name == &key)
            {
                return Ok(false);
            }
            let mut slots = props.borrow_mut();
            if let Some(index) = slots.iter().position(|(name, _)| name == &key) {
                slots.remove(index);
                drop(slots);
                props.meta.borrow_mut().forget(&key);
            }
            Ok(true)
        }
        Value::Class(class) => {
            if !class.statics.meta.borrow().attrs_of(&key).configurable
                && class.statics.borrow().iter().any(|(name, _)| name == &key)
            {
                return Ok(false);
            }
            let companion = format!("__setter:{}__", key);
            let mut slots = class.statics.borrow_mut();
            slots.retain(|(name, _)| name != &key && name != &companion);
            drop(slots);
            class.statics.meta.borrow_mut().forget(&key);
            class.statics.meta.borrow_mut().forget(&companion);
            Ok(true)
        }
        Value::Array(array) => {
            if key == "length" {
                return Ok(false);
            }
            if let Some(index) = crate::value::array_index(&key) {
                if index < array.borrow().len() {
                    array.borrow_mut()[index] = Value::Undefined;
                    array.set_index_presence(index, false);
                }
            } else {
                array.named.borrow_mut().retain(|(name, _)| name != &key);
            }
            Ok(true)
        }
        Value::Proxy(proxy) => napi_direct_delete_property(&proxy.target, &Value::String(key)),
        _ => Ok(true),
    }
}

fn napi_direct_own_property_names(object: &Value) -> Vec<String> {
    match object {
        Value::Object { props } => props.borrow().iter().map(|(key, _)| key.clone()).collect(),
        Value::Class(class) => class
            .statics
            .borrow()
            .iter()
            .map(|(key, _)| key.clone())
            .collect(),
        Value::Array(array) => {
            let mut names = vec!["length".to_owned()];
            names.extend(
                (0..array.borrow().len())
                    .filter(|index| array.has_index(*index))
                    .map(|index| index.to_string()),
            );
            names.extend(array.named.borrow().iter().map(|(key, _)| key.clone()));
            names
        }
        Value::Proxy(proxy) => napi_direct_own_property_names(&proxy.target),
        Value::Error(error) => {
            let mut names = vec!["name".to_owned(), "message".to_owned(), "stack".to_owned()];
            if error.code.is_some() {
                names.push("code".to_owned());
            }
            names
        }
        _ => Vec::new(),
    }
}

fn napi_direct_property_is_enumerable(object: &Value, key: &str) -> bool {
    match object {
        Value::Object { props } => {
            props.borrow().iter().any(|(name, _)| name == key)
                && props.meta.borrow().attrs_of(key).enumerable
        }
        Value::Class(class) => {
            class.statics.borrow().iter().any(|(name, _)| name == key)
                && class.statics.meta.borrow().attrs_of(key).enumerable
        }
        Value::Array(array) => {
            key != "length"
                && (crate::value::array_index(key).is_some_and(|index| array.has_index(index))
                    || array.named_prop(key).is_some())
        }
        Value::Proxy(proxy) => napi_direct_property_is_enumerable(&proxy.target, key),
        Value::GlobalObject => true,
        Value::Error(error) => key == "code" && error.code.is_some(),
        _ => false,
    }
}

fn napi_direct_prototype(object: &Value) -> Option<Rc<Value>> {
    match object {
        Value::Proxy(proxy) => proxy.target.proto_of(),
        _ => object.proto_of(),
    }
}

fn napi_direct_property_names(object: &Value) -> Result<Value, i32> {
    let mut current = object.clone();
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        for key in napi_direct_own_property_names(&current) {
            if crate::interpreter::is_internal_key(&key) || !seen.insert(key.clone()) {
                continue;
            }
            if seen.len() > crate::value::MAX_ARRAY_LEN {
                return Err(NAPI_GENERIC_FAILURE);
            }
            if napi_direct_property_is_enumerable(&current, &key) {
                names.push(Value::String(key));
            }
        }
        let Some(prototype) = napi_direct_prototype(&current) else {
            return Value::checked_array(names).map_err(|_| NAPI_GENERIC_FAILURE);
        };
        current = (*prototype).clone();
    }
    Err(NAPI_GENERIC_FAILURE)
}

fn napi_guest_get_property(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = args.first().cloned().unwrap_or(Value::Undefined);
    interpreter.get_prop_value(&receiver, &key)
}

fn napi_guest_resolve_deferred(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let promise = receiver
        .as_promise()
        .ok_or_else(|| VmErr::Msg("Node-API deferred does not reference a promise".into()))?;
    let resolution = args.into_iter().next().unwrap_or(Value::Undefined);
    if let Err(error) = interpreter.resolve_promise(&promise, resolution) {
        // Promise resolution converts errors while reading/calling a thenable
        // into rejection. They must not escape as a synchronous N-API throw.
        interpreter.reject_promise(&promise, exception_from_callback_error(error));
    }
    Ok(Value::Undefined)
}

fn napi_guest_reject_deferred(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let promise = receiver
        .as_promise()
        .ok_or_else(|| VmErr::Msg("Node-API deferred does not reference a promise".into()))?;
    let rejection = args.into_iter().next().unwrap_or(Value::Undefined);
    interpreter.reject_promise(&promise, rejection);
    Ok(Value::Undefined)
}

fn napi_guest_set_property(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = args.first().cloned().unwrap_or(Value::Undefined);
    let value = args.get(1).cloned().unwrap_or(Value::Undefined);
    interpreter.assign_member(&receiver, &key, value.clone())?;
    Ok(value)
}

fn napi_guest_has_property(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = args.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Bool(interpreter.has_property(&receiver, &key)?))
}

fn napi_guest_has_own_property(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = args.first().cloned().unwrap_or(Value::Undefined);
    let constructor = interpreter
        .global
        .borrow()
        .get("Object")
        .ok_or_else(|| VmErr::Msg("Object constructor is unavailable".into()))?;
    let method = interpreter.member(&constructor, "hasOwn")?;
    interpreter.call_this(&method, constructor, vec![receiver, key])
}

fn napi_guest_delete_property(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let key = args.first().cloned().unwrap_or(Value::Undefined);
    interpreter.delete_member(&receiver, &key)
}

fn napi_guest_get_property_names(
    interpreter: &mut Interpreter,
    receiver: Value,
    _: Vec<Value>,
) -> Result<Value, VmErr> {
    let mut current = receiver;
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        let trapped_keys = if matches!(current, Value::GlobalObject) {
            Some(interpreter.global_keys())
        } else if matches!(current, Value::Proxy(_)) {
            Some(interpreter.keys_with_proxy_trap(&current)?)
        } else {
            None
        };
        let own_keys = trapped_keys
            .as_ref()
            .cloned()
            .unwrap_or_else(|| napi_direct_own_property_names(&current));
        for key in &own_keys {
            if crate::interpreter::is_internal_key(key) || !seen.insert(key.clone()) {
                continue;
            }
            if seen.len() > crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err(
                    "Maximum property name count exceeded",
                ));
            }
            if napi_direct_property_is_enumerable(&current, key) {
                names.push(Value::String(key.clone()));
            }
        }
        // Non-enumerable own keys still shadow enumerable properties farther
        // up the prototype chain.
        seen.extend(napi_direct_own_property_names(&current));
        let prototype = match &current {
            Value::Proxy(proxy) => proxy.target.proto_of(),
            _ => current.proto_of(),
        };
        let Some(prototype) = prototype else {
            return Value::checked_array(names);
        };
        current = (*prototype).clone();
    }
    Err(crate::value::limit_err("Maximum prototype depth exceeded"))
}

fn napi_object_identity(value: &Value) -> Result<NapiObjectIdentity, i32> {
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
        Value::HostFunction { id, .. } => Ok(NapiObjectIdentity::HostFunction(*id)),
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
        Value::ArrayBuffer(buffer) => {
            Ok(NapiObjectIdentity::ArrayBuffer(Rc::as_ptr(buffer) as usize))
        }
        Value::TypedArray(view) => Ok(NapiObjectIdentity::TypedArray(Rc::as_ptr(view) as usize)),
        Value::DataView(view) => Ok(NapiObjectIdentity::DataView(Rc::as_ptr(view) as usize)),
        Value::RegExp(regexp) => Ok(NapiObjectIdentity::RegExp(Rc::as_ptr(regexp) as usize)),
        Value::Undefined
        | Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::HostPending { .. }
        | Value::Symbol(_)
        | Value::BigInt(_)
        | Value::Binding(_)
        | Value::Error(_) => Err(NAPI_OBJECT_EXPECTED),
        #[cfg(stackful_coroutines)]
        Value::AsyncTask(_) => Err(NAPI_OBJECT_EXPECTED),
    }
}

fn napi_is_external_value(environment: &NapiEnvironment, value: &Value) -> bool {
    napi_object_identity(value)
        .ok()
        .is_some_and(|identity| environment.externals.borrow().contains_key(&identity))
}

fn finalize_environment_wraps(environment: &Rc<NapiEnvironment>) {
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
}

fn run_environment_cleanup_hooks(environment: &Rc<NapiEnvironment>) {
    loop {
        let hook = environment.cleanup_hooks.borrow_mut().pop();
        let Some(hook) = hook else {
            break;
        };
        let scope = environment.handles.borrow_mut().open_scope().ok();
        unsafe { (hook.function)(hook.argument as *mut c_void) };
        environment.pending_exception.borrow_mut().take();
        if let Some(scope) = scope {
            let _ = environment.handles.borrow_mut().close_scope(scope);
        }
    }
}

fn register_environment(environment: &Rc<NapiEnvironment>) {
    let _ = NAPI_ENVIRONMENTS.try_with(|environments| {
        environments
            .borrow_mut()
            .insert(environment.raw() as usize, Rc::downgrade(environment));
    });
}

impl Drop for NapiEnvironment {
    fn drop(&mut self) {
        let _ = NAPI_ENVIRONMENTS.try_with(|environments| {
            environments
                .borrow_mut()
                .remove(&(self as *const Self as usize));
        });
    }
}

unsafe fn read_c_string(pointer: *const c_char) -> Result<String, i32> {
    if pointer.is_null() {
        return Err(NAPI_INVALID_ARG);
    }
    let bytes = unsafe { CStr::from_ptr(pointer) };
    bytes
        .to_str()
        .map(str::to_owned)
        .map_err(|_| NAPI_INVALID_ARG)
}

unsafe fn read_utf8(pointer: *const c_char, length: usize) -> Result<String, i32> {
    if pointer.is_null() {
        return Err(NAPI_INVALID_ARG);
    }
    let bytes = if length == usize::MAX {
        unsafe { CStr::from_ptr(pointer) }.to_bytes()
    } else {
        unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) }
    };
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

unsafe fn read_latin1(pointer: *const c_char, length: usize) -> Result<String, i32> {
    if pointer.is_null() {
        return Err(NAPI_INVALID_ARG);
    }
    let bytes = if length == usize::MAX {
        unsafe { CStr::from_ptr(pointer) }.to_bytes()
    } else {
        if length > MAX_NAPI_BUFFER_BYTES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) }
    };
    if bytes.len() > MAX_NAPI_BUFFER_BYTES {
        return Err(NAPI_GENERIC_FAILURE);
    }
    Ok(bytes.iter().copied().map(char::from).collect())
}

unsafe fn read_utf16(pointer: *const u16, length: usize) -> Result<String, i32> {
    if pointer.is_null() {
        return Err(NAPI_INVALID_ARG);
    }
    let length = if length == usize::MAX {
        let mut index = 0;
        loop {
            if index > MAX_NAPI_BUFFER_BYTES {
                return Err(NAPI_GENERIC_FAILURE);
            }
            if unsafe { pointer.add(index).read() } == 0 {
                break index;
            }
            index += 1;
        }
    } else {
        length
    };
    if length > MAX_NAPI_BUFFER_BYTES {
        return Err(NAPI_GENERIC_FAILURE);
    }
    let code_units = unsafe { std::slice::from_raw_parts(pointer, length) };
    let mut string = String::new();
    for decoded in std::char::decode_utf16(code_units.iter().copied()) {
        let character = decoded.map_err(|_| NAPI_GENERIC_FAILURE)?;
        if string.len().saturating_add(character.len_utf8()) > MAX_NAPI_BUFFER_BYTES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        string.push(character);
    }
    Ok(string)
}

unsafe extern "C" fn api_get_undefined(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment.handles.borrow_mut().create(Value::Undefined)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_global(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::GlobalObject)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_null(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment.handles.borrow_mut().create(Value::Null)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_boolean(env: NapiEnv, value: bool, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Bool(value))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_coerce_to_bool(
    env: NapiEnv,
    value: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Bool(value.deref_binding().is_truthy()))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

fn napi_guest_coerce_to_number(
    interpreter: &mut Interpreter,
    _receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let value = args.first().cloned().unwrap_or(Value::Undefined);
    Ok(Value::Number(interpreter.napi_to_number(&value)?))
}

unsafe extern "C" fn api_coerce_to_number(
    env: NapiEnv,
    value: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let value = run_napi_guest_operation(
            &environment,
            "napi_coerce_to_number",
            napi_guest_coerce_to_number,
            Value::Undefined,
            vec![value],
        )?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

fn napi_guest_coerce_to_string(
    interpreter: &mut Interpreter,
    _receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let value = args.first().cloned().unwrap_or(Value::Undefined);
    Value::checked_string(interpreter.napi_to_string(&value)?)
}

unsafe extern "C" fn api_coerce_to_string(
    env: NapiEnv,
    value: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let value = run_napi_guest_operation(
            &environment,
            "napi_coerce_to_string",
            napi_guest_coerce_to_string,
            Value::Undefined,
            vec![value],
        )?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_create_double(env: NapiEnv, value: f64, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Number(value))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_create_int32(env: NapiEnv, value: i32, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Number(value as f64))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_create_uint32(env: NapiEnv, value: u32, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Number(value as f64))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_create_int64(env: NapiEnv, value: i64, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::Number(value as f64))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_create_string_utf8(
    env: NapiEnv,
    value: *const c_char,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let value = unsafe { read_utf8(value, length)? };
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::String(value))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_create_string_latin1(
    env: NapiEnv,
    value: *const c_char,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let value = unsafe { read_latin1(value, length)? };
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::String(value))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_create_string_utf16(
    env: NapiEnv,
    value: *const u16,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    let utf16_input_error = Cell::new(false);
    let status = with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let value = unsafe { read_utf16(value, length) }.inspect_err(|status| {
            if *status == NAPI_GENERIC_FAILURE {
                utf16_input_error.set(true);
            }
        })?;
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::String(value))?;
        unsafe { result.write(handle) };
        Ok(())
    });
    if utf16_input_error.get()
        && status == NAPI_GENERIC_FAILURE
        && let Ok(environment) = environment(env)
    {
        environment
            .last_error
            .set(napi_extended_error_info_with_message(
                status,
                UTF16_INPUT_ERROR_MESSAGE,
            ));
    }
    status
}

unsafe extern "C" fn api_create_symbol(
    env: NapiEnv,
    description: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let description = if description.is_null() {
            None
        } else {
            let description = environment.handles.borrow().get(description)?;
            let Value::String(description) = &description else {
                return Err(NAPI_STRING_EXPECTED);
            };
            Some(description.clone())
        };
        let handle = environment
            .handles
            .borrow_mut()
            .create(crate::builtins::new_symbol(description))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_create_external(
    env: NapiEnv,
    data: *mut c_void,
    finalize: Option<NapiFinalize>,
    finalize_hint: *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        if environment.finalizing.get() {
            return Err(NAPI_GENERIC_FAILURE);
        }

        // Node-API externals are opaque JS values: they behave like
        // non-extensible, null-prototype objects in JS, while napi_typeof
        // reports the distinct napi_external tag.
        let value = Value::object_with_proto(Vec::new(), None);
        let Value::Object { props } = &value else {
            unreachable!("object_with_proto creates an object")
        };
        props.meta.borrow_mut().non_extensible = true;
        let identity = napi_object_identity(&value)?;
        let handle = environment.handles.borrow_mut().create(value.clone())?;
        environment.externals.borrow_mut().insert(
            identity,
            NapiExternal {
                _value: value,
                data,
                finalize,
                hint: finalize_hint,
            },
        );
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_typeof(env: NapiEnv, value: NapiValue, result: *mut i32) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        // Values are the Node-API `napi_valuetype` discriminants from
        // js_native_api_types.h. Proxy `typeof` follows its target.
        let value_type = if napi_is_external_value(&environment, &value) {
            8 // napi_external
        } else {
            napi_value_type(&value)
        };
        unsafe { result.write(value_type) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_value_external(
    env: NapiEnv,
    value: NapiValue,
    result: *mut *mut c_void,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let identity = napi_object_identity(&value)?;
        let data = environment
            .externals
            .borrow()
            .get(&identity)
            .map(|external| external.data)
            .ok_or(NAPI_INVALID_ARG)?;
        unsafe { result.write(data) };
        Ok(())
    })
}

fn napi_value_type(value: &Value) -> i32 {
    let resolved = value.deref_binding();
    if is_napi_function(&resolved) {
        return 7; // napi_function
    }
    match &resolved {
        Value::Undefined => 0, // napi_undefined
        Value::Null => 1,      // napi_null
        Value::Bool(_) => 2,   // napi_boolean
        Value::Number(_) => 3, // napi_number
        Value::String(_) => 4, // napi_string
        Value::Symbol(_) => 5, // napi_symbol
        Value::BigInt(_) => 9, // napi_bigint
        Value::Proxy(proxy) => napi_value_type(&proxy.target),
        _ => 6, // napi_object
    }
}

unsafe extern "C" fn api_get_value_double(env: NapiEnv, value: NapiValue, result: *mut f64) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Number(number) = value else {
            return Err(NAPI_NUMBER_EXPECTED);
        };
        unsafe { result.write(number) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_value_int32(env: NapiEnv, value: NapiValue, result: *mut i32) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Number(number) = value else {
            return Err(NAPI_NUMBER_EXPECTED);
        };
        unsafe { result.write(to_int32(number)) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_value_uint32(env: NapiEnv, value: NapiValue, result: *mut u32) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Number(number) = value else {
            return Err(NAPI_NUMBER_EXPECTED);
        };
        unsafe { result.write(to_int32(number) as u32) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_value_int64(env: NapiEnv, value: NapiValue, result: *mut i64) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Number(number) = value else {
            return Err(NAPI_NUMBER_EXPECTED);
        };
        // napi_get_value_int64 converts finite Numbers by truncating toward
        // zero, but maps NaN and infinities to zero. Rust's float-to-int cast
        // saturates infinities, so handle non-finite values explicitly.
        unsafe { result.write(if number.is_finite() { number as i64 } else { 0 }) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_value_bool(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Bool(value) = value else {
            return Err(NAPI_BOOLEAN_EXPECTED);
        };
        unsafe { result.write(value) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_value_string_utf8(
    env: NapiEnv,
    value: NapiValue,
    buffer: *mut c_char,
    buffer_size: usize,
    result: *mut usize,
) -> i32 {
    with_ffi_status(env, || {
        if buffer.is_null() && buffer_size != 0 || buffer.is_null() && result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::String(value) = &value else {
            return Err(NAPI_STRING_EXPECTED);
        };
        let bytes = value.as_bytes();
        let copied = if buffer.is_null() || buffer_size == 0 {
            0
        } else {
            let copied = bytes.len().min(buffer_size - 1);
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), copied);
                buffer.add(copied).write(0);
            }
            copied
        };
        if !result.is_null() {
            unsafe {
                result.write(if buffer.is_null() {
                    bytes.len()
                } else {
                    copied
                })
            };
        }
        Ok(())
    })
}

unsafe extern "C" fn api_get_value_string_latin1(
    env: NapiEnv,
    value: NapiValue,
    buffer: *mut c_char,
    buffer_size: usize,
    result: *mut usize,
) -> i32 {
    with_ffi_status(env, || {
        if (buffer.is_null() && buffer_size != 0) || (buffer.is_null() && result.is_null()) {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::String(value) = &value else {
            return Err(NAPI_STRING_EXPECTED);
        };
        let length = value.encode_utf16().count();
        if buffer.is_null() {
            unsafe { result.write(length) };
            return Ok(());
        }
        let copied = if buffer_size == 0 {
            0
        } else {
            let copied = length.min(buffer_size - 1);
            for (index, unit) in value.encode_utf16().take(copied).enumerate() {
                unsafe { buffer.add(index).write(unit as u8 as c_char) };
            }
            unsafe { buffer.add(copied).write(0) };
            copied
        };
        if !result.is_null() {
            unsafe { result.write(copied) };
        }
        Ok(())
    })
}

unsafe extern "C" fn api_get_value_string_utf16(
    env: NapiEnv,
    value: NapiValue,
    buffer: *mut u16,
    buffer_size: usize,
    result: *mut usize,
) -> i32 {
    with_ffi_status(env, || {
        if (buffer.is_null() && buffer_size != 0) || (buffer.is_null() && result.is_null()) {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::String(value) = &value else {
            return Err(NAPI_STRING_EXPECTED);
        };
        let length = value.encode_utf16().count();
        if buffer.is_null() {
            unsafe { result.write(length) };
            return Ok(());
        }
        let copied = if buffer_size == 0 {
            0
        } else {
            let copied = length.min(buffer_size - 1);
            for (index, unit) in value.encode_utf16().take(copied).enumerate() {
                unsafe { buffer.add(index).write(unit) };
            }
            unsafe { buffer.add(copied).write(0) };
            copied
        };
        if !result.is_null() {
            unsafe { result.write(copied) };
        }
        Ok(())
    })
}

unsafe extern "C" fn api_create_array(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let handle = environment
            .handles
            .borrow_mut()
            .create(Value::array(Vec::new()))?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_create_array_with_length(
    env: NapiEnv,
    length: usize,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        if length > crate::value::MAX_ARRAY_LEN {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let environment = environment(env)?;
        let array = Value::array_with_presence(vec![Value::Undefined; length], vec![false; length]);
        let handle = environment.handles.borrow_mut().create(array)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_is_array(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        unsafe { result.write(matches!(value, Value::Array(_))) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_array_length(env: NapiEnv, value: NapiValue, result: *mut u32) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Array(array) = &value else {
            return Err(NAPI_ARRAY_EXPECTED);
        };
        let length = array.borrow().len();
        unsafe { result.write(length as u32) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_prototype(
    env: NapiEnv,
    object: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        let prototype = match &object {
            Value::Object { props } => {
                let (prototype, uses_default_prototype) = {
                    let meta = props.meta.borrow();
                    (meta.proto.clone(), meta.uses_default_prototype)
                };
                match (prototype, uses_default_prototype) {
                    (Some(prototype), _) => prototype.as_ref().clone(),
                    (None, true) => {
                        let default_prototype = napi_default_object_prototype(&environment)?;
                        if super::strict_equals(&object, &default_prototype) {
                            Value::Null
                        } else {
                            default_prototype
                        }
                    }
                    (None, false) => Value::Null,
                }
            }
            Value::GlobalObject => napi_default_object_prototype(&environment)?,
            value if !is_napi_property_object(value) => return Err(NAPI_OBJECT_EXPECTED),
            // The VM does not yet materialize several built-in and proxy
            // prototypes. Failing clearly is safer than returning a plausible
            // but incorrect prototype object.
            _ => return Err(NAPI_GENERIC_FAILURE),
        };
        let handle = environment.handles.borrow_mut().create(prototype)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

fn napi_default_object_prototype(environment: &NapiEnvironment) -> Result<Value, i32> {
    let owner = environment.owner.upgrade().ok_or(NAPI_INVALID_ARG)?;
    owner
        .borrow()
        .object_prototype
        .clone()
        .ok_or(NAPI_GENERIC_FAILURE)
}

unsafe extern "C" fn api_get_element(
    env: NapiEnv,
    value: NapiValue,
    index: u32,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Array(array) = &value else {
            return Err(NAPI_ARRAY_EXPECTED);
        };
        let value = array
            .borrow()
            .get(index as usize)
            .cloned()
            .unwrap_or(Value::Undefined);
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_set_element(
    env: NapiEnv,
    value: NapiValue,
    index: u32,
    element: NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let element = environment.handles.borrow().get(element)?;
        let Value::Array(array) = &value else {
            return Err(NAPI_ARRAY_EXPECTED);
        };
        let index = index as usize;
        if index >= crate::value::MAX_ARRAY_LEN {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let old_length = array.borrow().len();
        if index >= old_length {
            let new_length = index + 1;
            array.borrow_mut().resize(new_length, Value::Undefined);
            array.resize_presence(old_length, new_length, false);
        }
        array.borrow_mut()[index] = element;
        array.set_index_presence(index, true);
        Ok(())
    })
}

unsafe extern "C" fn api_has_element(
    env: NapiEnv,
    value: NapiValue,
    index: u32,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Array(array) = &value else {
            return Err(NAPI_ARRAY_EXPECTED);
        };
        unsafe { result.write(array.has_index(index as usize)) };
        Ok(())
    })
}

fn create_napi_buffer(bytes: Vec<u8>) -> Result<(Value, *mut c_void), i32> {
    if bytes.len() > MAX_NAPI_BUFFER_BYTES {
        return Err(NAPI_GENERIC_FAILURE);
    }
    let length = bytes.len();
    let value = Value::TypedArray(Rc::new(TypedArrayData {
        kind: TypedKind::Uint8,
        buffer: Rc::new(RefCell::new(bytes)),
        byte_offset: 0,
        length,
    }));
    let (data, _) = napi_buffer_data(&value)?;
    Ok((value, data))
}

fn napi_buffer_data(value: &Value) -> Result<(*mut c_void, usize), i32> {
    let Value::TypedArray(view) = value else {
        return Err(NAPI_INVALID_ARG);
    };
    if view.kind != TypedKind::Uint8 {
        return Err(NAPI_INVALID_ARG);
    }
    let end = view
        .byte_offset
        .checked_add(view.length)
        .ok_or(NAPI_INVALID_ARG)?;
    let mut bytes = view.buffer.borrow_mut();
    if end > bytes.len() {
        return Err(NAPI_INVALID_ARG);
    }
    let data = unsafe { bytes.as_mut_ptr().add(view.byte_offset).cast::<c_void>() };
    Ok((data, view.length))
}

fn remember_napi_buffer(environment: &NapiEnvironment, value: &Value) -> Result<(), i32> {
    let Value::TypedArray(view) = value else {
        return Err(NAPI_INVALID_ARG);
    };
    let identity = napi_object_identity(value)?;
    let mut buffers = environment.buffer_values.borrow_mut();
    buffers.retain(|_, buffer| buffer.strong_count() > 0);
    buffers.insert(identity, Rc::downgrade(view));
    Ok(())
}

fn is_napi_buffer(environment: &NapiEnvironment, value: &Value) -> bool {
    let Value::TypedArray(view) = value else {
        return false;
    };
    let Ok(identity) = napi_object_identity(value) else {
        return false;
    };
    environment
        .buffer_values
        .borrow()
        .get(&identity)
        .and_then(Weak::upgrade)
        .is_some_and(|registered| Rc::ptr_eq(view, &registered))
}

unsafe extern "C" fn api_create_buffer(
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
        remember_napi_buffer(&environment, &value)?;
        if !data.is_null() {
            unsafe { data.write(data_pointer) };
        }
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_create_buffer_copy(
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
        remember_napi_buffer(&environment, &value)?;
        if !result_data.is_null() {
            unsafe { result_data.write(data_pointer) };
        }
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_buffer_info(
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

unsafe extern "C" fn api_is_buffer(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
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

fn napi_arraybuffer_data(buffer: &Buffer) -> (*mut c_void, usize) {
    let mut bytes = buffer.borrow_mut();
    (bytes.as_mut_ptr().cast::<c_void>(), bytes.len())
}

fn napi_typedarray_data(view: &TypedArrayData) -> Result<(*mut c_void, usize), i32> {
    let byte_length = view
        .length
        .checked_mul(view.kind.size())
        .ok_or(NAPI_INVALID_ARG)?;
    let end = view
        .byte_offset
        .checked_add(byte_length)
        .ok_or(NAPI_INVALID_ARG)?;
    let mut bytes = view.buffer.borrow_mut();
    if end > bytes.len() {
        return Err(NAPI_INVALID_ARG);
    }
    let data = unsafe { bytes.as_mut_ptr().add(view.byte_offset).cast::<c_void>() };
    Ok((data, byte_length))
}

fn napi_typed_kind(kind: i32) -> Option<TypedKind> {
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

fn napi_typed_kind_id(kind: TypedKind) -> i32 {
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

fn validate_arraybuffer_window(
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

unsafe extern "C" fn api_is_arraybuffer(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
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

unsafe extern "C" fn api_create_arraybuffer(
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
        let buffer = Rc::new(RefCell::new(vec![0; byte_length]));
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

unsafe extern "C" fn api_get_arraybuffer_info(
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

unsafe extern "C" fn api_is_typedarray(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
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

unsafe extern "C" fn api_create_typedarray(
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
            buffer: buffer.clone(),
            byte_offset,
            length,
        }));
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_typedarray_info(
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
                    .create(Value::ArrayBuffer(view.buffer.clone()))?,
            )
        };
        if !kind.is_null() {
            unsafe { kind.write(napi_typed_kind_id(view.kind)) };
        }
        if !length.is_null() {
            unsafe { length.write(view.length) };
        }
        if !data.is_null() {
            unsafe { data.write(data_pointer) };
        }
        if let Some(handle) = arraybuffer_handle {
            unsafe { arraybuffer.write(handle) };
        }
        if !byte_offset.is_null() {
            unsafe { byte_offset.write(view.byte_offset) };
        }
        Ok(())
    })
}

unsafe extern "C" fn api_create_dataview(
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
        validate_arraybuffer_window(buffer, byte_offset, byte_length, 1)?;
        let value = Value::DataView(Rc::new(TypedArrayData {
            kind: TypedKind::Uint8,
            buffer: buffer.clone(),
            byte_offset,
            length: byte_length,
        }));
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_is_dataview(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
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

unsafe extern "C" fn api_get_dataview_info(
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
                    .create(Value::ArrayBuffer(view.buffer.clone()))?,
            )
        };
        if !byte_length.is_null() {
            unsafe { byte_length.write(view.length) };
        }
        if !data.is_null() {
            unsafe { data.write(data_pointer) };
        }
        if let Some(handle) = arraybuffer_handle {
            unsafe { arraybuffer.write(handle) };
        }
        if !byte_offset.is_null() {
            unsafe { byte_offset.write(view.byte_offset) };
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

unsafe extern "C" fn api_create_error(
    env: NapiEnv,
    code: NapiValue,
    message: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    unsafe { api_create_error_with_name(env, code, message, result, "Error") }
}

unsafe extern "C" fn api_create_type_error(
    env: NapiEnv,
    code: NapiValue,
    message: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    unsafe { api_create_error_with_name(env, code, message, result, "TypeError") }
}

unsafe extern "C" fn api_create_range_error(
    env: NapiEnv,
    code: NapiValue,
    message: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    unsafe { api_create_error_with_name(env, code, message, result, "RangeError") }
}

fn set_pending_exception(environment: &NapiEnvironment, exception: Value) -> Result<(), i32> {
    let mut pending = environment.pending_exception.borrow_mut();
    if pending.is_some() {
        return Err(NAPI_PENDING_EXCEPTION);
    }
    *pending = Some(exception);
    Ok(())
}

unsafe extern "C" fn api_throw(env: NapiEnv, error: NapiValue) -> i32 {
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

unsafe extern "C" fn api_throw_error(
    env: NapiEnv,
    code: *const c_char,
    message: *const c_char,
) -> i32 {
    unsafe { api_throw_error_with_name(env, code, message, "Error") }
}

unsafe extern "C" fn api_throw_type_error(
    env: NapiEnv,
    code: *const c_char,
    message: *const c_char,
) -> i32 {
    unsafe { api_throw_error_with_name(env, code, message, "TypeError") }
}

unsafe extern "C" fn api_throw_range_error(
    env: NapiEnv,
    code: *const c_char,
    message: *const c_char,
) -> i32 {
    unsafe { api_throw_error_with_name(env, code, message, "RangeError") }
}

unsafe extern "C" fn api_is_exception_pending(env: NapiEnv, result: *mut bool) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        unsafe { result.write(environment.pending_exception.borrow().is_some()) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_and_clear_last_exception(env: NapiEnv, result: *mut NapiValue) -> i32 {
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

unsafe extern "C" fn api_is_error(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
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

unsafe extern "C" fn api_create_reference(
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
        let reference = new_opaque_handle()?;
        environment.references.borrow_mut().insert(
            reference as usize,
            NapiReference {
                value,
                ref_count: initial_ref_count,
            },
        );
        unsafe { result.write(reference) };
        Ok(())
    })
}

unsafe extern "C" fn api_delete_reference(env: NapiEnv, reference: NapiRef) -> i32 {
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

unsafe extern "C" fn api_reference_ref(env: NapiEnv, reference: NapiRef, result: *mut u32) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let mut references = environment.references.borrow_mut();
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

unsafe extern "C" fn api_reference_unref(
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
        if !result.is_null() {
            unsafe { result.write(reference.ref_count) };
        }
        Ok(())
    })
}

unsafe extern "C" fn api_get_reference_value(
    env: NapiEnv,
    reference: NapiRef,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment
            .references
            .borrow()
            .get(&(reference as usize))
            .map(|reference| reference.value.clone())
            .ok_or(NAPI_INVALID_ARG)?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_wrap(
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
                    value: value.clone(),
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

unsafe extern "C" fn api_unwrap(env: NapiEnv, object: NapiValue, result: *mut *mut c_void) -> i32 {
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

unsafe extern "C" fn api_remove_wrap(
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

unsafe extern "C" fn api_create_object(env: NapiEnv, result: *mut NapiValue) -> i32 {
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

unsafe extern "C" fn api_create_promise(
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

unsafe extern "C" fn api_resolve_deferred(
    env: NapiEnv,
    deferred: NapiDeferred,
    resolution: NapiValue,
) -> i32 {
    settle_deferred(env, deferred, resolution, false)
}

unsafe extern "C" fn api_reject_deferred(
    env: NapiEnv,
    deferred: NapiDeferred,
    rejection: NapiValue,
) -> i32 {
    settle_deferred(env, deferred, rejection, true)
}

unsafe extern "C" fn api_is_promise(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
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

unsafe extern "C" fn api_create_async_work(
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

unsafe extern "C" fn api_delete_async_work(env: NapiEnv, work: NapiAsyncWork) -> i32 {
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

unsafe extern "C" fn api_queue_async_work(env: NapiEnv, work: NapiAsyncWork) -> i32 {
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

unsafe extern "C" fn api_cancel_async_work(env: NapiEnv, work: NapiAsyncWork) -> i32 {
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

fn threadsafe_function_registry()
-> &'static Mutex<HashMap<usize, Arc<NapiThreadsafeFunctionShared>>> {
    THREADSAFE_FUNCTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn get_threadsafe_function(
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

unsafe extern "C" fn api_create_threadsafe_function(
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

unsafe extern "C" fn api_get_threadsafe_function_context(
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

unsafe extern "C" fn api_call_threadsafe_function(
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

unsafe extern "C" fn api_acquire_threadsafe_function(function: NapiThreadsafeFunction) -> i32 {
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

unsafe extern "C" fn api_release_threadsafe_function(
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

fn set_threadsafe_function_referenced(
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

unsafe extern "C" fn api_ref_threadsafe_function(
    env: NapiEnv,
    function: NapiThreadsafeFunction,
) -> i32 {
    with_ffi_status(env, || {
        set_threadsafe_function_referenced(env, function, true)
    })
}

unsafe extern "C" fn api_unref_threadsafe_function(
    env: NapiEnv,
    function: NapiThreadsafeFunction,
) -> i32 {
    with_ffi_status(env, || {
        set_threadsafe_function_referenced(env, function, false)
    })
}

unsafe extern "C" fn api_add_env_cleanup_hook(
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
        hooks.push(NapiCleanupHookRecord {
            function,
            function_address,
            argument,
        });
        Ok(())
    })
}

unsafe extern "C" fn api_remove_env_cleanup_hook(
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

fn settle_deferred(
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

fn settle_deferred_without_interpreter(
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

unsafe extern "C" fn api_define_properties(
    env: NapiEnv,
    object: NapiValue,
    property_count: usize,
    properties: *const NapiPropertyDescriptor,
) -> i32 {
    with_ffi_status(env, || {
        if property_count > crate::value::MAX_OBJECT_PROPS {
            return Err(NAPI_GENERIC_FAILURE);
        }
        if property_count > 0 && properties.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if napi_is_external_value(&environment, &object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let props = match &object {
            Value::Object { props } => props,
            Value::Class(class) => &class.statics,
            _ => return Err(NAPI_OBJECT_EXPECTED),
        };
        let descriptors = if property_count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(properties, property_count) }
        };
        for descriptor in descriptors {
            if descriptor.utf8name.is_null() == descriptor.name.is_null() {
                return Err(NAPI_INVALID_ARG);
            }
            let name = if descriptor.utf8name.is_null() {
                environment.handles.borrow().get(descriptor.name)?
            } else {
                Value::String(unsafe { read_c_string(descriptor.utf8name)? })
            };
            let symbol = match &name {
                Value::Symbol(symbol) => Some(symbol.clone()),
                Value::String(_) => None,
                _ => return Err(NAPI_INVALID_ARG),
            };
            let key = napi_property_key(&name)?;
            let has_accessor = descriptor.getter.is_some() || descriptor.setter.is_some();
            if has_accessor && (descriptor.method.is_some() || !descriptor.value.is_null()) {
                return Err(NAPI_INVALID_ARG);
            }
            if !has_accessor && descriptor.method.is_some() && !descriptor.value.is_null() {
                return Err(NAPI_INVALID_ARG);
            }
            let mut descriptor_properties = Vec::with_capacity(5);
            descriptor_properties.push((
                "enumerable".to_owned(),
                Value::Bool(descriptor.attributes & 0b010 != 0),
            ));
            descriptor_properties.push((
                "configurable".to_owned(),
                Value::Bool(descriptor.attributes & 0b100 != 0),
            ));
            if has_accessor {
                if let Some(getter) = descriptor.getter {
                    descriptor_properties.push((
                        "get".to_owned(),
                        create_native_callback_value(
                            &environment,
                            &format!("get {key}"),
                            getter,
                            descriptor.data,
                        )?,
                    ));
                }
                if let Some(setter) = descriptor.setter {
                    descriptor_properties.push((
                        "set".to_owned(),
                        create_native_callback_value(
                            &environment,
                            &format!("set {key}"),
                            setter,
                            descriptor.data,
                        )?,
                    ));
                }
            } else {
                let value = if let Some(method) = descriptor.method {
                    create_native_callback_value(&environment, &key, method, descriptor.data)?
                } else if descriptor.value.is_null() {
                    Value::Undefined
                } else {
                    environment.handles.borrow().get(descriptor.value)?
                };
                descriptor_properties.push(("value".to_owned(), value));
                descriptor_properties.push((
                    "writable".to_owned(),
                    Value::Bool(descriptor.attributes & 0b001 != 0),
                ));
            }
            let descriptor_value = Value::object(descriptor_properties);
            crate::builtins::object::define_property(&object, &key, &descriptor_value)
                .map_err(|_| NAPI_GENERIC_FAILURE)?;
            if let Some(symbol) = symbol {
                props.meta.borrow_mut().set_symbol_key(&key, symbol);
            }
        }
        Ok(())
    })
}

unsafe extern "C" fn api_define_class(
    env: NapiEnv,
    name: *const c_char,
    name_length: usize,
    constructor: Option<NapiCallback>,
    data: *mut c_void,
    property_count: usize,
    properties: *const NapiPropertyDescriptor,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        if property_count > crate::value::MAX_OBJECT_PROPS {
            return Err(NAPI_GENERIC_FAILURE);
        }
        if property_count > 0 && properties.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let constructor = constructor.ok_or(NAPI_FUNCTION_EXPECTED)?;
        let environment = environment(env)?;
        let class_name = if name_length == usize::MAX {
            unsafe { read_c_string(name)? }
        } else if name_length == 0 {
            String::new()
        } else {
            if name.is_null() {
                return Err(NAPI_INVALID_ARG);
            }
            let bytes = unsafe { std::slice::from_raw_parts(name.cast::<u8>(), name_length) };
            std::str::from_utf8(bytes)
                .map_err(|_| NAPI_INVALID_ARG)?
                .to_owned()
        };
        let descriptors = if property_count == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(properties, property_count) }
        };
        let native_constructor =
            create_native_callback_value(&environment, &class_name, constructor, data)?;
        let prototype = Value::object(Vec::new());
        let statics = Rc::new(crate::value::ObjectCell::new_with_default_proto(vec![
            ("name".to_owned(), Value::String(class_name.clone())),
            ("prototype".to_owned(), prototype.clone()),
            ("length".to_owned(), Value::Number(0.0)),
        ]));
        {
            let mut meta = statics.meta.borrow_mut();
            meta.set_attrs(
                "name",
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
            meta.set_attrs(
                "prototype",
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: false,
                },
            );
            meta.set_attrs(
                "length",
                PropAttrs {
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        let class = Value::Class(Box::new(ClassData {
            name: class_name,
            constructor: Box::new(native_constructor),
            prototype: Rc::new(prototype.clone()),
            statics,
        }));
        prototype
            .set_prop("constructor".to_owned(), class.clone())
            .map_err(|_| NAPI_GENERIC_FAILURE)?;
        if let Value::Object { props } = &prototype {
            props.meta.borrow_mut().set_attrs(
                "constructor",
                PropAttrs {
                    writable: true,
                    enumerable: false,
                    configurable: true,
                },
            );
        }
        let prototype_handle = environment.handles.borrow_mut().create(prototype)?;
        let class_handle = environment.handles.borrow_mut().create(class.clone())?;
        for descriptor in descriptors {
            let target = if descriptor.attributes & NAPI_PROPERTY_STATIC != 0 {
                class_handle
            } else {
                prototype_handle
            };
            let status = unsafe { api_define_properties(env, target, 1, descriptor) };
            if status != NAPI_OK {
                return Err(status);
            }
        }
        unsafe { result.write(class_handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_create_function(
    env: NapiEnv,
    name: *const c_char,
    name_length: usize,
    callback: Option<NapiCallback>,
    data: *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let callback = callback.ok_or(NAPI_FUNCTION_EXPECTED)?;
        let environment = environment(env)?;
        let function_name = if name_length == usize::MAX {
            unsafe { read_c_string(name)? }
        } else if name_length == 0 {
            String::new()
        } else {
            if name.is_null() {
                return Err(NAPI_INVALID_ARG);
            }
            let bytes = unsafe { std::slice::from_raw_parts(name.cast::<u8>(), name_length) };
            std::str::from_utf8(bytes)
                .map_err(|_| NAPI_INVALID_ARG)?
                .to_owned()
        };
        let value = create_native_callback_value(&environment, &function_name, callback, data)?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_set_named_property(
    env: NapiEnv,
    object: NapiValue,
    name: *const c_char,
    value: NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        let key = unsafe { read_c_string(name)? };
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        let value = environment.handles.borrow().get(value)?;
        if napi_is_external_value(&environment, &object) {
            // Node accepts these writes but an external has no property slots.
            return Ok(());
        }
        if !matches!(
            object,
            Value::Object { .. } | Value::Array(_) | Value::GlobalObject
        ) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_set_named_property",
                napi_guest_set_property,
                object,
                vec![Value::String(key), value],
            )?;
            Ok(())
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_set(&environment, &key, value)
            } else {
                object
                    .set_prop(key, value)
                    .map_err(|_| NAPI_GENERIC_FAILURE)
            }
        }
    })
}

unsafe extern "C" fn api_get_named_property(
    env: NapiEnv,
    object: NapiValue,
    name: *const c_char,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let key = unsafe { read_c_string(name)? };
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) && !matches!(object, Value::String(_)) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let value = if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_get_named_property",
                napi_guest_get_property,
                object,
                vec![Value::String(key)],
            )?
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_get(&environment, &key)?
            } else {
                object.get_prop(&key).unwrap_or(Value::Undefined)
            }
        };
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_property(
    env: NapiEnv,
    object: NapiValue,
    key: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let key = environment.handles.borrow().get(key)?;
        let value = if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_get_property",
                napi_guest_get_property,
                object,
                vec![key],
            )?
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_get(&environment, &napi_property_key(&key)?)?
            } else {
                napi_direct_get_property(&object, &key)?
            }
        };
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_set_property(
    env: NapiEnv,
    object: NapiValue,
    key: NapiValue,
    value: NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let key = environment.handles.borrow().get(key)?;
        let value = environment.handles.borrow().get(value)?;
        if napi_is_external_value(&environment, &object) {
            // Match napi_set_named_property: native writes to an external are
            // successful but cannot add JavaScript properties.
            return Ok(());
        }
        if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_set_property",
                napi_guest_set_property,
                object,
                vec![key, value],
            )?;
            Ok(())
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_set(&environment, &napi_property_key(&key)?, value)
            } else {
                napi_direct_set_property(&object, &key, value)
            }
        }
    })
}

unsafe extern "C" fn api_has_property(
    env: NapiEnv,
    object: NapiValue,
    key: NapiValue,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let key = environment.handles.borrow().get(key)?;
        let found = if has_guest_callback_dispatcher(&environment) {
            let value = run_napi_guest_operation(
                &environment,
                "napi_has_property",
                napi_guest_has_property,
                object,
                vec![key],
            )?;
            let Value::Bool(found) = value else {
                return Err(NAPI_GENERIC_FAILURE);
            };
            found
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_has(&environment, &napi_property_key(&key)?)?
            } else {
                object.has_prop(&napi_property_key(&key)?)
            }
        };
        unsafe { result.write(found) };
        Ok(())
    })
}

unsafe extern "C" fn api_delete_property(
    env: NapiEnv,
    object: NapiValue,
    key: NapiValue,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        let key = environment.handles.borrow().get(key)?;
        let deleted =
            napi_delete_property_value(&environment, object, key, "napi_delete_property")?;
        if !result.is_null() {
            unsafe { result.write(deleted) };
        }
        Ok(())
    })
}

unsafe extern "C" fn api_delete_element(
    env: NapiEnv,
    object: NapiValue,
    index: u32,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        let key = Value::String(index.to_string());
        let deleted = napi_delete_property_value(&environment, object, key, "napi_delete_element")?;
        if !result.is_null() {
            unsafe { result.write(deleted) };
        }
        Ok(())
    })
}

fn napi_delete_property_value(
    environment: &NapiEnvironment,
    object: Value,
    key: Value,
    operation_name: &'static str,
) -> Result<bool, i32> {
    if !is_napi_property_object(&object) {
        return Err(NAPI_OBJECT_EXPECTED);
    }
    if has_guest_callback_dispatcher(environment) {
        let value = run_napi_guest_operation(
            environment,
            operation_name,
            napi_guest_delete_property,
            object,
            vec![key],
        )?;
        let Value::Bool(deleted) = value else {
            return Err(NAPI_GENERIC_FAILURE);
        };
        Ok(deleted)
    } else if matches!(object, Value::GlobalObject) {
        napi_global_delete(environment, &napi_property_key(&key)?)
    } else {
        napi_direct_delete_property(&object, &key)
    }
}

unsafe extern "C" fn api_has_own_property(
    env: NapiEnv,
    object: NapiValue,
    key: NapiValue,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let key = environment.handles.borrow().get(key)?;
        if !matches!(key, Value::String(_) | Value::Symbol(_)) {
            set_pending_exception(
                &environment,
                Value::Error(ErrorData::new(
                    "TypeError",
                    "property key must be a string or symbol",
                )),
            )?;
            return Err(NAPI_PENDING_EXCEPTION);
        }
        let found = if has_guest_callback_dispatcher(&environment) {
            let value = run_napi_guest_operation(
                &environment,
                "napi_has_own_property",
                napi_guest_has_own_property,
                object,
                vec![key],
            )?;
            let Value::Bool(found) = value else {
                return Err(NAPI_GENERIC_FAILURE);
            };
            found
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_has_own(&environment, &napi_property_key(&key)?)?
            } else {
                napi_direct_has_own_property(&object, &key)?
            }
        };
        unsafe { result.write(found) };
        Ok(())
    })
}

unsafe extern "C" fn api_has_named_property(
    env: NapiEnv,
    object: NapiValue,
    name: *const c_char,
    result: *mut bool,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let name = unsafe { read_c_string(name)? };
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let found = if has_guest_callback_dispatcher(&environment) {
            let value = run_napi_guest_operation(
                &environment,
                "napi_has_named_property",
                napi_guest_has_property,
                object,
                vec![Value::String(name)],
            )?;
            let Value::Bool(found) = value else {
                return Err(NAPI_GENERIC_FAILURE);
            };
            found
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_has(&environment, &name)?
            } else {
                object.has_prop(&name)
            }
        };
        unsafe { result.write(found) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_property_names(
    env: NapiEnv,
    object: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let names = if has_guest_callback_dispatcher(&environment) {
            run_napi_guest_operation(
                &environment,
                "napi_get_property_names",
                napi_guest_get_property_names,
                object,
                Vec::new(),
            )?
        } else {
            if matches!(object, Value::GlobalObject) {
                Value::checked_array(
                    napi_global_scope(&environment)?
                        .borrow()
                        .all_keys()
                        .into_iter()
                        .filter(|key| !crate::interpreter::is_internal_key(key))
                        .map(Value::String)
                        .collect(),
                )
                .map_err(|_| NAPI_GENERIC_FAILURE)?
            } else {
                napi_direct_property_names(&object)?
            }
        };
        let handle = environment.handles.borrow_mut().create(names)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_call_function(
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
        let receiver = environment.handles.borrow().get(recv)?;
        let function = environment.handles.borrow().get(function)?;
        if !is_napi_function(&function) {
            return Err(NAPI_FUNCTION_EXPECTED);
        }
        let args = callback_arguments(&environment, argc, argv)?;
        let value = call_guest_callback(
            &environment,
            HostCallback {
                callback: function,
                this_value: receiver,
                args,
                kind: HostCallbackKind::Call,
            },
        )?;
        let handle = environment.handles.borrow_mut().create(value)?;
        unsafe { result.write(handle) };
        Ok(())
    })
}

unsafe extern "C" fn api_new_instance(
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

unsafe extern "C" fn api_instanceof(
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
        if !is_napi_function(&constructor) {
            return Err(NAPI_FUNCTION_EXPECTED);
        }
        let Value::Class(class) = &constructor else {
            // The VM does not yet materialize [[Prototype]] for ordinary
            // Function values or implement callable-proxy [[HasInstance]].
            return Err(NAPI_GENERIC_FAILURE);
        };
        let has_instance = Value::Object {
            props: class.statics.clone(),
        }
        .get_prop("__symbol:4__");
        if has_instance.is_some_and(|value| !matches!(value, Value::Undefined | Value::Null)) {
            // Honor custom @@hasInstance code only after this API has a safe
            // guest callback path for that call from addon code.
            return Err(NAPI_GENERIC_FAILURE);
        }
        let is_instance = napi_class_instanceof(&object, class)?;
        unsafe { result.write(is_instance) };
        Ok(())
    })
}

fn napi_class_instanceof(object: &Value, class: &ClassData) -> Result<bool, i32> {
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
        if super::strict_equals(current.as_ref(), class.prototype.as_ref()) {
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

unsafe extern "C" fn api_get_cb_info(
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

unsafe extern "C" fn api_get_new_target(
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

unsafe extern "C" fn api_adjust_external_memory(
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

unsafe extern "C" fn api_get_version(env: NapiEnv, result: *mut u32) -> i32 {
    with_ffi_status(env, || {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let _environment = environment(env)?;
        unsafe { result.write(MAX_NODE_API_VERSION as u32) };
        Ok(())
    })
}

unsafe extern "C" fn api_strict_equals(
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
        unsafe { result.write(super::strict_equals(&left, &right)) };
        Ok(())
    })
}

fn napi_guest_run_script(
    interpreter: &mut Interpreter,
    _receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let Some(Value::String(source)) = args.first() else {
        return Err(VmErr::Msg("TypeError: script must be a string".into()));
    };
    interpreter.run_script_source(source)
}

unsafe extern "C" fn api_run_script(
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

unsafe extern "C" fn api_open_handle_scope(env: NapiEnv, result: *mut NapiHandleScope) -> i32 {
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

unsafe extern "C" fn api_close_handle_scope(env: NapiEnv, scope: NapiHandleScope) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        environment.handles.borrow_mut().close_scope_handle(scope)
    })
}

unsafe extern "C" fn api_open_escapable_handle_scope(
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

unsafe extern "C" fn api_close_escapable_handle_scope(env: NapiEnv, scope: NapiHandleScope) -> i32 {
    with_ffi_status(env, || {
        let environment = environment(env)?;
        environment
            .handles
            .borrow_mut()
            .close_escapable_scope_handle(scope)
    })
}

unsafe extern "C" fn api_escape_handle(
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

fn to_int32(number: f64) -> i32 {
    if !number.is_finite() || number == 0.0 {
        return 0;
    }
    let modulo = number.trunc().rem_euclid(4_294_967_296.0) as u32;
    modulo as i32
}

struct NodeApiShim {
    _library: Library,
    path: PathBuf,
}

impl NodeApiShim {
    fn load() -> Result<Self, VmErr> {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let bytes = include_bytes!(env!("NAPI_VM_NODE_API_SHIM_PATH"));
        let root = loop {
            let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let candidate = std::env::temp_dir()
                .join(format!("napi-vm-node-api-{}-{nonce}", std::process::id()));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(VmErr::Msg(format!(
                        "cannot create private Node-API shim directory: {error}"
                    )));
                }
            }
        };
        #[cfg(target_os = "linux")]
        let path = root.join("libnapi_vm_node_api_shim.so");
        #[cfg(target_os = "macos")]
        let path = root.join("libnapi_vm_node_api_shim.dylib");
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o500))?;
            }
            Ok::<_, std::io::Error>(())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_dir_all(&root);
            return Err(VmErr::Msg(format!(
                "cannot materialize Node-API symbol shim: {error}"
            )));
        }

        let library = unsafe { Library::open(Some(path.as_os_str()), RTLD_NOW | RTLD_GLOBAL) }
            .map_err(|error| {
                let _ = fs::remove_dir_all(&root);
                VmErr::Msg(format!("cannot load Node-API symbol shim: {error}"))
            })?;
        let install: unsafe extern "C" fn(*const NapiVmApiTable) =
            match unsafe { library.get(b"napi_vm_install_node_api_table\0") } {
                Ok(symbol) => *symbol,
                Err(error) => {
                    drop(library);
                    let _ = fs::remove_dir_all(&root);
                    return Err(VmErr::Msg(format!("invalid Node-API symbol shim: {error}")));
                }
            };
        unsafe { install(&NAPI_VM_API_TABLE) };
        Ok(Self {
            _library: library,
            path,
        })
    }
}

impl Drop for NodeApiShim {
    fn drop(&mut self) {
        // Linux and macOS permit unlinking a loaded shared object; the mapping
        // remains live until the Library is dropped immediately after this method.
        if let Some(root) = self.path.parent() {
            let _ = fs::remove_dir_all(root);
        }
    }
}

fn create_async_work_pool(
    runtime_notification_sender: Sender<HostRuntimeNotification>,
) -> Result<AsyncWorkPool, VmErr> {
    let (task_sender, task_receiver) = mpsc::sync_channel(ASYNC_WORK_QUEUE_CAPACITY);
    let task_receiver = Arc::new(Mutex::new(task_receiver));
    let mut workers: Vec<JoinHandle<()>> = Vec::with_capacity(ASYNC_WORKER_COUNT);

    for worker_id in 0..ASYNC_WORKER_COUNT {
        let task_receiver = task_receiver.clone();
        let runtime_notification_sender = runtime_notification_sender.clone();
        let worker = thread::Builder::new()
            .name(format!("napi-vm-addon-{worker_id}"))
            .spawn(move || {
                loop {
                    let message = match task_receiver.lock() {
                        Ok(receiver) => receiver.recv(),
                        Err(_) => return,
                    };
                    match message {
                        Ok(AsyncWorkTaskMessage::Run(task)) => {
                            let status = if task
                                .state
                                .compare_exchange(
                                    ASYNC_WORK_QUEUED,
                                    ASYNC_WORK_RUNNING,
                                    Ordering::AcqRel,
                                    Ordering::Acquire,
                                )
                                .is_ok()
                            {
                                // Node-API forbids using env from an execute callback.
                                // Preserve the ABI argument for addons that only inspect it.
                                unsafe {
                                    (task.execute)(
                                        task.environment as NapiEnv,
                                        task.data as *mut c_void,
                                    );
                                }
                                NAPI_OK
                            } else if task.state.load(Ordering::Acquire) == ASYNC_WORK_CANCELLED {
                                NAPI_CANCELLED
                            } else {
                                NAPI_GENERIC_FAILURE
                            };
                            task.state.store(ASYNC_WORK_FINISHED, Ordering::Release);
                            task.completion_status
                                .store(status as u8, Ordering::Release);
                            let _ = runtime_notification_sender.send(
                                HostRuntimeNotification::AsyncWorkCompletion(AsyncWorkCompletion {
                                    work_id: task.work_id,
                                    status,
                                }),
                            );
                        }
                        Ok(AsyncWorkTaskMessage::Stop) | Err(_) => return,
                    }
                }
            })
            .map_err(|error| {
                for _ in 0..workers.len() {
                    let _ = task_sender.send(AsyncWorkTaskMessage::Stop);
                }
                for worker in workers.drain(..) {
                    let _ = worker.join();
                }
                VmErr::Msg(format!("cannot start Node-API worker pool: {error}"))
            })?;
        workers.push(worker);
    }

    Ok((task_sender, workers))
}

impl RustNodeApiHost {
    fn new(global: Env) -> Result<Self, VmErr> {
        let object_prototype = global
            .borrow()
            .get("Object")
            .and_then(|object| object.get_prop("prototype"));
        let shim = Rc::new(NodeApiShim::load()?);
        let (runtime_notification_sender, runtime_notifications) = mpsc::channel();
        let (async_work_sender, async_workers) =
            create_async_work_pool(runtime_notification_sender.clone())?;
        Ok(Self {
            state: Rc::new(RefCell::new(HostState {
                global,
                object_prototype,
                next_callback_id: 1,
                callbacks: HashMap::new(),
                environments: Vec::new(),
                libraries: Vec::new(),
                async_work_sender,
                runtime_notifications,
                runtime_notification_sender,
                async_workers,
                _shim: shim.clone(),
            })),
            _shim: shim,
        })
    }

    fn invoke_native(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        new_target: Option<Value>,
        mut callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        let callback = {
            let mut state = self.state.borrow_mut();
            let callback =
                state.callbacks.get(&id).cloned().ok_or_else(|| {
                    VmErr::Msg("native callback handle is no longer valid".into())
                })?;
            if callback.one_shot {
                state.callbacks.remove(&id);
            }
            callback
        };
        let scope = callback
            .env
            .handles
            .borrow_mut()
            .open_scope()
            .map_err(|status| napi_error("opening callback handle scope", status))?;
        let mut threadsafe_call = None;
        let result = (|| {
            let callback_handler_pointer: *mut &mut (
                     dyn FnMut(HostCallback) -> Result<Value, VmErr> + '_
                 ) = &mut callback_handler;
            let dispatcher = GuestCallbackDispatcher {
                context: callback_handler_pointer.cast(),
                invoke: dispatch_guest_callback,
            };
            let dispatcher_scope =
                GuestCallbackDispatcherScope::push(callback.env.clone(), dispatcher);
            let (returned, completion_work_id) = match callback.callback {
                NativeCallback::Function(callback_fn) => {
                    let this_arg = callback
                        .env
                        .handles
                        .borrow_mut()
                        .create(this_value)
                        .map_err(|status| {
                            napi_error("creating callback receiver handle", status)
                        })?;
                    let new_target = match new_target {
                        Some(new_target) => callback
                            .env
                            .handles
                            .borrow_mut()
                            .create(new_target)
                            .map_err(|status| napi_error("creating new.target handle", status))?,
                        None => std::ptr::null_mut(),
                    };
                    let arg_handles = args
                        .into_iter()
                        .map(|value| {
                            callback
                                .env
                                .handles
                                .borrow_mut()
                                .create(value)
                                .map_err(|status| {
                                    napi_error("creating callback argument handle", status)
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let frame = CallbackFrame {
                        args: arg_handles,
                        this_arg,
                        new_target,
                        data: callback.data,
                    };
                    let callback_info =
                        (&frame as *const CallbackFrame).cast_mut().cast::<c_void>();
                    let frame_key = callback_info as usize;
                    callback
                        .env
                        .active_callbacks
                        .borrow_mut()
                        .insert(frame_key, frame.clone());
                    let returned = unsafe { callback_fn(callback.env.raw(), callback_info) };
                    callback
                        .env
                        .active_callbacks
                        .borrow_mut()
                        .remove(&frame_key);
                    (returned, None)
                }
                NativeCallback::AsyncComplete {
                    callback: callback_fn,
                    status,
                    work_id,
                } => {
                    if let Some(work) = callback.env.async_works.borrow_mut().get_mut(&work_id) {
                        work.completion_callback_active = true;
                    }
                    unsafe { callback_fn(callback.env.raw(), status, callback.data) };
                    if let Some(work) = callback.env.async_works.borrow_mut().get_mut(&work_id) {
                        work.completion_callback_active = false;
                    }
                    (std::ptr::null_mut(), Some(work_id))
                }
                NativeCallback::ThreadsafeFunctionCall {
                    callback: js_callback,
                    call_js,
                    context,
                    shared,
                } => {
                    threadsafe_call = Some(shared);
                    if let Some(call_js) = call_js {
                        let callback_handle = match js_callback {
                            Some(js_callback) => callback
                                .env
                                .handles
                                .borrow_mut()
                                .create(js_callback)
                                .map_err(|status| {
                                    napi_error("creating thread-safe callback handle", status)
                                })?,
                            None => std::ptr::null_mut(),
                        };
                        unsafe {
                            call_js(callback.env.raw(), callback_handle, context, callback.data);
                        }
                    } else if let Some(js_callback) = js_callback {
                        let _ = call_guest_callback(
                            &callback.env,
                            HostCallback {
                                callback: js_callback,
                                this_value: Value::Undefined,
                                args: Vec::new(),
                                kind: HostCallbackKind::Call,
                            },
                        );
                    }
                    (std::ptr::null_mut(), None)
                }
            };
            drop(dispatcher_scope);
            if let Some(work_id) = completion_work_id
                && let Some(work) = callback.env.async_works.borrow_mut().get_mut(&work_id)
            {
                work.callback_run = true;
            }
            if let Some(exception) = callback.env.pending_exception.borrow_mut().take() {
                Err(VmErr::Throw(exception))
            } else if returned.is_null() {
                Ok(Value::Undefined)
            } else {
                callback
                    .env
                    .handles
                    .borrow()
                    .get(returned)
                    .map_err(|status| napi_error("reading native callback result", status))
            }
        })();
        let close_result = callback
            .env
            .handles
            .borrow_mut()
            .close_scope(scope)
            .map_err(|status| napi_error("closing callback handle scope", status));
        let threadsafe_result = if let Some(shared) = threadsafe_call {
            finish_threadsafe_call(&callback.env, &shared)
        } else {
            Ok(())
        };
        match (result, close_result, threadsafe_result) {
            (Ok(value), Ok(()), Ok(())) => Ok(value),
            (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
        }
    }
}

fn finish_threadsafe_call(
    environment: &Rc<NapiEnvironment>,
    shared: &Arc<NapiThreadsafeFunctionShared>,
) -> Result<(), VmErr> {
    {
        let mut state = shared
            .state
            .lock()
            .map_err(|_| VmErr::Msg("Node-API thread-safe function state is poisoned".into()))?;
        state.in_flight = state.in_flight.saturating_sub(1);
        shared.queue_space.notify_all();
    }
    finalize_threadsafe_function(environment, shared)
}

fn finalize_threadsafe_function(
    environment: &Rc<NapiEnvironment>,
    shared: &Arc<NapiThreadsafeFunctionShared>,
) -> Result<(), VmErr> {
    let ready = {
        let mut state = shared
            .state
            .lock()
            .map_err(|_| VmErr::Msg("Node-API thread-safe function state is poisoned".into()))?;
        if state.finalized
            || state.thread_count != 0
            || !state.values.is_empty()
            || state.in_flight != 0
        {
            false
        } else {
            state.finalized = true;
            true
        }
    };
    if !ready {
        return Ok(());
    }

    let function = environment
        .threadsafe_functions
        .borrow_mut()
        .remove(&shared.id);
    if let Ok(mut registry) = threadsafe_function_registry().lock() {
        registry.remove(&shared.id);
    }
    if let Some(function) = function
        && let Some(finalize) = function.finalize
    {
        let scope = environment
            .handles
            .borrow_mut()
            .open_scope()
            .map_err(|status| napi_error("opening thread-safe finalizer scope", status))?;
        unsafe {
            finalize(environment.raw(), function.finalize_data, function.context);
        }
        environment.pending_exception.borrow_mut().take();
        environment
            .handles
            .borrow_mut()
            .close_scope(scope)
            .map_err(|status| napi_error("closing thread-safe finalizer scope", status))?;
    }
    Ok(())
}

fn shutdown_threadsafe_functions(
    host_state: &Rc<RefCell<HostState>>,
    environments: &[Rc<NapiEnvironment>],
) -> bool {
    let mut has_active_native_threads = false;
    for environment in environments {
        let functions = environment
            .threadsafe_functions
            .borrow()
            .iter()
            .map(|(id, function)| {
                (
                    *id,
                    function.shared.clone(),
                    function.call_js,
                    function.context,
                )
            })
            .collect::<Vec<_>>();
        for (_, shared, call_js, context) in functions {
            let (queued, active) = match shared.state.lock() {
                Ok(mut queue) => {
                    queue.closing = true;
                    queue.orphaned = true;
                    let queued = queue.values.drain(..).collect::<Vec<_>>();
                    let active = queue.thread_count != 0;
                    queue.in_flight = 0;
                    shared.queue_space.notify_all();
                    (queued, active)
                }
                Err(_) => (Vec::new(), true),
            };

            let callbacks = {
                let mut state = host_state.borrow_mut();
                let ids = state
                    .callbacks
                    .iter()
                    .filter_map(|(callback_id, record)| {
                        matches!(
                            &record.callback,
                            NativeCallback::ThreadsafeFunctionCall {
                                shared: record_shared,
                                ..
                            } if Arc::ptr_eq(record_shared, &shared)
                        )
                        .then_some(*callback_id)
                    })
                    .collect::<Vec<_>>();
                ids.into_iter()
                    .filter_map(|callback_id| state.callbacks.remove(&callback_id))
                    .collect::<Vec<_>>()
            };

            if let Some(call_js) = call_js {
                for data in queued {
                    unsafe {
                        call_js(
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            context,
                            data as *mut c_void,
                        );
                    }
                }
                for callback in &callbacks {
                    unsafe {
                        call_js(
                            std::ptr::null_mut(),
                            std::ptr::null_mut(),
                            context,
                            callback.data,
                        );
                    }
                }
            }
            if active {
                has_active_native_threads = true;
            } else if let Err(error) = finalize_threadsafe_function(environment, &shared) {
                eprintln!("failed to finalize Node-API thread-safe function: {error}");
            }
        }
        environment.threadsafe_functions.borrow_mut().clear();
    }
    has_active_native_threads
}

fn take_threadsafe_function_queue(
    shared: &Arc<NapiThreadsafeFunctionShared>,
) -> Result<Vec<usize>, i32> {
    let mut state = shared.state.lock().map_err(|_| NAPI_GENERIC_FAILURE)?;
    let values = state.values.drain(..).collect::<Vec<_>>();
    state.in_flight = state
        .in_flight
        .checked_add(values.len())
        .ok_or(NAPI_GENERIC_FAILURE)?;
    shared.queue_space.notify_all();
    Ok(values)
}

fn thread_safe_function_events(function_id: usize) -> Result<Vec<HostEvent>, VmErr> {
    let Some(shared) = threadsafe_function_registry()
        .lock()
        .map_err(|_| VmErr::Msg("Node-API thread-safe function registry is poisoned".into()))?
        .get(&function_id)
        .cloned()
    else {
        return Ok(Vec::new());
    };
    let environment = environment(shared.environment as NapiEnv)
        .map_err(|status| napi_error("reading thread-safe function environment", status))?;
    let values = take_threadsafe_function_queue(&shared)
        .map_err(|status| napi_error("draining thread-safe function queue", status))?;
    let (js_callback, call_js, context) = environment
        .threadsafe_functions
        .borrow()
        .get(&function_id)
        .map(|function| {
            (
                function.callback.clone(),
                function.call_js,
                function.context,
            )
        })
        .ok_or_else(|| VmErr::Msg("Node-API thread-safe function was finalized early".into()))?;

    let mut events = Vec::with_capacity(values.len());
    let value_count = values.len();
    for data in values {
        let callback = match create_native_callback_value_with_kind(
            &environment,
            "napi_threadsafe_function_call",
            NativeCallback::ThreadsafeFunctionCall {
                callback: js_callback.clone(),
                call_js,
                context,
                shared: shared.clone(),
            },
            data as *mut c_void,
            true,
        ) {
            Ok(callback) => callback,
            Err(status) => {
                let mut state = shared.state.lock().map_err(|_| {
                    VmErr::Msg("Node-API thread-safe function state is poisoned".into())
                })?;
                state.in_flight = state.in_flight.saturating_sub(value_count - events.len());
                return Err(napi_error("creating thread-safe callback", status));
            }
        };
        events.push(HostEvent::Callback(HostCallback {
            callback,
            this_value: Value::Undefined,
            args: Vec::new(),
            kind: HostCallbackKind::Call,
        }));
    }
    if events.is_empty() {
        finalize_threadsafe_function(&environment, &shared)?;
    }
    Ok(events)
}

impl NativeAddonLoader for RustNodeApiHost {
    fn load(&self, filename: &Path) -> Result<Value, VmErr> {
        let filename = filename
            .to_str()
            .ok_or_else(|| VmErr::Msg("native addon path is not UTF-8".into()))?;
        let library = unsafe { Library::open(Some(Path::new(filename).as_os_str()), RTLD_NOW) }
            .map_err(|error| {
                VmErr::Msg(format!(
                    "cannot load Node-API addon {filename}: {error}; the binary may require an unavailable symbol or dependency"
                ))
            })?;
        let api_version: unsafe extern "C" fn() -> i32 = unsafe {
            *library
                .get(b"node_api_module_get_api_version_v1\0")
                .map_err(|error| {
                    VmErr::Msg(format!(
                        "{} is not a symbol-registered Node-API addon: {error}",
                        filename
                    ))
                })?
        };
        let version = unsafe { api_version() };
        if !(1..=MAX_NODE_API_VERSION).contains(&version) {
            return Err(VmErr::Msg(format!(
                "Node-API addon {filename} requests version {version}; this host supports Node-API versions 1 through {MAX_NODE_API_VERSION}"
            )));
        }
        let initialize: unsafe extern "C" fn(NapiEnv, NapiValue) -> NapiValue = unsafe {
            *library.get(b"napi_register_module_v1\0").map_err(|error| {
                VmErr::Msg(format!(
                    "{} has no Node-API v1 module initializer: {error}",
                    filename
                ))
            })?
        };

        let environment = Rc::new(NapiEnvironment {
            module_path: filename.to_string(),
            owner: Rc::downgrade(&self.state),
            handles: RefCell::new(NapiHandleArena::default()),
            references: RefCell::new(HashMap::new()),
            deferreds: RefCell::new(HashMap::new()),
            async_works: RefCell::new(HashMap::new()),
            threadsafe_functions: RefCell::new(HashMap::new()),
            cleanup_hooks: RefCell::new(Vec::new()),
            last_error: Cell::new(napi_extended_error_info(NAPI_OK)),
            wraps: RefCell::new(HashMap::new()),
            externals: RefCell::new(HashMap::new()),
            external_memory: Cell::new(0),
            buffer_values: RefCell::new(HashMap::new()),
            finalizing: Cell::new(false),
            active_callbacks: RefCell::new(HashMap::new()),
            guest_callback_dispatchers: RefCell::new(Vec::new()),
            pending_exception: RefCell::new(None),
        });
        register_environment(&environment);
        self.state
            .borrow_mut()
            .environments
            .push(environment.clone());
        let exports = Value::object(Vec::new());
        let scope = environment
            .handles
            .borrow_mut()
            .open_scope()
            .map_err(|status| napi_error("opening addon initialization scope", status))?;
        let exports_handle = environment
            .handles
            .borrow_mut()
            .create(exports)
            .map_err(|status| napi_error("creating addon exports handle", status))?;
        let returned = unsafe { initialize(environment.raw(), exports_handle) };
        let pending_exception = environment.pending_exception.borrow_mut().take();
        let result = if let Some(exception) = pending_exception {
            Err(VmErr::Throw(exception))
        } else if returned.is_null() {
            Err(VmErr::Msg(format!(
                "Node-API initializer returned a null napi_value: {filename}"
            )))
        } else {
            environment
                .handles
                .borrow()
                .get(returned)
                .map_err(|status| napi_error("reading addon exports", status))
        };
        let close_result = environment
            .handles
            .borrow_mut()
            .close_scope(scope)
            .map_err(|status| napi_error("closing addon initialization scope", status));
        let exports = match (result, close_result) {
            (Ok(exports), Ok(())) => exports,
            (Err(error), _) | (_, Err(error)) => {
                let has_async_work = !environment.async_works.borrow().is_empty();
                self.state
                    .borrow_mut()
                    .callbacks
                    .retain(|_, callback| callback.env.module_path != filename);
                if has_async_work {
                    // A queued execute callback can still be running addon
                    // code, and its completion callback owns the work data.
                    // Keep both environment and library alive until host
                    // shutdown joins the workers and delivers that callback.
                    self.state.borrow_mut().libraries.push(library);
                } else {
                    environment.finalizing.set(true);
                    finalize_environment_wraps(&environment);
                    self.state
                        .borrow_mut()
                        .environments
                        .retain(|env| env.module_path != filename);
                }
                return Err(error);
            }
        };
        self.state.borrow_mut().libraries.push(library);
        Ok(exports)
    }
}

impl HostBridge for RustNodeApiHost {
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        self.invoke_native(id, Value::Undefined, args, None, &mut reject_guest_callback)
    }

    fn call_host_with_this(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        self.invoke_native(id, this_value, args, None, &mut reject_guest_callback)
    }

    fn call_host_with_callback_handler(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.invoke_native(id, this_value, args, None, callback_handler)
    }

    fn construct_host_with_callback_handler(
        &self,
        id: usize,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.invoke_native(id, Value::Undefined, args, None, callback_handler)
    }

    fn construct_host_with_callback_handler_and_target(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        new_target: Value,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.invoke_native(id, this_value, args, Some(new_target), callback_handler)
    }

    fn call_host_constructor_with_callback_handler_and_target(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        new_target: Value,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.invoke_native(id, this_value, args, Some(new_target), callback_handler)
    }

    fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        let notifications = {
            let state = self.state.borrow();
            let first = if timeout.is_zero() {
                state.runtime_notifications.try_recv().ok()
            } else {
                state.runtime_notifications.recv_timeout(timeout).ok()
            };
            first
                .into_iter()
                .chain(state.runtime_notifications.try_iter())
                .collect::<Vec<_>>()
        };
        let mut events = Vec::with_capacity(notifications.len());
        for notification in notifications {
            let HostRuntimeNotification::AsyncWorkCompletion(completion) = notification else {
                let HostRuntimeNotification::ThreadsafeFunction(function_id) = notification else {
                    unreachable!();
                };
                events.extend(thread_safe_function_events(function_id)?);
                continue;
            };
            let work = {
                let state = self.state.borrow();
                state.environments.iter().find_map(|environment| {
                    environment
                        .async_works
                        .borrow()
                        .get(&completion.work_id)
                        .cloned()
                        .map(|work| (environment.clone(), work))
                })
            };
            let Some((environment, work)) = work else {
                continue;
            };
            let callback = create_native_async_complete_value(
                &environment,
                work.complete,
                completion.status,
                work.data,
                completion.work_id,
            )
            .map_err(|status| napi_error("creating async-work completion callback", status))?;
            events.push(HostEvent::Callback(HostCallback {
                callback,
                this_value: Value::Undefined,
                args: Vec::new(),
                kind: HostCallbackKind::Call,
            }));
        }
        Ok(events)
    }

    fn has_pending_host_work(&self, promise: &Rc<RefCell<PromiseInner>>) -> bool {
        promise.borrow().external_pending
    }
}

fn reject_guest_callback(_: HostCallback) -> Result<Value, VmErr> {
    Err(VmErr::Msg(
        "Node-API host callback dispatch is unavailable on this call path".into(),
    ))
}

fn napi_error(action: &str, status: i32) -> VmErr {
    let detail = match status {
        NAPI_INVALID_ARG => "invalid argument or stale handle",
        NAPI_OBJECT_EXPECTED => "object expected",
        NAPI_FUNCTION_EXPECTED => "function expected",
        NAPI_NUMBER_EXPECTED => "number expected",
        NAPI_ARRAY_EXPECTED => "array expected",
        NAPI_ARRAYBUFFER_EXPECTED => "ArrayBuffer expected",
        NAPI_STRING_EXPECTED => "string expected",
        NAPI_BOOLEAN_EXPECTED => "boolean expected",
        NAPI_QUEUE_FULL => "thread-safe function queue is full",
        NAPI_CLOSING => "thread-safe function is closing",
        NAPI_PENDING_EXCEPTION => "a JavaScript exception is already pending",
        _ => "generic Node-API failure",
    };
    VmErr::Msg(format!(
        "Node-API error while {action}: {detail} (status {status})"
    ))
}

/// Install the experimental Node-API host as this interpreter's CommonJS
/// addon loader and host bridge. The sidecar API remains the cross-platform
/// option for addons that require Node/V8-specific symbols.
impl Interpreter {
    pub fn enable_rust_node_api_addons(
        &mut self,
        options: RustNodeApiOptions,
    ) -> Result<Rc<RustNodeApiHost>, VmErr> {
        let mut loader = FileCommonJsLoader::new(options.roots.iter())?;
        for (addon, expected_sha256) in &options.allowed_addons {
            loader = match expected_sha256 {
                Some(expected_sha256) => {
                    loader.allow_native_addon_with_sha256(addon, *expected_sha256)?
                }
                None => loader.allow_native_addon(addon)?,
            };
        }
        let entry = validate_entry(&loader, options.entry)?;
        let host = Rc::new(RustNodeApiHost::new(self.persistent_global.clone())?);
        let loader = loader.with_native_addon_loader(host.clone());
        self.set_commonjs_loader(Rc::new(loader))?;
        self.set_host_bridge(host.clone());
        if let Some(entry) = entry {
            self.set_commonjs_entry(entry.to_string_lossy().into_owned());
        }
        Ok(host)
    }
}

fn validate_entry(
    loader: &FileCommonJsLoader,
    entry: Option<PathBuf>,
) -> Result<Option<PathBuf>, VmErr> {
    entry
        .map(|entry| {
            let canonical = fs::canonicalize(&entry).map_err(|error| {
                VmErr::Msg(format!(
                    "cannot use CommonJS entry {}: {error}",
                    entry.display()
                ))
            })?;
            if !canonical.is_file() {
                return Err(VmErr::Msg(format!(
                    "CommonJS entry is not a file: {}",
                    canonical.display()
                )));
            }
            if !loader
                .roots()
                .iter()
                .any(|root| canonical.starts_with(root))
            {
                return Err(VmErr::Msg(format!(
                    "CommonJS entry escapes configured roots: {}",
                    canonical.display()
                )));
            }
            Ok(canonical)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::Interpreter;
    use sha2::{Digest, Sha256};
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn assert_error_fields(value: &Value, name: &str, message: &str, code: Option<&str>) {
        assert!(matches!(
            value.get_prop("name"),
            Some(Value::String(ref actual)) if actual == name
        ));
        assert!(matches!(
            value.get_prop("message"),
            Some(Value::String(ref actual)) if actual == message
        ));
        match code {
            Some(code) => assert!(matches!(
                value.get_prop("code"),
                Some(Value::String(ref actual)) if actual == code
            )),
            None => assert!(matches!(
                value.get_prop("code"),
                None | Some(Value::Undefined)
            )),
        }
    }

    fn number_array(value: Value) -> Vec<u32> {
        let Value::Array(values) = &value else {
            panic!("expected a numeric array");
        };
        values
            .borrow()
            .iter()
            .map(|value| match value {
                Value::Number(number) => *number as u32,
                _ => panic!("expected a number in array"),
            })
            .collect()
    }

    #[test]
    fn napi_run_script_preserves_the_outer_loop_budget_and_source_context() {
        let mut interpreter = Interpreter::with_builtins();
        interpreter.set_source("outer script");
        interpreter.set_loop_budget(1);
        interpreter.consume_loop().unwrap();

        let error = interpreter
            .run_script_source("while (true) {}")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Maximum loop iterations exceeded")
        );
        assert_eq!(interpreter.get_source_line(1), Some("outer script"));
    }

    #[test]
    fn integer_conversion_matches_ecmascript_int32_wraparound() {
        assert_eq!(to_int32(4_294_967_297.0), 1);
        assert_eq!(to_int32(-1.0), -1);
        assert_eq!(to_int32(f64::NAN), 0);
        assert_eq!(to_int32(f64::INFINITY), 0);
    }

    #[test]
    fn local_handles_expire_at_scope_close_without_reusing_their_pointer() {
        let mut arena = NapiHandleArena::default();
        let outer = arena.create(Value::Number(1.0)).unwrap();
        let scope = arena.open_scope().unwrap();
        let inner = arena.create(Value::Number(2.0)).unwrap();
        assert!(matches!(arena.get(inner), Ok(Value::Number(value)) if value == 2.0));
        arena.close_scope(scope).unwrap();
        assert_eq!(arena.get(inner).unwrap_err(), NAPI_INVALID_ARG);
        assert!(matches!(arena.get(outer), Ok(Value::Number(value)) if value == 1.0));
        let replacement = arena.create(Value::Number(3.0)).unwrap();
        assert_ne!(replacement, inner);
        assert!(matches!(arena.get(replacement), Ok(Value::Number(value)) if value == 3.0));
    }

    #[test]
    fn repeated_local_scopes_release_handle_table_entries_and_reuse_slots() {
        let mut arena = NapiHandleArena::default();
        let outer = arena.create(Value::Number(0.0)).unwrap();
        for index in 0..64 {
            let scope = arena.open_scope().unwrap();
            let local = arena.create(Value::Number(index as f64)).unwrap();
            assert_eq!(arena.handles.len(), 2);
            arena.close_scope(scope).unwrap();
            assert_eq!(arena.handles.len(), 1);
            assert_eq!(arena.get(local).unwrap_err(), NAPI_INVALID_ARG);
            assert!(matches!(arena.get(outer), Ok(Value::Number(0.0))));
        }
        assert_eq!(arena.slots.len(), 2);
    }

    #[test]
    fn handle_scopes_must_close_in_lifo_order() {
        let mut arena = NapiHandleArena::default();
        let outer = arena.open_scope().unwrap();
        let inner = arena.open_scope().unwrap();
        assert_eq!(arena.close_scope(outer).unwrap_err(), NAPI_INVALID_ARG);
        arena.close_scope(inner).unwrap();
        arena.close_scope(outer).unwrap();
        assert_eq!(arena.close_scope(0).unwrap_err(), NAPI_INVALID_ARG);
    }

    #[test]
    fn native_handles_are_bound_to_one_environment() {
        let mut owner = NapiHandleArena::default();
        let other = NapiHandleArena::default();
        let handle = owner.create(Value::Number(7.0)).unwrap();
        assert_eq!(other.get(handle).unwrap_err(), NAPI_INVALID_ARG);
        assert!(matches!(owner.get(handle), Ok(Value::Number(value)) if value == 7.0));
    }

    #[test]
    fn closed_scope_handles_become_stale() {
        let mut arena = NapiHandleArena::default();
        let scope_id = arena.open_scope().unwrap();
        let scope_handle = arena.create_scope_handle(scope_id, false).unwrap();
        arena.close_scope_handle(scope_handle).unwrap();
        assert_eq!(
            arena.close_scope_handle(scope_handle).unwrap_err(),
            NAPI_INVALID_ARG
        );
    }

    #[test]
    fn escapable_scope_promotes_one_local_handle_to_its_parent() {
        let mut arena = NapiHandleArena::default();
        let outer = arena.create(Value::Number(7.0)).unwrap();
        let scope_id = arena.open_escapable_scope().unwrap();
        let scope_handle = arena.create_scope_handle(scope_id, true).unwrap();
        let escapee = arena.create(Value::String("escaped".into())).unwrap();
        let escaped = arena.escape_handle(scope_handle, escapee).unwrap();

        assert!(matches!(arena.get(escaped), Ok(Value::String(ref value)) if value == "escaped"));
        assert_eq!(
            arena.escape_handle(scope_handle, escapee).unwrap_err(),
            NAPI_ESCAPE_CALLED_TWICE
        );
        arena.close_escapable_scope_handle(scope_handle).unwrap();
        assert_eq!(arena.get(escapee).unwrap_err(), NAPI_INVALID_ARG);
        assert!(matches!(arena.get(escaped), Ok(Value::String(ref value)) if value == "escaped"));
        assert!(matches!(arena.get(outer), Ok(Value::Number(7.0))));
    }

    #[test]
    fn escapable_scope_rejects_parent_handles_and_regular_close_calls() {
        let mut arena = NapiHandleArena::default();
        let parent_handle = arena.create(Value::Number(7.0)).unwrap();
        let scope_id = arena.open_escapable_scope().unwrap();
        let scope_handle = arena.create_scope_handle(scope_id, true).unwrap();

        assert_eq!(
            arena
                .escape_handle(scope_handle, parent_handle)
                .unwrap_err(),
            NAPI_HANDLE_SCOPE_MISMATCH
        );
        assert_eq!(
            arena.close_scope_handle(scope_handle).unwrap_err(),
            NAPI_HANDLE_SCOPE_MISMATCH
        );

        let local = arena.create(Value::Null).unwrap();
        let escaped = arena.escape_handle(scope_handle, local).unwrap();
        arena.close_escapable_scope_handle(scope_handle).unwrap();
        assert!(matches!(arena.get(escaped), Ok(Value::Null)));
    }

    #[test]
    fn deferred_resolution_without_interpreter_rejects_objects_but_accepts_undefined() {
        let promise = Value::pending_promise();

        assert_eq!(
            settle_deferred_without_interpreter(&promise, Value::object(Vec::new()), false),
            Err(NAPI_GENERIC_FAILURE)
        );
        assert_eq!(promise.borrow().state, PromiseState::Pending);

        settle_deferred_without_interpreter(&promise, Value::Undefined, false).unwrap();
        let promise = promise.borrow();
        assert_eq!(promise.state, PromiseState::Fulfilled);
        assert!(matches!(promise.value, Value::Undefined));
    }

    #[test]
    fn loads_and_calls_a_real_napi_v4_addon_without_a_node_sidecar() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-rust-node-api-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let compiler = Command::new("cc").arg("--version").output();
        let mut include_dirs = Vec::new();
        if let Some(include) = std::env::var_os("NODE_INCLUDE_DIR") {
            include_dirs.push(PathBuf::from(include));
        }
        include_dirs.push(PathBuf::from("/usr/include/node"));
        include_dirs.push(PathBuf::from("/usr/local/include/node"));
        let include = include_dirs
            .into_iter()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(compiler), Some(include)) = (compiler, include) else {
            eprintln!("skipping Rust Node-API host fixture: cc or Node headers are unavailable");
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(compiler.status.success(), "cc --version failed");

        let source = root.join("fixture.c");
        let addon = root.join("fixture.node");
        fs::write(
            &source,
            r#"
#define _POSIX_C_SOURCE 200809L
#define NAPI_VERSION 4
#include <node_api.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

static napi_ref persistent_values;
static napi_ref removable_object;
static napi_ref wrapped_object_reference;
static napi_ref external_value_reference;
static int wrapped_finalizer_calls;
static int removed_finalizer_calls;
static int external_finalizer_calls;
static int finalizer_create_function_status = -1;
static int cleanup_hook_order[4];
static int cleanup_hook_count;
static int cleanup_before_wrap_finalizer;
static napi_env cleanup_env;
static int descriptor_setter_value = 5;
static int class_constructor_offset = 1;
static int counter_static_offset = 8;
static int target_call_count;
static int target_construct_count;
static bool counter_new_target_seen;
static bool counter_child_new_target_seen;
static int threadsafe_finalizer_calls;
static int threadsafe_worker_context_ok;
static int threadsafe_worker_call_status = -1;
static int threadsafe_worker_blocking_status = -1;
static int threadsafe_queue_first_status = -1;
static int threadsafe_queue_full_status = -1;
static int threadsafe_abort_call_status = -1;
static atomic_int threadsafe_abort_ready;
static char threadsafe_context_marker;
static char threadsafe_finalize_marker;
static napi_deferred pending_promise_deferred;
static char* copy_text(const char* text) {
  size_t length = strlen(text) + 1;
  char* copy = (char*)malloc(length);
  if (copy != NULL) memcpy(copy, text, length);
  return copy;
}
typedef struct async_work_context {
  napi_deferred deferred;
  napi_async_work work;
  int32_t result;
} async_work_context;
static char wrapped_native_data[] = "wrapped-native-data";
static char removable_native_data[] = "removed-native-data";
static char external_native_data[] = "external-native-data";
static char external_finalize_hint;

static napi_value finalizer_noop(napi_env env, napi_callback_info info) {
  (void)env;
  (void)info;
  return NULL;
}

static void finalize_probe(napi_env env, void* data, void* hint) {
  (void)hint;
  if (data == wrapped_native_data) {
    napi_value ignored;
    wrapped_finalizer_calls++;
    cleanup_before_wrap_finalizer = cleanup_hook_count == 2;
    finalizer_create_function_status = napi_create_function(
        env, "fromFinalizer", NAPI_AUTO_LENGTH, finalizer_noop, NULL, &ignored);
  }
  if (data == removable_native_data) removed_finalizer_calls++;
}

static void finalize_external_probe(napi_env env, void* data, void* hint) {
  (void)env;
  if (data == external_native_data && hint == &external_finalize_hint)
    external_finalizer_calls++;
}

static void cleanup_probe(void* arg) {
  if (cleanup_hook_count < 4) {
    cleanup_hook_order[cleanup_hook_count++] = (int)(intptr_t)arg;
  }
}

static void cleanup_remove_other(void* arg) {
  if (napi_remove_env_cleanup_hook(cleanup_env, cleanup_probe, arg) != napi_ok) {
    cleanup_probe((void*)(intptr_t)-1);
    return;
  }
  cleanup_probe((void*)(intptr_t)4);
}

static napi_value cleanup_misuse_status(napi_env env, napi_callback_info info) {
  napi_value result, status_value;
  napi_status duplicate_status, unmatched_status;
  (void)info;
  if (napi_add_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)5) != napi_ok)
    return NULL;
  duplicate_status = napi_add_env_cleanup_hook(env, cleanup_probe,
                                               (void*)(intptr_t)5);
  unmatched_status = napi_remove_env_cleanup_hook(env, cleanup_probe,
                                                  (void*)(intptr_t)6);
  if (napi_remove_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)5) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, duplicate_status, &status_value) != napi_ok ||
      napi_set_named_property(env, result, "duplicate", status_value) != napi_ok ||
      napi_create_int32(env, unmatched_status, &status_value) != napi_ok ||
      napi_set_named_property(env, result, "unmatched", status_value) != napi_ok)
    return NULL;
  return result;
}

static napi_value error_info_probe(napi_env env, napi_callback_info info) {
  napi_value input, result, field;
  const napi_extended_error_info* error_info = NULL;
  double number = 0;
  napi_status last_status;
  bool message_matches;
  (void)info;
  if (napi_create_string_utf8(env, "not a number", NAPI_AUTO_LENGTH,
                              &input) != napi_ok)
    return NULL;
  last_status = napi_get_value_double(env, input, &number);
  if (last_status != napi_number_expected ||
      napi_get_last_error_info(env, &error_info) != napi_ok ||
      error_info == NULL)
    return NULL;
  message_matches = strcmp(error_info->error_message, "number expected") == 0;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, last_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "lastStatus", field) != napi_ok ||
      napi_get_boolean(env, message_matches, &field) != napi_ok ||
      napi_set_named_property(env, result, "messageMatches", field) != napi_ok)
    return NULL;
  return result;
}

int napi_vm_test_cleanup_hook_count(void) {
  return cleanup_hook_count;
}

int napi_vm_test_cleanup_hook_value(int index) {
  return index >= 0 && index < cleanup_hook_count ? cleanup_hook_order[index] : -1;
}

int napi_vm_test_cleanup_before_wrap_finalizer(void) {
  return cleanup_before_wrap_finalizer;
}

int napi_vm_test_wrapped_finalizer_calls(void) {
  return wrapped_finalizer_calls;
}

int napi_vm_test_removed_finalizer_calls(void) {
  return removed_finalizer_calls;
}

int napi_vm_test_external_finalizer_calls(void) {
  return external_finalizer_calls;
}

int napi_vm_test_finalizer_create_function_status(void) {
  return finalizer_create_function_status;
}

static napi_value add(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  int32_t left = 0, right = 0;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_value_int32(env, argv[0], &left) != napi_ok ||
      napi_get_value_int32(env, argv[1], &right) != napi_ok ||
      napi_create_int32(env, left + right, &result) != napi_ok) return NULL;
  return result;
}

static napi_value round_trip(napi_env env, napi_callback_info info) {
  size_t argc = 5, length = 0, copied = 0;
  napi_value argv[5], result, field;
  napi_valuetype bool_type, number_type, string_type;
  bool flag = false;
  double number = 0;
  uint32_t uint32_value = 0;
  int64_t int64_value = 0;
  char text[128];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 5 ||
      napi_get_value_bool(env, argv[0], &flag) != napi_ok ||
      napi_get_value_double(env, argv[1], &number) != napi_ok ||
      napi_get_value_string_utf8(env, argv[2], NULL, 0, &length) != napi_ok ||
      length >= sizeof(text) ||
      napi_get_value_string_utf8(env, argv[2], text, sizeof(text), &copied) != napi_ok ||
      copied != length ||
      napi_get_value_uint32(env, argv[3], &uint32_value) != napi_ok ||
      napi_get_value_int64(env, argv[4], &int64_value) != napi_ok ||
      napi_typeof(env, argv[0], &bool_type) != napi_ok ||
      napi_typeof(env, argv[1], &number_type) != napi_ok ||
      napi_typeof(env, argv[2], &string_type) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_get_boolean(env, flag, &field) != napi_ok ||
      napi_set_named_property(env, result, "flag", field) != napi_ok ||
      napi_create_double(env, number + 0.5, &field) != napi_ok ||
      napi_set_named_property(env, result, "number", field) != napi_ok ||
      napi_create_string_utf8(env, text, copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "text", field) != napi_ok ||
      napi_create_uint32(env, uint32_value, &field) != napi_ok ||
      napi_set_named_property(env, result, "uint32", field) != napi_ok ||
      napi_create_int64(env, int64_value, &field) != napi_ok ||
      napi_set_named_property(env, result, "int64", field) != napi_ok ||
      napi_create_int32(env, bool_type, &field) != napi_ok ||
      napi_set_named_property(env, result, "boolType", field) != napi_ok ||
      napi_create_int32(env, number_type, &field) != napi_ok ||
      napi_set_named_property(env, result, "numberType", field) != napi_ok ||
      napi_create_int32(env, string_type, &field) != napi_ok ||
      napi_set_named_property(env, result, "stringType", field) != napi_ok) return NULL;
  return result;
}

static napi_value int64_conversion_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  int64_t value = 0;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_value_int64(env, argv[0], &value) != napi_ok ||
      napi_create_double(env, (double)value, &result) != napi_ok) return NULL;
  return result;
}

static napi_value string_encoding_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1, required = 0, copied = 0, truncated_copied = 0;
  size_t wrong_type_length = 0;
  napi_value argv[1], result, created, bytes, field, number, truncated_text;
  napi_status wrong_type_status;
  const char latin1[] = {'A', '\0', (char)0xE9, (char)0xFF};
  char buffer[16] = {0};
  char truncated[4] = {'x', 'x', 'x', 'x'};
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_value_string_latin1(env, argv[0], NULL, 0, &required) != napi_ok ||
      required >= sizeof(buffer) ||
      napi_get_value_string_latin1(env, argv[0], buffer, sizeof(buffer), &copied) != napi_ok ||
      napi_get_value_string_latin1(env, argv[0], truncated, sizeof(truncated),
                                   &truncated_copied) != napi_ok ||
      napi_create_string_latin1(env, latin1, sizeof(latin1), &created) != napi_ok ||
      napi_create_string_latin1(env, truncated, truncated_copied, &truncated_text) != napi_ok ||
      napi_create_array_with_length(env, copied, &bytes) != napi_ok ||
      napi_create_int32(env, 1, &number) != napi_ok)
    return NULL;
  for (size_t index = 0; index < copied; index++) {
    if (napi_create_uint32(env, (unsigned char)buffer[index], &field) != napi_ok ||
        napi_set_element(env, bytes, (uint32_t)index, field) != napi_ok)
      return NULL;
  }
  wrong_type_status = napi_get_value_string_latin1(
      env, number, NULL, 0, &wrong_type_length);
  if (wrong_type_status != napi_string_expected ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "created", created) != napi_ok ||
      napi_set_named_property(env, result, "bytes", bytes) != napi_ok ||
      napi_create_uint32(env, (uint32_t)required, &field) != napi_ok ||
      napi_set_named_property(env, result, "required", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "copied", field) != napi_ok ||
      napi_set_named_property(env, result, "truncatedText", truncated_text) != napi_ok ||
      napi_create_uint32(env, (uint32_t)truncated_copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "truncatedCopied", field) != napi_ok ||
      napi_create_int32(env, wrong_type_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "wrongTypeStatus", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value utf16_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1, required = 0, copied = 0, truncated_copied = 0;
  size_t wrong_type_length = 0;
  napi_value argv[1], result, round_trip, units, truncated_units, field, number;
  napi_value auto_length_value;
  napi_status wrong_type_status;
  const char16_t nul_terminated[] = {'T', 'E', 'R', 'M', '\0', 'X', '\0'};
  char16_t buffer[16] = {0};
  char16_t truncated[4] = {0};
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_value_string_utf16(env, argv[0], NULL, 0, &required) != napi_ok ||
      required >= sizeof(buffer) / sizeof(buffer[0]) ||
      napi_get_value_string_utf16(env, argv[0], buffer,
                                  sizeof(buffer) / sizeof(buffer[0]), &copied) != napi_ok ||
      copied != required ||
      napi_get_value_string_utf16(env, argv[0], truncated,
                                  sizeof(truncated) / sizeof(truncated[0]),
                                  &truncated_copied) != napi_ok ||
      napi_create_string_utf16(env, buffer, copied, &round_trip) != napi_ok ||
      napi_create_string_utf16(env, nul_terminated, NAPI_AUTO_LENGTH,
                               &auto_length_value) != napi_ok ||
      napi_create_array_with_length(env, copied, &units) != napi_ok ||
      napi_create_array_with_length(env, truncated_copied, &truncated_units) != napi_ok ||
      napi_create_int32(env, 1, &number) != napi_ok)
    return NULL;
  for (size_t index = 0; index < copied; index++) {
    if (napi_create_uint32(env, buffer[index], &field) != napi_ok ||
        napi_set_element(env, units, (uint32_t)index, field) != napi_ok)
      return NULL;
  }
  for (size_t index = 0; index < truncated_copied; index++) {
    if (napi_create_uint32(env, truncated[index], &field) != napi_ok ||
        napi_set_element(env, truncated_units, (uint32_t)index, field) != napi_ok)
      return NULL;
  }
  wrong_type_status = napi_get_value_string_utf16(
      env, number, NULL, 0, &wrong_type_length);
  if (wrong_type_status != napi_string_expected ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "roundTrip", round_trip) != napi_ok ||
      napi_set_named_property(env, result, "autoLength", auto_length_value) != napi_ok ||
      napi_set_named_property(env, result, "units", units) != napi_ok ||
      napi_create_uint32(env, (uint32_t)required, &field) != napi_ok ||
      napi_set_named_property(env, result, "length", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "copied", field) != napi_ok ||
      napi_set_named_property(env, result, "truncatedUnits", truncated_units) != napi_ok ||
      napi_create_uint32(env, (uint32_t)truncated_copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "truncatedCopied", field) != napi_ok ||
      napi_create_int32(env, wrong_type_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "wrongTypeStatus", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value invalid_utf16_status(napi_env env, napi_callback_info info) {
  const char16_t invalid[] = {0xD800};
  const napi_extended_error_info* error_info = NULL;
  napi_value ignored, result, field;
  napi_status status = napi_create_string_utf16(env, invalid, 1, &ignored);
  bool message_matches;
  (void)info;
  if (status != napi_generic_failure ||
      napi_get_last_error_info(env, &error_info) != napi_ok || error_info == NULL)
    return NULL;
  message_matches = strcmp(error_info->error_message,
      "UTF-16 input is malformed or exceeds napi-vm string limits") == 0;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, status, &field) != napi_ok ||
      napi_set_named_property(env, result, "status", field) != napi_ok ||
      napi_get_boolean(env, message_matches, &field) != napi_ok ||
      napi_set_named_property(env, result, "messageMatches", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value coerce_to_bool_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_coerce_to_bool(env, argv[0], &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value coerce_to_number_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_coerce_to_number(env, argv[0], &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value coerce_to_string_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_coerce_to_string(env, argv[0], &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value delete_element_probe(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result, field;
  uint32_t index = 0, length = 0;
  bool deleted = false, present = true;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_value_uint32(env, argv[1], &index) != napi_ok ||
      napi_delete_element(env, argv[0], index, &deleted) != napi_ok ||
      napi_delete_element(env, argv[0], index, NULL) != napi_ok ||
      napi_has_element(env, argv[0], index, &present) != napi_ok ||
      napi_get_array_length(env, argv[0], &length) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_get_boolean(env, deleted, &field) != napi_ok ||
      napi_set_named_property(env, result, "deleted", field) != napi_ok ||
      napi_get_boolean(env, present, &field) != napi_ok ||
      napi_set_named_property(env, result, "present", field) != napi_ok ||
      napi_create_uint32(env, length, &field) != napi_ok ||
      napi_set_named_property(env, result, "length", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value escapable_scope_probe(napi_env env, napi_callback_info info) {
  napi_escapable_handle_scope scope;
  napi_value local, escaped, ignored, field, result, escaped_value;
  napi_status second_escape_status;
  (void)info;
  if (napi_open_escapable_handle_scope(env, &scope) != napi_ok ||
      napi_create_object(env, &local) != napi_ok ||
      napi_create_int32(env, 42, &field) != napi_ok ||
      napi_set_named_property(env, local, "value", field) != napi_ok ||
      napi_escape_handle(env, scope, local, &escaped) != napi_ok)
    return NULL;
  second_escape_status = napi_escape_handle(env, scope, local, &ignored);
  if (second_escape_status != napi_escape_called_twice ||
      napi_close_escapable_handle_scope(env, scope) != napi_ok ||
      napi_get_named_property(env, escaped, "value", &escaped_value) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "escaped", escaped_value) != napi_ok ||
      napi_create_int32(env, (int32_t)second_escape_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "secondEscapeStatus", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value run_script_probe(napi_env env, napi_callback_info info) {
  const char source[] =
      "globalThis.napiRunScriptCount = (globalThis.napiRunScriptCount || 0) + 1; "
      "globalThis.napiRunScriptMicrotask = false; "
      "Promise.resolve().then(() => { globalThis.napiRunScriptMicrotask = true; }); "
      "6 * 7";
  napi_value script, value, global, microtask_value, result, field;
  bool microtask_ran_during_call = true;
  (void)info;
  if (napi_create_string_utf8(env, source, sizeof(source) - 1, &script) != napi_ok ||
      napi_run_script(env, script, &value) != napi_ok ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "napiRunScriptMicrotask",
                              &microtask_value) != napi_ok ||
      napi_get_value_bool(env, microtask_value, &microtask_ran_during_call) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "value", value) != napi_ok ||
      napi_get_boolean(env, microtask_ran_during_call, &field) != napi_ok ||
      napi_set_named_property(env, result, "microtaskRanDuringCall", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value invalid_environment(napi_env env, napi_callback_info info) {
  napi_value ignored, result;
  napi_status status = napi_get_null((napi_env)(uintptr_t)1, &ignored);
  (void)info;
  if (napi_create_int32(env, status, &result) != napi_ok) return NULL;
  return result;
}

static napi_value buffer_probe(napi_env env, napi_callback_info info) {
  const uint8_t seed[] = {65, 66, 67, 68};
  napi_value copied, allocated, array, result, field;
  void* copied_data = NULL;
  void* allocated_data = NULL;
  size_t copied_length = 0, allocated_length = 0;
  bool copied_is_buffer = false, allocated_is_buffer = false, array_is_buffer = true;
  (void)info;
  if (napi_create_buffer_copy(env, sizeof(seed), seed, &copied_data, &copied) != napi_ok ||
      copied_data == NULL ||
      napi_get_buffer_info(env, copied, &copied_data, &copied_length) != napi_ok ||
      copied_length != sizeof(seed) ||
      napi_create_buffer(env, 3, &allocated_data, &allocated) != napi_ok ||
      allocated_data == NULL ||
      napi_get_buffer_info(env, allocated, &allocated_data, &allocated_length) != napi_ok ||
      allocated_length != 3 ||
      napi_create_array(env, &array) != napi_ok ||
      napi_is_buffer(env, copied, &copied_is_buffer) != napi_ok ||
      napi_is_buffer(env, allocated, &allocated_is_buffer) != napi_ok ||
      napi_is_buffer(env, array, &array_is_buffer) != napi_ok) return NULL;
  ((uint8_t*)copied_data)[1] = 120;
  ((uint8_t*)allocated_data)[0] = 7;
  ((uint8_t*)allocated_data)[1] = 8;
  ((uint8_t*)allocated_data)[2] = 9;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "copy", copied) != napi_ok ||
      napi_set_named_property(env, result, "allocated", allocated) != napi_ok ||
      napi_get_boolean(env, copied_is_buffer, &field) != napi_ok ||
      napi_set_named_property(env, result, "copyIsBuffer", field) != napi_ok ||
      napi_get_boolean(env, allocated_is_buffer, &field) != napi_ok ||
      napi_set_named_property(env, result, "allocatedIsBuffer", field) != napi_ok ||
      napi_get_boolean(env, array_is_buffer, &field) != napi_ok ||
      napi_set_named_property(env, result, "arrayIsBuffer", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)copied_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "copyLength", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)allocated_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "allocatedLength", field) != napi_ok) return NULL;
  return result;
}

static napi_value invalid_typedarray(napi_env env, napi_callback_info info) {
  napi_value buffer, invalid;
  napi_value result;
  void* bytes = NULL;
  (void)info;
  if (napi_create_arraybuffer(env, 8, &bytes, &buffer) != napi_ok || bytes == NULL) return NULL;
  if (napi_create_typedarray(env, napi_uint16_array, 2, buffer, 3, &invalid) != napi_ok) return NULL;
  if (napi_get_undefined(env, &result) != napi_ok) return NULL;
  return result;
}

static napi_value call_guest(napi_env env, napi_callback_info info) {
  napi_value args[3], result;
  size_t argc = 3;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc < 3) return NULL;
  if (napi_call_function(env, args[1], args[0], 1, &args[2], &result) != napi_ok) return NULL;
  return result;
}

static napi_value construct_guest(napi_env env, napi_callback_info info) {
  napi_value args[2], result;
  size_t argc = 2;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc < 2) return NULL;
  if (napi_new_instance(env, args[0], 1, &args[1], &result) != napi_ok) return NULL;
  return result;
}

static napi_value resolved_promise(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, resolution;
  bool is_promise = false;
  (void)info;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok ||
      napi_is_promise(env, promise, &is_promise) != napi_ok || !is_promise ||
      napi_create_string_utf8(env, "resolved-from-addon", NAPI_AUTO_LENGTH,
                              &resolution) != napi_ok ||
      napi_resolve_deferred(env, deferred, resolution) != napi_ok) return NULL;
  return promise;
}

static void async_work_execute(napi_env env, void* data) {
  async_work_context* context = (async_work_context*)data;
  (void)env;
  context->result = 42;
}

static void async_work_complete(napi_env env, napi_status status, void* data) {
  async_work_context* context = (async_work_context*)data;
  napi_value result;
  if (status == napi_ok &&
      napi_create_int32(env, context->result, &result) == napi_ok) {
    (void)napi_resolve_deferred(env, context->deferred, result);
  } else {
    (void)napi_create_int32(env, status, &result);
    (void)napi_reject_deferred(env, context->deferred, result);
  }
  (void)napi_delete_async_work(env, context->work);
  free(context);
}

static napi_value run_async_work(napi_env env, napi_callback_info info) {
  async_work_context* context = (async_work_context*)calloc(1, sizeof(*context));
  napi_value promise, resource_name;
  (void)info;
  if (context == NULL ||
      napi_create_promise(env, &context->deferred, &promise) != napi_ok ||
      napi_create_string_utf8(env, "napi-vm-test-async-work", NAPI_AUTO_LENGTH,
                              &resource_name) != napi_ok ||
      napi_create_async_work(env, NULL, resource_name, async_work_execute,
                             async_work_complete, context, &context->work) != napi_ok) {
    free(context);
    return NULL;
  }
  if (napi_queue_async_work(env, context->work) != napi_ok) {
    (void)napi_delete_async_work(env, context->work);
    free(context);
    return NULL;
  }
  return promise;
}

typedef struct threadsafe_work {
  napi_threadsafe_function function;
} threadsafe_work;

static void threadsafe_finalize(napi_env env, void* data, void* hint) {
  (void)env;
  if (data == &threadsafe_finalize_marker && hint == &threadsafe_context_marker) {
    threadsafe_finalizer_calls++;
  }
}

static void threadsafe_call_js(napi_env env, napi_value callback,
                               void* context, void* data) {
  if (env != NULL && callback != NULL &&
      context == &threadsafe_context_marker && data != NULL) {
    napi_value receiver, argument, ignored;
    if (napi_get_undefined(env, &receiver) == napi_ok &&
        napi_create_string_utf8(env, (const char*)data, NAPI_AUTO_LENGTH,
                                &argument) == napi_ok) {
      (void)napi_call_function(env, receiver, callback, 1, &argument, &ignored);
    }
  }
  free(data);
}

static void* threadsafe_worker(void* data) {
  threadsafe_work* work = (threadsafe_work*)data;
  void* context = NULL;
  threadsafe_worker_context_ok =
      napi_get_threadsafe_function_context(work->function, &context) == napi_ok &&
      context == &threadsafe_context_marker;
  char* message = copy_text("threadsafe-value");
  threadsafe_worker_call_status = message != NULL
      ? napi_call_threadsafe_function(work->function, message,
                                      napi_tsfn_nonblocking)
      : napi_generic_failure;
  if (threadsafe_worker_call_status != napi_ok) free(message);
  char* second = copy_text("threadsafe-second");
  threadsafe_worker_blocking_status = second != NULL
      ? napi_call_threadsafe_function(work->function, second,
                                      napi_tsfn_blocking)
      : napi_generic_failure;
  if (threadsafe_worker_blocking_status != napi_ok) free(second);
  (void)napi_release_threadsafe_function(work->function, napi_tsfn_release);
  free(work);
  return NULL;
}

static void* threadsafe_aborted_worker(void* data) {
  threadsafe_work* work = (threadsafe_work*)data;
  while (!atomic_load_explicit(&threadsafe_abort_ready, memory_order_acquire)) { }
  threadsafe_abort_call_status =
      napi_call_threadsafe_function(work->function, NULL, napi_tsfn_nonblocking);
  (void)napi_release_threadsafe_function(work->function, napi_tsfn_release);
  return NULL;
}

static napi_value run_threadsafe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value callback, resource_name, result;
  napi_threadsafe_function function;
  threadsafe_work* work = (threadsafe_work*)calloc(1, sizeof(threadsafe_work));
  pthread_t thread;
  if (work == NULL ||
      napi_get_cb_info(env, info, &argc, &callback, NULL, NULL) != napi_ok ||
      argc != 1 ||
      napi_create_string_utf8(env, "napi-vm-threadsafe-worker", NAPI_AUTO_LENGTH,
                              &resource_name) != napi_ok ||
      napi_get_undefined(env, &result) != napi_ok ||
      napi_create_threadsafe_function(env, callback, NULL, resource_name, 1, 1,
                                      &threadsafe_finalize_marker,
                                      threadsafe_finalize,
                                      &threadsafe_context_marker,
                                      threadsafe_call_js,
                                      &work->function) != napi_ok ||
      napi_unref_threadsafe_function(env, work->function) != napi_ok ||
      napi_ref_threadsafe_function(env, work->function) != napi_ok ||
                                      napi_acquire_threadsafe_function(work->function) != napi_ok) {
    free(work);
    return NULL;
  }
  function = work->function;
  if (pthread_create(&thread, NULL, threadsafe_worker, work) != 0) {
    (void)napi_release_threadsafe_function(work->function, napi_tsfn_abort);
    (void)napi_release_threadsafe_function(work->function, napi_tsfn_release);
    free(work);
    return NULL;
  }
  (void)pthread_detach(thread);
  (void)napi_release_threadsafe_function(function, napi_tsfn_release);
  return result;
}

static napi_value probe_threadsafe_queue(napi_env env,
                                         napi_callback_info info) {
  size_t argc = 1;
  napi_value callback, resource_name, result;
  napi_threadsafe_function function;
  char* first = copy_text("queue-first");
  char* second = copy_text("queue-second");
  if (napi_get_cb_info(env, info, &argc, &callback, NULL, NULL) != napi_ok ||
      argc != 1 || first == NULL || second == NULL ||
      napi_create_string_utf8(env, "napi-vm-threadsafe-queue", NAPI_AUTO_LENGTH,
                              &resource_name) != napi_ok ||
      napi_create_threadsafe_function(env, callback, NULL, resource_name, 1, 1,
                                      NULL, NULL, &threadsafe_context_marker,
                                      threadsafe_call_js, &function) != napi_ok) {
    free(first);
    free(second);
    return NULL;
  }
  threadsafe_queue_first_status =
      napi_call_threadsafe_function(function, first, napi_tsfn_nonblocking);
  if (threadsafe_queue_first_status != napi_ok) free(first);
  threadsafe_queue_full_status =
      napi_call_threadsafe_function(function, second, napi_tsfn_nonblocking);
  if (threadsafe_queue_full_status != napi_ok) free(second);
  if (napi_release_threadsafe_function(function, napi_tsfn_release) != napi_ok ||
      napi_create_int32(env, threadsafe_queue_full_status, &result) != napi_ok) {
    return NULL;
  }
  return result;
}

static napi_value probe_threadsafe_abort(napi_env env,
                                         napi_callback_info info) {
  napi_value resource_name, result;
  threadsafe_work work = {0};
  pthread_t thread;
  (void)info;
  if (napi_create_string_utf8(env, "napi-vm-threadsafe-abort", NAPI_AUTO_LENGTH,
                              &resource_name) != napi_ok ||
      napi_create_threadsafe_function(env, NULL, NULL, resource_name, 1, 2,
                                      &threadsafe_finalize_marker,
                                      threadsafe_finalize,
                                      &threadsafe_context_marker,
                                      threadsafe_call_js, &work.function) != napi_ok) {
    return NULL;
  }
  atomic_store_explicit(&threadsafe_abort_ready, 0, memory_order_release);
  if (pthread_create(&thread, NULL, threadsafe_aborted_worker, &work) != 0) {
    (void)napi_release_threadsafe_function(work.function, napi_tsfn_abort);
    return NULL;
  }
  if (napi_release_threadsafe_function(work.function, napi_tsfn_abort) != napi_ok) {
    atomic_store_explicit(&threadsafe_abort_ready, 1, memory_order_release);
    (void)pthread_join(thread, NULL);
    return NULL;
  }
  atomic_store_explicit(&threadsafe_abort_ready, 1, memory_order_release);
  (void)pthread_join(thread, NULL);
  if (napi_create_int32(env, threadsafe_abort_call_status, &result) != napi_ok) {
    return NULL;
  }
  return result;
}

int napi_vm_test_threadsafe_finalizer_calls(void) {
  return threadsafe_finalizer_calls;
}

int napi_vm_test_threadsafe_worker_context_ok(void) {
  return threadsafe_worker_context_ok;
}

int napi_vm_test_threadsafe_worker_call_status(void) {
  return threadsafe_worker_call_status;
}

int napi_vm_test_threadsafe_worker_blocking_status(void) {
  return threadsafe_worker_blocking_status;
}

static napi_value rejected_promise(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, rejection;
  bool is_promise = false;
  (void)info;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok ||
      napi_is_promise(env, promise, &is_promise) != napi_ok || !is_promise ||
      napi_create_string_utf8(env, "rejected-from-addon", NAPI_AUTO_LENGTH,
                              &rejection) != napi_ok ||
      napi_reject_deferred(env, deferred, rejection) != napi_ok) return NULL;
  return promise;
}

static napi_value pending_promise(napi_env env, napi_callback_info info) {
  napi_value promise;
  bool is_promise = false;
  (void)info;
  if (pending_promise_deferred != NULL ||
      napi_create_promise(env, &pending_promise_deferred, &promise) != napi_ok ||
      napi_is_promise(env, promise, &is_promise) != napi_ok || !is_promise) return NULL;
  return promise;
}

static napi_value resolve_pending_promise(napi_env env, napi_callback_info info) {
  napi_value resolution, result;
  size_t argc = 1;
  if (pending_promise_deferred == NULL ||
      napi_get_cb_info(env, info, &argc, &resolution, NULL, NULL) != napi_ok || argc < 1 ||
      napi_resolve_deferred(env, pending_promise_deferred, resolution) != napi_ok ||
      napi_get_boolean(env, true, &result) != napi_ok) return NULL;
  pending_promise_deferred = NULL;
  return result;
}

static napi_value property_probe(napi_env env, napi_callback_info info) {
  napi_value args[1], object, computed_key, inherited_key, assigned_key;
  napi_value remove_key, computed, assigned, names, result, field;
  size_t argc = 1;
  bool has_inherited = false, has_named_inherited = false;
  bool has_own_inherited = true, deleted = false;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc < 1) return NULL;
  object = args[0];
  if (napi_create_string_utf8(env, "computed", NAPI_AUTO_LENGTH, &computed_key) != napi_ok ||
      napi_create_string_utf8(env, "inherited", NAPI_AUTO_LENGTH, &inherited_key) != napi_ok ||
      napi_create_string_utf8(env, "assigned", NAPI_AUTO_LENGTH, &assigned_key) != napi_ok ||
      napi_create_string_utf8(env, "removeMe", NAPI_AUTO_LENGTH, &remove_key) != napi_ok ||
      napi_get_property(env, object, computed_key, &computed) != napi_ok ||
      napi_has_property(env, object, inherited_key, &has_inherited) != napi_ok ||
      napi_has_named_property(env, object, "inherited", &has_named_inherited) != napi_ok ||
      napi_has_own_property(env, object, inherited_key, &has_own_inherited) != napi_ok ||
      napi_create_string_utf8(env, "set through addon", NAPI_AUTO_LENGTH, &assigned) != napi_ok ||
      napi_set_property(env, object, assigned_key, assigned) != napi_ok ||
      napi_delete_property(env, object, remove_key, &deleted) != napi_ok ||
      napi_get_property_names(env, object, &names) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "computed", computed) != napi_ok ||
      napi_get_boolean(env, has_inherited, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasInherited", field) != napi_ok ||
      napi_get_boolean(env, has_named_inherited, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasNamedInherited", field) != napi_ok ||
      napi_get_boolean(env, has_own_inherited, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasOwnInherited", field) != napi_ok ||
      napi_get_boolean(env, deleted, &field) != napi_ok ||
      napi_set_named_property(env, result, "deleted", field) != napi_ok ||
      napi_set_named_property(env, result, "names", names) != napi_ok) return NULL;
  return result;
}

static napi_value global_probe(napi_env env, napi_callback_info info) {
  napi_value global, object_constructor, result;
  napi_valuetype type;
  (void)info;
  if (napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Object", &object_constructor) != napi_ok ||
      napi_typeof(env, object_constructor, &type) != napi_ok ||
      napi_get_boolean(env, type == napi_function, &result) != napi_ok) return NULL;
  return result;
}

static napi_value defined_method(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_create_int32(env, 42, &result) != napi_ok) return NULL;
  return result;
}

static napi_value defined_getter(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_create_int32(env, descriptor_setter_value, &result) != napi_ok) return NULL;
  return result;
}

static napi_value defined_setter(napi_env env, napi_callback_info info) {
  napi_value value;
  size_t argc = 1;
  if (napi_get_cb_info(env, info, &argc, &value, NULL, NULL) != napi_ok || argc < 1 ||
      napi_get_value_int32(env, value, &descriptor_setter_value) != napi_ok) return NULL;
  return NULL;
}

static napi_value counter_constructor(napi_env env, napi_callback_info info) {
  napi_value argument, this_arg, initial_value, new_target, target_name;
  size_t argc = 1;
  void* data = NULL;
  int32_t value = 0;
  char target_name_text[64] = {0};
  if (napi_get_cb_info(env, info, &argc, &argument, &this_arg, &data) != napi_ok ||
      this_arg == NULL || data != &class_constructor_offset ||
      napi_get_new_target(env, info, &new_target) != napi_ok) return NULL;
  counter_new_target_seen = new_target != NULL;
  counter_child_new_target_seen = false;
  if (new_target != NULL &&
      napi_get_named_property(env, new_target, "name", &target_name) == napi_ok) {
    size_t target_name_length = 0;
    if (napi_get_value_string_utf8(env, target_name, target_name_text,
                                   sizeof(target_name_text),
                                   &target_name_length) == napi_ok) {
      counter_child_new_target_seen =
          strcmp(target_name_text, "CounterChild") == 0;
    }
  }
  if (argc > 0 && napi_get_value_int32(env, argument, &value) != napi_ok) return NULL;
  if (napi_create_int32(env, value + *(int*)data, &initial_value) != napi_ok ||
      napi_set_named_property(env, this_arg, "value", initial_value) != napi_ok) return NULL;
  return NULL;
}

static napi_value target_probe(napi_env env, napi_callback_info info) {
  napi_value new_target;
  if (napi_get_new_target(env, info, &new_target) != napi_ok) return NULL;
  if (new_target == NULL) target_call_count++;
  else target_construct_count++;
  return NULL;
}

static napi_value target_counts(napi_env env, napi_callback_info info) {
  napi_value result, field;
  (void)info;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, target_call_count, &field) != napi_ok ||
      napi_set_named_property(env, result, "calls", field) != napi_ok ||
      napi_create_int32(env, target_construct_count, &field) != napi_ok ||
      napi_set_named_property(env, result, "constructs", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value strict_equal_probe(napi_env env, napi_callback_info info) {
  napi_value args[2], result;
  size_t argc = 2;
  bool equal = false;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc < 2 ||
      napi_strict_equals(env, args[0], args[1], &equal) != napi_ok ||
      napi_get_boolean(env, equal, &result) != napi_ok) return NULL;
  return result;
}

static napi_value counter_new_target_info(napi_env env, napi_callback_info info) {
  napi_value result, field;
  (void)info;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_get_boolean(env, counter_new_target_seen, &field) != napi_ok ||
      napi_set_named_property(env, result, "seen", field) != napi_ok ||
      napi_get_boolean(env, counter_child_new_target_seen, &field) != napi_ok ||
      napi_set_named_property(env, result, "child", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value counter_increment(napi_env env, napi_callback_info info) {
  napi_value this_arg, value, result;
  size_t argc = 0;
  int32_t current;
  if (napi_get_cb_info(env, info, &argc, NULL, &this_arg, NULL) != napi_ok ||
      napi_get_named_property(env, this_arg, "value", &value) != napi_ok ||
      napi_get_value_int32(env, value, &current) != napi_ok ||
      napi_create_int32(env, current + 1, &result) != napi_ok ||
      napi_set_named_property(env, this_arg, "value", result) != napi_ok) return NULL;
  return result;
}

static napi_value counter_static_method(napi_env env, napi_callback_info info) {
  napi_value this_arg, base_value, result;
  size_t argc = 0;
  int32_t base;
  if (napi_get_cb_info(env, info, &argc, NULL, &this_arg, NULL) != napi_ok ||
      napi_get_named_property(env, this_arg, "baseValue", &base_value) != napi_ok ||
      napi_get_value_int32(env, base_value, &base) != napi_ok ||
      napi_create_int32(env, base + 99, &result) != napi_ok) return NULL;
  return result;
}

static napi_value counter_static_getter(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_create_int32(env, counter_static_offset, &result) != napi_ok) return NULL;
  return result;
}

static napi_value counter_static_read_only_getter(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_create_int32(env, 21, &result) != napi_ok) return NULL;
  return result;
}

static napi_value counter_static_setter(napi_env env, napi_callback_info info) {
  napi_value value;
  size_t argc = 1;
  if (napi_get_cb_info(env, info, &argc, &value, NULL, NULL) != napi_ok || argc < 1 ||
      napi_get_value_int32(env, value, &counter_static_offset) != napi_ok) return NULL;
  return NULL;
}

static napi_value symbol_probe(napi_env env, napi_callback_info info) {
  napi_value description, key, other_key, no_description, object;
  napi_value value, actual, names, result, field;
  napi_valuetype no_description_type;
  uint32_t string_key_count = 0;
  bool has_key = false, has_other_key = true;
  (void)info;
  if (napi_create_string_utf8(env, "native-symbol", NAPI_AUTO_LENGTH, &description) != napi_ok ||
      napi_create_symbol(env, description, &key) != napi_ok ||
      napi_create_symbol(env, description, &other_key) != napi_ok ||
      napi_create_symbol(env, NULL, &no_description) != napi_ok ||
      napi_typeof(env, no_description, &no_description_type) != napi_ok ||
      napi_create_object(env, &object) != napi_ok ||
      napi_create_int32(env, 42, &value) != napi_ok ||
      napi_set_property(env, object, key, value) != napi_ok ||
      napi_get_property(env, object, key, &actual) != napi_ok ||
      napi_has_own_property(env, object, key, &has_key) != napi_ok ||
      napi_has_own_property(env, object, other_key, &has_other_key) != napi_ok ||
      napi_get_property_names(env, object, &names) != napi_ok ||
      napi_get_array_length(env, names, &string_key_count) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "value", actual) != napi_ok ||
      napi_get_boolean(env, has_key, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasKey", field) != napi_ok ||
      napi_get_boolean(env, has_other_key, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasOtherKey", field) != napi_ok ||
      napi_get_boolean(env, no_description_type == napi_symbol, &field) != napi_ok ||
      napi_set_named_property(env, result, "noDescriptionIsSymbol", field) != napi_ok ||
      napi_create_uint32(env, string_key_count, &field) != napi_ok ||
      napi_set_named_property(env, result, "stringKeyCount", field) != napi_ok) return NULL;
  return result;
}

static napi_value typedarray_probe(napi_env env, napi_callback_info info) {
  napi_value buffer, typed, typed_buffer, view, view_buffer, result, field;
  void* bytes = NULL;
  void* typed_bytes = NULL;
  void* view_bytes = NULL;
  size_t buffer_length = 0, typed_length = 0, typed_offset = 0;
  size_t view_length = 0, view_offset = 0;
  napi_typedarray_type typed_kind = napi_int8_array;
  bool is_arraybuffer = false, is_typedarray = false, is_dataview = false;
  (void)info;
  if (napi_create_arraybuffer(env, 8, &bytes, &buffer) != napi_ok || bytes == NULL ||
      napi_get_arraybuffer_info(env, buffer, &bytes, &buffer_length) != napi_ok ||
      buffer_length != 8 ||
      napi_is_arraybuffer(env, buffer, &is_arraybuffer) != napi_ok || !is_arraybuffer) return NULL;
  for (size_t i = 0; i < buffer_length; i++) ((uint8_t*)bytes)[i] = (uint8_t)(10 + i);
  if (napi_create_typedarray(env, napi_uint16_array, 2, buffer, 2, &typed) != napi_ok ||
      napi_get_typedarray_info(env, typed, &typed_kind, &typed_length, &typed_bytes,
                               &typed_buffer, &typed_offset) != napi_ok ||
      typed_kind != napi_uint16_array || typed_length != 2 || typed_offset != 2 ||
      typed_bytes == NULL ||
      napi_is_typedarray(env, typed, &is_typedarray) != napi_ok || !is_typedarray ||
      napi_create_dataview(env, 3, buffer, 4, &view) != napi_ok ||
      napi_get_dataview_info(env, view, &view_length, &view_bytes, &view_buffer,
                             &view_offset) != napi_ok ||
      view_length != 3 || view_offset != 4 || view_bytes == NULL ||
      napi_is_dataview(env, view, &is_dataview) != napi_ok || !is_dataview) return NULL;
  ((uint8_t*)typed_bytes)[1] = 55;
  ((uint8_t*)view_bytes)[0] = 77;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "buffer", buffer) != napi_ok ||
      napi_set_named_property(env, result, "typed", typed) != napi_ok ||
      napi_set_named_property(env, result, "view", view) != napi_ok ||
      napi_get_boolean(env, is_arraybuffer, &field) != napi_ok ||
      napi_set_named_property(env, result, "isArrayBuffer", field) != napi_ok ||
      napi_get_boolean(env, is_typedarray, &field) != napi_ok ||
      napi_set_named_property(env, result, "isTypedArray", field) != napi_ok ||
      napi_get_boolean(env, is_dataview, &field) != napi_ok ||
      napi_set_named_property(env, result, "isDataView", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)typed_kind, &field) != napi_ok ||
      napi_set_named_property(env, result, "typedKind", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)typed_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "typedLength", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)typed_offset, &field) != napi_ok ||
      napi_set_named_property(env, result, "typedOffset", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)view_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "viewLength", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)view_offset, &field) != napi_ok ||
      napi_set_named_property(env, result, "viewOffset", field) != napi_ok) return NULL;
  return result;
}

static napi_value array_probe(napi_env env, napi_callback_info info) {
  napi_value array, empty, result, value, field;
  uint32_t length = 0, empty_length = 0;
  bool is_array = false, first_present = true, second_present = false;
  bool hole_present = true, read_value = false;
  (void)info;
  if (napi_create_array_with_length(env, 3, &array) != napi_ok ||
      napi_create_array(env, &empty) != napi_ok ||
      napi_get_boolean(env, true, &value) != napi_ok ||
      napi_set_element(env, array, 1, value) != napi_ok ||
      napi_set_element(env, array, 4, value) != napi_ok ||
      napi_get_array_length(env, array, &length) != napi_ok ||
      napi_get_array_length(env, empty, &empty_length) != napi_ok ||
      napi_is_array(env, array, &is_array) != napi_ok ||
      napi_has_element(env, array, 0, &first_present) != napi_ok ||
      napi_has_element(env, array, 1, &second_present) != napi_ok ||
      napi_has_element(env, array, 2, &hole_present) != napi_ok ||
      napi_get_element(env, array, 1, &value) != napi_ok ||
      napi_get_value_bool(env, value, &read_value) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_get_boolean(env, is_array, &field) != napi_ok ||
      napi_set_named_property(env, result, "isArray", field) != napi_ok ||
      napi_create_uint32(env, length, &field) != napi_ok ||
      napi_set_named_property(env, result, "length", field) != napi_ok ||
      napi_create_uint32(env, empty_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "emptyLength", field) != napi_ok ||
      napi_get_boolean(env, first_present, &field) != napi_ok ||
      napi_set_named_property(env, result, "firstPresent", field) != napi_ok ||
      napi_get_boolean(env, second_present, &field) != napi_ok ||
      napi_set_named_property(env, result, "secondPresent", field) != napi_ok ||
      napi_get_boolean(env, hole_present, &field) != napi_ok ||
      napi_set_named_property(env, result, "holePresent", field) != napi_ok ||
      napi_get_boolean(env, read_value, &field) != napi_ok ||
      napi_set_named_property(env, result, "value", field) != napi_ok) return NULL;
  return result;
}

static napi_value get_prototype_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value object, prototype;
  if (napi_get_cb_info(env, info, &argc, &object, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_prototype(env, object, &prototype) != napi_ok) return NULL;
  return prototype;
}

static napi_value instanceof_probe(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  bool is_instance = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_instanceof(env, argv[0], argv[1], &is_instance) != napi_ok ||
      napi_get_boolean(env, is_instance, &result) != napi_ok) return NULL;
  return result;
}

static napi_value returns_undefined(napi_env env, napi_callback_info info) {
  (void)env;
  (void)info;
  return NULL;
}

static napi_value create_errors(napi_env env, napi_callback_info info) {
  napi_value result, message, code, error, type_error, range_error;
  (void)info;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_create_string_utf8(env, "created", NAPI_AUTO_LENGTH, &message) != napi_ok ||
      napi_create_string_utf8(env, "E_CREATED", NAPI_AUTO_LENGTH, &code) != napi_ok ||
      napi_create_error(env, code, message, &error) != napi_ok ||
      napi_set_named_property(env, result, "error", error) != napi_ok ||
      napi_create_type_error(env, code, message, &type_error) != napi_ok ||
      napi_set_named_property(env, result, "typeError", type_error) != napi_ok ||
      napi_create_range_error(env, code, message, &range_error) != napi_ok ||
      napi_set_named_property(env, result, "rangeError", range_error) != napi_ok) return NULL;
  return result;
}

static napi_value throw_type_error(napi_env env, napi_callback_info info) {
  (void)info;
  napi_throw_type_error(env, "E_TYPE", "type failure");
  return NULL;
}

static napi_value throw_range_error(napi_env env, napi_callback_info info) {
  (void)info;
  napi_throw_range_error(env, NULL, "range failure");
  return NULL;
}

static napi_value throw_created_error(napi_env env, napi_callback_info info) {
  napi_value message, error;
  (void)info;
  if (napi_create_string_utf8(env, "thrown", NAPI_AUTO_LENGTH, &message) != napi_ok ||
      napi_create_type_error(env, NULL, message, &error) != napi_ok ||
      napi_throw(env, error) != napi_ok) return NULL;
  return NULL;
}

static napi_value throw_and_clear(napi_env env, napi_callback_info info) {
  napi_value error;
  bool pending = false, is_error = false;
  (void)info;
  if (napi_throw_error(env, "E_CLEARED", "cleared failure") != napi_ok ||
      napi_is_exception_pending(env, &pending) != napi_ok || !pending ||
      napi_get_and_clear_last_exception(env, &error) != napi_ok ||
      napi_is_exception_pending(env, &pending) != napi_ok || pending ||
      napi_is_error(env, error, &is_error) != napi_ok || !is_error) return NULL;
  return error;
}

static napi_value reference_probe(napi_env env, napi_callback_info info) {
  napi_value value, result, field;
  uint32_t count_after_unref = 0, count_after_ref = 0;
  (void)info;
  if (napi_reference_unref(env, persistent_values, &count_after_unref) != napi_ok ||
      napi_get_reference_value(env, persistent_values, &value) != napi_ok || value == NULL ||
      napi_reference_ref(env, persistent_values, &count_after_ref) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "value", value) != napi_ok ||
      napi_create_uint32(env, count_after_unref, &field) != napi_ok ||
      napi_set_named_property(env, result, "countAfterUnref", field) != napi_ok ||
      napi_create_uint32(env, count_after_ref, &field) != napi_ok ||
      napi_set_named_property(env, result, "countAfterRef", field) != napi_ok) return NULL;
  return result;
}

static napi_value release_reference(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_delete_reference(env, persistent_values) != napi_ok ||
      napi_delete_reference(env, wrapped_object_reference) != napi_ok ||
      napi_get_boolean(env, true, &result) != napi_ok) return NULL;
  return result;
}

static napi_value wrap_probe(napi_env env, napi_callback_info info) {
  napi_value object, result;
  void* data = NULL;
  (void)info;
  if (napi_get_reference_value(env, wrapped_object_reference, &object) != napi_ok ||
      napi_unwrap(env, object, &data) != napi_ok || data != wrapped_native_data ||
      napi_create_string_utf8(env, (const char*)data, NAPI_AUTO_LENGTH, &result) != napi_ok) return NULL;
  return result;
}

static napi_value remove_wrap_probe(napi_env env, napi_callback_info info) {
  napi_value object, result;
  void* data = NULL;
  (void)info;
  if (napi_get_reference_value(env, removable_object, &object) != napi_ok ||
      napi_remove_wrap(env, object, &data) != napi_ok || data != removable_native_data ||
      napi_create_string_utf8(env, (const char*)data, NAPI_AUTO_LENGTH, &result) != napi_ok) return NULL;
  return result;
}

static napi_value duplicate_wrap_status(napi_env env, napi_callback_info info) {
  napi_value object, result;
  napi_status status;
  (void)info;
  if (napi_get_reference_value(env, wrapped_object_reference, &object) != napi_ok) return NULL;
  status = napi_wrap(env, object, wrapped_native_data, finalize_probe, NULL, NULL);
  if (napi_create_int32(env, status, &result) != napi_ok) return NULL;
  return result;
}

static napi_value external_probe(napi_env env, napi_callback_info info) {
  napi_value external, ordinary, result;
  napi_valuetype type;
  void* data = NULL;
  void* invalid_data = NULL;
  (void)info;
  if (napi_get_reference_value(env, external_value_reference, &external) != napi_ok ||
      napi_get_value_external(env, external, &data) != napi_ok ||
      data != external_native_data ||
      napi_typeof(env, external, &type) != napi_ok || type != napi_external ||
      napi_create_object(env, &ordinary) != napi_ok ||
      napi_get_value_external(env, ordinary, &invalid_data) != napi_invalid_arg ||
      napi_get_boolean(env, true, &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value external_property_probe(napi_env env, napi_callback_info info) {
  napi_value external, ignored, property, result;
  napi_valuetype type;
  (void)info;
  if (napi_get_reference_value(env, external_value_reference, &external) != napi_ok ||
      napi_create_object(env, &property) != napi_ok ||
      napi_set_named_property(env, external, "ignored", property) != napi_ok ||
      napi_get_named_property(env, external, "ignored", &ignored) != napi_ok ||
      napi_typeof(env, ignored, &type) != napi_ok || type != napi_undefined ||
      napi_get_boolean(env, true, &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value external_memory_probe(napi_env env, napi_callback_info info) {
  int64_t first = 0, second = 0;
  napi_value result;
  (void)info;
  if (napi_adjust_external_memory(env, INT64_C(65536), &first) != napi_ok ||
      napi_adjust_external_memory(env, -INT64_C(4096), &second) != napi_ok ||
      napi_create_int64(env, second - first, &result) != napi_ok)
    return NULL;
  return result;
}

NAPI_MODULE_INIT() {
  napi_handle_scope scope;
  napi_value scratch, function, metadata, version, values, field, external;
  uint32_t supported_api_version = 0;
  napi_deferred initialized_deferred;
  napi_value initialized_promise, initialized_promise_value;
  bool initialized_is_promise = false;
  napi_value descriptor_value, descriptor_symbol, descriptor_symbol_description;
  napi_value descriptor_symbol_value;
  napi_value global, global_key, global_object_constructor;
  napi_valuetype global_object_type;
  bool global_object_own = false;
  napi_property_descriptor defined_properties[4] = {
      { .utf8name = "definedMethod", .method = defined_method,
        .attributes = napi_default },
      { .utf8name = "definedValue", .getter = defined_getter,
        .setter = defined_setter, .attributes = napi_enumerable },
      { .utf8name = "definedConstant", .value = NULL,
        .attributes = napi_writable | napi_enumerable | napi_configurable },
      { .name = NULL, .value = NULL,
        .attributes = napi_writable | napi_enumerable | napi_configurable },
  };
  napi_property_descriptor counter_methods[] = {
      { .utf8name = "increment", .method = counter_increment,
        .attributes = napi_default },
      { .utf8name = "constant", .method = counter_static_method,
        .attributes = napi_static },
      { .utf8name = "baseValue", .value = NULL,
        .attributes = napi_static | napi_writable | napi_enumerable | napi_configurable },
      { .utf8name = "offset", .getter = counter_static_getter,
        .setter = counter_static_setter,
        .attributes = napi_static | napi_enumerable },
      { .utf8name = "readOnly", .getter = counter_static_read_only_getter,
        .attributes = napi_static | napi_enumerable },
  };
  napi_value counter_class, counter_base_value;
  int32_t checked_version = 0;
  if (napi_get_version(env, &supported_api_version) != napi_ok ||
      supported_api_version < NAPI_VERSION ||
      napi_get_version(env, NULL) != napi_invalid_arg ||
      napi_get_boolean(env, supported_api_version >= NAPI_VERSION, &field) != napi_ok ||
      napi_set_named_property(env, exports, "supportsNapiV4", field) != napi_ok)
    return NULL;
  if (napi_create_external(env, external_native_data, finalize_external_probe,
                           &external_finalize_hint, &external) != napi_ok ||
      napi_create_reference(env, external, 1, &external_value_reference) != napi_ok ||
      napi_set_named_property(env, exports, "external", external) != napi_ok)
    return NULL;
  cleanup_env = env;
  if (napi_add_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)1) != napi_ok ||
      napi_add_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)2) != napi_ok ||
      napi_add_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)3) != napi_ok ||
      napi_remove_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)2) != napi_ok ||
      napi_add_env_cleanup_hook(env, cleanup_remove_other,
                                (void*)(intptr_t)1) != napi_ok)
    return NULL;
  if (napi_create_int32(env, 7, &descriptor_value) != napi_ok ||
      napi_create_string_utf8(env, "descriptor", NAPI_AUTO_LENGTH,
                              &descriptor_symbol_description) != napi_ok ||
      napi_create_symbol(env, descriptor_symbol_description,
                         &descriptor_symbol) != napi_ok ||
      napi_create_int32(env, 17, &descriptor_symbol_value) != napi_ok ||
      napi_create_int32(env, 6, &counter_base_value) != napi_ok) return NULL;
  defined_properties[2].value = descriptor_value;
  defined_properties[3].name = descriptor_symbol;
  defined_properties[3].value = descriptor_symbol_value;
      counter_methods[2].value = counter_base_value;
  if (napi_get_global(env, &global) != napi_ok ||
      napi_create_string_utf8(env, "Object", NAPI_AUTO_LENGTH, &global_key) != napi_ok ||
      napi_get_named_property(env, global, "Object", &global_object_constructor) != napi_ok ||
      napi_typeof(env, global_object_constructor, &global_object_type) != napi_ok ||
      global_object_type != napi_function ||
      napi_has_own_property(env, global, global_key, &global_object_own) != napi_ok ||
      !global_object_own ||
      napi_open_handle_scope(env, &scope) != napi_ok ||
      napi_create_object(env, &scratch) != napi_ok ||
      napi_close_handle_scope(env, scope) != napi_ok) return NULL;
  if (napi_is_promise(env, exports, &initialized_is_promise) != napi_ok ||
      initialized_is_promise ||
      napi_create_promise(env, &initialized_deferred, &initialized_promise) != napi_ok ||
      napi_is_promise(env, initialized_promise, &initialized_is_promise) != napi_ok ||
      !initialized_is_promise ||
      napi_create_string_utf8(env, "resolved-during-init", NAPI_AUTO_LENGTH,
                              &initialized_promise_value) != napi_ok ||
      napi_resolve_deferred(env, initialized_deferred, initialized_promise_value) != napi_ok ||
      napi_set_named_property(env, exports, "initializedPromise", initialized_promise) != napi_ok)
    return NULL;
  if (napi_set_named_property(env, exports, "descriptorSymbol", descriptor_symbol) != napi_ok ||
      napi_define_properties(env, exports, 4, defined_properties) != napi_ok) return NULL;
      if (napi_define_class(env, "Counter", NAPI_AUTO_LENGTH, counter_constructor,
                        &class_constructor_offset, 5, counter_methods,
                        &counter_class) != napi_ok ||
      napi_set_named_property(env, exports, "Counter", counter_class) != napi_ok) return NULL;
  if (napi_create_function(env, "add", NAPI_AUTO_LENGTH, add, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "add", function) != napi_ok ||
      napi_create_function(env, "externalProbe", NAPI_AUTO_LENGTH,
                           external_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "externalProbe", function) != napi_ok ||
      napi_create_function(env, "externalPropertyProbe", NAPI_AUTO_LENGTH,
                           external_property_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "externalPropertyProbe", function) != napi_ok ||
      napi_create_function(env, "externalMemoryProbe", NAPI_AUTO_LENGTH,
                           external_memory_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "externalMemoryProbe", function) != napi_ok ||
      napi_create_function(env, "coerceToBoolean", NAPI_AUTO_LENGTH,
                           coerce_to_bool_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "coerceToBoolean", function) != napi_ok ||
      napi_create_function(env, "coerceToNumber", NAPI_AUTO_LENGTH,
                           coerce_to_number_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "coerceToNumber", function) != napi_ok ||
      napi_create_function(env, "coerceToString", NAPI_AUTO_LENGTH,
                           coerce_to_string_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "coerceToString", function) != napi_ok ||
      napi_create_function(env, "deleteElementProbe", NAPI_AUTO_LENGTH,
                           delete_element_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "deleteElementProbe", function) != napi_ok ||
      napi_create_function(env, "escapableScopeProbe", NAPI_AUTO_LENGTH,
                           escapable_scope_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "escapableScopeProbe", function) != napi_ok ||
      napi_create_function(env, "runScriptProbe", NAPI_AUTO_LENGTH,
                           run_script_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "runScriptProbe", function) != napi_ok ||
      napi_create_function(env, "stringEncodingProbe", NAPI_AUTO_LENGTH,
                           string_encoding_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "stringEncodingProbe", function) != napi_ok ||
      napi_create_function(env, "utf16Probe", NAPI_AUTO_LENGTH,
                           utf16_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "utf16Probe", function) != napi_ok ||
      napi_create_function(env, "invalidUtf16Status", NAPI_AUTO_LENGTH,
                           invalid_utf16_status, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "invalidUtf16Status", function) != napi_ok ||
      napi_create_object(env, &metadata) != napi_ok ||
      napi_create_int32(env, 1, &version) != napi_ok ||
      napi_set_named_property(env, metadata, "version", version) != napi_ok ||
      napi_get_named_property(env, metadata, "version", &field) != napi_ok ||
      napi_get_value_int32(env, field, &checked_version) != napi_ok ||
      checked_version != 1 ||
      napi_set_named_property(env, exports, "metadata", metadata) != napi_ok) return NULL;
  if (napi_create_function(env, "targetProbe", NAPI_AUTO_LENGTH, target_probe,
                           NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "targetProbe", function) != napi_ok ||
      napi_create_function(env, "targetCounts", NAPI_AUTO_LENGTH, target_counts,
                           NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "targetCounts", function) != napi_ok ||
      napi_create_function(env, "strictEqualProbe", NAPI_AUTO_LENGTH,
                           strict_equal_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "strictEqualProbe", function) != napi_ok ||
      napi_create_function(env, "counterNewTargetInfo", NAPI_AUTO_LENGTH,
                           counter_new_target_info, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "counterNewTargetInfo", function) != napi_ok)
    return NULL;
  if (napi_create_object(env, &values) != napi_ok ||
      napi_get_boolean(env, true, &field) != napi_ok ||
      napi_set_named_property(env, values, "truth", field) != napi_ok ||
      napi_get_null(env, &field) != napi_ok ||
      napi_set_named_property(env, values, "nothing", field) != napi_ok ||
      napi_get_undefined(env, &field) != napi_ok ||
      napi_set_named_property(env, values, "missing", field) != napi_ok ||
      napi_create_string_utf8(env, "Node-API ✓", NAPI_AUTO_LENGTH, &field) != napi_ok ||
      napi_set_named_property(env, values, "greeting", field) != napi_ok ||
      napi_create_double(env, 1.25, &field) != napi_ok ||
      napi_set_named_property(env, values, "fraction", field) != napi_ok ||
      napi_create_uint32(env, UINT32_MAX, &field) != napi_ok ||
      napi_set_named_property(env, values, "maxUint32", field) != napi_ok ||
      napi_create_int64(env, INT64_C(2147483648), &field) != napi_ok ||
      napi_set_named_property(env, values, "int64", field) != napi_ok ||
      napi_set_named_property(env, exports, "values", values) != napi_ok ||
      napi_wrap(env, values, wrapped_native_data, finalize_probe, NULL, &wrapped_object_reference) != napi_ok ||
      napi_create_reference(env, values, 1, &persistent_values) != napi_ok ||
      napi_create_object(env, &field) != napi_ok ||
      napi_wrap(env, field, removable_native_data, finalize_probe, NULL, NULL) != napi_ok ||
      napi_create_reference(env, field, 1, &removable_object) != napi_ok ||
      napi_create_function(env, "roundTrip", NAPI_AUTO_LENGTH, round_trip, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "roundTrip", function) != napi_ok ||
      napi_create_function(env, "int64ConversionProbe", NAPI_AUTO_LENGTH,
                           int64_conversion_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "int64ConversionProbe", function) != napi_ok ||
      napi_create_function(env, "getPrototype", NAPI_AUTO_LENGTH,
                           get_prototype_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "getPrototype", function) != napi_ok ||
      napi_create_function(env, "instanceofProbe", NAPI_AUTO_LENGTH,
                           instanceof_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "instanceofProbe", function) != napi_ok ||
      napi_create_function(env, "arrayProbe", NAPI_AUTO_LENGTH, array_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "arrayProbe", function) != napi_ok ||
      napi_create_function(env, "returnsUndefined", NAPI_AUTO_LENGTH, returns_undefined, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "returnsUndefined", function) != napi_ok ||
      napi_create_function(env, "createErrors", NAPI_AUTO_LENGTH, create_errors, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "createErrors", function) != napi_ok ||
      napi_create_function(env, "throwTypeError", NAPI_AUTO_LENGTH, throw_type_error, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "throwTypeError", function) != napi_ok ||
      napi_create_function(env, "throwRangeError", NAPI_AUTO_LENGTH, throw_range_error, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "throwRangeError", function) != napi_ok ||
      napi_create_function(env, "throwCreatedError", NAPI_AUTO_LENGTH, throw_created_error, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "throwCreatedError", function) != napi_ok ||
      napi_create_function(env, "throwAndClear", NAPI_AUTO_LENGTH, throw_and_clear, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "throwAndClear", function) != napi_ok ||
      napi_create_function(env, "referenceProbe", NAPI_AUTO_LENGTH, reference_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "referenceProbe", function) != napi_ok ||
      napi_create_function(env, "releaseReference", NAPI_AUTO_LENGTH, release_reference, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "releaseReference", function) != napi_ok ||
      napi_create_function(env, "wrapProbe", NAPI_AUTO_LENGTH, wrap_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "wrapProbe", function) != napi_ok ||
      napi_create_function(env, "removeWrapProbe", NAPI_AUTO_LENGTH, remove_wrap_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "removeWrapProbe", function) != napi_ok ||
      napi_create_function(env, "duplicateWrapStatus", NAPI_AUTO_LENGTH, duplicate_wrap_status, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "duplicateWrapStatus", function) != napi_ok ||
      napi_create_function(env, "bufferProbe", NAPI_AUTO_LENGTH, buffer_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "bufferProbe", function) != napi_ok ||
      napi_create_function(env, "typedArrayProbe", NAPI_AUTO_LENGTH, typedarray_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "typedArrayProbe", function) != napi_ok ||
      napi_create_function(env, "invalidTypedArray", NAPI_AUTO_LENGTH, invalid_typedarray, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "invalidTypedArray", function) != napi_ok ||
      napi_create_function(env, "callGuest", NAPI_AUTO_LENGTH, call_guest, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "callGuest", function) != napi_ok ||
      napi_create_function(env, "constructGuest", NAPI_AUTO_LENGTH, construct_guest, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "constructGuest", function) != napi_ok ||
      napi_create_function(env, "resolvedPromise", NAPI_AUTO_LENGTH, resolved_promise, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "resolvedPromise", function) != napi_ok ||
      napi_create_function(env, "runAsync", NAPI_AUTO_LENGTH, run_async_work, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "runAsync", function) != napi_ok ||
      napi_create_function(env, "runThreadsafe", NAPI_AUTO_LENGTH, run_threadsafe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "runThreadsafe", function) != napi_ok ||
      napi_create_function(env, "probeThreadsafeQueue", NAPI_AUTO_LENGTH, probe_threadsafe_queue, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "probeThreadsafeQueue", function) != napi_ok ||
      napi_create_function(env, "probeThreadsafeAbort", NAPI_AUTO_LENGTH, probe_threadsafe_abort, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "probeThreadsafeAbort", function) != napi_ok ||
      napi_create_function(env, "rejectedPromise", NAPI_AUTO_LENGTH, rejected_promise, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "rejectedPromise", function) != napi_ok ||
      napi_create_function(env, "pendingPromise", NAPI_AUTO_LENGTH, pending_promise, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "pendingPromise", function) != napi_ok ||
      napi_create_function(env, "resolvePendingPromise", NAPI_AUTO_LENGTH, resolve_pending_promise, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "resolvePendingPromise", function) != napi_ok ||
      napi_create_function(env, "propertyProbe", NAPI_AUTO_LENGTH, property_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "propertyProbe", function) != napi_ok ||
      napi_create_function(env, "globalProbe", NAPI_AUTO_LENGTH, global_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "globalProbe", function) != napi_ok ||
      napi_create_function(env, "symbolProbe", NAPI_AUTO_LENGTH, symbol_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "symbolProbe", function) != napi_ok ||
      napi_create_function(env, "invalidEnvironment", NAPI_AUTO_LENGTH, invalid_environment, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "invalidEnvironment", function) != napi_ok ||
      napi_create_function(env, "cleanupMisuseStatus", NAPI_AUTO_LENGTH, cleanup_misuse_status, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "cleanupMisuseStatus", function) != napi_ok ||
      napi_create_function(env, "errorInfoProbe", NAPI_AUTO_LENGTH, error_info_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "errorInfoProbe", function) != napi_ok) return NULL;
  return exports;
}
"#,
        )
        .unwrap();
        let mut build = Command::new("cc");
        build.args(["-std=c11", "-O2", "-fPIC"]);
        #[cfg(target_os = "linux")]
        build.arg("-shared");
        #[cfg(target_os = "macos")]
        build.args(["-dynamiclib", "-undefined", "dynamic_lookup"]);
        let built = build
            .args(["-pthread", "-DNAPI_VERSION=4", "-I"])
            .arg(include)
            .arg(&source)
            .arg("-o")
            .arg(&addon)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "Node-API fixture compilation failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        fs::write(
            root.join("main.cjs"),
            r#"
const addon = require('./fixture.node');
const values = addon.values;
const external = addon.external;
const externalProbe = addon.externalProbe();
const externalType = typeof external;
const externalKeys = Object.keys(external);
const externalJson = JSON.stringify(external);
const definedMethod = addon.definedMethod();
const definedValueBefore = addon.definedValue;
addon.definedValue = 23;
const definedValueAfter = addon.definedValue;
const definedMethodDescriptor = Object.getOwnPropertyDescriptor(addon, 'definedMethod');
const definedValueDescriptor = Object.getOwnPropertyDescriptor(addon, 'definedValue');
const definedConstantDescriptor = Object.getOwnPropertyDescriptor(addon, 'definedConstant');
addon.targetProbe();
const targetCallCounts = addon.targetCounts();
new addon.targetProbe();
const targetConstructCounts = addon.targetCounts();
const identityError = new Error('same');
const strictEqual = {
  sameObject: addon.strictEqualProbe(values, values),
  distinctObjects: addon.strictEqualProbe({}, {}),
  equalNumbers: addon.strictEqualProbe(42, 42),
  nan: addon.strictEqualProbe(0 / 0, 0 / 0),
  sameError: addon.strictEqualProbe(identityError, identityError),
  distinctErrors: addon.strictEqualProbe(new Error('same'), new Error('same')),
};
const counter = new addon.Counter(40);
const counterNewTargetInfo = addon.counterNewTargetInfo();
const counterIncremented = counter.increment();
const counterStaticDescriptor = Object.getOwnPropertyDescriptor(addon.Counter, 'offset');
const counterStaticMethodDescriptor = Object.getOwnPropertyDescriptor(addon.Counter, 'constant');
const counterStaticBaseDescriptor = Object.getOwnPropertyDescriptor(addon.Counter, 'baseValue');
const counterStaticBefore = addon.Counter.offset;
addon.Counter.offset = 18;
const counterStaticAfter = addon.Counter.offset;
const counterReadOnlyBefore = addon.Counter.readOnly;
addon.Counter.readOnly = 99;
const counterReadOnlyAfter = addon.Counter.readOnly;
const counterStaticDeleteRejected = delete addon.Counter.offset;
Object.setPrototypeOf(addon.Counter, { inheritedStatic: 'inherited' });
class CounterChild extends addon.Counter {}
const counterInheritedStatic = CounterChild.inheritedStatic;
const counterHasInheritedStatic = 'inheritedStatic' in CounterChild;
const counterInheritedBaseValue = CounterChild.baseValue;
const counterInheritedStaticMethod = CounterChild.constant();
const childCounter = new CounterChild(5);
const childNewTargetInfo = addon.counterNewTargetInfo();
const instanceChecks = {
  counterIsCounter: addon.instanceofProbe(counter, addon.Counter),
  counterIsChild: addon.instanceofProbe(counter, CounterChild),
  childIsCounter: addon.instanceofProbe(childCounter, addon.Counter),
  childIsChild: addon.instanceofProbe(childCounter, CounterChild),
  numberIsCounter: addon.instanceofProbe(3, addon.Counter),
  typeErrorIsError: addon.instanceofProbe(new TypeError('fixture'), Error),
  typeErrorIsTypeError: addon.instanceofProbe(new TypeError('fixture'), TypeError),
};
const errors = addon.createErrors();
let typeError;
let rangeError;
let createdThrow;
try { addon.throwTypeError(); } catch (error) {
  typeError = {name: error.name, message: error.message, code: error.code,
    isTypeError: error instanceof TypeError, isError: error instanceof Error};
}
try { addon.throwRangeError(); } catch (error) {
  rangeError = {name: error.name, message: error.message,
    isRangeError: error instanceof RangeError, isError: error instanceof Error};
}
try { addon.throwCreatedError(); } catch (error) {
  createdThrow = {name: error.name, message: error.message,
    isTypeError: error instanceof TypeError, isError: error instanceof Error};
}
const cleared = addon.throwAndClear();
const wrapped = addon.wrapProbe();
const removedWrap = addon.removeWrapProbe();
const duplicateWrapStatus = addon.duplicateWrapStatus();
const reference = addon.referenceProbe();
const referenceReleased = addon.releaseReference();
const makeClosure = () => () => {};
const firstClosure = makeClosure();
const secondClosure = makeClosure();
const buffers = addon.bufferProbe();
const typedArrays = addon.typedArrayProbe();
const stringEncodings = addon.stringEncodingProbe('Aé€😀');
const utf16 = addon.utf16Probe('Aé😀\0Z');
const elementDeleteTarget = [10, 20, 30];
const elementDelete = addon.deleteElementProbe(elementDeleteTarget, 1);
const escapableScope = addon.escapableScopeProbe();
const runScriptObservation = addon.runScriptProbe();
const runScriptResult = runScriptObservation.value;
const booleanCoercions = [undefined, null, false, 0, -0, NaN, '', 0n, [], {}]
  .map(value => addon.coerceToBoolean(value));
const numberCoercions = [undefined, null, false, true, '',
  ' ' + String.fromCharCode(0xFEFF) + ' ', '0x10', '0b11',
  '0o10', '1.5', 'Infinity', 'not a number'].map(value => addon.coerceToNumber(value));
const stringCoercions = [0, -0, null, undefined, true, 12n, [1, 2], {}]
  .map(value => addon.coerceToString(value));
const coercionEvents = [];
const guestNumberCoercion = addon.coerceToNumber({valueOf() {
  coercionEvents.push('number.valueOf');
  return '42';
}});
const guestStringCoercion = addon.coerceToString({toString() {
  coercionEvents.push('string.toString');
  return 23;
}});
const exoticCoercion = {[Symbol.toPrimitive](hint) {
  coercionEvents.push(`symbol:${hint}`);
  return hint === 'number' ? '44' : 'exotic';
}};
const exoticNumberCoercion = addon.coerceToNumber(exoticCoercion);
const exoticStringCoercion = addon.coerceToString(exoticCoercion);
function captureCoercionError(operation) {
  try { operation(); } catch (error) {
    return {name: error.name, isTypeError: error instanceof TypeError};
  }
}
const coercionErrors = {
  symbolNumber: captureCoercionError(() => addon.coerceToNumber(Symbol('value'))),
  symbolString: captureCoercionError(() => addon.coerceToString(Symbol('value'))),
  bigintNumber: captureCoercionError(() => addon.coerceToNumber(1n)),
};
let typedArrayError;
try { addon.invalidTypedArray(); } catch (error) {
  typedArrayError = {name: error.name, message: error.message, code: error.code,
    isRangeError: error instanceof RangeError, isError: error instanceof Error};
}
const callbackReceiver = {base: 40};
const callbackResult = addon.callGuest(function (amount) {
  this.base += addon.add(amount, 1);
  return this.base;
}, callbackReceiver, 1);
let callbackError;
try {
  addon.callGuest(function () { throw new RangeError('guest callback failure'); }, {}, 0);
} catch (error) {
  callbackError = {name: error.name, message: error.message,
    isRangeError: error instanceof RangeError, isError: error instanceof Error};
}
let callbackThrown;
try {
  addon.callGuest(function () { throw 'guest primitive failure'; }, {}, 0);
} catch (error) {
  callbackThrown = error;
}
class GuestBox {
  constructor(value) { this.value = value; }
}
const constructed = addon.constructGuest(GuestBox, 'constructed');
let propertyGetterCount = 0;
let propertySetterValue;
const propertyTarget = Object.create({inherited: true});
Object.defineProperty(propertyTarget, 'computed', {
  enumerable: true,
  configurable: true,
  get() { propertyGetterCount++; return 23; },
});
Object.defineProperty(propertyTarget, 'assigned', {
  enumerable: true,
  configurable: true,
  set(value) { propertySetterValue = value; },
});
propertyTarget.removeMe = true;
const properties = addon.propertyProbe(propertyTarget);
const backingBytes = new Uint8Array(typedArrays.buffer);
const customPrototype = {marker: 'prototype'};
const customPrototypeTarget = Object.create(customPrototype);
const nullPrototypeTarget = Object.create(null);
module.exports = {
  same: addon === require('./fixture.node'),
  global: addon.globalProbe(),
  globalHasObject: Object.hasOwn(globalThis, 'Object'),
  definedMethod,
  definedValueBefore,
  definedValueAfter,
  targetCallCounts,
  targetConstructCounts,
  strictEqual,
  counterNewTargetInfo,
  childNewTargetInfo,
  childCounterValue: childCounter.value,
  instanceChecks,
  supportsNapiV4: addon.supportsNapiV4,
  definedConstant: addon.definedConstant,
  definedSymbolValue: addon[addon.descriptorSymbol],
  definedMethodEnumerable: definedMethodDescriptor.enumerable,
  definedValueEnumerable: definedValueDescriptor.enumerable,
  definedConstantWritable: definedConstantDescriptor.writable,
  counterValue: counter.value,
  counterIncremented,
  counterInstance: counter instanceof addon.Counter,
  counterConstructor: counter.constructor === addon.Counter,
  counterStaticMethod: addon.Counter.constant(),
  counterStaticBaseValue: addon.Counter.baseValue,
  counterStaticBefore,
  counterStaticAfter,
  counterReadOnlyBefore,
  counterReadOnlyAfter,
  counterStaticDeleteRejected,
  counterInheritedStatic,
  counterHasInheritedStatic,
  counterInheritedBaseValue,
  counterInheritedStaticMethod,
  externalProbe,
  externalType,
  externalKeys,
  externalJson,
  counterStaticEnumerable: counterStaticDescriptor.enumerable,
  counterStaticMethodEnumerable: counterStaticMethodDescriptor.enumerable,
  counterStaticBaseWritable: counterStaticBaseDescriptor.writable,
  counterStaticKeys: Object.keys(addon.Counter).sort().join(','),
  symbols: addon.symbolProbe(),
  sum: addon.add(19, 23),
  version: addon.metadata.version,
  truth: values.truth,
  nothing: values.nothing === null,
  missing: values.missing === undefined,
  greeting: values.greeting,
  stringEncodings,
  utf16,
  elementDelete,
  escapableScope,
  runScriptResult,
  runScriptMicrotaskRanDuringCall: runScriptObservation.microtaskRanDuringCall,
  runScriptSideEffect: globalThis.napiRunScriptCount,
  elementDeleteLength: elementDeleteTarget.length,
  elementDeleteRemaining: [elementDeleteTarget[0], elementDeleteTarget[2]],
  elementDeleteHole: !(1 in elementDeleteTarget),
  booleanCoercions,
  numberCoercions,
  stringCoercions,
  guestNumberCoercion,
  guestStringCoercion,
  exoticNumberCoercion,
  exoticStringCoercion,
  coercionErrors,
  coercionEvents,
  fraction: values.fraction,
  maxUint32: values.maxUint32,
  int64: values.int64,
  roundTrip: addon.roundTrip(true, 4.25, 'native ✓', 4294967295, -2.5),
  int64Conversions: [NaN, Infinity, -Infinity, -0, 3.9, -3.9, 1e20, -1e20]
    .map(value => addon.int64ConversionProbe(value)),
  prototypes: {
    defaultMatches: addon.getPrototype({}) === Object.prototype,
    customMatches: addon.getPrototype(customPrototypeTarget) === customPrototype,
    nullMatches: addon.getPrototype(nullPrototypeTarget) === null,
  },
  array: addon.arrayProbe(),
  wrapped,
  removedWrap,
  duplicateWrapStatus,
  distinctFunctionIdentity: firstClosure !== secondClosure,
  buffers: {
    copy: [buffers.copy[0], buffers.copy[1], buffers.copy[2], buffers.copy[3]],
    allocated: [buffers.allocated[0], buffers.allocated[1], buffers.allocated[2]],
    copyIsBuffer: buffers.copyIsBuffer,
    allocatedIsBuffer: buffers.allocatedIsBuffer,
    arrayIsBuffer: buffers.arrayIsBuffer,
    copyLength: buffers.copyLength,
    allocatedLength: buffers.allocatedLength,
  },
  typedArrays: {
    bytes: [backingBytes[0], backingBytes[1], backingBytes[2], backingBytes[3],
      backingBytes[4], backingBytes[5], backingBytes[6], backingBytes[7]],
    isArrayBuffer: typedArrays.isArrayBuffer,
    isTypedArray: typedArrays.isTypedArray,
    isDataView: typedArrays.isDataView,
    kind: typedArrays.typedKind,
    typedLength: typedArrays.typedLength,
    typedOffset: typedArrays.typedOffset,
    viewLength: typedArrays.viewLength,
    viewOffset: typedArrays.viewOffset,
  },
  typedArrayError,
  callbackResult,
  callbackReceiverBase: callbackReceiver.base,
  callbackError,
  callbackThrown,
  constructedValue: constructed.value,
  properties: {
    computed: properties.computed,
    hasInherited: properties.hasInherited,
    hasNamedInherited: properties.hasNamedInherited,
    hasOwnInherited: properties.hasOwnInherited,
    deleted: properties.deleted,
    getterCount: propertyGetterCount,
    setterValue: propertySetterValue,
    removedFromGuest: !Object.hasOwn(propertyTarget, 'removeMe'),
    names: properties.names,
  },
  undefinedResult: addon.returnsUndefined() === undefined,
  errors: {
    error: {name: errors.error.name, message: errors.error.message,
      code: errors.error.code, isError: errors.error instanceof Error},
    typeError: {name: errors.typeError.name, message: errors.typeError.message,
      code: errors.typeError.code, isTypeError: errors.typeError instanceof TypeError,
      isError: errors.typeError instanceof Error},
    rangeError: {name: errors.rangeError.name, message: errors.rangeError.message,
      code: errors.rangeError.code, isRangeError: errors.rangeError instanceof RangeError,
      isError: errors.rangeError instanceof Error},
  },
  typeError,
  rangeError,
  createdThrow,
  cleared: {name: cleared.name, message: cleared.message, code: cleared.code},
  reference: {
    sameValue: reference.value === values,
    countAfterUnref: reference.countAfterUnref,
    countAfterRef: reference.countAfterRef,
    released: referenceReleased,
  },
};
"#,
        )
        .unwrap();
        let digest: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();

        let mut interpreter = Interpreter::with_builtins();
        interpreter
            .enable_rust_node_api_addons(
                RustNodeApiOptions::new([root.clone()])
                    .allow_native_addon_with_sha256(&addon, digest)
                    .entry(root.join("main.cjs")),
            )
            .unwrap();
        let observer = unsafe { Library::open(Some(addon.as_os_str()), RTLD_NOW) }.unwrap();
        let wrapped_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_wrapped_finalizer_calls\0")
                .unwrap()
        };
        let removed_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_removed_finalizer_calls\0")
                .unwrap()
        };
        let external_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_external_finalizer_calls\0")
                .unwrap()
        };
        let finalizer_create_function_status: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_finalizer_create_function_status\0")
                .unwrap()
        };
        let cleanup_hook_count: unsafe extern "C" fn() -> i32 =
            unsafe { *observer.get(b"napi_vm_test_cleanup_hook_count\0").unwrap() };
        let cleanup_hook_value: unsafe extern "C" fn(i32) -> i32 =
            unsafe { *observer.get(b"napi_vm_test_cleanup_hook_value\0").unwrap() };
        let cleanup_before_wrap_finalizer: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_cleanup_before_wrap_finalizer\0")
                .unwrap()
        };
        let threadsafe_finalizer_calls: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_threadsafe_finalizer_calls\0")
                .unwrap()
        };
        let threadsafe_worker_context_ok: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_threadsafe_worker_context_ok\0")
                .unwrap()
        };
        let threadsafe_worker_call_status: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_threadsafe_worker_call_status\0")
                .unwrap()
        };
        let threadsafe_worker_blocking_status: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_threadsafe_worker_blocking_status\0")
                .unwrap()
        };
        assert_eq!(unsafe { wrapped_finalizer_calls() }, 0);
        assert_eq!(unsafe { removed_finalizer_calls() }, 0);
        assert_eq!(unsafe { external_finalizer_calls() }, 0);
        assert_eq!(unsafe { cleanup_hook_count() }, 0);
        let result = interpreter.eval_source("require('./main.cjs');").unwrap();
        let invalid_utf16 = interpreter
            .eval_source("require('./fixture.node').invalidUtf16Status();")
            .unwrap();
        assert!(matches!(
            invalid_utf16.get_prop("status"),
            Some(Value::Number(status)) if status == NAPI_GENERIC_FAILURE as f64
        ));
        assert!(matches!(
            invalid_utf16.get_prop("messageMatches"),
            Some(Value::Bool(true))
        ));
        let initialized_promise_value = interpreter
            .eval_source("await require('./fixture.node').initializedPromise;")
            .unwrap();
        assert!(
            matches!(&initialized_promise_value, Value::String(value) if value == "resolved-during-init"),
            "unexpected initialized promise result: {initialized_promise_value:?}"
        );
        assert!(matches!(
            interpreter
                .eval_source("await require('./fixture.node').resolvedPromise();")
                .unwrap(),
            Value::String(ref value) if value == "resolved-from-addon"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "try { await require('./fixture.node').rejectedPromise(); } catch (reason) { reason; }"
                )
                .unwrap(),
            Value::String(ref value) if value == "rejected-from-addon"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "globalThis.napiHostPendingPromise = require('./fixture.node').pendingPromise(); globalThis.napiHostPendingResult = 'waiting'; globalThis.napiHostPendingPromise.then(value => { globalThis.napiHostPendingResult = value; }); 'created';"
                )
                .unwrap(),
            Value::String(ref value) if value == "created"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "require('./fixture.node').resolvePendingPromise({ then: resolve => resolve('settled-after-callback') });"
                )
                .unwrap(),
            Value::Bool(true)
        ));
        assert!(matches!(
            interpreter
                .eval_source("await globalThis.napiHostPendingPromise;")
                .unwrap(),
            Value::String(ref value) if value == "settled-after-callback"
        ));
        assert!(matches!(
            interpreter
                .eval_source("globalThis.napiHostPendingResult;")
                .unwrap(),
            Value::String(ref value) if value == "settled-after-callback"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "globalThis.napiHostAdoptedPromise = require('./fixture.node').pendingPromise(); 'created';"
                )
                .unwrap(),
            Value::String(ref value) if value == "created"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "require('./fixture.node').resolvePendingPromise(require('./fixture.node').resolvedPromise());"
                )
                .unwrap(),
            Value::Bool(true)
        ));
        assert!(matches!(
            interpreter
                .eval_source("await globalThis.napiHostAdoptedPromise;")
                .unwrap(),
            Value::String(ref value) if value == "resolved-from-addon"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "globalThis.napiHostRejectedThenable = require('./fixture.node').pendingPromise(); 'created';"
                )
                .unwrap(),
            Value::String(ref value) if value == "created"
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "require('./fixture.node').resolvePendingPromise({ then() { throw new TypeError('thenable failed'); } });"
                )
                .unwrap(),
            Value::Bool(true)
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "try { await globalThis.napiHostRejectedThenable; } catch (error) { error.message; }"
                )
                .unwrap(),
            Value::String(ref value) if value == "thenable failed"
        ));
        assert_eq!(unsafe { threadsafe_finalizer_calls() }, 0);
        let threadsafe_statuses = interpreter
            .eval_source(
                "globalThis.threadsafeValues = []; const addon = require('./fixture.node'); addon.runThreadsafe(value => { threadsafeValues.push(value); if (value === 'threadsafe-value') queueMicrotask(() => threadsafeValues.push('worker-microtask')); }); const queueFullStatus = addon.probeThreadsafeQueue(value => threadsafeValues.push(value)); const abortStatus = addon.probeThreadsafeAbort(); globalThis.threadsafeStatuses = {queueFullStatus, abortStatus}; threadsafeStatuses;",
            )
            .unwrap();
        assert!(matches!(
            threadsafe_statuses.get_prop("queueFullStatus"),
            Some(Value::Number(15.0))
        ));
        assert!(matches!(
            threadsafe_statuses.get_prop("abortStatus"),
            Some(Value::Number(16.0))
        ));
        assert_eq!(unsafe { threadsafe_worker_context_ok() }, 1);
        assert_eq!(unsafe { threadsafe_worker_call_status() }, 0);
        let has_valid_threadsafe_order = |events: &Value| {
            matches!(events, Value::String(events)
                if events == "threadsafe-value,worker-microtask,queue-first,threadsafe-second"
                    || events == "threadsafe-value,worker-microtask,threadsafe-second,queue-first"
                    || events == "queue-first,threadsafe-value,worker-microtask,threadsafe-second")
        };
        for _ in 0..10 {
            let _ = interpreter
                .run_event_loop_once(Duration::from_millis(250))
                .unwrap();
            let received = interpreter
                .eval_source("threadsafeValues.join(',');")
                .unwrap();
            if has_valid_threadsafe_order(&received) && unsafe { threadsafe_finalizer_calls() } == 2
            {
                break;
            }
        }
        let received = interpreter
            .eval_source("threadsafeValues.join(',');")
            .unwrap();
        assert!(
            has_valid_threadsafe_order(&received),
            "unexpected thread-safe callback events: {received:?}; finalizers={}; worker_status={}; blocking_status={}",
            unsafe { threadsafe_finalizer_calls() },
            unsafe { threadsafe_worker_call_status() },
            unsafe { threadsafe_worker_blocking_status() }
        );
        assert_eq!(unsafe { threadsafe_finalizer_calls() }, 2);
        assert_eq!(unsafe { threadsafe_worker_blocking_status() }, 0);
        let vm_threadsafe_json = interpreter
            .eval_source(
                "JSON.stringify({events: threadsafeValues.slice().sort(), queueStatus: threadsafeStatuses.queueFullStatus, abortStatus: threadsafeStatuses.abortStatus});",
            )
            .unwrap();
        let Value::String(ref vm_threadsafe_json) = vm_threadsafe_json else {
            panic!("thread-safe Node-API fixture did not return JSON");
        };
        let vm_threadsafe_result: serde_json::Value =
            serde_json::from_str(vm_threadsafe_json).expect("VM thread-safe result is valid JSON");
        let threadsafe_runner = "(async function() { const addon = require('./fixture.node'); const events = []; let finish; const done = new Promise(resolve => { finish = resolve; }); const callback = value => { events.push(value); if (value === 'threadsafe-value') queueMicrotask(() => events.push('worker-microtask')); if (events.includes('threadsafe-value') && events.includes('threadsafe-second') && events.includes('queue-first')) finish(); }; addon.runThreadsafe(callback); const queueStatus = addon.probeThreadsafeQueue(callback); const abortStatus = addon.probeThreadsafeAbort(); const timeout = setTimeout(() => { console.error('thread-safe function timed out'); process.exitCode = 1; }, 3000); timeout.unref?.(); await done; clearTimeout(timeout); process.stdout.write(JSON.stringify({events: events.sort(), queueStatus, abortStatus})); })().catch(error => { console.error(error); process.exitCode = 1; });";

        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let reference = Command::new("node")
                .current_dir(&root)
                .args(["-e", threadsafe_runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Node thread-safe reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let node_result: serde_json::Value = serde_json::from_slice(&reference.stdout)
                .expect("Node thread-safe result is valid JSON");
            assert_eq!(vm_threadsafe_result, node_result);
        }

        if let Ok(bun_version) = Command::new("bun").arg("--version").output()
            && bun_version.status.success()
        {
            let reference = Command::new("bun")
                .current_dir(&root)
                .args(["-e", threadsafe_runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Bun thread-safe reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let bun_result: serde_json::Value = serde_json::from_slice(&reference.stdout)
                .expect("Bun thread-safe result is valid JSON");
            assert_eq!(vm_threadsafe_result, bun_result);
        }
        assert!(matches!(result.get_prop("same"), Some(Value::Bool(true))));
        assert!(matches!(result.get_prop("global"), Some(Value::Bool(true))));
        assert!(matches!(
            result.get_prop("globalHasObject"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("definedMethod"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("definedValueBefore"),
            Some(Value::Number(5.0))
        ));
        assert!(matches!(
            result.get_prop("definedValueAfter"),
            Some(Value::Number(23.0))
        ));
        let target_call_counts = result.get_prop("targetCallCounts").unwrap();
        assert!(matches!(
            target_call_counts.get_prop("calls"),
            Some(Value::Number(1.0))
        ));
        assert!(matches!(
            target_call_counts.get_prop("constructs"),
            Some(Value::Number(0.0))
        ));
        let target_construct_counts = result.get_prop("targetConstructCounts").unwrap();
        assert!(matches!(
            target_construct_counts.get_prop("calls"),
            Some(Value::Number(1.0))
        ));
        assert!(matches!(
            target_construct_counts.get_prop("constructs"),
            Some(Value::Number(1.0))
        ));
        let strict_equal = result.get_prop("strictEqual").unwrap();
        assert!(matches!(
            strict_equal.get_prop("sameObject"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            strict_equal.get_prop("distinctObjects"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            strict_equal.get_prop("equalNumbers"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            strict_equal.get_prop("nan"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            strict_equal.get_prop("sameError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            strict_equal.get_prop("distinctErrors"),
            Some(Value::Bool(false))
        ));
        let counter_new_target_info = result.get_prop("counterNewTargetInfo").unwrap();
        assert!(matches!(
            counter_new_target_info.get_prop("seen"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            counter_new_target_info.get_prop("child"),
            Some(Value::Bool(false))
        ));
        let child_new_target_info = result.get_prop("childNewTargetInfo").unwrap();
        assert!(matches!(
            child_new_target_info.get_prop("seen"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            child_new_target_info.get_prop("child"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("childCounterValue"),
            Some(Value::Number(6.0))
        ));
        let instance_checks = result.get_prop("instanceChecks").unwrap();
        for (name, expected) in [
            ("counterIsCounter", true),
            ("counterIsChild", false),
            ("childIsCounter", true),
            ("childIsChild", true),
            ("numberIsCounter", false),
            ("typeErrorIsError", true),
            ("typeErrorIsTypeError", true),
        ] {
            assert!(matches!(
                instance_checks.get_prop(name),
                Some(Value::Bool(value)) if value == expected
            ));
        }
        assert!(matches!(
            result.get_prop("supportsNapiV4"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("externalProbe"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("externalType"),
            Some(Value::String(ref value)) if value == "object"
        ));
        assert!(matches!(
            result.get_prop("externalKeys"),
            Some(Value::Array(ref values)) if values.borrow().is_empty()
        ));
        assert!(matches!(
            result.get_prop("externalJson"),
            Some(Value::String(ref value)) if value == "{}"
        ));
        assert!(matches!(
            result.get_prop("definedConstant"),
            Some(Value::Number(7.0))
        ));
        assert!(matches!(
            result.get_prop("definedSymbolValue"),
            Some(Value::Number(17.0))
        ));
        assert!(matches!(
            result.get_prop("definedMethodEnumerable"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            result.get_prop("definedValueEnumerable"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("definedConstantWritable"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterValue"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("counterIncremented"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("counterInstance"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterConstructor"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterStaticMethod"),
            Some(Value::Number(105.0))
        ));
        assert!(matches!(
            result.get_prop("counterStaticBaseValue"),
            Some(Value::Number(6.0))
        ));
        assert!(matches!(
            result.get_prop("counterStaticBefore"),
            Some(Value::Number(8.0))
        ));
        assert!(matches!(
            result.get_prop("counterStaticAfter"),
            Some(Value::Number(18.0))
        ));
        assert!(matches!(
            result.get_prop("counterReadOnlyBefore"),
            Some(Value::Number(21.0))
        ));
        assert!(matches!(
            result.get_prop("counterReadOnlyAfter"),
            Some(Value::Number(21.0))
        ));
        assert!(matches!(
            result.get_prop("counterStaticDeleteRejected"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            result.get_prop("counterInheritedStatic"),
            Some(Value::String(ref value)) if value == "inherited"
        ));
        assert!(matches!(
            result.get_prop("counterHasInheritedStatic"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterInheritedBaseValue"),
            Some(Value::Number(6.0))
        ));
        assert!(matches!(
            result.get_prop("counterInheritedStaticMethod"),
            Some(Value::Number(105.0))
        ));
        assert!(matches!(
            result.get_prop("counterStaticEnumerable"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterStaticMethodEnumerable"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            result.get_prop("counterStaticBaseWritable"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("counterStaticKeys"),
            Some(Value::String(ref value)) if value == "baseValue,offset,readOnly"
        ));
        let symbols = result.get_prop("symbols").unwrap();
        assert!(matches!(
            symbols.get_prop("value"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            symbols.get_prop("hasKey"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            symbols.get_prop("hasOtherKey"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            symbols.get_prop("noDescriptionIsSymbol"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            symbols.get_prop("stringKeyCount"),
            Some(Value::Number(0.0))
        ));
        assert!(matches!(
            result.get_prop("sum"),
            Some(Value::Number(value)) if value == 42.0
        ));
        assert!(matches!(
            result.get_prop("version"),
            Some(Value::Number(value)) if value == 1.0
        ));
        assert!(matches!(result.get_prop("truth"), Some(Value::Bool(true))));
        assert!(matches!(
            result.get_prop("nothing"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("missing"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("greeting"),
            Some(Value::String(ref value)) if value == "Node-API ✓"
        ));
        let string_encodings = result.get_prop("stringEncodings").unwrap();
        assert!(matches!(
            string_encodings.get_prop("created"),
            Some(Value::String(ref value)) if value == "A\0éÿ"
        ));
        assert!(matches!(
            string_encodings.get_prop("required"),
            Some(Value::Number(5.0))
        ));
        assert!(matches!(
            string_encodings.get_prop("copied"),
            Some(Value::Number(5.0))
        ));
        assert!(matches!(
            string_encodings.get_prop("truncatedText"),
            Some(Value::String(ref value)) if value == "Aé¬"
        ));
        assert!(matches!(
            string_encodings.get_prop("truncatedCopied"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            string_encodings.get_prop("wrongTypeStatus"),
            Some(Value::Number(value)) if value == NAPI_STRING_EXPECTED as f64
        ));
        let latin1_byte_values = string_encodings.get_prop("bytes").unwrap();
        let Value::Array(string_bytes) = &latin1_byte_values else {
            panic!("Latin-1 extraction did not return an array");
        };
        let extracted_bytes = string_bytes
            .borrow()
            .iter()
            .map(|value| match value {
                Value::Number(value) => *value as u8,
                _ => panic!("Latin-1 byte array contains a non-number"),
            })
            .collect::<Vec<_>>();
        assert_eq!(extracted_bytes, [b'A', 0xE9, 0xAC, 0x3D, 0x00]);
        let utf16 = result.get_prop("utf16").unwrap();
        assert!(matches!(
            utf16.get_prop("roundTrip"),
            Some(Value::String(ref value)) if value == "Aé😀\0Z"
        ));
        assert!(matches!(
            utf16.get_prop("autoLength"),
            Some(Value::String(ref value)) if value == "TERM"
        ));
        assert!(matches!(utf16.get_prop("length"), Some(Value::Number(6.0))));
        assert!(matches!(utf16.get_prop("copied"), Some(Value::Number(6.0))));
        assert!(matches!(
            utf16.get_prop("truncatedCopied"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            utf16.get_prop("wrongTypeStatus"),
            Some(Value::Number(value)) if value == NAPI_STRING_EXPECTED as f64
        ));
        assert_eq!(
            number_array(utf16.get_prop("units").unwrap()),
            [65, 233, 0xD83D, 0xDE00, 0, 90]
        );
        assert_eq!(
            number_array(utf16.get_prop("truncatedUnits").unwrap()),
            [65, 233, 0xD83D]
        );
        let element_delete = result.get_prop("elementDelete").unwrap();
        assert!(matches!(
            element_delete.get_prop("deleted"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            element_delete.get_prop("present"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            element_delete.get_prop("length"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            result.get_prop("elementDeleteLength"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            result.get_prop("elementDeleteHole"),
            Some(Value::Bool(true))
        ));
        assert_eq!(
            number_array(result.get_prop("elementDeleteRemaining").unwrap()),
            [10, 30]
        );
        let escapable_scope = result.get_prop("escapableScope").unwrap();
        assert!(matches!(
            escapable_scope.get_prop("escaped"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            escapable_scope.get_prop("secondEscapeStatus"),
            Some(Value::Number(value)) if value == NAPI_ESCAPE_CALLED_TWICE as f64
        ));
        assert!(matches!(
            result.get_prop("runScriptResult"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("runScriptSideEffect"),
            Some(Value::Number(1.0))
        ));
        assert!(matches!(
            result.get_prop("runScriptMicrotaskRanDuringCall"),
            Some(Value::Bool(false))
        ));
        let boolean_coercions = result.get_prop("booleanCoercions").unwrap();
        let Value::Array(boolean_coercions) = &boolean_coercions else {
            panic!("Node-API boolean coercion fixture did not return an array");
        };
        let boolean_coercion_values = boolean_coercions
            .borrow()
            .iter()
            .map(|value| matches!(value, Value::Bool(true)))
            .collect::<Vec<_>>();
        assert_eq!(
            boolean_coercion_values,
            [
                false, false, false, false, false, false, false, false, true, true
            ]
        );
        let number_coercions = result.get_prop("numberCoercions").unwrap();
        let Value::Array(number_coercions) = &number_coercions else {
            panic!("Node-API number coercion fixture did not return an array");
        };
        let number_coercion_values = number_coercions.borrow();
        let number_coercion_values = number_coercion_values
            .iter()
            .map(|value| match value {
                Value::Number(value) => *value,
                _ => panic!("Node-API number coercion returned a non-number"),
            })
            .collect::<Vec<_>>();
        assert!(number_coercion_values[0].is_nan());
        assert_eq!(
            &number_coercion_values[1..11],
            &[0.0, 0.0, 1.0, 0.0, 0.0, 16.0, 3.0, 8.0, 1.5, f64::INFINITY]
        );
        assert!(number_coercion_values[11].is_nan());
        let string_coercions = result.get_prop("stringCoercions").unwrap();
        let Value::Array(string_coercions) = &string_coercions else {
            panic!("Node-API string coercion fixture did not return an array");
        };
        let string_coercion_values = {
            let values = string_coercions.borrow();
            values
                .iter()
                .map(|value| match value {
                    Value::String(value) => value.clone(),
                    _ => panic!("Node-API string coercion returned a non-string"),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            string_coercion_values,
            [
                "0",
                "0",
                "null",
                "undefined",
                "true",
                "12",
                "1,2",
                "[object Object]"
            ]
        );
        assert!(matches!(
            result.get_prop("guestNumberCoercion"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("guestStringCoercion"),
            Some(Value::String(ref value)) if value == "23"
        ));
        assert!(matches!(
            result.get_prop("exoticNumberCoercion"),
            Some(Value::Number(44.0))
        ));
        assert!(matches!(
            result.get_prop("exoticStringCoercion"),
            Some(Value::String(ref value)) if value == "exotic"
        ));
        let coercion_errors = result.get_prop("coercionErrors").unwrap();
        for name in ["symbolNumber", "symbolString", "bigintNumber"] {
            let error = coercion_errors.get_prop(name).unwrap();
            assert!(matches!(
                error.get_prop("name"),
                Some(Value::String(ref value)) if value == "TypeError"
            ));
            assert!(matches!(
                error.get_prop("isTypeError"),
                Some(Value::Bool(true))
            ));
        }
        let coercion_events = result.get_prop("coercionEvents").unwrap();
        let Value::Array(coercion_events) = &coercion_events else {
            panic!("Node-API coercion event fixture did not return an array");
        };
        assert!(matches!(
            coercion_events.borrow().as_slice(),
            [
                Value::String(number),
                Value::String(string),
                Value::String(number_hint),
                Value::String(string_hint)
            ] if number == "number.valueOf"
                && string == "string.toString"
                && number_hint == "symbol:number"
                && string_hint == "symbol:string"
        ));
        assert!(matches!(
            result.get_prop("fraction"),
            Some(Value::Number(value)) if value == 1.25
        ));
        assert!(matches!(
            result.get_prop("maxUint32"),
            Some(Value::Number(value)) if value == u32::MAX as f64
        ));
        assert!(matches!(
            result.get_prop("int64"),
            Some(Value::Number(value)) if value == 2_147_483_648.0
        ));
        let int64_conversions = result.get_prop("int64Conversions").unwrap();
        let Value::Array(int64_conversions) = &int64_conversions else {
            panic!("Node-API int64 conversion fixture did not return an array");
        };
        let int64_conversion_values = int64_conversions
            .borrow()
            .iter()
            .map(|value| match value {
                Value::Number(value) => *value,
                _ => panic!("Node-API int64 conversion fixture returned a non-number"),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            int64_conversion_values,
            [
                0.0,
                0.0,
                0.0,
                0.0,
                3.0,
                -3.0,
                i64::MAX as f64,
                i64::MIN as f64
            ]
        );
        let prototypes = result.get_prop("prototypes").unwrap();
        assert!(matches!(
            prototypes.get_prop("defaultMatches"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            prototypes.get_prop("customMatches"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            prototypes.get_prop("nullMatches"),
            Some(Value::Bool(true))
        ));
        let round_trip = result.get_prop("roundTrip").unwrap();
        assert!(matches!(
            round_trip.get_prop("flag"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            round_trip.get_prop("number"),
            Some(Value::Number(value)) if value == 4.75
        ));
        assert!(matches!(
            round_trip.get_prop("text"),
            Some(Value::String(ref value)) if value == "native ✓"
        ));
        assert!(matches!(
            round_trip.get_prop("uint32"),
            Some(Value::Number(value)) if value == u32::MAX as f64
        ));
        assert!(matches!(
            round_trip.get_prop("int64"),
            Some(Value::Number(value)) if value == -2.0
        ));
        assert!(matches!(
            round_trip.get_prop("boolType"),
            Some(Value::Number(2.0))
        ));
        assert!(matches!(
            round_trip.get_prop("numberType"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            round_trip.get_prop("stringType"),
            Some(Value::Number(4.0))
        ));
        let array = result.get_prop("array").unwrap();
        assert!(matches!(array.get_prop("isArray"), Some(Value::Bool(true))));
        assert!(matches!(array.get_prop("length"), Some(Value::Number(5.0))));
        assert!(matches!(
            array.get_prop("emptyLength"),
            Some(Value::Number(0.0))
        ));
        assert!(matches!(
            array.get_prop("firstPresent"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            array.get_prop("secondPresent"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            array.get_prop("holePresent"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(array.get_prop("value"), Some(Value::Bool(true))));
        assert!(
            matches!(result.get_prop("wrapped"), Some(Value::String(ref value)) if value == "wrapped-native-data")
        );
        assert!(
            matches!(result.get_prop("removedWrap"), Some(Value::String(ref value)) if value == "removed-native-data")
        );
        assert!(
            matches!(result.get_prop("duplicateWrapStatus"), Some(Value::Number(value)) if value == NAPI_INVALID_ARG as f64)
        );
        assert!(matches!(
            result.get_prop("distinctFunctionIdentity"),
            Some(Value::Bool(true))
        ));
        let buffers = result.get_prop("buffers").unwrap();
        let copied = buffers.get_prop("copy").unwrap();
        let Value::Array(copied) = &copied else {
            panic!("copied buffer values are not an array");
        };
        let copied = copied.borrow();
        assert!(matches!(copied.first(), Some(Value::Number(65.0))));
        assert!(matches!(copied.get(1), Some(Value::Number(120.0))));
        assert!(matches!(copied.get(2), Some(Value::Number(67.0))));
        assert!(matches!(copied.get(3), Some(Value::Number(68.0))));
        let allocated = buffers.get_prop("allocated").unwrap();
        let Value::Array(allocated) = &allocated else {
            panic!("allocated buffer values are not an array");
        };
        let allocated = allocated.borrow();
        assert!(matches!(allocated.first(), Some(Value::Number(7.0))));
        assert!(matches!(allocated.get(1), Some(Value::Number(8.0))));
        assert!(matches!(allocated.get(2), Some(Value::Number(9.0))));
        assert!(matches!(
            buffers.get_prop("copyIsBuffer"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            buffers.get_prop("allocatedIsBuffer"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            buffers.get_prop("arrayIsBuffer"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            buffers.get_prop("copyLength"),
            Some(Value::Number(4.0))
        ));
        assert!(matches!(
            buffers.get_prop("allocatedLength"),
            Some(Value::Number(3.0))
        ));
        let typed_arrays = result.get_prop("typedArrays").unwrap();
        let bytes = typed_arrays.get_prop("bytes").unwrap();
        let Value::Array(bytes) = &bytes else {
            panic!("backing bytes are not an array");
        };
        let bytes = bytes.borrow();
        let expected = [10.0, 11.0, 12.0, 55.0, 77.0, 15.0, 16.0, 17.0];
        for (byte, expected) in bytes.iter().zip(expected) {
            assert!(matches!(byte, Value::Number(value) if *value == expected));
        }
        assert!(matches!(
            typed_arrays.get_prop("isArrayBuffer"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            typed_arrays.get_prop("isTypedArray"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            typed_arrays.get_prop("isDataView"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            typed_arrays.get_prop("kind"),
            Some(Value::Number(4.0))
        ));
        assert!(matches!(
            typed_arrays.get_prop("typedLength"),
            Some(Value::Number(2.0))
        ));
        assert!(matches!(
            typed_arrays.get_prop("typedOffset"),
            Some(Value::Number(2.0))
        ));
        assert!(matches!(
            typed_arrays.get_prop("viewLength"),
            Some(Value::Number(3.0))
        ));
        assert!(matches!(
            typed_arrays.get_prop("viewOffset"),
            Some(Value::Number(4.0))
        ));
        assert_error_fields(
            &result.get_prop("typedArrayError").unwrap(),
            "RangeError",
            "start offset of Uint16Array should be a multiple of 2",
            Some("ERR_NAPI_INVALID_TYPEDARRAY_ALIGNMENT"),
        );
        assert!(matches!(
            result
                .get_prop("typedArrayError")
                .unwrap()
                .get_prop("isRangeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result
                .get_prop("typedArrayError")
                .unwrap()
                .get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("callbackResult"),
            Some(Value::Number(42.0))
        ));
        assert!(matches!(
            result.get_prop("callbackReceiverBase"),
            Some(Value::Number(42.0))
        ));
        assert_error_fields(
            &result.get_prop("callbackError").unwrap(),
            "RangeError",
            "guest callback failure",
            None,
        );
        assert!(matches!(
            result
                .get_prop("callbackError")
                .unwrap()
                .get_prop("isRangeError"),
            Some(Value::Bool(true))
        ));
        assert!(
            matches!(
                result
                    .get_prop("callbackError")
                    .unwrap()
                    .get_prop("isError"),
                Some(Value::Bool(true))
            ),
            "callback error: {:?}",
            result.get_prop("callbackError")
        );
        assert!(matches!(
            result.get_prop("constructedValue"),
            Some(Value::String(ref value)) if value == "constructed"
        ));
        assert!(matches!(
            result.get_prop("callbackThrown"),
            Some(Value::String(ref value)) if value == "guest primitive failure"
        ));
        let properties = result.get_prop("properties").unwrap();
        assert!(matches!(
            properties.get_prop("computed"),
            Some(Value::Number(23.0))
        ));
        assert!(matches!(
            properties.get_prop("hasInherited"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            properties.get_prop("hasNamedInherited"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            properties.get_prop("hasOwnInherited"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            properties.get_prop("deleted"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            properties.get_prop("getterCount"),
            Some(Value::Number(1.0))
        ));
        assert!(matches!(
            properties.get_prop("setterValue"),
            Some(Value::String(ref value)) if value == "set through addon"
        ));
        assert!(matches!(
            properties.get_prop("removedFromGuest"),
            Some(Value::Bool(true))
        ));
        let names = properties.get_prop("names").unwrap();
        let Value::Array(names) = &names else {
            panic!("property names are not an array");
        };
        let names = names.borrow();
        assert!(
            matches!(names.as_slice(), [Value::String(a), Value::String(b), Value::String(c)] if a == "computed" && b == "assigned" && c == "inherited")
        );
        assert!(matches!(
            result.get_prop("undefinedResult"),
            Some(Value::Bool(true))
        ));
        let reference = result.get_prop("reference").unwrap();
        assert!(matches!(
            reference.get_prop("sameValue"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            reference.get_prop("countAfterUnref"),
            Some(Value::Number(0.0))
        ));
        assert!(matches!(
            reference.get_prop("countAfterRef"),
            Some(Value::Number(1.0))
        ));
        assert!(matches!(
            reference.get_prop("released"),
            Some(Value::Bool(true))
        ));
        let errors = result.get_prop("errors").unwrap();
        assert_error_fields(
            &errors.get_prop("error").unwrap(),
            "Error",
            "created",
            Some("E_CREATED"),
        );
        assert!(matches!(
            errors.get_prop("error").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &errors.get_prop("typeError").unwrap(),
            "TypeError",
            "created",
            Some("E_CREATED"),
        );
        assert!(matches!(
            errors
                .get_prop("typeError")
                .unwrap()
                .get_prop("isTypeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            errors.get_prop("typeError").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &errors.get_prop("rangeError").unwrap(),
            "RangeError",
            "created",
            Some("E_CREATED"),
        );
        assert!(matches!(
            errors
                .get_prop("rangeError")
                .unwrap()
                .get_prop("isRangeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            errors.get_prop("rangeError").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &result.get_prop("typeError").unwrap(),
            "TypeError",
            "type failure",
            Some("E_TYPE"),
        );
        assert!(matches!(
            result
                .get_prop("typeError")
                .unwrap()
                .get_prop("isTypeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("typeError").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &result.get_prop("rangeError").unwrap(),
            "RangeError",
            "range failure",
            None,
        );
        assert!(matches!(
            result
                .get_prop("rangeError")
                .unwrap()
                .get_prop("isRangeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("rangeError").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &result.get_prop("createdThrow").unwrap(),
            "TypeError",
            "thrown",
            None,
        );
        assert!(matches!(
            result
                .get_prop("createdThrow")
                .unwrap()
                .get_prop("isTypeError"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            result.get_prop("createdThrow").unwrap().get_prop("isError"),
            Some(Value::Bool(true))
        ));
        assert_error_fields(
            &result.get_prop("cleared").unwrap(),
            "Error",
            "cleared failure",
            Some("E_CLEARED"),
        );

        assert!(matches!(
            interpreter
                .eval_source("require('./fixture.node').externalProbe();")
                .unwrap(),
            Value::Bool(true)
        ));
        assert!(matches!(
            interpreter
                .eval_source("require('./fixture.node').externalPropertyProbe();")
                .unwrap(),
            Value::Bool(true)
        ));
        assert!(matches!(
            interpreter
                .eval_source("require('./fixture.node').externalMemoryProbe();")
                .unwrap(),
            Value::Number(value) if value == -4096.0
        ));
        assert!(matches!(
            interpreter
                .eval_source("(() => { const e = require('./fixture.node').external; return Object.getPrototypeOf(e) === null && !Object.isExtensible(e) && e.missing === undefined && Object.keys(e).length === 0; })();")
                .unwrap(),
            Value::Bool(true)
        ));

        // Bun 1.4.0 currently returns zero for this accounting API, so keep
        // the Node-semantic check separate from the shared Node/Bun fixture.
        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let reference = Command::new("node")
                .current_dir(&root)
                .args([
                    "-e",
                    "process.stdout.write(String(require('./fixture.node').externalMemoryProbe()))",
                ])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Node external-memory reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let node_delta = String::from_utf8_lossy(&reference.stdout)
                .trim()
                .parse::<i64>()
                .expect("Node external-memory delta is an integer");
            assert_eq!(node_delta, -4096);
        }

        let guest_json = interpreter
            .eval_source("JSON.stringify(require('./main.cjs'));")
            .unwrap();
        let Value::String(ref guest_json) = guest_json else {
            panic!("JSON.stringify did not return a string");
        };
        let guest_result: serde_json::Value =
            serde_json::from_str(guest_json).expect("guest result is valid JSON");

        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let reference = Command::new("node")
                .args([
                    "-e",
                    "process.stdout.write(JSON.stringify(require(process.argv[1])))",
                ])
                .arg(root.join("main.cjs"))
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Node reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let node_result: serde_json::Value =
                serde_json::from_slice(&reference.stdout).expect("Node result is valid JSON");
            assert_eq!(node_result, guest_result);
        }

        if let Ok(bun_version) = Command::new("bun").arg("--version").output()
            && bun_version.status.success()
        {
            let reference = Command::new("bun")
                .args([
                    "-e",
                    "process.stdout.write(JSON.stringify(require(process.argv[1])))",
                ])
                .arg(root.join("main.cjs"))
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Bun reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let mut bun_result: serde_json::Value =
                serde_json::from_slice(&reference.stdout).expect("Bun result is valid JSON");
            let mut normalized_guest_result = guest_result.clone();
            for output in [&mut bun_result, &mut normalized_guest_result] {
                if let Some(error) = output
                    .get_mut("typedArrayError")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    // Node and Bun both throw a RangeError for this invalid
                    // typed-array view, but the message and Node error code
                    // are runtime-specific details.
                    error.remove("message");
                    error.remove("code");
                }
            }
            assert_eq!(bun_result, normalized_guest_result);
        }

        let async_runner = "(async function() { const addon = require('./fixture.node'); process.stdout.write(JSON.stringify({result: await addon.runAsync()})); })().catch(error => { console.error(error); process.exitCode = 1; });";
        let vm_async_result = interpreter
            .eval_source("let asyncResult = await require('./fixture.node').runAsync(); JSON.stringify({result: asyncResult});")
            .unwrap();
        let Value::String(ref vm_async_json) = vm_async_result else {
            panic!("async Node-API fixture did not return JSON: {vm_async_result:?}");
        };
        let vm_async_result: serde_json::Value =
            serde_json::from_str(vm_async_json).expect("VM async result is valid JSON");

        if let Ok(node_version) = Command::new("node").arg("--version").output()
            && node_version.status.success()
        {
            let reference = Command::new("node")
                .current_dir(&root)
                .args(["-e", async_runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Node async reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let node_async_result: serde_json::Value =
                serde_json::from_slice(&reference.stdout).expect("Node async result is valid JSON");
            assert_eq!(vm_async_result, node_async_result);
        }

        if let Ok(bun_version) = Command::new("bun").arg("--version").output()
            && bun_version.status.success()
        {
            let reference = Command::new("bun")
                .current_dir(&root)
                .args(["-e", async_runner])
                .output()
                .unwrap();
            assert!(
                reference.status.success(),
                "Bun async reference failed: {}",
                String::from_utf8_lossy(&reference.stderr)
            );
            let bun_async_result: serde_json::Value =
                serde_json::from_slice(&reference.stdout).expect("Bun async result is valid JSON");
            assert_eq!(vm_async_result, bun_async_result);
        }

        let invalid_env = interpreter
            .eval_source("require('./fixture.node').invalidEnvironment();")
            .unwrap();
        assert!(matches!(
            invalid_env,
            Value::Number(value) if value == NAPI_INVALID_ARG as f64
        ));
        assert!(matches!(
            interpreter
                .eval_source(
                    "JSON.stringify(require('./fixture.node').cleanupMisuseStatus());"
                )
                .unwrap(),
            Value::String(ref value) if value == "{\"duplicate\":1,\"unmatched\":1}"
        ));
        assert!(matches!(
            interpreter
                .eval_source("JSON.stringify(require('./fixture.node').errorInfoProbe());")
                .unwrap(),
            Value::String(ref value)
                if value == "{\"lastStatus\":6,\"messageMatches\":true}"
        ));

        drop(result);
        drop(invalid_env);
        drop(interpreter);
        assert_eq!(unsafe { wrapped_finalizer_calls() }, 1);
        assert_eq!(unsafe { removed_finalizer_calls() }, 0);
        assert_eq!(unsafe { external_finalizer_calls() }, 1);
        assert_eq!(unsafe { finalizer_create_function_status() }, NAPI_OK);
        assert_eq!(unsafe { cleanup_hook_count() }, 2);
        assert_eq!(unsafe { cleanup_hook_value(0) }, 4);
        assert_eq!(unsafe { cleanup_hook_value(1) }, 3);
        assert_eq!(unsafe { cleanup_before_wrap_finalizer() }, 1);
        drop(observer);
        fs::remove_dir_all(root).unwrap();
    }
}
