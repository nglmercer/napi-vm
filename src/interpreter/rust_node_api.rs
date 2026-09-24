//! Experimental in-process host for the Node-API C ABI.
//!
//! The in-process host intentionally implements a selected Node-API surface.
//! Unimplemented imports fail during dynamic loading; this backend does not
//! emulate Node, V8, NAN, or libuv.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
#[cfg(target_os = "windows")]
use std::ffi::OsStr;
use std::ffi::{CStr, CString, c_char, c_void};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak as SyncWeak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(unix)]
use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};
#[cfg(target_os = "windows")]
use libloading::os::windows::{
    LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    LOAD_LIBRARY_SEARCH_USER_DIRS, Library,
};

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn AddDllDirectory(new_directory: *const u16) -> *mut c_void;
    fn GetModuleHandleW(module_name: *const u16) -> *mut c_void;
    fn RemoveDllDirectory(cookie: *mut c_void) -> i32;
}

#[cfg(target_os = "windows")]
static WINDOWS_NODE_API_SHIM_DIRECTORIES: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
// Native addons can retain imports from the ABI provider beyond a VM/plugin
// environment (napi-rs does this for some generated modules). Keep one provider
// mapped for the process lifetime so a later plugin generation never calls a
// symbol in an unloaded shim.
static PROCESS_NODE_API_SHIM: OnceLock<Mutex<Option<Arc<NodeApiShim>>>> = OnceLock::new();
static PROCESS_NODE_API_ADDON_LIBRARIES: OnceLock<Mutex<HashMap<PathBuf, SyncWeak<Library>>>> =
    OnceLock::new();
static PINNED_NODE_API_ADDON_LIBRARIES: OnceLock<Mutex<HashMap<PathBuf, Arc<Library>>>> =
    OnceLock::new();

use crate::error::VmErr;
use crate::host::{HostBridge, HostCallback, HostCallbackKind, HostEvent};
use crate::interpreter::commonjs::NativeAddonLoader;
use crate::interpreter::native_addon::NativeAddonPolicy;
use crate::interpreter::native_addon_binary::validate_native_addon_binary;
#[cfg(all(test, target_os = "windows"))]
use crate::interpreter::native_addon_binary::validate_native_addon_header;
use crate::interpreter::{Env, FileCommonJsLoader, Interpreter};
use crate::value::{
    BoxedPrimitive, Buffer, ClassData, ErrorData, PromiseInner, PromiseState, PropAttrs,
    SharedBuffer, TypedArrayData, TypedKind, Value,
};

const NAPI_OK: i32 = 0;
const NAPI_INVALID_ARG: i32 = 1;
const NAPI_OBJECT_EXPECTED: i32 = 2;
const NAPI_STRING_EXPECTED: i32 = 3;
const NAPI_DATE_EXPECTED: i32 = 18;
const NAPI_FUNCTION_EXPECTED: i32 = 5;
const NAPI_NUMBER_EXPECTED: i32 = 6;
const NAPI_BOOLEAN_EXPECTED: i32 = 7;
const NAPI_ARRAY_EXPECTED: i32 = 8;
const NAPI_GENERIC_FAILURE: i32 = 9;
const NAPI_PENDING_EXCEPTION: i32 = 10;
const NAPI_CANCELLED: i32 = 11;
const NAPI_ESCAPE_CALLED_TWICE: i32 = 12;
const NAPI_HANDLE_SCOPE_MISMATCH: i32 = 13;
const NAPI_CALLBACK_SCOPE_MISMATCH: i32 = 14;
const NAPI_QUEUE_FULL: i32 = 15;
const NAPI_CLOSING: i32 = 16;
const NAPI_BIGINT_EXPECTED: i32 = 17;
const NAPI_ARRAYBUFFER_EXPECTED: i32 = 19;
const NAPI_DETACHABLE_ARRAYBUFFER_EXPECTED: i32 = 20;
const NAPI_SYMBOL_TYPE: i32 = 5;
const NAPI_OBJECT_TYPE: i32 = 6;
const NAPI_FUNCTION_TYPE: i32 = 7;
const NAPI_PROPERTY_STATIC: i32 = 1 << 10;
// Node uses Node-API v8 for symbol-registered addons that omit the optional
// node_api_module_get_api_version_v1 export.
const DEFAULT_NODE_API_MODULE_VERSION: i32 = 8;
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
const MAX_BIGINT_WORDS: usize = 2048;
const MAX_PENDING_FATAL_EXCEPTIONS: usize = 1024;
const MAX_NODE_API_VERSION: i32 = 10;
const UTF16_INPUT_ERROR_MESSAGE: &[u8] =
    b"UTF-16 input is malformed or exceeds napi-vm string limits\0";
static NEXT_OPAQUE_HANDLE_ID: AtomicUsize = AtomicUsize::new(1);

type NapiEnv = *mut c_void;
type NapiValue = *mut c_void;
type NapiCallbackInfo = *mut c_void;
type NapiHandleScope = *mut c_void;
type NapiAsyncContext = *mut c_void;
type NapiCallbackScope = *mut c_void;
type NapiRef = *mut c_void;
type NapiDeferred = *mut c_void;
type NapiAsyncWork = *mut c_void;
type NapiCallback = unsafe extern "C" fn(NapiEnv, NapiCallbackInfo) -> NapiValue;
type NapiFinalize = unsafe extern "C" fn(NapiEnv, *mut c_void, *mut c_void);
type NodeApiNoEnvFinalize = unsafe extern "C" fn(*mut c_void, *mut c_void);
type NapiCleanupHook = unsafe extern "C" fn(*mut c_void);
type NapiAsyncCleanupHookHandle = *mut c_void;
type NapiAsyncCleanupHook = unsafe extern "C" fn(NapiAsyncCleanupHookHandle, *mut c_void);
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
    pub(crate) policy: NativeAddonPolicy,
    native_prebuild_aliases: Vec<NativePrebuildAlias>,
    native_package_prebuilds: Vec<NativePackagePrebuild>,
    node_gyp_build_prebuilds_only: Option<bool>,
    node_gyp_build_exec_path: Option<PathBuf>,
    reported_node_version: ReportedNodeVersion,
    max_napi_version: u32,
}

#[derive(Clone, Debug)]
struct NativePrebuildAlias {
    request: String,
    package_root: PathBuf,
    expected_sha256: Option<[u8; 32]>,
}

#[derive(Clone, Debug)]
struct NativePackagePrebuild {
    package_root: PathBuf,
    expected_sha256: Option<[u8; 32]>,
}

/// Version numbers returned by `napi_get_node_version` in the Rust Node-API
/// host. The release name remains `napi-vm` so addons can distinguish this
/// runtime from Node.js.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReportedNodeVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl ReportedNodeVersion {
    /// The default profile clearly identifies the runtime as napi-vm.
    pub const NAPI_VM: Self = Self {
        major: 0,
        minor: 0,
        patch: 0,
    };

    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl RustNodeApiOptions {
    pub fn new<I, P>(roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        Self::with_policy(NativeAddonPolicy::new(roots))
    }

    /// Configure the in-process backend with a shared addon policy.
    pub fn with_policy(policy: NativeAddonPolicy) -> Self {
        Self {
            policy,
            native_prebuild_aliases: Vec::new(),
            native_package_prebuilds: Vec::new(),
            node_gyp_build_prebuilds_only: None,
            node_gyp_build_exec_path: None,
            reported_node_version: ReportedNodeVersion::NAPI_VM,
            max_napi_version: MAX_NODE_API_VERSION as u32,
        }
    }

    pub fn allow_native_addon(mut self, path: impl Into<PathBuf>) -> Self {
        self.policy = self.policy.allow_native_addon(path);
        self
    }

    pub fn allow_native_addon_with_sha256(
        mut self,
        path: impl Into<PathBuf>,
        expected_sha256: [u8; 32],
    ) -> Self {
        self.policy = self
            .policy
            .allow_native_addon_with_sha256(path, expected_sha256);
        self
    }

    /// Resolve a Node-API prebuild from the package's standard build/prebuilds
    /// directories and expose it through a bare guest `require()` request.
    /// The selected binary is pinned to its current SHA-256 at configuration
    /// time, just like [`Self::allow_native_addon`]. This alias replaces the
    /// package's JavaScript entry for that request; use it when the native
    /// addon exports are the package's public API.
    pub fn allow_native_prebuild(
        mut self,
        request: impl Into<String>,
        package_root: impl Into<PathBuf>,
    ) -> Self {
        self.native_prebuild_aliases.push(NativePrebuildAlias {
            request: request.into(),
            package_root: package_root.into(),
            expected_sha256: None,
        });
        self
    }

    /// Resolve a Node-API prebuild and require it to match a digest from
    /// trusted host metadata before making it available to guest `require()`.
    pub fn allow_native_prebuild_with_sha256(
        mut self,
        request: impl Into<String>,
        package_root: impl Into<PathBuf>,
        expected_sha256: [u8; 32],
    ) -> Self {
        self.native_prebuild_aliases.push(NativePrebuildAlias {
            request: request.into(),
            package_root: package_root.into(),
            expected_sha256: Some(expected_sha256),
        });
        self
    }

    /// Allow the selected Node-API prebuild to load through a package's
    /// JavaScript wrapper that calls `require('node-gyp-build')(__dirname)`.
    /// Unlike [`Self::allow_native_prebuild`], this preserves the JavaScript
    /// package entry point and pins the binary selected for this host.
    pub fn allow_native_package_prebuild(mut self, package_root: impl Into<PathBuf>) -> Self {
        self.native_package_prebuilds.push(NativePackagePrebuild {
            package_root: package_root.into(),
            expected_sha256: None,
        });
        self
    }

    /// Allow a package wrapper to load a selected prebuild only when it
    /// matches a digest from trusted host metadata.
    pub fn allow_native_package_prebuild_with_sha256(
        mut self,
        package_root: impl Into<PathBuf>,
        expected_sha256: [u8; 32],
    ) -> Self {
        self.native_package_prebuilds.push(NativePackagePrebuild {
            package_root: package_root.into(),
            expected_sha256: Some(expected_sha256),
        });
        self
    }

    /// Restrict package prebuild lookup to `prebuilds/<platform>-<arch>`,
    /// following the `PREBUILDS_ONLY` behavior from `node-gyp-build`. If this
    /// option is unset, the host process environment variable is used.
    pub fn node_gyp_build_prebuilds_only(mut self, enabled: bool) -> Self {
        self.node_gyp_build_prebuilds_only = Some(enabled);
        self
    }

    /// Set the executable path used by the nearby-prebuild fallback in
    /// `node-gyp-build`. The default is the Rust embedding process executable.
    pub fn node_gyp_build_exec_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.node_gyp_build_exec_path = Some(path.into());
        self
    }

    pub fn entry(mut self, path: impl Into<PathBuf>) -> Self {
        self.policy = self.policy.entry(path);
        self
    }

    /// Configure the numeric Node compatibility version reported to native
    /// addons. The release name returned by Node-API remains `napi-vm`.
    pub fn reported_node_version(mut self, version: ReportedNodeVersion) -> Self {
        self.reported_node_version = version;
        self
    }

    /// Set the highest Node-API version this host will report and accept from
    /// addon registration. The value must be in the supported range 1 through
    /// 10; addon functions outside the implemented compatibility surface can
    /// still fail when called.
    pub fn max_napi_version(mut self, version: u32) -> Self {
        self.max_napi_version = version;
        self
    }
}

/// In-process Node-API addon host. This is opt-in and loads Linux ELF, macOS
/// Mach-O, and Windows PE addons for the selected Node-API v1-v10 calls below.
/// Linux is runtime-tested; Windows GNU was cross-compiled and tested under
/// Wine. Native Windows and macOS still need runtime CI verification.
pub struct RustNodeApiHost {
    state: Rc<RefCell<HostState>>,
    _shim: Arc<NodeApiShim>,
    allowed_roots: Vec<PathBuf>,
    allowed_addons: HashMap<PathBuf, [u8; 32]>,
    shutdown_started: Cell<bool>,
}

impl Drop for RustNodeApiHost {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

impl RustNodeApiHost {
    fn shutdown_inner(&self) -> Result<(), VmErr> {
        if self.shutdown_started.replace(true) {
            return Ok(());
        }

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
                let Ok(callback) = create_native_async_complete_value(
                    environment,
                    work.complete,
                    status,
                    work.data,
                    work_id,
                ) else {
                    continue;
                };
                let Some(id) = callback.host_function_id() else {
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

        // Node runs cleanup hooks in reverse registration order before N-API
        // finalizers. Keep addon libraries loaded while synchronous hooks run
        // and asynchronous hooks complete.
        for environment in environments.iter().rev() {
            run_environment_cleanup_hooks(environment);
        }

        // Queue callbacks are reclaimed with a null env at shutdown. If a
        // native producer has not released its TSFN yet, keep the addon and
        // ABI shim mapped so its eventual closing/release calls stay valid.
        let active_threadsafe_workers = shutdown_threadsafe_functions(&self.state, &environments);

        // Stop accepting new finalizer work, then run everything that was
        // successfully posted before unloading an addon's library.
        close_post_finalizer_senders(&environments);
        let notifications = {
            let state = self.state.borrow();
            state.runtime_notifications.try_iter().collect::<Vec<_>>()
        };
        for notification in notifications {
            let HostRuntimeNotification::PostedFinalizer(finalizer) = notification else {
                continue;
            };
            let Some(environment) = environments
                .iter()
                .find(|environment| environment.raw() as usize == finalizer.environment)
            else {
                continue;
            };
            let Ok(callback) = create_posted_finalizer_value(
                environment,
                finalizer.finalize,
                finalizer.data as *mut c_void,
                finalizer.hint as *mut c_void,
            ) else {
                continue;
            };
            let Some(id) = callback.host_function_id() else {
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

        if active_threadsafe_workers {
            let libraries = std::mem::take(&mut self.state.borrow_mut().libraries);
            let mut pinned = PINNED_NODE_API_ADDON_LIBRARIES
                .get_or_init(|| Mutex::new(HashMap::new()))
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (filename, library) in libraries {
                pinned.entry(filename).or_insert(library);
            }
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

        if !active_threadsafe_workers {
            self.state.borrow_mut().libraries.clear();
        }
        Ok(())
    }
}

struct HostState {
    global: Env,
    object_prototype: Option<Value>,
    reported_node_version: ReportedNodeVersion,
    max_napi_version: u32,
    next_callback_id: usize,
    callbacks: HashMap<usize, NativeCallbackRecord>,
    environments: Vec<Rc<NapiEnvironment>>,
    // Type tags belong to the JavaScript object, not the addon environment:
    // multiple addons must be able to recognize a tag set by another addon.
    // Retain tagged values because this interpreter does not have a tracing GC
    // and pointer identities must not be recycled while a tag is observable.
    type_tags: HashMap<NapiObjectIdentity, (NapiTypeTag, Value)>,
    libraries: HashMap<PathBuf, Arc<Library>>,
    async_work_sender: SyncSender<AsyncWorkTaskMessage>,
    runtime_notifications: Receiver<HostRuntimeNotification>,
    runtime_notification_sender: Sender<HostRuntimeNotification>,
    async_workers: Vec<JoinHandle<()>>,
    // Keep the process-global ABI shim loaded until every addon library closes.
    _shim: Arc<NodeApiShim>,
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

struct NapiEnvironment {
    module_path: String,
    module_file_url: CString,
    api_version: u32,
    node_version: NapiNodeVersion,
    owner: Weak<RefCell<HostState>>,
    handles: RefCell<NapiHandleArena>,
    references: RefCell<HashMap<usize, NapiReference>>,
    deferreds: RefCell<HashMap<usize, NapiDeferredState>>,
    async_works: RefCell<HashMap<usize, NapiAsyncWorkState>>,
    threadsafe_functions: RefCell<HashMap<usize, NapiThreadsafeFunctionState>>,
    async_contexts: RefCell<HashMap<usize, NapiAsyncContextState>>,
    callback_scopes: RefCell<Vec<NapiCallbackScopeState>>,
    cleanup_hooks: RefCell<Vec<NapiCleanupHookRecord>>,
    async_cleanup_hooks: RefCell<Vec<NapiAsyncCleanupHookRecord>>,
    next_cleanup_hook_order: Cell<u64>,
    last_error: Cell<NapiExtendedErrorInfo>,
    wraps: RefCell<HashMap<NapiObjectIdentity, NapiWrap>>,
    added_finalizers: RefCell<Vec<NapiAddedFinalizer>>,
    instance_data: RefCell<Option<NapiInstanceData>>,
    externals: RefCell<HashMap<NapiObjectIdentity, NapiExternal>>,
    external_buffers: RefCell<HashMap<NapiObjectIdentity, NapiExternalBuffer>>,
    external_memory: Cell<i64>,
    finalizing: Cell<bool>,
    active_callbacks: RefCell<HashMap<usize, CallbackFrame>>,
    guest_callback_dispatchers: RefCell<Vec<GuestCallbackDispatcher>>,
    pending_exception: RefCell<Option<Value>>,
    fatal_exceptions: RefCell<VecDeque<Value>>,
}

#[repr(C)]
struct NapiNodeVersion {
    major: u32,
    minor: u32,
    patch: u32,
    release: *const c_char,
}

const NAPI_VM_RELEASE: &[u8] = b"napi-vm\0";

type NapiAddonRegister = unsafe extern "C" fn(NapiEnv, NapiValue) -> NapiValue;

#[repr(C)]
struct NapiModule {
    version: i32,
    flags: u32,
    filename: *const c_char,
    register: Option<NapiAddonRegister>,
    module_name: *const c_char,
    private_data: *mut c_void,
    reserved: [*mut c_void; 4],
}

#[derive(Clone, Copy)]
struct NapiCleanupHookRecord {
    function: NapiCleanupHook,
    function_address: usize,
    argument: usize,
    order: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NapiTypeTag {
    lower: u64,
    upper: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AsyncCleanupHookPhase {
    Registered,
    Running,
    Removed,
}

struct AsyncCleanupHookControl {
    phase: Mutex<AsyncCleanupHookPhase>,
    completed: Condvar,
}

#[derive(Clone)]
struct NapiAsyncCleanupHookRecord {
    function: NapiAsyncCleanupHook,
    function_address: usize,
    argument: usize,
    handle: usize,
    order: u64,
    control: Arc<AsyncCleanupHookControl>,
}

fn async_cleanup_hook_registry() -> &'static Mutex<HashMap<usize, Arc<AsyncCleanupHookControl>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<usize, Arc<AsyncCleanupHookControl>>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remove_async_cleanup_hook_handle(handle: NapiAsyncCleanupHookHandle) {
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

fn next_environment_cleanup_hook_order(environment: &NapiEnvironment) -> Result<u64, i32> {
    let order = environment.next_cleanup_hook_order.get();
    environment
        .next_cleanup_hook_order
        .set(order.checked_add(1).ok_or(NAPI_GENERIC_FAILURE)?);
    Ok(order)
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
    value: Option<Value>,
    ref_count: u32,
}

struct NapiDeferredState {
    promise: Rc<RefCell<PromiseInner>>,
    settling: bool,
}

struct NapiAsyncContextState {
    _resource: Value,
    _resource_name: Value,
}

#[derive(Clone, Copy)]
struct NapiCallbackScopeState {
    token: usize,
    async_context: usize,
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
    PostedFinalizer(PostedFinalizer),
}

#[derive(Clone, Copy)]
struct PostedFinalizer {
    environment: usize,
    finalize: NapiFinalize,
    data: usize,
    hint: usize,
}

static POST_FINALIZER_SENDERS: OnceLock<Mutex<HashMap<usize, Sender<HostRuntimeNotification>>>> =
    OnceLock::new();

fn post_finalizer_senders() -> &'static Mutex<HashMap<usize, Sender<HostRuntimeNotification>>> {
    POST_FINALIZER_SENDERS.get_or_init(|| Mutex::new(HashMap::new()))
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

struct NapiAddedFinalizer {
    // As with wraps and externals, the VM has no tracing GC, so preserve the
    // associated guest value until the owning environment shuts down.
    _value: Value,
    data: *mut c_void,
    finalize: NapiFinalize,
    hint: *mut c_void,
    reference: Option<usize>,
}

#[derive(Clone, Copy)]
struct NapiInstanceData {
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

struct NapiExternalBuffer {
    // Retain each external buffer value and its native memory until host
    // shutdown, when its Node-API finalizer runs on the owning thread.
    _value: Value,
    data: *mut c_void,
    finalize: NapiExternalBufferFinalizer,
    hint: *mut c_void,
}

#[derive(Clone, Copy)]
enum NapiExternalBufferFinalizer {
    Napi(Option<NapiFinalize>),
    NoEnv(Option<NodeApiNoEnvFinalize>),
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
    SharedArrayBuffer(usize),
    TypedArray(usize),
    DataView(usize),
    RegExp(usize),
    Error(usize),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum NapiReferenceIdentity {
    Object(NapiObjectIdentity),
    Symbol(u64),
}

thread_local! {
    /// Only environments created on this thread may enter the synchronous
    /// Node-API surface. Looking up the opaque pointer before dereferencing it
    /// makes invalid `napi_env` values fail with `napi_invalid_arg` instead of
    /// causing undefined behavior in the host.
    static NAPI_ENVIRONMENTS: RefCell<HashMap<usize, Weak<NapiEnvironment>>> =
        RefCell::new(HashMap::new());
    /// Deprecated `napi_module_register` modules register from a static
    /// constructor while `dlopen` is in progress. Keep registrations scoped
    /// to that load so nested addon loads cannot steal each other's entries.
    static NAPI_MODULE_REGISTRATIONS: RefCell<Vec<Vec<usize>>> = const { RefCell::new(Vec::new()) };
}

struct NapiModuleRegistrationScope {
    active: bool,
}

impl NapiModuleRegistrationScope {
    fn new() -> Self {
        NAPI_MODULE_REGISTRATIONS.with(|registrations| {
            registrations.borrow_mut().push(Vec::new());
        });
        Self { active: true }
    }

    fn finish(mut self) -> Vec<usize> {
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

mod api;
#[allow(unused_imports)]
use api::*;

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
    coerce_to_object: unsafe extern "C" fn(NapiEnv, NapiValue, *mut NapiValue) -> i32,
    create_external_arraybuffer: unsafe extern "C" fn(
        NapiEnv,
        *mut c_void,
        usize,
        Option<NapiFinalize>,
        *mut c_void,
        *mut NapiValue,
    ) -> i32,
    create_external_buffer: unsafe extern "C" fn(
        NapiEnv,
        usize,
        *mut c_void,
        Option<NapiFinalize>,
        *mut c_void,
        *mut NapiValue,
    ) -> i32,
    async_init: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, *mut NapiAsyncContext) -> i32,
    async_destroy: unsafe extern "C" fn(NapiEnv, NapiAsyncContext) -> i32,
    make_callback: unsafe extern "C" fn(
        NapiEnv,
        NapiAsyncContext,
        NapiValue,
        NapiValue,
        usize,
        *const NapiValue,
        *mut NapiValue,
    ) -> i32,
    open_callback_scope:
        unsafe extern "C" fn(NapiEnv, NapiValue, NapiAsyncContext, *mut NapiCallbackScope) -> i32,
    close_callback_scope: unsafe extern "C" fn(NapiEnv, NapiCallbackScope) -> i32,
    create_date: unsafe extern "C" fn(NapiEnv, f64, *mut NapiValue) -> i32,
    is_date: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    get_date_value: unsafe extern "C" fn(NapiEnv, NapiValue, *mut f64) -> i32,
    add_finalizer: unsafe extern "C" fn(
        NapiEnv,
        NapiValue,
        *mut c_void,
        Option<NapiFinalize>,
        *mut c_void,
        *mut NapiRef,
    ) -> i32,
    create_bigint_int64: unsafe extern "C" fn(NapiEnv, i64, *mut NapiValue) -> i32,
    create_bigint_uint64: unsafe extern "C" fn(NapiEnv, u64, *mut NapiValue) -> i32,
    create_bigint_words:
        unsafe extern "C" fn(NapiEnv, i32, usize, *const u64, *mut NapiValue) -> i32,
    get_value_bigint_int64: unsafe extern "C" fn(NapiEnv, NapiValue, *mut i64, *mut bool) -> i32,
    get_value_bigint_uint64: unsafe extern "C" fn(NapiEnv, NapiValue, *mut u64, *mut bool) -> i32,
    get_value_bigint_words:
        unsafe extern "C" fn(NapiEnv, NapiValue, *mut i32, *mut usize, *mut u64) -> i32,
    get_all_property_names:
        unsafe extern "C" fn(NapiEnv, NapiValue, i32, i32, i32, *mut NapiValue) -> i32,
    set_instance_data:
        unsafe extern "C" fn(NapiEnv, *mut c_void, Option<NapiFinalize>, *mut c_void) -> i32,
    get_instance_data: unsafe extern "C" fn(NapiEnv, *mut *mut c_void) -> i32,
    detach_arraybuffer: unsafe extern "C" fn(NapiEnv, NapiValue) -> i32,
    is_detached_arraybuffer: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    type_tag_object: unsafe extern "C" fn(NapiEnv, NapiValue, *const NapiTypeTag) -> i32,
    check_object_type_tag:
        unsafe extern "C" fn(NapiEnv, NapiValue, *const NapiTypeTag, *mut bool) -> i32,
    object_freeze: unsafe extern "C" fn(NapiEnv, NapiValue) -> i32,
    object_seal: unsafe extern "C" fn(NapiEnv, NapiValue) -> i32,
    add_async_cleanup_hook: unsafe extern "C" fn(
        NapiEnv,
        Option<NapiAsyncCleanupHook>,
        *mut c_void,
        *mut NapiAsyncCleanupHookHandle,
    ) -> i32,
    remove_async_cleanup_hook: unsafe extern "C" fn(NapiAsyncCleanupHookHandle),
    node_api_symbol_for: unsafe extern "C" fn(NapiEnv, *const c_char, usize, *mut NapiValue) -> i32,
    create_syntax_error: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue, *mut NapiValue) -> i32,
    throw_syntax_error: unsafe extern "C" fn(NapiEnv, *const c_char, *const c_char) -> i32,
    get_module_file_name: unsafe extern "C" fn(NapiEnv, *mut *const c_char) -> i32,
    create_external_string_latin1: unsafe extern "C" fn(
        NapiEnv,
        *mut c_char,
        usize,
        Option<NapiFinalize>,
        *mut c_void,
        *mut NapiValue,
        *mut bool,
    ) -> i32,
    create_external_string_utf16: unsafe extern "C" fn(
        NapiEnv,
        *mut u16,
        usize,
        Option<NapiFinalize>,
        *mut c_void,
        *mut NapiValue,
        *mut bool,
    ) -> i32,
    create_property_key_latin1:
        unsafe extern "C" fn(NapiEnv, *const c_char, usize, *mut NapiValue) -> i32,
    create_property_key_utf8:
        unsafe extern "C" fn(NapiEnv, *const c_char, usize, *mut NapiValue) -> i32,
    create_property_key_utf16:
        unsafe extern "C" fn(NapiEnv, *const u16, usize, *mut NapiValue) -> i32,
    create_buffer_from_arraybuffer:
        unsafe extern "C" fn(NapiEnv, NapiValue, usize, usize, *mut NapiValue) -> i32,
    get_node_version: unsafe extern "C" fn(NapiEnv, *mut *const NapiNodeVersion) -> i32,
    get_uv_event_loop: unsafe extern "C" fn(NapiEnv, *mut *mut c_void) -> i32,
    module_register: unsafe extern "C" fn(*mut c_void),
    fatal_error: unsafe extern "C" fn(*const c_char, usize, *const c_char, usize) -> !,
    fatal_exception: unsafe extern "C" fn(NapiEnv, NapiValue) -> i32,
    set_prototype: unsafe extern "C" fn(NapiEnv, NapiValue, NapiValue) -> i32,
    create_object_with_properties: unsafe extern "C" fn(
        NapiEnv,
        NapiValue,
        *const NapiValue,
        *const NapiValue,
        usize,
        *mut NapiValue,
    ) -> i32,
    post_finalizer:
        unsafe extern "C" fn(NapiEnv, Option<NapiFinalize>, *mut c_void, *mut c_void) -> i32,
    create_sharedarraybuffer:
        unsafe extern "C" fn(NapiEnv, usize, *mut *mut c_void, *mut NapiValue) -> i32,
    create_external_sharedarraybuffer: unsafe extern "C" fn(
        NapiEnv,
        *mut c_void,
        usize,
        Option<NodeApiNoEnvFinalize>,
        *mut c_void,
        *mut NapiValue,
    ) -> i32,
    is_sharedarraybuffer: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
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
    coerce_to_object: api_coerce_to_object,
    create_external_arraybuffer: api_create_external_arraybuffer,
    create_external_buffer: api_create_external_buffer,
    async_init: api_async_init,
    async_destroy: api_async_destroy,
    make_callback: api_make_callback,
    open_callback_scope: api_open_callback_scope,
    close_callback_scope: api_close_callback_scope,
    create_date: api_create_date,
    is_date: api_is_date,
    get_date_value: api_get_date_value,
    add_finalizer: api_add_finalizer,
    create_bigint_int64: api_create_bigint_int64,
    create_bigint_uint64: api_create_bigint_uint64,
    create_bigint_words: api_create_bigint_words,
    get_value_bigint_int64: api_get_value_bigint_int64,
    get_value_bigint_uint64: api_get_value_bigint_uint64,
    get_value_bigint_words: api_get_value_bigint_words,
    get_all_property_names: api_get_all_property_names,
    set_instance_data: api_set_instance_data,
    get_instance_data: api_get_instance_data,
    detach_arraybuffer: api_detach_arraybuffer,
    is_detached_arraybuffer: api_is_detached_arraybuffer,
    type_tag_object: api_type_tag_object,
    check_object_type_tag: api_check_object_type_tag,
    object_freeze: api_object_freeze,
    object_seal: api_object_seal,
    add_async_cleanup_hook: api_add_async_cleanup_hook,
    remove_async_cleanup_hook: api_remove_async_cleanup_hook,
    node_api_symbol_for: api_node_symbol_for,
    create_syntax_error: api_create_syntax_error,
    throw_syntax_error: api_throw_syntax_error,
    get_module_file_name: api_get_module_file_name,
    create_external_string_latin1: api_create_external_string_latin1,
    create_external_string_utf16: api_create_external_string_utf16,
    create_property_key_latin1: api_create_property_key_latin1,
    create_property_key_utf8: api_create_property_key_utf8,
    create_property_key_utf16: api_create_property_key_utf16,
    create_buffer_from_arraybuffer: api_create_buffer_from_arraybuffer,
    get_node_version: api_get_node_version,
    get_uv_event_loop: api_get_uv_event_loop,
    module_register: api_module_register,
    fatal_error: api_fatal_error,
    fatal_exception: api_fatal_exception,
    set_prototype: api_set_prototype,
    create_object_with_properties: api_create_object_with_properties,
    post_finalizer: api_post_finalizer,
    create_sharedarraybuffer: api_create_sharedarraybuffer,
    create_external_sharedarraybuffer: api_create_external_sharedarraybuffer,
    is_sharedarraybuffer: api_is_sharedarraybuffer,
};

fn napi_extended_error_info(status: i32) -> NapiExtendedErrorInfo {
    let message: &'static [u8] = match status {
        NAPI_OK => b"success\0",
        NAPI_INVALID_ARG => b"invalid argument\0",
        NAPI_OBJECT_EXPECTED => b"object expected\0",
        NAPI_STRING_EXPECTED => b"string expected\0",
        NAPI_DATE_EXPECTED => b"date expected\0",
        NAPI_BIGINT_EXPECTED => b"bigint expected\0",
        NAPI_FUNCTION_EXPECTED => b"function expected\0",
        NAPI_NUMBER_EXPECTED => b"number expected\0",
        NAPI_BOOLEAN_EXPECTED => b"boolean expected\0",
        NAPI_ARRAY_EXPECTED => b"array expected\0",
        NAPI_PENDING_EXCEPTION => b"pending exception\0",
        NAPI_CANCELLED => b"cancelled\0",
        NAPI_ESCAPE_CALLED_TWICE => b"escape called twice\0",
        NAPI_HANDLE_SCOPE_MISMATCH => b"handle scope mismatch\0",
        NAPI_CALLBACK_SCOPE_MISMATCH => b"callback scope mismatch\0",
        NAPI_QUEUE_FULL => b"thread-safe function queue is full\0",
        NAPI_CLOSING => b"thread-safe function is closing\0",
        NAPI_ARRAYBUFFER_EXPECTED => b"ArrayBuffer expected\0",
        NAPI_DETACHABLE_ARRAYBUFFER_EXPECTED => b"detachable ArrayBuffer expected\0",
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
            | Value::SharedArrayBuffer(_)
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

fn create_posted_finalizer_value(
    environment: &Rc<NapiEnvironment>,
    finalize: NapiFinalize,
    data: *mut c_void,
    hint: *mut c_void,
) -> Result<Value, i32> {
    create_native_callback_value_with_kind(
        environment,
        "node_api_post_finalizer",
        NativeCallback::PostedFinalizer {
            finalize,
            data,
            hint,
        },
        std::ptr::null_mut(),
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
    Ok(Value::napi_callback_function(Rc::from(function_name), id))
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
        Value::Function(function) => {
            function.ensure_name_length_properties();
            function.prototype_value(object);
            object
                .set_prop(key.clone(), value)
                .map_err(|_| NAPI_GENERIC_FAILURE)?;
            if let Some(symbol) = symbol {
                function
                    .properties
                    .meta
                    .borrow_mut()
                    .set_symbol_key(&key, symbol);
            }
            Ok(())
        }
        Value::HostFunction { properties, .. } => {
            object
                .set_prop(key.clone(), value)
                .map_err(|_| NAPI_GENERIC_FAILURE)?;
            if let Some(symbol) = symbol {
                properties.meta.borrow_mut().set_symbol_key(&key, symbol);
            }
            Ok(())
        }
        Value::Array(array) => {
            napi_array_set_property(array, &key, value)?;
            if let Some(symbol) = symbol {
                array.set_symbol_key(&key, symbol);
            }
            Ok(())
        }
        _ => Err(NAPI_OBJECT_EXPECTED),
    }
}

fn napi_array_set_property(
    array: &crate::value::ArrayCell,
    key: &str,
    value: Value,
) -> Result<(), i32> {
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
        if array.meta.borrow().attrs_of("length").writable {
            array.set_length(length as usize);
        }
        return Ok(());
    }
    if let Some(index) = crate::value::array_index(key) {
        if index >= crate::value::MAX_ARRAY_LEN {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let old_length = array.borrow().len();
        let exists = index < old_length && array.has_index(index);
        if (exists && !array.meta.borrow().attrs_of(key).writable)
            || (!exists && array.meta.borrow().non_extensible)
            || (index >= old_length && !array.meta.borrow().attrs_of("length").writable)
        {
            return Ok(());
        }
        if index >= old_length {
            let new_length = index + 1;
            array.borrow_mut().resize(new_length, Value::Undefined);
            array.resize_presence(old_length, new_length, false);
        }
        array.borrow_mut()[index] = value;
        array.set_index_presence(index, true);
        return Ok(());
    }
    let exists = array.named_prop(key).is_some();
    if (exists && !array.meta.borrow().attrs_of(key).writable)
        || (!exists && array.meta.borrow().non_extensible)
    {
        return Ok(());
    }
    array.set_named(key.to_owned(), value);
    Ok(())
}

fn napi_direct_has_own_property(object: &Value, key: &Value) -> Result<bool, i32> {
    let key = napi_property_key(key)?;
    if let Value::Function(function) = object {
        function.ensure_name_length_properties();
        function.prototype_value(object);
    }
    Ok(match object {
        Value::Object { props } => props.borrow().iter().any(|(name, _)| name == &key),
        Value::Function(function) => function
            .properties
            .borrow()
            .iter()
            .any(|(name, _)| name == &key),
        Value::HostFunction { properties, .. } => {
            properties.borrow().iter().any(|(name, _)| name == &key)
        }
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
    if let Value::Function(function) = object {
        function.ensure_name_length_properties();
        function.prototype_value(object);
    }
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
        Value::Function(function) => {
            if !function
                .properties
                .meta
                .borrow()
                .attrs_of(&key)
                .configurable
                && function
                    .properties
                    .borrow()
                    .iter()
                    .any(|(name, _)| name == &key)
            {
                return Ok(false);
            }
            let companion = format!("__setter:{}__", key);
            function
                .properties
                .borrow_mut()
                .retain(|(name, _)| name != &key && name != &companion);
            function.properties.meta.borrow_mut().forget(&key);
            function.properties.meta.borrow_mut().forget(&companion);
            Ok(true)
        }
        Value::HostFunction { properties, .. } => {
            if !properties.meta.borrow().attrs_of(&key).configurable
                && properties.borrow().iter().any(|(name, _)| name == &key)
            {
                return Ok(false);
            }
            let companion = format!("__setter:{}__", key);
            properties
                .borrow_mut()
                .retain(|(name, _)| name != &key && name != &companion);
            properties.meta.borrow_mut().forget(&key);
            properties.meta.borrow_mut().forget(&companion);
            Ok(true)
        }
        Value::Array(array) => {
            if key == "length" {
                return Ok(false);
            }
            if let Some(index) = crate::value::array_index(&key) {
                if index < array.borrow().len() && array.has_index(index) {
                    if !array.meta.borrow().attrs_of(&key).configurable {
                        return Ok(false);
                    }
                    array.borrow_mut()[index] = Value::Undefined;
                    array.set_index_presence(index, false);
                }
            } else {
                if array.named_prop(&key).is_some()
                    && !array.meta.borrow().attrs_of(&key).configurable
                {
                    return Ok(false);
                }
                array.named.borrow_mut().retain(|(name, _)| name != &key);
                array.forget_symbol_key(&key);
            }
            Ok(true)
        }
        Value::Proxy(proxy) => napi_direct_delete_property(&proxy.target, &Value::String(key)),
        _ => Ok(true),
    }
}

fn napi_direct_own_property_names(object: &Value) -> Vec<String> {
    if let Value::Function(function) = object {
        function.ensure_name_length_properties();
        function.prototype_value(object);
    }
    match object {
        Value::Object { props } => props.borrow().iter().map(|(key, _)| key.clone()).collect(),
        Value::Function(function) => function
            .properties
            .borrow()
            .iter()
            .map(|(key, _)| key.clone())
            .collect(),
        Value::HostFunction { properties, .. } => properties
            .borrow()
            .iter()
            .map(|(key, _)| key.clone())
            .collect(),
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
        Value::Function(function) => {
            function
                .properties
                .borrow()
                .iter()
                .any(|(name, _)| name == key)
                && function.properties.meta.borrow().attrs_of(key).enumerable
        }
        Value::HostFunction { properties, .. } => {
            properties.borrow().iter().any(|(name, _)| name == key)
                && properties.meta.borrow().attrs_of(key).enumerable
        }
        Value::Array(array) => {
            key != "length"
                && (crate::value::array_index(key).is_some_and(|index| array.has_index(index))
                    || array.named_prop(key).is_some()
                        && array.meta.borrow().attrs_of(key).enumerable)
        }
        Value::Proxy(proxy) => napi_direct_property_is_enumerable(&proxy.target, key),
        Value::GlobalObject => true,
        Value::Error(error) => key == "code" && error.code.is_some(),
        _ => false,
    }
}

#[derive(Clone)]
enum NapiPropertyKey {
    String(String),
    Symbol(Rc<crate::value::SymbolData>),
}

impl NapiPropertyKey {
    fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::String(left), Self::String(right)) => left == right,
            (Self::Symbol(left), Self::Symbol(right)) => left.id == right.id,
            _ => false,
        }
    }
}

fn napi_property_key_from_guest(value: &Value) -> Result<NapiPropertyKey, VmErr> {
    match value {
        Value::String(key) => Ok(NapiPropertyKey::String(key.clone())),
        Value::Symbol(symbol) => Ok(NapiPropertyKey::Symbol(symbol.clone())),
        _ => Err(VmErr::Msg(
            "TypeError: Proxy ownKeys trap returned a non-key".into(),
        )),
    }
}

fn napi_guest_own_property_keys(
    interpreter: &mut Interpreter,
    object: &Value,
    depth: usize,
) -> Result<Vec<(NapiPropertyKey, PropAttrs)>, VmErr> {
    if depth >= crate::value::MAX_PROTOTYPE_DEPTH {
        return Err(crate::value::limit_err("Maximum prototype depth exceeded"));
    }
    if matches!(object, Value::GlobalObject) {
        let mut keys = interpreter
            .global_keys()
            .into_iter()
            .filter(|key| !crate::interpreter::is_internal_key(key))
            .map(|key| (NapiPropertyKey::String(key), PropAttrs::default()))
            .collect::<Vec<_>>();
        napi_sort_property_keys(&mut keys);
        return Ok(keys);
    }
    let Value::Proxy(proxy) = object else {
        return napi_direct_all_property_keys(object).map_err(|_| {
            VmErr::Msg("Node-API property key collection is unsupported for this value".into())
        });
    };

    let target = proxy.target.clone();
    let Some(trap) = interpreter.proxy_trap(proxy, "ownKeys") else {
        return napi_guest_own_property_keys(interpreter, &target, depth + 1);
    };
    let result = interpreter.call_this(&trap, proxy.handler.clone(), vec![target.clone()])?;
    let Value::Array(trap_keys) = &result else {
        return Err(VmErr::Msg(
            "TypeError: Proxy ownKeys trap must return an array".into(),
        ));
    };

    let length = trap_keys.borrow().len();
    if length > crate::value::MAX_ARRAY_LEN {
        return Err(crate::value::limit_err(
            "Maximum proxy property key count exceeded",
        ));
    }
    let mut keys = Vec::with_capacity(length);
    for index in 0..length {
        let value = interpreter.get_prop_value(&result, &Value::Number(index as f64))?;
        let key = napi_property_key_from_guest(&value)?;
        if keys
            .iter()
            .any(|(existing, _): &(NapiPropertyKey, PropAttrs)| existing.matches(&key))
        {
            return Err(VmErr::Msg(
                "TypeError: Proxy ownKeys trap returned duplicate keys".into(),
            ));
        }
        keys.push((key, PropAttrs::default()));
    }

    let target_keys = napi_guest_own_property_keys(interpreter, &target, depth + 1)?;
    for (key, attributes) in &mut keys {
        let enumerable = target_keys
            .iter()
            .find(|(target_key, _)| target_key.matches(key))
            .is_some_and(|(_, target_attributes)| target_attributes.enumerable);
        // Node's napi_get_all_property_names preserves Proxy ownKeys results
        // for writable/configurable filters, while enumerable still consults
        // the target descriptor. Bun applies all three filters to descriptors.
        // The Rust host follows Node's behavior; the differential fixture
        // records Bun's distinct result.
        *attributes = PropAttrs {
            writable: true,
            enumerable,
            configurable: true,
        };
    }

    if target_keys.iter().any(|(key, attrs)| {
        !attrs.configurable && !keys.iter().any(|(found, _)| found.matches(key))
    }) {
        return Err(VmErr::Msg(
            "TypeError: Proxy ownKeys trap omitted a non-configurable key".into(),
        ));
    }
    if !napi_guest_object_is_extensible(&target)
        && (keys.len() != target_keys.len()
            || target_keys
                .iter()
                .any(|(key, _)| !keys.iter().any(|(found, _)| found.matches(key))))
    {
        return Err(VmErr::Msg(
            "TypeError: Proxy ownKeys trap returned keys for a non-extensible target".into(),
        ));
    }
    Ok(keys)
}

fn napi_guest_object_is_extensible(object: &Value) -> bool {
    match object {
        Value::Object { props } => !props.meta.borrow().non_extensible,
        Value::Array(array) => !array.meta.borrow().non_extensible,
        Value::Function(function) => !function.properties.meta.borrow().non_extensible,
        Value::HostFunction { properties, .. } => !properties.meta.borrow().non_extensible,
        Value::Class(class) => !class.statics.meta.borrow().non_extensible,
        Value::Proxy(proxy) => napi_guest_object_is_extensible(&proxy.target),
        _ => true,
    }
}

fn napi_guest_property_key_value(key: NapiPropertyKey, key_conversion: i32) -> Value {
    match key {
        NapiPropertyKey::String(key) if key_conversion == 0 => crate::value::array_index(&key)
            .map_or_else(|| Value::String(key), |index| Value::Number(index as f64)),
        NapiPropertyKey::String(key) => Value::String(key),
        NapiPropertyKey::Symbol(symbol) => Value::Symbol(symbol),
    }
}

fn napi_guest_get_all_property_names(
    interpreter: &mut Interpreter,
    receiver: Value,
    args: Vec<Value>,
) -> Result<Value, VmErr> {
    let number_arg = |index: usize| match args.get(index) {
        Some(Value::Number(value)) => *value as i32,
        _ => 0,
    };
    let key_mode = number_arg(0);
    let key_filter = number_arg(1);
    let key_conversion = number_arg(2);

    let mut current = receiver;
    let mut seen = Vec::<NapiPropertyKey>::new();
    let mut names = Vec::new();
    for _ in 0..crate::value::MAX_PROTOTYPE_DEPTH {
        for (key, attributes) in napi_guest_own_property_keys(interpreter, &current, 0)? {
            if seen.iter().any(|existing| existing.matches(&key)) {
                continue;
            }
            seen.push(key.clone());
            if seen.len() > crate::value::MAX_ARRAY_LEN {
                return Err(crate::value::limit_err(
                    "Maximum property name count exceeded",
                ));
            }
            let filtered = (key_filter & 1 != 0 && !attributes.writable)
                || (key_filter & 2 != 0 && !attributes.enumerable)
                || (key_filter & 4 != 0 && !attributes.configurable)
                || (key_filter & 8 != 0 && matches!(key, NapiPropertyKey::String(_)))
                || (key_filter & 16 != 0 && matches!(key, NapiPropertyKey::Symbol(_)));
            if !filtered {
                names.push(napi_guest_property_key_value(key, key_conversion));
            }
        }
        if key_mode == 1 {
            return Value::checked_array(names);
        }
        let prototype = match &current {
            Value::Proxy(proxy) => interpreter.prototype_of(&proxy.target),
            _ => interpreter.prototype_of(&current),
        };
        let Some(prototype) = prototype else {
            return Value::checked_array(names);
        };
        current = (*prototype).clone();
    }
    Err(crate::value::limit_err("Maximum prototype depth exceeded"))
}

fn napi_push_direct_property_key(
    keys: &mut Vec<(NapiPropertyKey, PropAttrs)>,
    key: &str,
    symbol: Option<Rc<crate::value::SymbolData>>,
    attributes: PropAttrs,
) {
    if crate::interpreter::is_internal_key(key) && symbol.is_none() {
        return;
    }
    let key = symbol.map_or_else(
        || NapiPropertyKey::String(key.to_owned()),
        NapiPropertyKey::Symbol,
    );
    if !keys.iter().any(|(existing, _)| existing.matches(&key)) {
        keys.push((key, attributes));
    }
}

fn napi_sort_property_keys(keys: &mut [(NapiPropertyKey, PropAttrs)]) {
    keys.sort_by(|(left, _), (right, _)| match (left, right) {
        (NapiPropertyKey::String(left), NapiPropertyKey::String(right)) => {
            match (
                crate::value::array_index(left),
                crate::value::array_index(right),
            ) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        }
        (NapiPropertyKey::String(_), NapiPropertyKey::Symbol(_)) => std::cmp::Ordering::Less,
        (NapiPropertyKey::Symbol(_), NapiPropertyKey::String(_)) => std::cmp::Ordering::Greater,
        (NapiPropertyKey::Symbol(_), NapiPropertyKey::Symbol(_)) => std::cmp::Ordering::Equal,
    });
}

fn napi_direct_all_property_keys(object: &Value) -> Result<Vec<(NapiPropertyKey, PropAttrs)>, i32> {
    let mut keys = Vec::new();
    match object {
        Value::Object { props } => {
            let slots = props.borrow();
            let metadata = props.meta.borrow();
            for (key, _) in slots.iter() {
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    metadata.symbol_key(key),
                    metadata.attrs_of(key),
                );
            }
        }
        Value::Function(function) => {
            function.ensure_name_length_properties();
            function.prototype_value(object);
            let slots = function.properties.borrow();
            let metadata = function.properties.meta.borrow();
            for (key, _) in slots.iter() {
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    metadata.symbol_key(key),
                    metadata.attrs_of(key),
                );
            }
        }
        Value::HostFunction { properties, .. } => {
            let slots = properties.borrow();
            let metadata = properties.meta.borrow();
            for (key, _) in slots.iter() {
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    metadata.symbol_key(key),
                    metadata.attrs_of(key),
                );
            }
        }
        Value::Class(class) => {
            let slots = class.statics.borrow();
            let metadata = class.statics.meta.borrow();
            // Node's native class constructor creates these own properties
            // in the order length, name, prototype before addon statics.
            for key in ["length", "name", "prototype"] {
                if slots.iter().any(|(name, _)| name == key) {
                    napi_push_direct_property_key(
                        &mut keys,
                        key,
                        metadata.symbol_key(key),
                        metadata.attrs_of(key),
                    );
                }
            }
            for (key, _) in slots.iter() {
                if matches!(key.as_str(), "length" | "name" | "prototype") {
                    continue;
                }
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    metadata.symbol_key(key),
                    metadata.attrs_of(key),
                );
            }
        }
        Value::Array(array) => {
            let length = array.borrow().len();
            for index in 0..length {
                if array.has_index(index) {
                    let index_key = index.to_string();
                    napi_push_direct_property_key(
                        &mut keys,
                        &index_key,
                        None,
                        array.meta.borrow().attrs_of(&index_key),
                    );
                }
            }
            napi_push_direct_property_key(
                &mut keys,
                "length",
                None,
                array.meta.borrow().attrs_of("length"),
            );
            for (key, _) in array.named.borrow().iter() {
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    array.symbol_key(key),
                    array.meta.borrow().attrs_of(key),
                );
            }
        }
        Value::Error(error) => {
            for key in ["name", "message", "stack"] {
                napi_push_direct_property_key(
                    &mut keys,
                    key,
                    None,
                    PropAttrs {
                        writable: true,
                        enumerable: false,
                        configurable: true,
                    },
                );
            }
            if error.code.is_some() {
                napi_push_direct_property_key(&mut keys, "code", None, PropAttrs::default());
            }
        }
        Value::RegExp(_) => napi_push_direct_property_key(
            &mut keys,
            "lastIndex",
            None,
            PropAttrs {
                writable: true,
                enumerable: false,
                configurable: false,
            },
        ),
        Value::TypedArray(view) => {
            for index in 0..view.effective_length() {
                napi_push_direct_property_key(
                    &mut keys,
                    &index.to_string(),
                    None,
                    PropAttrs::default(),
                );
            }
        }
        Value::Date(_)
        | Value::Promise(_)
        | Value::ArrayBuffer(_)
        | Value::SharedArrayBuffer(_)
        | Value::DataView(_)
        | Value::StringIterator { .. }
        | Value::Generator { .. } => {}
        Value::Proxy(_) | Value::NativeFunction { .. } | Value::GlobalObject => {
            return Err(NAPI_GENERIC_FAILURE);
        }
        Value::Undefined
        | Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::HostPending { .. }
        | Value::Symbol(_)
        | Value::BigInt(_)
        | Value::Binding(_) => return Err(NAPI_OBJECT_EXPECTED),
        #[cfg(stackful_coroutines)]
        Value::AsyncTask(_) => return Err(NAPI_OBJECT_EXPECTED),
    }
    napi_sort_property_keys(&mut keys);
    Ok(keys)
}

fn napi_direct_prototype(
    environment: &NapiEnvironment,
    object: &Value,
) -> Result<Option<Rc<Value>>, i32> {
    let direct = match object {
        Value::Proxy(proxy) => proxy.target.proto_of(),
        _ => object.proto_of(),
    };
    if direct.is_some() || matches!(object, Value::Proxy(_)) {
        return Ok(direct);
    }
    if !matches!(
        object,
        Value::Object { .. }
            | Value::Array(_)
            | Value::Function(_)
            | Value::Class(_)
            | Value::GlobalObject
            | Value::NativeFunction { .. }
            | Value::HostFunction { .. }
            | Value::Promise(_)
            | Value::Date(_)
            | Value::ArrayBuffer(_)
            | Value::SharedArrayBuffer(_)
            | Value::TypedArray(_)
            | Value::DataView(_)
    ) {
        return Ok(None);
    }
    let prototype = napi_effective_prototype(environment, object)?;
    Ok((!matches!(prototype, Value::Null)).then(|| Rc::new(prototype)))
}

fn napi_direct_property_names(environment: &NapiEnvironment, object: &Value) -> Result<Value, i32> {
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
        let Some(prototype) = napi_direct_prototype(environment, &current)? else {
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
            Value::Proxy(proxy) => interpreter.prototype_of(&proxy.target),
            _ => interpreter.prototype_of(&current),
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

fn napi_is_external_value(environment: &NapiEnvironment, value: &Value) -> bool {
    napi_object_identity(value)
        .ok()
        .is_some_and(|identity| environment.externals.borrow().contains_key(&identity))
}

fn napi_reference_identity(
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

fn napi_reference_value_strong_count(value: &Value) -> Option<usize> {
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

fn napi_collect_weak_reference(
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

fn napi_collect_weak_references(environment: &NapiEnvironment) {
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

fn finalize_environment_wraps(environment: &Rc<NapiEnvironment>) {
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

enum NapiEnvironmentCleanupHook {
    Sync(NapiCleanupHookRecord),
    Async(NapiAsyncCleanupHookRecord),
}

fn take_next_environment_cleanup_hook(
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

fn run_environment_cleanup_hooks(environment: &Rc<NapiEnvironment>) {
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

fn register_environment(environment: &Rc<NapiEnvironment>) {
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

fn close_post_finalizer_senders(environments: &[Rc<NapiEnvironment>]) {
    if let Ok(mut senders) = post_finalizer_senders().lock() {
        for environment in environments {
            senders.remove(&(environment.raw() as usize));
        }
    }
}

impl Drop for NapiEnvironment {
    fn drop(&mut self) {
        for hook in self.async_cleanup_hooks.get_mut().drain(..) {
            remove_async_cleanup_hook_handle(hook.handle as NapiAsyncCleanupHookHandle);
        }
        let _ = NAPI_ENVIRONMENTS.try_with(|environments| {
            environments
                .borrow_mut()
                .remove(&(self as *const Self as usize));
        });
        if let Ok(mut senders) = post_finalizer_senders().lock() {
            senders.remove(&(self as *const Self as usize));
        }
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

unsafe fn read_latin1_allow_empty(pointer: *const c_char, length: usize) -> Result<String, i32> {
    if pointer.is_null() && length == 0 {
        return Ok(String::new());
    }
    unsafe { read_latin1(pointer, length) }
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

unsafe fn read_utf16_allow_empty(pointer: *const u16, length: usize) -> Result<String, i32> {
    if pointer.is_null() && length == 0 {
        return Ok(String::new());
    }
    unsafe { read_utf16(pointer, length) }
}

struct NodeApiShim {
    _library: Option<Library>,
    path: PathBuf,
    #[cfg(target_os = "windows")]
    dll_directory_cookie: usize,
}

impl NodeApiShim {
    fn load() -> Result<Arc<Self>, VmErr> {
        let process_shim = PROCESS_NODE_API_SHIM.get_or_init(|| Mutex::new(None));
        let mut process_shim = process_shim
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(shim) = process_shim.as_ref() {
            return Ok(shim.clone());
        }
        let shim = Arc::new(Self::load_uncached()?);
        *process_shim = Some(shim.clone());
        Ok(shim)
    }

    fn load_uncached() -> Result<Self, VmErr> {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let bytes = include_bytes!(env!("NAPI_VM_NODE_API_SHIM_PATH"));
        let root = loop {
            let nonce = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let candidate = std::env::temp_dir()
                .join(format!("napi-vm-node-api-{}-{nonce}", std::process::id()));
            #[cfg(unix)]
            let mut builder = fs::DirBuilder::new();
            #[cfg(not(unix))]
            let builder = fs::DirBuilder::new();
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
        #[cfg(target_os = "windows")]
        let path = root.join("node.exe");
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

        #[cfg(target_os = "windows")]
        let dll_directory_cookie = {
            use std::os::windows::ffi::OsStrExt;
            let mut directory: Vec<u16> = root.as_os_str().encode_wide().collect();
            directory.push(0);
            let cookie = unsafe { AddDllDirectory(directory.as_ptr()) };
            if cookie.is_null() {
                let error = std::io::Error::last_os_error();
                let _ = fs::remove_dir_all(&root);
                return Err(VmErr::Msg(format!(
                    "cannot register private Node-API shim directory: {error}"
                )));
            }
            cookie as usize
        };

        #[cfg(unix)]
        let library_result =
            unsafe { Library::open(Some(path.as_os_str()), RTLD_NOW | RTLD_GLOBAL) };
        #[cfg(target_os = "windows")]
        let library_result = unsafe { Library::new(path.as_os_str()) };
        let library = library_result.map_err(|error| {
            #[cfg(target_os = "windows")]
            unsafe {
                let _ = RemoveDllDirectory(dll_directory_cookie as *mut c_void);
            }
            {
                let _ = fs::remove_dir_all(&root);
                VmErr::Msg(format!("cannot load Node-API symbol shim: {error}"))
            }
        })?;
        let install: unsafe extern "C" fn(*const NapiVmApiTable) =
            match unsafe { library.get(b"napi_vm_install_node_api_table\0") } {
                Ok(symbol) => *symbol,
                Err(error) => {
                    drop(library);
                    #[cfg(target_os = "windows")]
                    unsafe {
                        let _ = RemoveDllDirectory(dll_directory_cookie as *mut c_void);
                    }
                    let _ = fs::remove_dir_all(&root);
                    return Err(VmErr::Msg(format!("invalid Node-API symbol shim: {error}")));
                }
            };
        unsafe { install(&NAPI_VM_API_TABLE) };
        #[cfg(unix)]
        {
            // The process singleton keeps the mapping live. Unix permits
            // unlinking a mapped shared object, so do not leave a temp copy
            // behind for each application run.
            let _ = fs::remove_dir_all(&root);
        }
        #[cfg(target_os = "windows")]
        WINDOWS_NODE_API_SHIM_DIRECTORIES
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(root.clone());
        Ok(Self {
            _library: Some(library),
            path,
            #[cfg(target_os = "windows")]
            dll_directory_cookie,
        })
    }

    fn load_addon(&self, filename: &Path) -> Result<Arc<Library>, libloading::Error> {
        let key = filename.to_path_buf();
        if let Some(library) = PINNED_NODE_API_ADDON_LIBRARIES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .cloned()
        {
            return Ok(library);
        }
        if let Some(library) = PROCESS_NODE_API_ADDON_LIBRARIES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
            .and_then(SyncWeak::upgrade)
        {
            return Ok(library);
        }

        let library = Arc::new(self.open_addon(filename)?);
        let pinned = PINNED_NODE_API_ADDON_LIBRARIES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(library) = pinned.get(&key) {
            return Ok(library.clone());
        }
        let mut cache = PROCESS_NODE_API_ADDON_LIBRARIES
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.retain(|_, library| library.strong_count() != 0);
        if let Some(library) = cache.get(&key).and_then(SyncWeak::upgrade) {
            return Ok(library);
        }
        cache.insert(key, Arc::downgrade(&library));
        Ok(library)
    }

    fn open_addon(&self, filename: &Path) -> Result<Library, libloading::Error> {
        #[cfg(unix)]
        {
            unsafe { Library::open(Some(filename.as_os_str()), RTLD_NOW) }
        }
        #[cfg(target_os = "windows")]
        {
            unsafe {
                Library::load_with_flags(
                    filename.as_os_str(),
                    LOAD_LIBRARY_SEARCH_DEFAULT_DIRS
                        | LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
                        | LOAD_LIBRARY_SEARCH_USER_DIRS,
                )
            }
        }
    }
}

impl Drop for NodeApiShim {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::ffi::OsStrExt;

            // Addon library handles are stored before the shim in HostState,
            // so they have closed before the Node-API import provider unloads.
            drop(self._library.take());
            unsafe {
                let _ = RemoveDllDirectory(self.dll_directory_cookie as *mut c_void);
            }
            let mut module_name: Vec<u16> = OsStr::new("node.exe").encode_wide().collect();
            module_name.push(0);
            if unsafe { GetModuleHandleW(module_name.as_ptr()) }.is_null()
                && let Some(directories) = WINDOWS_NODE_API_SHIM_DIRECTORIES.get()
            {
                let mut directories = directories
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut roots: HashSet<PathBuf> = directories.drain(..).collect();
                if let Some(root) = self.path.parent() {
                    roots.insert(root.to_path_buf());
                }
                for root in roots {
                    let _ = fs::remove_dir_all(root);
                }
            }
        }
        #[cfg(unix)]
        {
            // Unix permits unlinking a loaded shared object; the mapping
            // remains live until the Library is dropped immediately after this method.
            if let Some(root) = self.path.parent() {
                let _ = fs::remove_dir_all(root);
            }
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
    fn new(
        global: Env,
        reported_node_version: ReportedNodeVersion,
        max_napi_version: u32,
        allowed_roots: Vec<PathBuf>,
        allowed_addons: HashMap<PathBuf, [u8; 32]>,
    ) -> Result<Self, VmErr> {
        let object_prototype = global
            .borrow()
            .get("Object")
            .and_then(|object| object.get_prop("prototype"));
        let shim = NodeApiShim::load()?;
        let (runtime_notification_sender, runtime_notifications) = mpsc::channel();
        let (async_work_sender, async_workers) =
            create_async_work_pool(runtime_notification_sender.clone())?;
        Ok(Self {
            state: Rc::new(RefCell::new(HostState {
                global,
                object_prototype,
                reported_node_version,
                max_napi_version,
                next_callback_id: 1,
                callbacks: HashMap::new(),
                environments: Vec::new(),
                type_tags: HashMap::new(),
                libraries: HashMap::new(),
                async_work_sender,
                runtime_notifications,
                runtime_notification_sender,
                async_workers,
                _shim: shim.clone(),
            })),
            _shim: shim,
            allowed_roots,
            allowed_addons,
            shutdown_started: Cell::new(false),
        })
    }

    /// Stop native workers and run addon cleanup hooks and finalizers.
    /// Repeated calls are safe.
    pub fn shutdown(&self) -> Result<(), VmErr> {
        self.shutdown_inner()
    }

    /// Whether this host has completed shutdown.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown_started.get()
    }

    fn ensure_running(&self) -> Result<(), VmErr> {
        if self.is_shutdown() {
            Err(VmErr::Msg("Rust Node-API host has been shut down".into()))
        } else {
            Ok(())
        }
    }

    fn preflight_addon_path(&self, filename: &Path) -> Result<PathBuf, VmErr> {
        self.ensure_running()?;
        let filename = fs::canonicalize(filename).map_err(|error| {
            VmErr::Msg(format!(
                "cannot resolve native addon {}: {error}",
                filename.display()
            ))
        })?;
        if !self
            .allowed_roots
            .iter()
            .any(|root| filename.starts_with(root))
        {
            return Err(VmErr::Msg(format!(
                "native addon escapes configured roots: {}",
                filename.display()
            )));
        }
        if filename
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("node")
        {
            return Err(VmErr::Msg(format!(
                "native addon path must use the .node extension: {}",
                filename.display()
            )));
        }
        let expected_digest = self.allowed_addons.get(&filename).ok_or_else(|| {
            VmErr::Msg(format!(
                "native addon is not allowlisted: {}",
                filename.display()
            ))
        })?;
        let actual_digest =
            crate::interpreter::commonjs::sha256_file(&filename).map_err(|error| {
                VmErr::Msg(format!(
                    "cannot verify native addon {}: {error}",
                    filename.display()
                ))
            })?;
        if &actual_digest != expected_digest {
            return Err(VmErr::Msg(format!(
                "native addon integrity check failed before loading: {}",
                filename.display()
            )));
        }
        validate_native_addon_binary(&filename)?;
        Ok(filename)
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
                NativeCallback::PostedFinalizer {
                    finalize,
                    data,
                    hint,
                } => {
                    unsafe { finalize(callback.env.raw(), data, hint) };
                    (std::ptr::null_mut(), None)
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
        napi_collect_weak_references(&callback.env);
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
    fn preflight_addon(&self, filename: &Path) -> Result<(), VmErr> {
        self.preflight_addon_path(filename).map(|_| ())
    }

    fn load(&self, filename: &Path) -> Result<Value, VmErr> {
        self.load_with_exports(filename, Value::object(Vec::new()))
    }

    fn load_with_exports(&self, filename: &Path, exports: Value) -> Result<Value, VmErr> {
        self.load_with_callback_handler(filename, exports, &mut reject_guest_callback)
    }

    fn load_with_callback_handler(
        &self,
        filename: &Path,
        exports: Value,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        let filename = self.preflight_addon_path(filename)?;
        let filename = filename
            .to_str()
            .ok_or_else(|| VmErr::Msg("native addon path is not UTF-8".into()))?;
        let registration_scope = NapiModuleRegistrationScope::new();
        let filename_path = Path::new(filename);
        let library_result = self._shim.load_addon(filename_path);
        let registered_modules = registration_scope.finish();
        let max_napi_version = self.state.borrow().max_napi_version as i32;
        let library = library_result
            .map_err(|error| native_addon_loader_error(filename, &error, max_napi_version))?;
        let symbol_api_version = unsafe {
            library
                .get::<unsafe extern "C" fn() -> i32>(b"node_api_module_get_api_version_v1\0")
                .ok()
                .map(|symbol| *symbol)
        };
        let symbol_initializer = unsafe {
            library
                .get::<NapiAddonRegister>(b"napi_register_module_v1\0")
                .ok()
                .map(|symbol| *symbol)
        };
        let (version, initialize) = if let Some(api_version) = symbol_api_version {
            let initialize = symbol_initializer.ok_or_else(|| {
                VmErr::Msg(format!(
                    "{filename} exports a Node-API version getter but no napi_register_module_v1 initializer"
                ))
            })?;
            (unsafe { api_version() }, initialize)
        } else if registered_modules.is_empty()
            && let Some(initialize) = symbol_initializer
        {
            // Node-API's module version getter is optional. Node uses its
            // default Node-API module version (currently 8) for modules that
            // export only napi_register_module_v1, including napi-rs addons.
            (DEFAULT_NODE_API_MODULE_VERSION, initialize)
        } else {
            match registered_modules.as_slice() {
                [module] => {
                    // SAFETY: napi_module_register receives a static module
                    // descriptor from this library's constructor; the library
                    // remains mapped for the entire registration and init.
                    let module = unsafe { &*(*module as *const NapiModule) };
                    if module.version != 1 {
                        return Err(VmErr::Msg(format!(
                            "Node-API addon {filename} uses unsupported legacy module descriptor version {}",
                            module.version
                        )));
                    }
                    let initialize = module.register.ok_or_else(|| {
                        VmErr::Msg(format!(
                            "Node-API addon {filename} registered a module without an initializer"
                        ))
                    })?;
                    // The legacy descriptor carries no Node-API version
                    // request, so use the conservative minimum for validation.
                    (1, initialize)
                }
                [] => {
                    return Err(VmErr::Msg(format!(
                        "{filename} is not a symbol-registered or legacy-registered Node-API addon"
                    )));
                }
                _ => {
                    return Err(VmErr::Msg(format!(
                        "{filename} registered multiple legacy Node-API modules; one .node file must select a single module"
                    )));
                }
            }
        };
        if !(1..=max_napi_version).contains(&version) {
            return Err(VmErr::Msg(format!(
                "Node-API addon {filename} requests version {version}; this host is configured for Node-API versions 1 through {max_napi_version}"
            )));
        }
        let module_file_url = url::Url::from_file_path(filename)
            .map(|url| CString::new(url.as_str()).expect("file URLs cannot contain NUL bytes"))
            .unwrap_or_default();

        let reported_node_version = self.state.borrow().reported_node_version;
        let environment = Rc::new(NapiEnvironment {
            module_path: filename.to_string(),
            module_file_url,
            api_version: version as u32,
            node_version: NapiNodeVersion {
                major: reported_node_version.major,
                minor: reported_node_version.minor,
                patch: reported_node_version.patch,
                release: NAPI_VM_RELEASE.as_ptr().cast(),
            },
            owner: Rc::downgrade(&self.state),
            handles: RefCell::new(NapiHandleArena::default()),
            references: RefCell::new(HashMap::new()),
            deferreds: RefCell::new(HashMap::new()),
            async_works: RefCell::new(HashMap::new()),
            threadsafe_functions: RefCell::new(HashMap::new()),
            async_contexts: RefCell::new(HashMap::new()),
            callback_scopes: RefCell::new(Vec::new()),
            cleanup_hooks: RefCell::new(Vec::new()),
            async_cleanup_hooks: RefCell::new(Vec::new()),
            next_cleanup_hook_order: Cell::new(1),
            last_error: Cell::new(napi_extended_error_info(NAPI_OK)),
            wraps: RefCell::new(HashMap::new()),
            added_finalizers: RefCell::new(Vec::new()),
            instance_data: RefCell::new(None),
            externals: RefCell::new(HashMap::new()),
            external_buffers: RefCell::new(HashMap::new()),
            external_memory: Cell::new(0),
            finalizing: Cell::new(false),
            active_callbacks: RefCell::new(HashMap::new()),
            guest_callback_dispatchers: RefCell::new(Vec::new()),
            pending_exception: RefCell::new(None),
            fatal_exceptions: RefCell::new(VecDeque::new()),
        });
        register_environment(&environment);
        self.state
            .borrow_mut()
            .environments
            .push(environment.clone());
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
        let mut callback_handler = callback_handler;
        let callback_handler_pointer: *mut &mut (
                 dyn FnMut(HostCallback) -> Result<Value, VmErr> + '_
             ) = &mut callback_handler;
        let dispatcher = GuestCallbackDispatcher {
            context: callback_handler_pointer.cast(),
            invoke: dispatch_guest_callback,
        };
        let dispatcher_scope = GuestCallbackDispatcherScope::push(environment.clone(), dispatcher);
        let returned = unsafe { initialize(environment.raw(), exports_handle) };
        drop(dispatcher_scope);
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
        napi_collect_weak_references(&environment);
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
                    self.state
                        .borrow_mut()
                        .libraries
                        .insert(filename_path.to_path_buf(), library);
                } else {
                    run_environment_cleanup_hooks(&environment);
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
        self.state
            .borrow_mut()
            .libraries
            .insert(filename_path.to_path_buf(), library);
        Ok(exports)
    }
}

impl HostBridge for RustNodeApiHost {
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        self.ensure_running()?;
        self.invoke_native(id, Value::Undefined, args, None, &mut reject_guest_callback)
    }

    fn call_host_with_this(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        self.ensure_running()?;
        self.invoke_native(id, this_value, args, None, &mut reject_guest_callback)
    }

    fn call_host_with_callback_handler(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.ensure_running()?;
        self.invoke_native(id, this_value, args, None, callback_handler)
    }

    fn construct_host_with_callback_handler(
        &self,
        id: usize,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.ensure_running()?;
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
        self.ensure_running()?;
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
        self.ensure_running()?;
        self.invoke_native(id, this_value, args, Some(new_target), callback_handler)
    }

    fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        if self.is_shutdown() {
            return Ok(Vec::new());
        }
        let mut events = {
            let state = self.state.borrow();
            state
                .environments
                .iter()
                .flat_map(|environment| {
                    environment
                        .fatal_exceptions
                        .borrow_mut()
                        .drain(..)
                        .map(HostEvent::UncaughtException)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        let notifications = {
            let state = self.state.borrow();
            let first = if timeout.is_zero() || !events.is_empty() {
                state.runtime_notifications.try_recv().ok()
            } else {
                state.runtime_notifications.recv_timeout(timeout).ok()
            };
            first
                .into_iter()
                .chain(state.runtime_notifications.try_iter())
                .collect::<Vec<_>>()
        };
        events.reserve(notifications.len());
        for notification in notifications {
            match notification {
                HostRuntimeNotification::AsyncWorkCompletion(completion) => {
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
                    .map_err(|status| {
                        napi_error("creating async-work completion callback", status)
                    })?;
                    events.push(HostEvent::Callback(HostCallback {
                        callback,
                        this_value: Value::Undefined,
                        args: Vec::new(),
                        kind: HostCallbackKind::Call,
                    }));
                }
                HostRuntimeNotification::ThreadsafeFunction(function_id) => {
                    events.extend(thread_safe_function_events(function_id)?);
                }
                HostRuntimeNotification::PostedFinalizer(finalizer) => {
                    let environment = {
                        let state = self.state.borrow();
                        state
                            .environments
                            .iter()
                            .find(|environment| environment.raw() as usize == finalizer.environment)
                            .cloned()
                    };
                    let Some(environment) = environment else {
                        continue;
                    };
                    let callback = create_posted_finalizer_value(
                        &environment,
                        finalizer.finalize,
                        finalizer.data as *mut c_void,
                        finalizer.hint as *mut c_void,
                    )
                    .map_err(|status| napi_error("creating posted finalizer callback", status))?;
                    events.push(HostEvent::Callback(HostCallback {
                        callback,
                        this_value: Value::Undefined,
                        args: Vec::new(),
                        kind: HostCallbackKind::Call,
                    }));
                }
            }
        }
        Ok(events)
    }

    fn has_pending_host_work(&self, promise: &Rc<RefCell<PromiseInner>>) -> bool {
        if self.is_shutdown() {
            return false;
        }
        if promise.borrow().external_pending {
            return true;
        }

        // A Node-API callback can settle an ordinary guest Promise rather
        // than a Promise created by napi_create_promise. Keep top-level await
        // pumping while async work or queued thread-safe-function calls can
        // still enter JavaScript and settle that Promise.
        self.state.borrow().environments.iter().any(|environment| {
            let has_async_work = environment.async_works.borrow().values().any(|work| {
                matches!(
                    work.state.load(Ordering::Acquire),
                    ASYNC_WORK_QUEUED
                        | ASYNC_WORK_RUNNING
                        | ASYNC_WORK_FINISHED
                        | ASYNC_WORK_CANCELLED
                )
            });
            let has_threadsafe_work =
                environment
                    .threadsafe_functions
                    .borrow()
                    .values()
                    .any(|function| {
                        function
                            .shared
                            .state
                            .lock()
                            .is_ok_and(|queue| !queue.values.is_empty() || queue.in_flight > 0)
                    });
            has_async_work || has_threadsafe_work
        })
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
        NAPI_DATE_EXPECTED => "date expected",
        NAPI_FUNCTION_EXPECTED => "function expected",
        NAPI_NUMBER_EXPECTED => "number expected",
        NAPI_ARRAY_EXPECTED => "array expected",
        NAPI_ARRAYBUFFER_EXPECTED => "ArrayBuffer expected",
        NAPI_DETACHABLE_ARRAYBUFFER_EXPECTED => "detachable ArrayBuffer expected",
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

fn missing_node_api_import_from_loader_error(error: &str) -> Option<&str> {
    let symbol_start = [
        "undefined symbol:",
        "Symbol not found:",
        "procedure entry point ",
    ]
    .into_iter()
    .find_map(|marker| error.find(marker).map(|index| index + marker.len()))?;

    error[symbol_start..]
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .map(|token| token.trim_start_matches('_'))
        .find(|token| token.starts_with("napi_") || token.starts_with("node_api_"))
}

fn native_addon_loader_error(
    filename: &str,
    error: &libloading::Error,
    max_napi_version: i32,
) -> VmErr {
    let detail = std::error::Error::source(error)
        .map(ToString::to_string)
        .unwrap_or_else(|| error.to_string());
    VmErr::Msg(native_addon_loader_error_message(
        filename,
        &detail,
        max_napi_version,
    ))
}

fn native_addon_loader_error_message(
    filename: &str,
    detail: &str,
    max_napi_version: i32,
) -> String {
    if let Some(symbol) = missing_node_api_import_from_loader_error(detail) {
        return format!(
            "[UNSUPPORTED_NODE_API] cannot load Node-API addon {filename}: imported symbol `{symbol}` is not provided by the Rust Node-API backend, configured for Node-API versions 1 through {max_napi_version}. The addon's declared version could not be read because symbol resolution failed before initialization. Use the Node sidecar backend with a compatible Node runtime if it supplies this API. Loader detail: {detail}"
        );
    }

    format!(
        "cannot load Node-API addon {filename}: {detail}; the binary may require an unavailable symbol or dependency"
    )
}

/// Install the experimental Node-API host as this interpreter's CommonJS
/// addon loader and host bridge. The sidecar API remains the cross-platform
/// option for addons that require Node/V8-specific symbols.
impl Interpreter {
    pub fn enable_rust_node_api_addons(
        &mut self,
        options: RustNodeApiOptions,
    ) -> Result<Rc<RustNodeApiHost>, VmErr> {
        if !(1..=MAX_NODE_API_VERSION as u32).contains(&options.max_napi_version) {
            return Err(VmErr::Msg(format!(
                "configured maximum Node-API version {} is outside the supported range 1 through {MAX_NODE_API_VERSION}",
                options.max_napi_version
            )));
        }
        let mut loader = FileCommonJsLoader::new(options.policy.roots().iter())?;
        if let Some(enabled) = options.node_gyp_build_prebuilds_only {
            loader = loader.with_node_gyp_build_prebuilds_only(enabled);
        }
        if let Some(exec_path) = &options.node_gyp_build_exec_path {
            loader = loader.with_node_gyp_build_exec_path(exec_path.clone());
        }
        for (addon, expected_sha256) in options.policy.allowed_addons() {
            loader = match expected_sha256 {
                Some(expected_sha256) => {
                    loader.allow_native_addon_with_sha256(addon, *expected_sha256)?
                }
                None => loader.allow_native_addon(addon)?,
            };
        }
        for alias in &options.native_prebuild_aliases {
            let addon = loader.resolve_node_api_prebuild(&alias.package_root)?;
            let addon_path = PathBuf::from(addon.filename);
            loader = match alias.expected_sha256 {
                Some(expected_sha256) => {
                    loader.allow_native_addon_with_sha256(&addon_path, expected_sha256)?
                }
                None => loader.allow_native_addon(&addon_path)?,
            };
            loader = loader.with_native_addon_alias(&alias.request, &addon_path)?;
        }
        for package in &options.native_package_prebuilds {
            let addon = loader.resolve_node_api_prebuild(&package.package_root)?;
            let addon_path = PathBuf::from(addon.filename);
            loader = match package.expected_sha256 {
                Some(expected_sha256) => {
                    loader.allow_native_addon_with_sha256(&addon_path, expected_sha256)?
                }
                None => loader.allow_native_addon(&addon_path)?,
            };
        }
        if !options.native_package_prebuilds.is_empty() {
            loader = loader.with_node_gyp_build_compat();
        }
        let entry = validate_entry(&loader, options.policy.entry_path().map(PathBuf::from))?;
        let host = Rc::new(RustNodeApiHost::new(
            self.persistent_global.clone(),
            options.reported_node_version,
            options.max_napi_version,
            loader.roots().to_vec(),
            loader.allowed_native_addon_digests().clone(),
        )?);
        self.install_native_addon_backend(loader, host, entry)
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

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests;

#[cfg(all(test, target_os = "windows"))]
#[path = "rust_node_api/tests/windows.rs"]
mod windows_tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "rust_node_api/tests/macos.rs"]
mod macos_tests;
