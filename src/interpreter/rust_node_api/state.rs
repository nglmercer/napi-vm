//! Node-API environment state, handle arena, and callback scopes.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::{CString, c_char, c_void};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

#[cfg(unix)]
use libloading::os::unix::Library;
#[cfg(target_os = "windows")]
use libloading::os::windows::Library;

use crate::error::VmErr;
use crate::host::HostCallback;
use crate::interpreter::Env;
use crate::value::{PromiseInner, Value};

use super::shim::NodeApiShim;
use super::{
    MAX_LOCAL_HANDLES, NAPI_ESCAPE_CALLED_TWICE, NAPI_GENERIC_FAILURE, NAPI_HANDLE_SCOPE_MISMATCH,
    NAPI_INVALID_ARG, NEXT_OPAQUE_HANDLE_ID, NapiAsyncCleanupHook, NapiAsyncCleanupHookHandle,
    NapiAsyncCompleteCallback, NapiAsyncExecuteCallback, NapiCallback, NapiCleanupHook, NapiEnv,
    NapiFinalize, NapiHandleScope, NapiThreadsafeFunctionCallJs, NapiValue, NodeApiNoEnvFinalize,
    ReportedNodeVersion,
};

pub(super) struct HostState {
    pub(super) global: Env,
    pub(super) object_prototype: Option<Value>,
    pub(super) reported_node_version: ReportedNodeVersion,
    pub(super) max_napi_version: u32,
    pub(super) next_callback_id: usize,
    pub(super) callbacks: HashMap<usize, NativeCallbackRecord>,
    pub(super) environments: Vec<Rc<NapiEnvironment>>,
    // Type tags belong to the JavaScript object, not the addon environment:
    // multiple addons must be able to recognize a tag set by another addon.
    // Retain tagged values because this interpreter does not have a tracing GC
    // and pointer identities must not be recycled while a tag is observable.
    pub(super) type_tags: HashMap<NapiObjectIdentity, (NapiTypeTag, Value)>,
    pub(super) libraries: HashMap<PathBuf, Arc<Library>>,
    pub(super) async_work_sender: SyncSender<AsyncWorkTaskMessage>,
    pub(super) runtime_notifications: Receiver<HostRuntimeNotification>,
    pub(super) runtime_notification_sender: Sender<HostRuntimeNotification>,
    pub(super) async_workers: Vec<JoinHandle<()>>,
    // Keep the process-global ABI shim loaded until every addon library closes.
    pub(super) _shim: Arc<NodeApiShim>,
}

#[derive(Clone)]
pub(super) struct NativeCallbackRecord {
    pub(super) env: Rc<NapiEnvironment>,
    pub(super) callback: NativeCallback,
    pub(super) data: *mut c_void,
    pub(super) one_shot: bool,
}

#[derive(Clone)]
pub(super) enum NativeCallback {
    Function(NapiCallback),
    PostedFinalizer {
        finalize: NapiFinalize,
        data: *mut c_void,
        hint: *mut c_void,
    },
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

pub(super) struct NapiEnvironment {
    pub(super) module_path: String,
    pub(super) module_file_url: CString,
    pub(super) api_version: u32,
    pub(super) node_version: NapiNodeVersion,
    pub(super) owner: Weak<RefCell<HostState>>,
    pub(super) handles: RefCell<NapiHandleArena>,
    pub(super) references: RefCell<HashMap<usize, NapiReference>>,
    pub(super) deferreds: RefCell<HashMap<usize, NapiDeferredState>>,
    pub(super) async_works: RefCell<HashMap<usize, NapiAsyncWorkState>>,
    pub(super) threadsafe_functions: RefCell<HashMap<usize, NapiThreadsafeFunctionState>>,
    pub(super) async_contexts: RefCell<HashMap<usize, NapiAsyncContextState>>,
    pub(super) callback_scopes: RefCell<Vec<NapiCallbackScopeState>>,
    pub(super) cleanup_hooks: RefCell<Vec<NapiCleanupHookRecord>>,
    pub(super) async_cleanup_hooks: RefCell<Vec<NapiAsyncCleanupHookRecord>>,
    pub(super) next_cleanup_hook_order: Cell<u64>,
    pub(super) last_error: Cell<NapiExtendedErrorInfo>,
    pub(super) wraps: RefCell<HashMap<NapiObjectIdentity, NapiWrap>>,
    pub(super) added_finalizers: RefCell<Vec<NapiAddedFinalizer>>,
    pub(super) instance_data: RefCell<Option<NapiInstanceData>>,
    pub(super) externals: RefCell<HashMap<NapiObjectIdentity, NapiExternal>>,
    pub(super) external_buffers: RefCell<HashMap<NapiObjectIdentity, NapiExternalBuffer>>,
    pub(super) external_memory: Cell<i64>,
    pub(super) finalizing: Cell<bool>,
    pub(super) active_callbacks: RefCell<HashMap<usize, CallbackFrame>>,
    pub(super) guest_callback_dispatchers: RefCell<Vec<GuestCallbackDispatcher>>,
    pub(super) pending_exception: RefCell<Option<Value>>,
    pub(super) fatal_exceptions: RefCell<VecDeque<Value>>,
}

#[repr(C)]
pub(super) struct NapiNodeVersion {
    pub(super) major: u32,
    pub(super) minor: u32,
    pub(super) patch: u32,
    pub(super) release: *const c_char,
}

pub(super) const NAPI_VM_RELEASE: &[u8] = b"napi-vm\0";

pub(super) type NapiAddonRegister = unsafe extern "C" fn(NapiEnv, NapiValue) -> NapiValue;

#[repr(C)]
pub(super) struct NapiModule {
    pub(super) version: i32,
    pub(super) flags: u32,
    pub(super) filename: *const c_char,
    pub(super) register: Option<NapiAddonRegister>,
    pub(super) module_name: *const c_char,
    pub(super) private_data: *mut c_void,
    pub(super) reserved: [*mut c_void; 4],
}

#[derive(Clone, Copy)]
pub(super) struct NapiCleanupHookRecord {
    pub(super) function: NapiCleanupHook,
    pub(super) function_address: usize,
    pub(super) argument: usize,
    pub(super) order: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NapiTypeTag {
    pub(super) lower: u64,
    pub(super) upper: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AsyncCleanupHookPhase {
    Registered,
    Running,
    Removed,
}

pub(super) struct AsyncCleanupHookControl {
    pub(super) phase: Mutex<AsyncCleanupHookPhase>,
    pub(super) completed: Condvar,
}

#[derive(Clone)]
pub(super) struct NapiAsyncCleanupHookRecord {
    pub(super) function: NapiAsyncCleanupHook,
    pub(super) function_address: usize,
    pub(super) argument: usize,
    pub(super) handle: usize,
    pub(super) order: u64,
    pub(super) control: Arc<AsyncCleanupHookControl>,
}

pub(super) fn async_cleanup_hook_registry()
-> &'static Mutex<HashMap<usize, Arc<AsyncCleanupHookControl>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<usize, Arc<AsyncCleanupHookControl>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn remove_async_cleanup_hook_handle(handle: NapiAsyncCleanupHookHandle) {
    if handle.is_null() {
        return;
    }
    let key = handle as usize;
    let control = async_cleanup_hook_registry()
        .lock()
        .ok()
        .and_then(|mut registry| registry.remove(&key));
    if let Some(control) = control
        && let Ok(mut phase) = control.phase.lock()
    {
        *phase = AsyncCleanupHookPhase::Removed;
        control.completed.notify_all();
    }
}

pub(super) fn next_environment_cleanup_hook_order(
    environment: &NapiEnvironment,
) -> Result<u64, i32> {
    let order = environment.next_cleanup_hook_order.get();
    environment
        .next_cleanup_hook_order
        .set(order.checked_add(1).ok_or(NAPI_GENERIC_FAILURE)?);
    Ok(order)
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct NapiExtendedErrorInfo {
    pub(super) error_message: *const c_char,
    pub(super) engine_reserved: *mut c_void,
    pub(super) engine_error_code: u32,
    pub(super) error_code: i32,
}

#[derive(Clone, Copy)]
pub(super) struct GuestCallbackDispatcher {
    pub(super) context: *mut c_void,
    pub(super) invoke: unsafe fn(*mut c_void, HostCallback) -> Result<Value, VmErr>,
}

/// The dispatcher is pushed only while a native callback is running. Its
/// context points at that callback's borrowed interpreter handler and is
/// removed before the handler's stack frame returns.
pub(super) struct GuestCallbackDispatcherScope {
    pub(super) environment: Rc<NapiEnvironment>,
}

impl GuestCallbackDispatcherScope {
    pub(super) fn push(
        environment: Rc<NapiEnvironment>,
        dispatcher: GuestCallbackDispatcher,
    ) -> Self {
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

pub(super) struct NapiReference {
    pub(super) value: Option<Value>,
    pub(super) ref_count: u32,
}

pub(super) struct NapiDeferredState {
    pub(super) promise: Rc<RefCell<PromiseInner>>,
    pub(super) settling: bool,
}

pub(super) struct NapiAsyncContextState {
    pub(super) _resource: Value,
    pub(super) _resource_name: Value,
}

#[derive(Clone, Copy)]
pub(super) struct NapiCallbackScopeState {
    pub(super) token: usize,
    pub(super) async_context: usize,
}

#[derive(Clone)]
pub(super) struct NapiAsyncWorkState {
    pub(super) execute: NapiAsyncExecuteCallback,
    pub(super) complete: NapiAsyncCompleteCallback,
    pub(super) data: *mut c_void,
    pub(super) state: Arc<AtomicU8>,
    pub(super) completion_status: Arc<AtomicU8>,
    pub(super) completion_callback_active: bool,
    pub(super) callback_run: bool,
}

pub(super) struct NapiThreadsafeFunctionState {
    pub(super) shared: Arc<NapiThreadsafeFunctionShared>,
    pub(super) callback: Option<Value>,
    pub(super) call_js: Option<NapiThreadsafeFunctionCallJs>,
    pub(super) context: *mut c_void,
    pub(super) finalize_data: *mut c_void,
    pub(super) finalize: Option<NapiFinalize>,
    pub(super) referenced: bool,
}

pub(super) struct NapiThreadsafeFunctionShared {
    pub(super) id: usize,
    pub(super) environment: usize,
    pub(super) context: usize,
    pub(super) max_queue_size: usize,
    pub(super) owner_thread: thread::ThreadId,
    pub(super) notifications: Sender<HostRuntimeNotification>,
    pub(super) state: Mutex<NapiThreadsafeFunctionQueue>,
    pub(super) queue_space: Condvar,
}

pub(super) struct NapiThreadsafeFunctionQueue {
    pub(super) values: VecDeque<usize>,
    pub(super) thread_count: usize,
    pub(super) in_flight: usize,
    pub(super) closing: bool,
    pub(super) orphaned: bool,
    pub(super) finalized: bool,
}

pub(super) static THREADSAFE_FUNCTIONS: OnceLock<
    Mutex<HashMap<usize, Arc<NapiThreadsafeFunctionShared>>>,
> = OnceLock::new();

pub(super) enum HostRuntimeNotification {
    AsyncWorkCompletion(AsyncWorkCompletion),
    ThreadsafeFunction(usize),
    PostedFinalizer(PostedFinalizer),
}

#[derive(Clone, Copy)]
pub(super) struct PostedFinalizer {
    pub(super) environment: usize,
    pub(super) finalize: NapiFinalize,
    pub(super) data: usize,
    pub(super) hint: usize,
}

pub(super) static POST_FINALIZER_SENDERS: OnceLock<
    Mutex<HashMap<usize, Sender<HostRuntimeNotification>>>,
> = OnceLock::new();

pub(super) fn post_finalizer_senders()
-> &'static Mutex<HashMap<usize, Sender<HostRuntimeNotification>>> {
    POST_FINALIZER_SENDERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) struct AsyncWorkTask {
    pub(super) work_id: usize,
    pub(super) environment: usize,
    pub(super) execute: NapiAsyncExecuteCallback,
    pub(super) data: usize,
    pub(super) state: Arc<AtomicU8>,
    pub(super) completion_status: Arc<AtomicU8>,
}

pub(super) enum AsyncWorkTaskMessage {
    Run(AsyncWorkTask),
    Stop,
}

#[derive(Clone, Copy)]
pub(super) struct AsyncWorkCompletion {
    pub(super) work_id: usize,
    pub(super) status: i32,
}

pub(super) type AsyncWorkPool = (SyncSender<AsyncWorkTaskMessage>, Vec<JoinHandle<()>>);

pub(super) struct NapiWrap {
    // Keeping the guest value alive prevents its identity pointer from being
    // reused while native data is still attached to it.
    pub(super) _value: Value,
    pub(super) data: *mut c_void,
    pub(super) finalize: Option<NapiFinalize>,
    pub(super) hint: *mut c_void,
}

pub(super) struct NapiAddedFinalizer {
    // As with wraps and externals, the VM has no tracing GC, so preserve the
    // associated guest value until the owning environment shuts down.
    pub(super) _value: Value,
    pub(super) data: *mut c_void,
    pub(super) finalize: NapiFinalize,
    pub(super) hint: *mut c_void,
    pub(super) reference: Option<usize>,
}

#[derive(Clone, Copy)]
pub(super) struct NapiInstanceData {
    pub(super) data: *mut c_void,
    pub(super) finalize: Option<NapiFinalize>,
    pub(super) hint: *mut c_void,
}

pub(super) struct NapiExternal {
    // The Node-API external has no own properties or prototype, but it must
    // stay alive until its finalizer runs during environment shutdown.
    pub(super) _value: Value,
    pub(super) data: *mut c_void,
    pub(super) finalize: Option<NapiFinalize>,
    pub(super) hint: *mut c_void,
}

pub(super) struct NapiExternalBuffer {
    // Retain each external buffer value and its native memory until host
    // shutdown, when its Node-API finalizer runs on the owning thread.
    pub(super) _value: Value,
    pub(super) data: *mut c_void,
    pub(super) finalize: NapiExternalBufferFinalizer,
    pub(super) hint: *mut c_void,
}

#[derive(Clone, Copy)]
pub(super) enum NapiExternalBufferFinalizer {
    Napi(Option<NapiFinalize>),
    NoEnv(Option<NodeApiNoEnvFinalize>),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum NapiObjectIdentity {
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
    SharedArrayBuffer(usize),
    TypedArray(usize),
    DataView(usize),
    RegExp(usize),
    Error(usize),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum NapiReferenceIdentity {
    Object(NapiObjectIdentity),
    Symbol(u64),
}

thread_local! {
    /// Only environments created on this thread may enter the synchronous
    /// Node-API surface. Looking up the opaque pointer before dereferencing it
    /// makes invalid `napi_env` values fail with `napi_invalid_arg` instead of
    /// causing undefined behavior in the host.
    pub(super) static NAPI_ENVIRONMENTS: RefCell<HashMap<usize, Weak<NapiEnvironment>>> =
        RefCell::new(HashMap::new());
    /// Deprecated `napi_module_register` modules register from a static
    /// constructor while `dlopen` is in progress. Keep registrations scoped
    /// to that load so nested addon loads cannot steal each other's entries.
    pub(super) static NAPI_MODULE_REGISTRATIONS: RefCell<Vec<Vec<usize>>> = const { RefCell::new(Vec::new()) };
}

pub(super) struct NapiModuleRegistrationScope {
    pub(super) active: bool,
}

impl NapiModuleRegistrationScope {
    pub(super) fn new() -> Self {
        NAPI_MODULE_REGISTRATIONS.with(|registrations| {
            registrations.borrow_mut().push(Vec::new());
        });
        Self { active: true }
    }

    pub(super) fn finish(mut self) -> Vec<usize> {
        self.active = false;
        NAPI_MODULE_REGISTRATIONS
            .with(|registrations| registrations.borrow_mut().pop().unwrap_or_default())
    }
}

impl Drop for NapiModuleRegistrationScope {
    fn drop(&mut self) {
        if self.active {
            NAPI_MODULE_REGISTRATIONS.with(|registrations| {
                registrations.borrow_mut().pop();
            });
        }
    }
}

#[derive(Clone)]
pub(super) struct CallbackFrame {
    pub(super) args: Vec<NapiValue>,
    pub(super) this_arg: NapiValue,
    pub(super) new_target: NapiValue,
    pub(super) data: *mut c_void,
}

#[derive(Clone, Copy)]
pub(super) struct HandleRef {
    pub(super) slot: usize,
    pub(super) generation: u64,
}

pub(super) struct HandleSlot {
    pub(super) generation: u64,
    pub(super) value: Option<Value>,
}

pub(super) struct HandleScope {
    pub(super) id: u64,
    pub(super) slots: Vec<usize>,
    pub(super) escapable: bool,
    pub(super) escape_used: bool,
}

pub(super) struct NapiHandleArena {
    pub(super) slots: Vec<HandleSlot>,
    pub(super) free_slots: Vec<usize>,
    pub(super) scopes: Vec<HandleScope>,
    pub(super) next_scope_id: u64,
    pub(super) handles: HashMap<usize, HandleRef>,
    pub(super) scope_handles: HashMap<usize, (u64, bool)>,
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
    pub(super) fn create(&mut self, value: Value) -> Result<NapiValue, i32> {
        self.create_in_scope(value, self.scopes.len() - 1)
    }

    pub(super) fn create_in_scope(
        &mut self,
        value: Value,
        scope_index: usize,
    ) -> Result<NapiValue, i32> {
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

    pub(super) fn get(&self, pointer: NapiValue) -> Result<Value, i32> {
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

    pub(super) fn open_scope(&mut self) -> Result<u64, i32> {
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

    pub(super) fn open_escapable_scope(&mut self) -> Result<u64, i32> {
        let id = self.open_scope()?;
        self.scopes
            .last_mut()
            .expect("scope was just opened")
            .escapable = true;
        Ok(id)
    }

    pub(super) fn create_scope_handle(
        &mut self,
        id: u64,
        escapable: bool,
    ) -> Result<NapiHandleScope, i32> {
        if self.scope_handles.len() >= MAX_LOCAL_HANDLES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let pointer = new_opaque_handle()?;
        self.scope_handles.insert(pointer as usize, (id, escapable));
        Ok(pointer)
    }

    pub(super) fn close_scope_handle(&mut self, pointer: NapiHandleScope) -> Result<(), i32> {
        self.close_scope_handle_with_kind(pointer, false)
    }

    pub(super) fn close_escapable_scope_handle(
        &mut self,
        pointer: NapiHandleScope,
    ) -> Result<(), i32> {
        self.close_scope_handle_with_kind(pointer, true)
    }

    pub(super) fn close_scope_handle_with_kind(
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

    pub(super) fn escape_handle(
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

    pub(super) fn close_scope(&mut self, id: u64) -> Result<(), i32> {
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
pub(super) fn new_opaque_handle() -> Result<*mut c_void, i32> {
    NEXT_OPAQUE_HANDLE_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map(|id| id as *mut c_void)
        .map_err(|_| NAPI_GENERIC_FAILURE)
}

impl NapiEnvironment {
    pub(super) fn raw(&self) -> NapiEnv {
        (self as *const Self).cast_mut().cast()
    }
}
