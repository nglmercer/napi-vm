//! Experimental in-process host for the Node-API C ABI.
//!
//! The first vertical slice intentionally implements the APIs needed by a
//! small real addon. Unimplemented imports fail during dynamic loading; this
//! backend does not emulate Node, V8, NAN, or libuv.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, c_char, c_void};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};

use crate::error::VmErr;
use crate::host::{HostBridge, HostCallback, HostCallbackKind, HostEvent};
use crate::interpreter::commonjs::NativeAddonLoader;
use crate::interpreter::{Env, FileCommonJsLoader, Interpreter};
use crate::value::{Buffer, ErrorData, TypedArrayData, TypedKind, Value};

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
const NAPI_ARRAYBUFFER_EXPECTED: i32 = 19;
const MAX_LOCAL_HANDLES: usize = 1_048_576;
const MAX_NAPI_BUFFER_BYTES: usize = crate::value::MAX_STRING_LEN;
static NEXT_OPAQUE_HANDLE_ID: AtomicUsize = AtomicUsize::new(1);

type NapiEnv = *mut c_void;
type NapiValue = *mut c_void;
type NapiCallbackInfo = *mut c_void;
type NapiHandleScope = *mut c_void;
type NapiRef = *mut c_void;
type NapiCallback = unsafe extern "C" fn(NapiEnv, NapiCallbackInfo) -> NapiValue;
type NapiFinalize = unsafe extern "C" fn(NapiEnv, *mut c_void, *mut c_void);

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

/// In-process Node-API addon host. This is opt-in and currently supports Linux
/// ELF modules using the small Node-API v1 surface implemented below.
pub struct RustNodeApiHost {
    state: Rc<RefCell<HostState>>,
    _shim: Rc<NodeApiShim>,
}

impl Drop for RustNodeApiHost {
    fn drop(&mut self) {
        // Keep HostState strongly reachable while finalizers run so ordinary
        // Node-API calls made by a finalizer can still access the host. The
        // addon libraries and symbol shim remain loaded in HostState until
        // this callback pass is complete.
        let environments = self.state.borrow().environments.clone();
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
    next_callback_id: usize,
    callbacks: HashMap<usize, NativeCallbackRecord>,
    environments: Vec<Rc<NapiEnvironment>>,
    libraries: Vec<Library>,
    // Keep the process-global ABI shim loaded until every addon library closes.
    _shim: Rc<NodeApiShim>,
}

#[derive(Clone)]
struct NativeCallbackRecord {
    env: Rc<NapiEnvironment>,
    callback: NapiCallback,
    data: *mut c_void,
}

struct NapiEnvironment {
    module_path: String,
    owner: Weak<RefCell<HostState>>,
    self_weak: Weak<NapiEnvironment>,
    handles: RefCell<NapiHandleArena>,
    references: RefCell<HashMap<usize, NapiReference>>,
    wraps: RefCell<HashMap<NapiObjectIdentity, NapiWrap>>,
    buffer_values: RefCell<HashMap<NapiObjectIdentity, Weak<TypedArrayData>>>,
    finalizing: Cell<bool>,
    active_callbacks: RefCell<HashMap<usize, CallbackFrame>>,
    guest_callback_dispatchers: RefCell<Vec<GuestCallbackDispatcher>>,
    pending_exception: RefCell<Option<Value>>,
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

struct NapiWrap {
    // Keeping the guest value alive prevents its identity pointer from being
    // reused while native data is still attached to it.
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
}

struct NapiHandleArena {
    slots: Vec<HandleSlot>,
    free_slots: Vec<usize>,
    scopes: Vec<HandleScope>,
    next_scope_id: u64,
    handles: HashMap<usize, HandleRef>,
    scope_handles: HashMap<usize, u64>,
}

impl Default for NapiHandleArena {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free_slots: Vec::new(),
            scopes: vec![HandleScope {
                id: 0,
                slots: Vec::new(),
            }],
            next_scope_id: 1,
            handles: HashMap::new(),
            scope_handles: HashMap::new(),
        }
    }
}

impl NapiHandleArena {
    fn create(&mut self, value: Value) -> Result<NapiValue, i32> {
        if self.handles.len() >= MAX_LOCAL_HANDLES {
            return Err(NAPI_GENERIC_FAILURE);
        }
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
        self.scopes
            .last_mut()
            .expect("the root handle scope is permanent")
            .slots
            .push(slot_index);

        let pointer = new_opaque_handle()?;
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
        });
        Ok(id)
    }

    fn create_scope_handle(&mut self, id: u64) -> Result<NapiHandleScope, i32> {
        if self.scope_handles.len() >= MAX_LOCAL_HANDLES {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let pointer = new_opaque_handle()?;
        self.scope_handles.insert(pointer as usize, id);
        Ok(pointer)
    }

    fn close_scope_handle(&mut self, pointer: NapiHandleScope) -> Result<(), i32> {
        let id = *self
            .scope_handles
            .get(&(pointer as usize))
            .ok_or(NAPI_INVALID_ARG)?;
        self.close_scope(id)?;
        self.scope_handles.remove(&(pointer as usize));
        Ok(())
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
    create_double: unsafe extern "C" fn(NapiEnv, f64, *mut NapiValue) -> i32,
    create_int32: unsafe extern "C" fn(NapiEnv, i32, *mut NapiValue) -> i32,
    create_uint32: unsafe extern "C" fn(NapiEnv, u32, *mut NapiValue) -> i32,
    create_int64: unsafe extern "C" fn(NapiEnv, i64, *mut NapiValue) -> i32,
    create_string_utf8: unsafe extern "C" fn(NapiEnv, *const c_char, usize, *mut NapiValue) -> i32,
    create_symbol: unsafe extern "C" fn(NapiEnv, NapiValue, *mut NapiValue) -> i32,
    typeof_value: unsafe extern "C" fn(NapiEnv, NapiValue, *mut i32) -> i32,
    get_value_double: unsafe extern "C" fn(NapiEnv, NapiValue, *mut f64) -> i32,
    get_value_int32: unsafe extern "C" fn(NapiEnv, NapiValue, *mut i32) -> i32,
    get_value_uint32: unsafe extern "C" fn(NapiEnv, NapiValue, *mut u32) -> i32,
    get_value_int64: unsafe extern "C" fn(NapiEnv, NapiValue, *mut i64) -> i32,
    get_value_bool: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    get_value_string_utf8:
        unsafe extern "C" fn(NapiEnv, NapiValue, *mut c_char, usize, *mut usize) -> i32,
    create_array: unsafe extern "C" fn(NapiEnv, *mut NapiValue) -> i32,
    create_array_with_length: unsafe extern "C" fn(NapiEnv, usize, *mut NapiValue) -> i32,
    is_array: unsafe extern "C" fn(NapiEnv, NapiValue, *mut bool) -> i32,
    get_array_length: unsafe extern "C" fn(NapiEnv, NapiValue, *mut u32) -> i32,
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
}

static NAPI_VM_API_TABLE: NapiVmApiTable = NapiVmApiTable {
    get_undefined: api_get_undefined,
    get_global: api_get_global,
    get_null: api_get_null,
    get_boolean: api_get_boolean,
    create_double: api_create_double,
    create_int32: api_create_int32,
    create_uint32: api_create_uint32,
    create_int64: api_create_int64,
    create_string_utf8: api_create_string_utf8,
    create_symbol: api_create_symbol,
    typeof_value: api_typeof,
    get_value_double: api_get_value_double,
    get_value_int32: api_get_value_int32,
    get_value_uint32: api_get_value_uint32,
    get_value_int64: api_get_value_int64,
    get_value_bool: api_get_value_bool,
    get_value_string_utf8: api_get_value_string_utf8,
    create_array: api_create_array,
    create_array_with_length: api_create_array_with_length,
    is_array: api_is_array,
    get_array_length: api_get_array_length,
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
    create_function: api_create_function,
    set_named_property: api_set_named_property,
    get_named_property: api_get_named_property,
    get_property: api_get_property,
    set_property: api_set_property,
    has_property: api_has_property,
    delete_property: api_delete_property,
    has_own_property: api_has_own_property,
    has_named_property: api_has_named_property,
    get_property_names: api_get_property_names,
    call_function: api_call_function,
    new_instance: api_new_instance,
    get_cb_info: api_get_cb_info,
    open_handle_scope: api_open_handle_scope,
    close_handle_scope: api_close_handle_scope,
};

fn with_ffi_status(callback: impl FnOnce() -> Result<(), i32>) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback))
        .unwrap_or(Err(NAPI_GENERIC_FAILURE))
        .map_or_else(|status| status, |_| NAPI_OK)
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

unsafe extern "C" fn api_get_undefined(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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

unsafe extern "C" fn api_create_double(env: NapiEnv, value: f64, result: *mut NapiValue) -> i32 {
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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

unsafe extern "C" fn api_create_symbol(
    env: NapiEnv,
    description: NapiValue,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(|| {
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

unsafe extern "C" fn api_typeof(env: NapiEnv, value: NapiValue, result: *mut i32) -> i32 {
    with_ffi_status(|| {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        // Values are the Node-API `napi_valuetype` discriminants from
        // js_native_api_types.h. Proxy `typeof` follows its target.
        unsafe { result.write(napi_value_type(&value)) };
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(value)?;
        let Value::Number(number) = value else {
            return Err(NAPI_NUMBER_EXPECTED);
        };
        unsafe { result.write(number as i64) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_value_bool(env: NapiEnv, value: NapiValue, result: *mut bool) -> i32 {
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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

unsafe extern "C" fn api_create_array(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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

unsafe extern "C" fn api_get_element(
    env: NapiEnv,
    value: NapiValue,
    index: u32,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        unsafe { result.write(environment.pending_exception.borrow().is_some()) };
        Ok(())
    })
}

unsafe extern "C" fn api_get_and_clear_last_exception(env: NapiEnv, result: *mut NapiValue) -> i32 {
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
        let environment = environment(env)?;
        if environment.finalizing.get() {
            return Err(NAPI_GENERIC_FAILURE);
        }
        let value = environment.handles.borrow().get(object)?;
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
    with_ffi_status(|| {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(object)?;
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
    with_ffi_status(|| {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let value = environment.handles.borrow().get(object)?;
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
    with_ffi_status(|| {
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

unsafe extern "C" fn api_create_function(
    env: NapiEnv,
    name: *const c_char,
    name_length: usize,
    callback: Option<NapiCallback>,
    data: *mut c_void,
    result: *mut NapiValue,
) -> i32 {
    with_ffi_status(|| {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let callback = callback.ok_or(NAPI_FUNCTION_EXPECTED)?;
        let environment = environment(env)?;
        let environment_rc = environment
            .self_weak
            .upgrade()
            .ok_or(NAPI_GENERIC_FAILURE)?;
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
        let owner = environment.owner.upgrade().ok_or(NAPI_GENERIC_FAILURE)?;
        let id = {
            let mut state = owner.borrow_mut();
            let id = state.next_callback_id;
            state.next_callback_id = id.checked_add(1).ok_or(NAPI_GENERIC_FAILURE)?;
            state.callbacks.insert(
                id,
                NativeCallbackRecord {
                    env: environment_rc,
                    callback,
                    data,
                },
            );
            id
        };
        let value = Value::HostFunction {
            name: Rc::from(function_name.as_str()),
            id,
        };
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
    with_ffi_status(|| {
        let key = unsafe { read_c_string(name)? };
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        let value = environment.handles.borrow().get(value)?;
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let key = environment.handles.borrow().get(key)?;
        let value = environment.handles.borrow().get(value)?;
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
        let environment = environment(env)?;
        let object = environment.handles.borrow().get(object)?;
        if !is_napi_property_object(&object) {
            return Err(NAPI_OBJECT_EXPECTED);
        }
        let key = environment.handles.borrow().get(key)?;
        let deleted = if has_guest_callback_dispatcher(&environment) {
            let value = run_napi_guest_operation(
                &environment,
                "napi_delete_property",
                napi_guest_delete_property,
                object,
                vec![key],
            )?;
            let Value::Bool(deleted) = value else {
                return Err(NAPI_GENERIC_FAILURE);
            };
            deleted
        } else {
            if matches!(object, Value::GlobalObject) {
                napi_global_delete(&environment, &napi_property_key(&key)?)?
            } else {
                napi_direct_delete_property(&object, &key)?
            }
        };
        if !result.is_null() {
            unsafe { result.write(deleted) };
        }
        Ok(())
    })
}

unsafe extern "C" fn api_has_own_property(
    env: NapiEnv,
    object: NapiValue,
    key: NapiValue,
    result: *mut bool,
) -> i32 {
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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
    with_ffi_status(|| {
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

unsafe extern "C" fn api_get_cb_info(
    env: NapiEnv,
    info: NapiCallbackInfo,
    argc: *mut usize,
    argv: *mut NapiValue,
    this_arg: *mut NapiValue,
    data: *mut *mut c_void,
) -> i32 {
    with_ffi_status(|| {
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

unsafe extern "C" fn api_open_handle_scope(env: NapiEnv, result: *mut NapiHandleScope) -> i32 {
    with_ffi_status(|| {
        if result.is_null() {
            return Err(NAPI_INVALID_ARG);
        }
        let environment = environment(env)?;
        let mut handles = environment.handles.borrow_mut();
        let scope = handles.open_scope()?;
        let handle = match handles.create_scope_handle(scope) {
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
    with_ffi_status(|| {
        let environment = environment(env)?;
        environment.handles.borrow_mut().close_scope_handle(scope)
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
        let path = root.join("libnapi_vm_node_api_shim.so");
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
        // Linux permits unlinking a loaded shared object; the mapping remains
        // live until the Library is dropped immediately after this method.
        if let Some(root) = self.path.parent() {
            let _ = fs::remove_dir_all(root);
        }
    }
}

impl RustNodeApiHost {
    fn new(global: Env) -> Result<Self, VmErr> {
        let shim = Rc::new(NodeApiShim::load()?);
        Ok(Self {
            state: Rc::new(RefCell::new(HostState {
                global,
                next_callback_id: 1,
                callbacks: HashMap::new(),
                environments: Vec::new(),
                libraries: Vec::new(),
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
        mut callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        let callback = self
            .state
            .borrow()
            .callbacks
            .get(&id)
            .cloned()
            .ok_or_else(|| VmErr::Msg("native callback handle is no longer valid".into()))?;
        let scope = callback
            .env
            .handles
            .borrow_mut()
            .open_scope()
            .map_err(|status| napi_error("opening callback handle scope", status))?;
        let result = (|| {
            let this_arg = callback
                .env
                .handles
                .borrow_mut()
                .create(this_value)
                .map_err(|status| napi_error("creating callback receiver handle", status))?;
            let arg_handles = args
                .into_iter()
                .map(|value| {
                    callback
                        .env
                        .handles
                        .borrow_mut()
                        .create(value)
                        .map_err(|status| napi_error("creating callback argument handle", status))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let frame = CallbackFrame {
                args: arg_handles,
                this_arg,
                data: callback.data,
            };
            let callback_info = (&frame as *const CallbackFrame).cast_mut().cast::<c_void>();
            let frame_key = callback_info as usize;
            callback
                .env
                .active_callbacks
                .borrow_mut()
                .insert(frame_key, frame.clone());
            let callback_handler_pointer: *mut &mut (
                     dyn FnMut(HostCallback) -> Result<Value, VmErr> + '_
                 ) = &mut callback_handler;
            let dispatcher = GuestCallbackDispatcher {
                context: callback_handler_pointer.cast(),
                invoke: dispatch_guest_callback,
            };
            let dispatcher_scope =
                GuestCallbackDispatcherScope::push(callback.env.clone(), dispatcher);
            let returned = unsafe { (callback.callback)(callback.env.raw(), callback_info) };
            drop(dispatcher_scope);
            callback
                .env
                .active_callbacks
                .borrow_mut()
                .remove(&frame_key);
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
        match (result, close_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    }
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
        if version != 1 {
            return Err(VmErr::Msg(format!(
                "Node-API addon {filename} requests version {version}; this host currently supports Node-API version 1 only"
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

        let environment = Rc::new_cyclic(|weak| NapiEnvironment {
            module_path: filename.to_string(),
            owner: Rc::downgrade(&self.state),
            self_weak: weak.clone(),
            handles: RefCell::new(NapiHandleArena::default()),
            references: RefCell::new(HashMap::new()),
            wraps: RefCell::new(HashMap::new()),
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
                environment.finalizing.set(true);
                finalize_environment_wraps(&environment);
                self.state
                    .borrow_mut()
                    .callbacks
                    .retain(|_, callback| callback.env.module_path != filename);
                self.state
                    .borrow_mut()
                    .environments
                    .retain(|env| env.module_path != filename);
                return Err(error);
            }
        };
        self.state.borrow_mut().libraries.push(library);
        Ok(exports)
    }
}

impl HostBridge for RustNodeApiHost {
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        self.invoke_native(id, Value::Undefined, args, &mut reject_guest_callback)
    }

    fn call_host_with_this(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        self.invoke_native(id, this_value, args, &mut reject_guest_callback)
    }

    fn call_host_with_callback_handler(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.invoke_native(id, this_value, args, callback_handler)
    }

    fn construct_host_with_callback_handler(
        &self,
        id: usize,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        self.invoke_native(id, Value::Undefined, args, callback_handler)
    }

    fn poll_host_events(&self, _timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        Ok(Vec::new())
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
        let scope_handle = arena.create_scope_handle(scope_id).unwrap();
        arena.close_scope_handle(scope_handle).unwrap();
        assert_eq!(
            arena.close_scope_handle(scope_handle).unwrap_err(),
            NAPI_INVALID_ARG
        );
    }

    #[test]
    fn loads_and_calls_a_real_napi_v1_addon_without_a_node_sidecar() {
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
#define NAPI_VERSION 1
#include <node_api.h>
#include <stdint.h>

static napi_ref persistent_values;
static napi_ref removable_object;
static napi_ref wrapped_object_reference;
static int wrapped_finalizer_calls;
static int removed_finalizer_calls;
static int finalizer_create_function_status = -1;
static char wrapped_native_data[] = "wrapped-native-data";
static char removable_native_data[] = "removed-native-data";

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
    finalizer_create_function_status = napi_create_function(
        env, "fromFinalizer", NAPI_AUTO_LENGTH, finalizer_noop, NULL, &ignored);
  }
  if (data == removable_native_data) removed_finalizer_calls++;
}

int napi_vm_test_wrapped_finalizer_calls(void) {
  return wrapped_finalizer_calls;
}

int napi_vm_test_removed_finalizer_calls(void) {
  return removed_finalizer_calls;
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

NAPI_MODULE_INIT() {
  napi_handle_scope scope;
  napi_value scratch, function, metadata, version, values, field;
  napi_value global, global_key, global_object_constructor;
  napi_valuetype global_object_type;
  bool global_object_own = false;
  int32_t checked_version = 0;
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
  if (napi_create_function(env, "add", NAPI_AUTO_LENGTH, add, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "add", function) != napi_ok ||
      napi_create_object(env, &metadata) != napi_ok ||
      napi_create_int32(env, 1, &version) != napi_ok ||
      napi_set_named_property(env, metadata, "version", version) != napi_ok ||
      napi_get_named_property(env, metadata, "version", &field) != napi_ok ||
      napi_get_value_int32(env, field, &checked_version) != napi_ok ||
      checked_version != 1 ||
      napi_set_named_property(env, exports, "metadata", metadata) != napi_ok) return NULL;
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
      napi_create_function(env, "propertyProbe", NAPI_AUTO_LENGTH, property_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "propertyProbe", function) != napi_ok ||
      napi_create_function(env, "globalProbe", NAPI_AUTO_LENGTH, global_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "globalProbe", function) != napi_ok ||
      napi_create_function(env, "symbolProbe", NAPI_AUTO_LENGTH, symbol_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "symbolProbe", function) != napi_ok ||
      napi_create_function(env, "invalidEnvironment", NAPI_AUTO_LENGTH, invalid_environment, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "invalidEnvironment", function) != napi_ok) return NULL;
  return exports;
}
"#,
        )
        .unwrap();
        let built = Command::new("cc")
            .args([
                "-std=c11",
                "-O2",
                "-fPIC",
                "-shared",
                "-DNAPI_VERSION=1",
                "-I",
            ])
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
module.exports = {
  same: addon === require('./fixture.node'),
  global: addon.globalProbe(),
  globalHasObject: Object.hasOwn(globalThis, 'Object'),
  symbols: addon.symbolProbe(),
  sum: addon.add(19, 23),
  version: addon.metadata.version,
  truth: values.truth,
  nothing: values.nothing === null,
  missing: values.missing === undefined,
  greeting: values.greeting,
  fraction: values.fraction,
  maxUint32: values.maxUint32,
  int64: values.int64,
  roundTrip: addon.roundTrip(true, 4.25, 'native ✓', 4294967295, -2.5),
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
        let finalizer_create_function_status: unsafe extern "C" fn() -> i32 = unsafe {
            *observer
                .get(b"napi_vm_test_finalizer_create_function_status\0")
                .unwrap()
        };
        assert_eq!(unsafe { wrapped_finalizer_calls() }, 0);
        assert_eq!(unsafe { removed_finalizer_calls() }, 0);
        let result = interpreter.eval_source("require('./main.cjs');").unwrap();
        assert!(matches!(result.get_prop("same"), Some(Value::Bool(true))));
        assert!(matches!(result.get_prop("global"), Some(Value::Bool(true))));
        assert!(matches!(
            result.get_prop("globalHasObject"),
            Some(Value::Bool(true))
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

        let invalid_env = interpreter
            .eval_source("require('./fixture.node').invalidEnvironment();")
            .unwrap();
        assert!(matches!(
            invalid_env,
            Value::Number(value) if value == NAPI_INVALID_ARG as f64
        ));

        drop(result);
        drop(invalid_env);
        drop(interpreter);
        assert_eq!(unsafe { wrapped_finalizer_calls() }, 1);
        assert_eq!(unsafe { removed_finalizer_calls() }, 0);
        assert_eq!(unsafe { finalizer_create_function_status() }, NAPI_OK);
        drop(observer);
        fs::remove_dir_all(root).unwrap();
    }
}
