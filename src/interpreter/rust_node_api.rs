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

mod api;
#[allow(unused_imports)]
use api::*;

mod state;
use state::*;
mod api_table;
use api_table::*;
mod guest;
use guest::*;
mod lifecycle;
use lifecycle::*;
mod shim;
use shim::*;
mod async_work;
use async_work::*;

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
