//! The `#[napi]` VM surface and its single-owner execution gate.
//!
//! The interpreter remains deliberately single-threaded (`Rc`/`RefCell`). A
//! `VM` owns it behind `RuntimeCell`, whose mutex is the only way either the
//! Node thread or a `runAsync` worker can access it. The worker captures an
//! `Arc<VMState>`, never a raw pointer into the N-API object, so dropping the
//! JavaScript wrapper cannot leave a use-after-free behind.

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::JsObjectValue;
use napi::bindgen_prelude::{Object, Unknown};
use napi::{Env, JsValue, sys};
use napi_derive::napi;

use crate::error::VmErr;
use crate::format::try_to_string;
use crate::interpreter::Interpreter;
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::value::{PromiseState, Value};

pub use super::async_session::{AsyncSession, AsyncSessionOptions};
use super::bridge::{NapiHostBridge, run_async_done_cb};
use super::marshal::{chk, from_napi, make_str, to_napi};

/// Implemented runtime globals are installed only after an explicit request.
#[napi(object)]
pub struct RuntimeCapabilities {
    pub console: Option<bool>,
    pub timers: Option<bool>,
    pub web_apis: Option<bool>,
    pub node_compat: Option<bool>,
}

/// Encode a module name into a global-name prefix.
///
/// The encoding is injective: alphanumerics pass through and every other byte
/// becomes `_<hex>`, so `a:b` and `a/b` cannot collapse onto the same prefix
/// and hand one module another's bridge globals. Names remain conventional
/// rather than a security boundary — the host functions enforce their own
/// rules — but two modules must never share a namespace.
fn host_module_prefix(name: &str) -> String {
    let mut out = String::from("__hostmod_");
    for byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("_{byte:02x}"));
        }
    }
    out.push('_');
    out
}

/// A key usable both as an ES export name and as part of a global identifier.
///
/// `registerHostModule` pastes each key into generated source twice -- once as
/// `export function <key>` and once as part of the bridge global's name -- so
/// the key has to be something the lexer actually hands back as an identifier.
/// That question is answered by the lexer itself (`is_binding_identifier`)
/// rather than a keyword list maintained here, which would drift out of sync
/// and turn a rejectable input into ungrammatical generated code.
fn is_export_identifier(key: &str) -> bool {
    crate::lexer::is_binding_identifier(key)
        && !matches!(
            key,
            "async" | "from" | "as" | "of" | "get" | "set" | "constructor"
        )
}

// Unexecuted code templates contain no guest Value/Env or owner-local shape
// guards. Keep this cache thread-local (Rc code is not Send), bounded, and
// fork all feedback before running a template in a fresh owner.
#[derive(Default)]
struct FreshProgramCache {
    programs: HashMap<Arc<str>, (crate::interpreter::PreparedProgram, bool)>,
    order: std::collections::VecDeque<Arc<str>>,
    bytes: usize,
}
impl FreshProgramCache {
    #[cfg(test)]
    fn prepare(&mut self, source: &str) -> Result<crate::interpreter::PreparedProgram, VmErr> {
        self.prepare_for(source, false)
    }
    fn prepare_for(
        &mut self,
        source: &str,
        feedback_disabled: bool,
    ) -> Result<crate::interpreter::PreparedProgram, VmErr> {
        if let Some((program, no_guards)) = self.programs.get(source) {
            return Ok(if feedback_disabled && *no_guards {
                program.clone()
            } else {
                program.fork_for_owner()
            });
        }
        let program = Interpreter::compile(source)?;
        // Only misses walk the tree. Without feedback or property sites,
        // verified code contains no mutable owner-specific state.
        let no_guards = program.stats().is_none_or(|stats| stats.ic_sites == 0);
        const MAX_BYTES: usize = 2 * 1024 * 1024;
        if source.len() <= MAX_BYTES {
            while self.programs.len() >= 64 || self.bytes + source.len() > MAX_BYTES {
                let old = self.order.pop_front().expect("cached program");
                self.programs.remove(&old);
                self.bytes -= old.len();
            }
            let key: Arc<str> = source.into();
            self.bytes += key.len();
            self.order.push_back(key.clone());
            self.programs.insert(key, (program.clone(), no_guards));
        }
        Ok(if feedback_disabled && no_guards {
            program
        } else {
            program.fork_for_owner()
        })
    }
}
thread_local! {
    static FRESH_PROGRAMS: std::cell::RefCell<FreshProgramCache> = Default::default();
    static EMPTY_OWNER: std::cell::RefCell<Option<crate::runtime::OwnerContext>> = const { std::cell::RefCell::new(None) };
}

pub fn run_source(source: &str, is_main: bool) -> Result<String, VmErr> {
    let mut context = EMPTY_OWNER
        .with(|slot| slot.borrow_mut().take())
        .unwrap_or_default();
    let lease = context.enter();
    let result = {
        let mut interp = Interpreter::with_builtins();
        interp.is_main = is_main;
        let result = FRESH_PROGRAMS
            .with(|cache| {
                cache
                    .borrow_mut()
                    .prepare_for(source, interp.feedback_disabled())
            })
            .and_then(|program| interp.execute(&program))
            .and_then(|value| try_to_string(&value))
            .map_err(|error| VmErr::Msg(interp.enrich_error(error, None).to_string()));
        // Only a formatted string leaves this fresh runtime. Sever its
        // discarded global edges before sweeping the remaining cycles.
        interp.global.borrow_mut().clear_edges();
        result
    };
    let collected = crate::heap::collect_after_interpreter_drop();
    drop(lease);
    if collected.skipped.is_none() && context.reset_empty() {
        EMPTY_OWNER.with(|slot| *slot.borrow_mut() = Some(context));
    }
    result
}

/// All interpreter and bridge state lives here. It is never cloned or exposed
/// independently of the runtime gate.
pub(super) struct VmRuntime {
    pub(super) interp: Interpreter,
    modules: HashMap<String, Arc<str>>,
    /// VM functions handed to the host, indexed by the id their host-side
    /// wrapper carries. A slot is `None` once its wrapper is collected.
    pub(super) exports: super::export_slots::ExportSlots,
    export_releases: std::sync::mpsc::Receiver<super::export_slots::ExportId>,
    /// Bridge globals generated per `registerHostModule` name, so they can be
    /// revoked when the module is replaced or removed.
    host_module_globals: HashMap<String, Vec<String>>,
    bridge: Option<std::rc::Rc<NapiHostBridge>>,
}

/// A mutex-backed owner for a non-`Send` interpreter.
///
/// Every operation holds the gate and leases the VM's detached runtime arena.
/// The busy guard covers host marshalling too. Neither another VM nor a TLS
/// registry can concurrently reach this arena's guest graphs.
struct RuntimeCell {
    gate: Mutex<()>,
    runtime: UnsafeCell<Option<VmRuntime>>,
    context: UnsafeCell<crate::runtime::OwnerContext>,
}

// SAFETY: `gate` grants an exclusive lease over the runtime AND its heap,
// shapes, symbols and collection caches. `with_mut` installs that arena only
// for the lease, then detaches it (including during unwinding). No guest Rc
// remains in a native thread's TLS after the lease. Node methods and the
// persistent executor use the same gate; public busy guards also cover guest
// arguments/results during marshalling outside the gate. Finalizers enqueue
// integer export IDs and never touch guest values. The N-API bridge transfers
// only wire values and integer handles. VM does not install the in-process
// native-addon backend (its NAPI environment registry is owner-affine).
// This is a narrow arena migration boundary, not a Send impl for guest values.
unsafe impl Send for RuntimeCell {}
unsafe impl Sync for RuntimeCell {}

impl RuntimeCell {
    fn new(make: impl FnOnce() -> VmRuntime) -> Self {
        let mut context = crate::runtime::OwnerContext::default();
        let runtime = {
            let _lease = context.enter();
            make()
        };
        Self {
            gate: Mutex::new(()),
            runtime: UnsafeCell::new(Some(runtime)),
            context: UnsafeCell::new(context),
        }
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut VmRuntime) -> R) -> R {
        let _guard = self
            .gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // SAFETY: the mutex guard above excludes every other access.
        unsafe {
            let _lease = (&mut *self.context.get()).enter();
            let runtime = (&mut *self.runtime.get())
                .as_mut()
                .expect("live VM runtime");
            runtime.drain_export_releases();
            f(runtime)
        }
    }
}

impl Drop for RuntimeCell {
    fn drop(&mut self) {
        let _lease = self.context.get_mut().enter();
        drop(self.runtime.get_mut().take());
        let _ = crate::heap::collect_after_interpreter_drop();
    }
}

impl VmRuntime {
    /// The value an exported function id refers to, if it is still live.
    pub(super) fn export(&self, index: super::export_slots::ExportId) -> Option<Value> {
        self.exports.get(index)
    }
    fn drain_export_releases(&mut self) {
        while let Ok(id) = self.export_releases.try_recv() {
            self.exports.release(id);
        }
    }
}

type ExecutorJob = Box<dyn FnOnce() + Send>;
type ExecutorSender = std::sync::mpsc::Sender<ExecutorJob>;

pub(super) struct VMState {
    runtime: RuntimeCell,
    busy: Arc<AtomicBool>,
    /// Kept outside `RuntimeCell` so `VM::drop` can release N-API resources
    /// without waiting for a worker that may currently be awaiting Node.
    bridge_state: Mutex<Option<Arc<super::bridge::BridgeState>>>,
    release_tx: std::sync::mpsc::Sender<super::export_slots::ExportId>,
    executor: Mutex<Option<ExecutorSender>>,
}

impl VMState {
    /// Record a VM value the host is taking a reference to, returning its id.
    ///
    /// Called while the runtime gate is *not* held — the marshalling that
    /// needs it runs inside `with_mut` — so it takes the gate itself.
    pub(super) fn register_export(&self, value: Value) -> super::export_slots::ExportId {
        self.runtime
            .with_mut(|runtime| runtime.exports.insert(value))
    }
    #[cfg(test)]
    pub(super) fn assert_no_queued_export_releases(&self) {
        let _guard = self.runtime.gate.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: the production gate excludes every runtime access. Only
        // integer release messages are inspected; no guest value leaves it.
        let runtime = unsafe { &*self.runtime.runtime.get() }.as_ref().unwrap();
        assert!(matches!(
            runtime.export_releases.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
    }

    /// Failed export setup rolls back its pin before returning to the host.
    pub(super) fn rollback_export(&self, index: super::export_slots::ExportId) {
        self.runtime
            .with_mut(|runtime| runtime.exports.release(index));
    }

    /// Finalizers enqueue only an ID and never acquire the runtime gate.
    pub(super) fn release_export(&self, index: super::export_slots::ExportId) {
        let _ = self.release_tx.send(index);
    }

    pub(super) fn with_runtime<R>(&self, f: impl FnOnce(&mut VmRuntime) -> R) -> R {
        self.runtime.with_mut(f)
    }

    fn dispatch_async(&self, job: ExecutorJob) -> std::io::Result<()> {
        let mut executor = self.executor.lock().unwrap_or_else(|e| e.into_inner());
        if executor.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<ExecutorJob>();
            std::thread::Builder::new()
                .name("napi-vm-owner".into())
                .stack_size(8 * 1024 * 1024)
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        job();
                    }
                })?;
            *executor = Some(tx);
        }
        executor
            .as_ref()
            .expect("initialized executor")
            .send(job)
            .map_err(|_| std::io::Error::other("VM executor stopped"))
    }

    pub(super) fn try_start(&self) -> napi::Result<BusyGuard> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| BusyGuard {
                busy: self.busy.clone(),
            })
            .map_err(|_| napi::Error::from_reason("VM is busy with another execution"))
    }
}

pub(super) struct BusyGuard {
    busy: Arc<AtomicBool>,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
    }
}

#[napi]
pub struct VM {
    state: Arc<VMState>,
}

/// One parse-only diagnostic returned by `VM.validateModule`.
#[napi(object)]
pub struct ValidationDiagnostic {
    pub line: u32,
    pub column: u32,
    pub message: String,
    #[napi(ts_type = "\"syntax\" | \"unsupported-syntax\" | \"parse-limit\"")]
    pub kind: String,
}

/// Result of lexing and parsing guest source without executing it.
#[napi(object)]
pub struct ValidationResult {
    pub valid: bool,
    pub diagnostics: Vec<ValidationDiagnostic>,
}

impl Default for VM {
    fn default() -> Self {
        Self::new()
    }
}

impl VM {
    pub(super) fn new_state() -> Arc<VMState> {
        let (release_tx, export_releases) = std::sync::mpsc::channel();
        Arc::new(VMState {
            runtime: RuntimeCell::new(|| VmRuntime {
                interp: Interpreter::with_builtins(),
                modules: HashMap::new(),
                exports: super::export_slots::ExportSlots::default(),
                export_releases,
                host_module_globals: HashMap::new(),
                bridge: None,
            }),
            busy: Arc::new(AtomicBool::new(false)),
            bridge_state: Mutex::new(None),
            executor: Mutex::new(None),
            release_tx,
        })
    }

    /// Attach the host bridge on first use. Must be called while the runtime
    /// gate is held on Node's main thread.
    fn ensure_bridge(
        state: &Arc<VMState>,
        runtime: &mut VmRuntime,
        env: Env,
    ) -> Result<std::rc::Rc<NapiHostBridge>, VmErr> {
        if let Some(bridge) = runtime.bridge.as_ref() {
            return Ok(bridge.clone());
        }
        let bridge = std::rc::Rc::new(NapiHostBridge::new(env.raw()));
        runtime.interp.host = Some(bridge.clone());
        *state
            .bridge_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(bridge.shared_state());
        runtime.bridge = Some(bridge.clone());
        Ok(bridge)
    }

    fn current_bridge(runtime: &VmRuntime) -> Option<std::rc::Rc<NapiHostBridge>> {
        runtime.bridge.clone()
    }

    /// Drop a global and release its bridge handle. Must be called while the
    /// runtime gate is held.
    fn revoke_global(runtime: &mut VmRuntime, name: &str) -> bool {
        let old = runtime.interp.global_value(name);
        let removed = runtime.interp.persistent_global.borrow_mut().remove(name);
        if removed
            && let Some(id) = old.as_ref().and_then(Value::host_function_id)
            && let Some(bridge) = Self::current_bridge(runtime)
        {
            bridge.unregister(id);
        }
        removed
    }
}

impl Drop for VM {
    fn drop(&mut self) {
        // This runs on Node's main thread. It deliberately does not lock the
        // runtime: an async worker may be parked waiting for a Promise, and
        // waiting here would deadlock the Node event loop. The shared bridge
        // marks handles retired and releases its initial TSFN reference; any
        // in-flight callback owns the remaining lease until it finishes.
        if let Some(bridge) = self
            .state
            .bridge_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
        {
            bridge.shutdown_on_main();
        }
    }
}

#[napi]
impl VM {
    #[napi(constructor)]
    pub fn new() -> Self {
        Self {
            state: Self::new_state(),
        }
    }

    /// Enable selected globals; compile-time features are required and do not
    /// grant filesystem, network, environment, process or addon permission.
    #[napi]
    pub fn enable_runtime(&mut self, options: RuntimeCapabilities) -> napi::Result<()> {
        if (options.console.unwrap_or(false) || options.timers.unwrap_or(false))
            && !cfg!(feature = "runtime")
        {
            return Err(napi::Error::from_reason(
                "runtime globals require the runtime Cargo feature",
            ));
        }
        if options.web_apis.unwrap_or(false) && !cfg!(feature = "runtime-web") {
            return Err(napi::Error::from_reason("Web globals require runtime-web"));
        }
        if options.node_compat.unwrap_or(false) && !cfg!(feature = "runtime-node") {
            return Err(napi::Error::from_reason(
                "Node Buffer requires runtime-node",
            ));
        }
        let _busy = self.state.try_start()?;
        self.state.runtime.with_mut(|_runtime| {
            #[cfg(feature = "runtime")]
            {
                let mut global = _runtime.interp.global.borrow_mut();
                if options.console.unwrap_or(false) {
                    crate::runtime::install_console(&mut global);
                }
                if options.timers.unwrap_or(false) {
                    crate::runtime::install_timers(&mut global);
                }
                #[cfg(feature = "runtime-web")]
                if options.web_apis.unwrap_or(false) {
                    crate::runtime::install_web(&mut global);
                }
                #[cfg(feature = "runtime-node")]
                if options.node_compat.unwrap_or(false) {
                    crate::runtime::install_buffer(&mut global);
                }
            }
            Ok(())
        })
    }

    #[napi]
    pub fn run(&mut self, source: napi::bindgen_prelude::Utf16String) -> napi::Result<String> {
        let source = crate::JsString::from_units(source.to_vec());
        let _busy = self.state.try_start()?;
        let state = self.state.clone();
        state.runtime.with_mut(|runtime| {
            let result = execute_source_utf16(&mut runtime.interp, &source)
                .and_then(|value| try_to_string(&value))
                .map_err(|error| {
                    napi::Error::from_reason(runtime.interp.enrich_error(error, None).to_string())
                });
            runtime.interp.maybe_collect_cycles();
            result
        })
    }

    /// Lex and parse module source without evaluating it or resolving imports.
    ///
    /// This uses the same lexer and parser as execution. In particular, it
    /// does not register or run the module, invoke host functions, or apply
    /// side effects. Callers can use it to decide whether optional source
    /// transformation is needed before registration.
    #[napi(js_name = "validateModule")]
    pub fn validate_module(&self, source: napi::bindgen_prelude::Utf16String) -> ValidationResult {
        validate_module_source(&crate::JsString::from_units(source.to_vec()))
    }

    /// Define a guest module *without* evaluating it.
    ///
    /// The body runs the first time something imports the module. Deferring it
    /// is what makes a cyclic import graph expressible: define every module in
    /// the cycle, then import any one of them, and each body runs once with
    /// the partner's partially-populated exports visible through live
    /// bindings — which is what the ES module specification describes.
    ///
    /// `registerModule` remains the eager form, and reports a body's error at
    /// registration time; with `defineModule` the error surfaces at the import.
    #[napi]
    pub fn define_module(
        &mut self,
        name: napi::bindgen_prelude::Utf16String,
        source: napi::bindgen_prelude::Utf16String,
    ) -> napi::Result<()> {
        let name = strict_host_text(&name)?;
        let source = strict_host_text(&source)?;
        let _busy = self.state.try_start()?;
        self.state.runtime.with_mut(|runtime| {
            let source: Arc<str> = source.into();
            runtime.interp.define_module_shared(&name, source.clone());
            runtime.modules.insert(name, source);
        });
        Ok(())
    }

    /// Register (or replace) a guest module.
    ///
    /// Registration is transactional over the module's export table: the body
    /// evaluates into a fresh export record, and that record replaces the
    /// previous one only once evaluation succeeds. A body that throws leaves
    /// the previously registered version of the module exactly as it was,
    /// rather than a half-populated one.
    ///
    /// The transaction covers exports, not global side effects. A body that
    /// assigns a global and *then* throws leaves that global set; unwinding
    /// arbitrary interpreter state is not something this layer can promise.
    /// Callers who need that isolation should use a fresh `Vm`.
    #[napi]
    pub fn register_module(
        &mut self,
        name: napi::bindgen_prelude::Utf16String,
        source: napi::bindgen_prelude::Utf16String,
    ) -> napi::Result<()> {
        let name = strict_host_text(&name)?;
        let source = strict_host_text(&source)?;
        let _busy = self.state.try_start()?;
        self.state.runtime.with_mut(|runtime| {
            let displaced = runtime.interp.begin_module(&name);
            match execute_module_source(&mut runtime.interp, &name, &source) {
                Ok(_) => {
                    runtime.interp.commit_module();
                    // Keep the source too, so a module registered eagerly can
                    // still take part in a cycle that `defineModule` links.
                    let source: Arc<str> = source.into();
                    runtime.interp.define_module_shared(&name, source.clone());
                    runtime.modules.insert(name, source);
                    Ok(())
                }
                Err(error) => {
                    runtime.interp.restore_module(&name, displaced);
                    Err(napi::Error::from_reason(error.to_string()))
                }
            }
        })
    }

    /// Register a module whose exports are host functions.
    ///
    /// This is the generic half of `exposeFunction` + `registerModule`: the
    /// core bridges each function to a hidden global and generates the wrapper
    /// module that re-exports it. What those functions *do* — including any
    /// permission checks — stays entirely on the host side.
    ///
    /// Returns the generated global names so the host can tear them down with
    /// `removeGlobal` when it removes the module.
    #[napi(
        ts_args_type = "name: string, exports: Record<string, Function>, options?: { async?: Array<string> }"
    )]
    pub fn register_host_module(
        &mut self,
        env: Env,
        name: napi::bindgen_prelude::Utf16String,
        exports: Object,
        options: Option<Object>,
    ) -> napi::Result<Vec<String>> {
        let name = strict_host_text(&name)?;
        let _busy = self.state.try_start()?;

        let async_names: Vec<String> = match options.as_ref() {
            Some(options) => options.get::<Vec<String>>("async")?.unwrap_or_default(),
            None => Vec::new(),
        };

        let keys = Object::keys(&exports)?;
        if keys.is_empty() {
            return Err(napi::Error::from_reason(format!(
                "registerHostModule: '{name}' must export at least one function"
            )));
        }

        let prefix = host_module_prefix(&name);
        let mut bindings: Vec<(String, sys::napi_value, bool)> = Vec::with_capacity(keys.len());
        let mut source = String::new();

        for key in keys {
            if !is_export_identifier(&key) {
                return Err(napi::Error::from_reason(format!(
                    "registerHostModule: '{key}' is not a usable export name"
                )));
            }
            let value: Unknown = exports.get_named_property_unchecked(&key)?;
            let raw = value.raw();
            let mut value_type: sys::napi_valuetype = 0;
            chk(unsafe { sys::napi_typeof(env.raw(), raw, &mut value_type) })
                .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            if value_type != sys::ValueType::napi_function {
                return Err(napi::Error::from_reason(format!(
                    "registerHostModule: export '{key}' must be a function"
                )));
            }

            let global = format!("{prefix}{key}");
            source.push_str(&format!(
                "export function {key}(...args) {{ return {global}(...args); }}\n"
            ));
            let is_async = async_names.iter().any(|entry| entry == &key);
            bindings.push((global, raw, is_async));
        }

        if let Some(unknown) = async_names
            .iter()
            .find(|entry| !source.contains(&format!("export function {entry}(")))
        {
            return Err(napi::Error::from_reason(format!(
                "registerHostModule: options.async names '{unknown}', which is not an export"
            )));
        }

        let state = self.state.clone();
        let globals = state
            .runtime
            .with_mut(|runtime| -> napi::Result<Vec<String>> {
                let bridge = Self::ensure_bridge(&state, runtime, env)
                    .map_err(|error| napi::Error::from_reason(error.to_string()))?;

                // Everything this call can touch, snapshotted first: a failure
                // part-way through must not leave half the capability swapped
                // in. Old bridge handles stay registered until the whole
                // operation succeeds, so a restored global is still callable.
                let previous_globals = runtime
                    .host_module_globals
                    .get(&name)
                    .cloned()
                    .unwrap_or_default();
                let prior_values: Vec<(String, Option<Value>)> = bindings
                    .iter()
                    .map(|(global, _, _)| (global.clone(), runtime.interp.global_value(global)))
                    .collect();
                let prior_module = runtime.interp.module(&name);
                let prior_source = runtime.modules.get(&name).cloned();

                // Phase 1 — register every callback. Nothing is visible yet.
                let mut new_ids = Vec::with_capacity(bindings.len());
                let mut outcome: napi::Result<()> = Ok(());
                for (_, raw, is_async) in &bindings {
                    let registered = if *is_async {
                        bridge.register_async(*raw)
                    } else {
                        bridge.register(*raw)
                    };
                    match registered {
                        Ok(id) => new_ids.push(id),
                        Err(error) => {
                            outcome = Err(napi::Error::from_reason(error.to_string()));
                            break;
                        }
                    }
                }

                // Phase 2 — install the globals.
                if outcome.is_ok() {
                    for ((global, _, _), id) in bindings.iter().zip(&new_ids) {
                        if let Err(error) = runtime
                            .interp
                            .set_global_checked(global, Value::host_function(global.as_str(), *id))
                        {
                            outcome = Err(napi::Error::from_reason(error.to_string()));
                            break;
                        }
                    }
                }

                // Phase 3 — evaluate the generated wrapper module.
                if outcome.is_ok() {
                    // `begin_module` starts from an empty export record, so an
                    // export dropped from this registration cannot survive in
                    // the old one. Its return value is the same snapshot as
                    // `prior_module`, which the rollback below already holds.
                    let _displaced = runtime.interp.begin_module(&name);
                    let result = execute_source(&mut runtime.interp, &source);
                    runtime.interp.commit_module();
                    outcome = result
                        .map(|_| ())
                        .map_err(|error| napi::Error::from_reason(error.to_string()));
                }

                if let Err(error) = outcome {
                    // Roll back to the snapshot, including bindings that had
                    // already been replaced by this call.
                    for (global, prior) in &prior_values {
                        match prior {
                            Some(value) => {
                                let _ = runtime.interp.set_global_checked(global, value.clone());
                            }
                            None => {
                                runtime.interp.persistent_global.borrow_mut().remove(global);
                            }
                        }
                    }
                    match prior_module {
                        Some(module) => runtime
                            .interp
                            .modules
                            .borrow_mut()
                            .insert(name.clone(), module),
                        None => runtime.interp.modules.borrow_mut().remove(&name),
                    };
                    match prior_source {
                        Some(source) => runtime.modules.insert(name.clone(), source),
                        None => runtime.modules.remove(&name),
                    };
                    for id in new_ids {
                        bridge.unregister(id);
                    }
                    return Err(error);
                }

                // Committed: retire the handles the replaced globals owned.
                for (_, prior) in &prior_values {
                    if let Some(id) = prior.as_ref().and_then(Value::host_function_id) {
                        bridge.unregister(id);
                    }
                }

                // An export that disappeared must lose its bridge global, or a
                // privileged function stays callable after the host drops it.
                let created: Vec<String> = bindings
                    .iter()
                    .map(|(global, _, _)| global.clone())
                    .collect();
                for stale in previous_globals.iter().filter(|g| !created.contains(g)) {
                    Self::revoke_global(runtime, stale);
                }

                runtime
                    .host_module_globals
                    .insert(name.clone(), created.clone());
                runtime.modules.insert(name, source.into());
                Ok(created)
            })?;

        Ok(globals)
    }

    /// Release the host resources this VM holds, deterministically.
    ///
    /// A VM that has run `runAsync` owns a threadsafe-function for dispatching
    /// host calls, and that handle keeps the N-API environment -- and so the
    /// Node process -- alive. Dropping the `Vm` releases it, but a `Vm` held in
    /// a module-level binding is never dropped, so a script that used
    /// `runAsync` would hang at exit instead of returning to the shell.
    ///
    /// Calling this is idempotent and safe while an async worker is still in
    /// flight: handles are marked retired and the last in-flight callback
    /// releases them. After it returns, host functions are no longer callable
    /// from guest code.
    #[napi]
    pub fn dispose(&mut self) {
        if let Some(bridge) = self
            .state
            .bridge_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
        {
            bridge.shutdown_on_main();
        }
    }

    #[napi]
    pub fn evaluation_stats(&self) -> napi::Result<String> {
        let _busy = self.state.try_start()?;
        Ok(self
            .state
            .runtime
            .with_mut(|r| r.interp.evaluation_diagnostics()))
    }

    /// Collect unreachable cycles at a quiescent VM boundary.
    #[napi]
    pub fn collect_cycles(&self) -> napi::Result<u32> {
        let _busy = self.state.try_start()?;
        Ok(self
            .state
            .runtime
            .with_mut(|r| r.interp.collect_cycles().collected.min(u32::MAX as usize) as u32))
    }
    #[napi]
    pub fn heap_stats(&self) -> napi::Result<String> {
        let _busy = self.state.try_start()?;
        Ok(self.state.runtime.with_mut(|r| {
            let stats = r.interp.runtime_stats();
            format!(
                "{{\"tracked\":{},\"collectedTotal\":{}}}",
                stats.heap_tracked, stats.heap_collected_total
            )
        }))
    }

    #[napi]
    pub fn set_import_meta_main(&mut self, is_main: bool) -> napi::Result<()> {
        let _busy = self.state.try_start()?;
        self.state.runtime.with_mut(|runtime| {
            runtime.interp.is_main = is_main;
        });
        Ok(())
    }

    /// Cap the number of loop iterations in a single execution.
    #[napi]
    pub fn set_loop_limit(&mut self, n: u32) -> napi::Result<()> {
        let _busy = self.state.try_start()?;
        self.state
            .runtime
            .with_mut(|runtime| runtime.interp.set_loop_budget(n as u64));
        Ok(())
    }

    #[napi]
    pub fn get_global(&self, name: napi::bindgen_prelude::Utf16String) -> napi::Result<String> {
        let name = crate::JsString::from_units(name.to_vec()).to_key();
        let _busy = self.state.try_start()?;
        self.state
            .runtime
            .with_mut(|runtime| -> napi::Result<String> {
                Ok(runtime
                    .interp
                    .global_value(&name)
                    .map(|value| try_to_string(&value))
                    .transpose()
                    .map_err(|error| napi::Error::from_reason(error.to_string()))?
                    .unwrap_or_else(|| "undefined".to_string()))
            })
    }

    #[napi]
    pub fn set_global(
        &mut self,
        env: Env,
        name: napi::bindgen_prelude::Utf16String,
        value: Unknown,
    ) -> napi::Result<()> {
        let name = crate::JsString::from_units(name.to_vec()).to_key();
        let _busy = self.state.try_start()?;
        self.state.runtime.with_mut(|runtime| -> napi::Result<()> {
            let value = from_napi(env.raw(), value.raw())
                .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            if let Some(id) = runtime
                .interp
                .global_value(&name)
                .and_then(|value| value.host_function_id())
                && let Some(bridge) = Self::current_bridge(runtime)
            {
                bridge.unregister(id);
            }
            runtime
                .interp
                .set_global_checked(&name, value)
                .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            Ok(())
        })
    }

    #[napi]
    pub fn expose_function(
        &mut self,
        env: Env,
        name: napi::bindgen_prelude::Utf16String,
        func: Unknown,
    ) -> napi::Result<()> {
        self.expose_function_inner(
            env,
            crate::JsString::from_units(name.to_vec()).to_key(),
            func,
            false,
        )
    }

    #[napi]
    pub fn expose_async_function(
        &mut self,
        env: Env,
        name: napi::bindgen_prelude::Utf16String,
        func: Unknown,
    ) -> napi::Result<()> {
        self.expose_function_inner(
            env,
            crate::JsString::from_units(name.to_vec()).to_key(),
            func,
            true,
        )
    }

    fn expose_function_inner(
        &mut self,
        env: Env,
        name: String,
        func: Unknown,
        async_fn: bool,
    ) -> napi::Result<()> {
        let _busy = self.state.try_start()?;
        let raw = func.raw();
        let mut value_type: sys::napi_valuetype = 0;
        chk(unsafe { sys::napi_typeof(env.raw(), raw, &mut value_type) })
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        if value_type != sys::ValueType::napi_function {
            return Err(napi::Error::from_reason(format!(
                "{}: '{}' must be a function",
                if async_fn {
                    "exposeAsyncFunction"
                } else {
                    "exposeFunction"
                },
                name
            )));
        }

        let state = self.state.clone();
        state.runtime.with_mut(|runtime| {
            Self::bind_host_function(&state, runtime, env, &name, raw, async_fn)
        })
    }

    /// Bridge one Node function into the interpreter as a global.
    ///
    /// Must be called while the runtime gate is held; callers own the busy
    /// guard so this can be used repeatedly inside one gated operation.
    fn bind_host_function(
        state: &Arc<VMState>,
        runtime: &mut VmRuntime,
        env: Env,
        name: &str,
        raw: sys::napi_value,
        async_fn: bool,
    ) -> napi::Result<()> {
        if let Some(id) = runtime
            .interp
            .global_value(name)
            .and_then(|value| value.host_function_id())
            && let Some(bridge) = Self::current_bridge(runtime)
        {
            bridge.unregister(id);
        }
        let bridge = Self::ensure_bridge(state, runtime, env)
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        let id = if async_fn {
            bridge.register_async(raw)
        } else {
            bridge.register(raw)
        }
        .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        runtime
            .interp
            .set_global_checked(name, Value::host_function(name, id))
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        Ok(())
    }

    /// Execute code that may await host functions. The worker captures only
    /// the `Arc<VMState>`; the interpreter itself remains under `RuntimeCell`'s
    /// mutex and is never accessed concurrently with a Node method.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn run_async(
        &mut self,
        env: Env,
        source: napi::bindgen_prelude::Utf16String,
    ) -> napi::Result<Unknown<'_>> {
        let source = crate::JsString::from_units(source.to_vec());
        let busy = self.state.try_start()?;
        let raw_env = env.raw();

        let mut deferred: sys::napi_deferred = ptr::null_mut();
        let mut promise = ptr::null_mut();
        chk(unsafe { sys::napi_create_promise(raw_env, &mut deferred, &mut promise) })
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;

        let mut done_tsfn: sys::napi_threadsafe_function = ptr::null_mut();
        let tsfn_name = make_str(raw_env, "vm-run-async-done")
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        chk(unsafe {
            sys::napi_create_threadsafe_function(
                raw_env,
                ptr::null_mut(),
                ptr::null_mut(),
                tsfn_name,
                1,
                1,
                ptr::null_mut(),
                None,
                deferred as *mut std::ffi::c_void,
                Some(run_async_done_cb),
                &mut done_tsfn,
            )
        })
        .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        // Keep the completion TSFN referenced until the worker releases it.
        // `runAsync` must keep Node alive even when the guest performs no
        // host I/O; unref'ing this handle here lets a process exit before its
        // returned Promise is settled.

        let state = self.state.clone();
        let prepared = state.runtime.with_mut(|runtime| {
            if let Some(bridge) = runtime.bridge.as_ref() {
                bridge
                    .prepare_for_async()
                    .map_err(|error| napi::Error::from_reason(error.to_string()))?;
                bridge.on_vm_thread.store(1, Ordering::Release);
            }
            Ok::<(), napi::Error>(())
        });
        if let Err(error) = prepared {
            reject_deferred_now(raw_env, deferred, error.to_string());
            let _ = unsafe {
                sys::napi_release_threadsafe_function(
                    done_tsfn,
                    sys::ThreadsafeFunctionReleaseMode::release,
                )
            };
            return Err(error);
        }

        let done_handle = done_tsfn as usize;
        let source_for_worker = source.clone();
        let worker_state = state.clone();
        let spawn = state.dispatch_async(Box::new(move || {
            let _busy = busy;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                worker_state.runtime.with_mut(|runtime| {
                    let result = match execute_source_utf16(&mut runtime.interp, &source_for_worker)
                    {
                        Ok(value) => async_result_string(value),
                        Err(error) => Err(runtime.interp.enrich_error(error, None).to_string()),
                    };
                    runtime.interp.maybe_collect_cycles();
                    result
                })
            }))
            .unwrap_or_else(|_| Err("Error: VM execution panicked".to_string()));

            worker_state.runtime.with_mut(|runtime| {
                if let Some(bridge) = runtime.bridge.as_ref() {
                    bridge.finish_async_worker();
                }
            });

            // Completion may immediately resume Node and admit the next operation.
            // Release admission only after guest access and bridge cleanup finish.
            drop(_busy);
            let message = Box::new(result);
            let raw_message = Box::into_raw(message) as *mut std::ffi::c_void;
            let tsfn = done_handle as sys::napi_threadsafe_function;
            let status = unsafe {
                sys::napi_call_threadsafe_function(
                    tsfn,
                    raw_message,
                    sys::ThreadsafeFunctionCallMode::nonblocking,
                )
            };
            if status != sys::Status::napi_ok {
                drop(unsafe { Box::from_raw(raw_message as *mut Result<String, String>) });
            }
            let release_status = unsafe {
                sys::napi_release_threadsafe_function(
                    tsfn,
                    sys::ThreadsafeFunctionReleaseMode::release,
                )
            };
            if release_status != sys::Status::napi_ok {
                // The environment is already closing; the returned
                // Promise cannot be observed after teardown.
            }
        }));

        if let Err(error) = spawn {
            state.runtime.with_mut(|runtime| {
                if let Some(bridge) = runtime.bridge.as_ref() {
                    bridge.finish_async_worker();
                }
            });
            reject_deferred_now(
                raw_env,
                deferred,
                format!("failed to spawn VM thread: {}", error),
            );
            let _ = unsafe {
                sys::napi_release_threadsafe_function(
                    done_tsfn,
                    sys::ThreadsafeFunctionReleaseMode::release,
                )
            };
            return Err(napi::Error::from_reason(error.to_string()));
        }

        Ok(unsafe { Unknown::from_raw_unchecked(raw_env, promise) })
    }

    /// Unregister a module, making it unresolvable to `import` and revoking
    /// any bridge globals a `registerHostModule` created for it.
    ///
    /// This is the capability-revocation primitive: after it returns, guest
    /// code that imports `name` gets `Module not found`, and host functions
    /// the module exported are no longer reachable through any binding it
    /// installed. Bindings a *previous* execution already imported into a
    /// global keep the value they captured -- revocation applies to
    /// resolution, not to references the guest already holds.
    ///
    /// Returns whether anything was registered under `name`.
    #[napi]
    pub fn remove_module(
        &mut self,
        name: napi::bindgen_prelude::Utf16String,
    ) -> napi::Result<bool> {
        let name = strict_host_text(&name)?;
        let _busy = self.state.try_start()?;
        Ok(self.state.runtime.with_mut(|runtime| {
            // Two registries have to move together. `runtime.modules` is the
            // source bookkeeping the public API reports on, but `import`
            // resolves through the interpreter's export table -- dropping only
            // the first leaves `hasModule` answering false for a module the
            // guest can still import.
            let removed_source = runtime.modules.remove(&name).is_some();
            let removed_exports = runtime.interp.remove_module(&name);
            let removed = removed_source || removed_exports;
            // A host module's capability is its bridge globals, not the wrapper
            // source: removing the module must revoke them too.
            if let Some(globals) = runtime.host_module_globals.remove(&name) {
                for global in globals {
                    Self::revoke_global(runtime, &global);
                }
            }
            removed
        }))
    }

    /// Whether `name` is registered. This never disagrees with what `import`
    /// can resolve: registration and removal move the source registry and the
    /// interpreter's export table together.
    #[napi]
    pub fn has_module(&self, name: napi::bindgen_prelude::Utf16String) -> napi::Result<bool> {
        let name = strict_host_text(&name)?;
        let _busy = self.state.try_start()?;
        Ok(self
            .state
            .runtime
            .with_mut(|runtime| runtime.modules.contains_key(&name)))
    }

    #[napi]
    pub fn list_modules(&self) -> napi::Result<Vec<String>> {
        let _busy = self.state.try_start()?;
        Ok(self
            .state
            .runtime
            .with_mut(|runtime| runtime.modules.keys().cloned().collect()))
    }

    #[napi]
    pub fn remove_global(
        &mut self,
        name: napi::bindgen_prelude::Utf16String,
    ) -> napi::Result<bool> {
        let name = crate::JsString::from_units(name.to_vec()).to_key();
        let _busy = self.state.try_start()?;
        Ok(self
            .state
            .runtime
            .with_mut(|runtime| Self::revoke_global(runtime, &name)))
    }

    #[napi]
    pub fn has_global(&self, name: napi::bindgen_prelude::Utf16String) -> napi::Result<bool> {
        let name = crate::JsString::from_units(name.to_vec()).to_key();
        let _busy = self.state.try_start()?;
        Ok(self
            .state
            .runtime
            .with_mut(|runtime| runtime.interp.global_value(&name).is_some()))
    }

    #[napi]
    pub fn call_function(
        &mut self,
        env: Env,
        name: napi::bindgen_prelude::Utf16String,
        args: Vec<Unknown>,
    ) -> napi::Result<Unknown<'_>> {
        let name = crate::JsString::from_units(name.to_vec()).to_key();
        let _busy = self.state.try_start()?;
        let raw_env = env.raw();
        if args.len() > crate::value::MAX_ARRAY_LEN {
            return Err(napi::Error::from_reason(
                "RangeError: Maximum argument count exceeded",
            ));
        }
        let state = self.state.clone();
        let result = self.state.runtime.with_mut(|runtime| {
            let mut vm_args = Vec::with_capacity(args.len());
            for arg in &args {
                vm_args.push(
                    from_napi(raw_env, arg.raw())
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?,
                );
            }
            let callee = runtime.interp.global_value(&name).ok_or_else(|| {
                napi::Error::from_reason(format!("callFunction: '{}' is not defined", name))
            })?;
            runtime.interp.begin_execution();
            let result = runtime
                .interp
                .call_this(&callee, Value::Undefined, vm_args)
                .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            // Run the event loop before the value crosses out, so a promise
            // the call produced is settled by the time the host sees it.
            runtime
                .interp
                .drain_jobs()
                .map_err(|error| napi::Error::from_reason(error.to_string()))?;
            Ok::<Value, napi::Error>(result)
        })?;
        // Marshalled *outside* the runtime gate: exporting a function needs
        // the gate itself, to record the value in the runtime's export table.
        // The `BusyGuard` above still excludes every other execution, so
        // nothing can touch the interpreter while the value is read.
        let out = super::marshal::exporting_from(&state, || to_napi(raw_env, &result))
            .map_err(|error| napi::Error::from_reason(error.to_string()))?;
        Ok(unsafe { Unknown::from_raw_unchecked(raw_env, out) })
    }
}

/// Run a module body in the module's own top-level scope.
///
/// A module's declarations belong to the module, not to the global object, so
/// the interpreter's current scope is swapped for the module's while the body
/// runs and restored afterwards — on the error path too, or a failed
/// registration would leave the VM evaluating in a module scope.
fn execute_module_source(
    interp: &mut Interpreter,
    name: &str,
    source: &str,
) -> Result<Value, VmErr> {
    let scope = interp.module_scope(name);
    let outer = interp.take_scope(scope);
    let result = interp.eval_module_with_options(
        source,
        crate::interpreter::EvaluationOptions {
            resume_pending_checkpoint: true,
            ..Default::default()
        },
    );
    interp.take_scope(outer);
    result
}

pub(super) fn execute_source(interp: &mut Interpreter, source: &str) -> Result<Value, VmErr> {
    interp.eval_source_with_options(
        source,
        crate::interpreter::EvaluationOptions {
            resume_pending_checkpoint: true,
            ..Default::default()
        },
    )
}

pub(super) fn execute_source_utf16(
    interp: &mut Interpreter,
    source: &crate::JsString,
) -> Result<Value, VmErr> {
    interp.eval_utf16_with_options(
        source,
        crate::interpreter::EvaluationOptions {
            resume_pending_checkpoint: true,
            ..Default::default()
        },
    )
}

/// Parse module source without touching interpreter state.
fn validate_module_source(source: &crate::JsString) -> ValidationResult {
    let tokens = Lexer::from_js_string(source).tokenize_with_spans();
    let mut parser = Parser::new_with_spans(tokens);
    match parser.parse_program() {
        Ok(_) => ValidationResult {
            valid: true,
            diagnostics: Vec::new(),
        },
        Err(error) => {
            let (kind, message) = if parser.depth_exceeded {
                ("parse-limit", "Maximum parse depth exceeded".to_string())
            } else {
                ("syntax", error.message)
            };
            ValidationResult {
                valid: false,
                diagnostics: vec![ValidationDiagnostic {
                    line: u32::try_from(error.span.line).unwrap_or(u32::MAX),
                    column: u32::try_from(error.span.col).unwrap_or(u32::MAX),
                    message,
                    kind: kind.to_string(),
                }],
            }
        }
    }
}

pub(super) fn async_result_string(value: Value) -> Result<String, String> {
    match &value {
        Value::Promise(inner) => {
            let inner = inner.borrow();
            let rendered = try_to_string(&inner.value).map_err(|e| e.to_string());
            match inner.state {
                PromiseState::Rejected => Err(rendered.unwrap_or_else(|e| e)),
                // A promise still pending after the event loop drained can
                // never settle: nothing is left to settle it.
                PromiseState::Pending => Err("promise never settled".to_string()),
                PromiseState::Fulfilled => rendered,
            }
        }
        _ => try_to_string(&value).map_err(|e| e.to_string()),
    }
}

pub(super) fn reject_deferred_now(
    env: sys::napi_env,
    deferred: sys::napi_deferred,
    message: String,
) {
    let Ok(js_message) = make_str(env, &message) else {
        return;
    };
    let mut js_error = ptr::null_mut();
    let error_status =
        unsafe { sys::napi_create_error(env, ptr::null_mut(), js_message, &mut js_error) };
    if error_status == sys::Status::napi_ok {
        let reject_status = unsafe { sys::napi_reject_deferred(env, deferred, js_error) };
        if reject_status != sys::Status::napi_ok {
            // The environment may already be closing; there is no safe
            // follow-up operation for this deferred.
        }
    }
}

#[napi]
pub fn create_vm() -> VM {
    VM::new()
}

#[napi]
pub fn run_code(source: napi::bindgen_prelude::Utf16String) -> napi::Result<String> {
    let text = crate::JsString::from_units(source.to_vec());
    match text.to_utf8() {
        Ok(text) => {
            run_source(&text, false).map_err(|error| napi::Error::from_reason(error.to_string()))
        }
        Err(_) => VM::new().run(source),
    }
}

#[napi]
pub fn debug_parse(source: napi::bindgen_prelude::Utf16String) -> napi::Result<String> {
    let source = crate::JsString::from_units(source.to_vec());
    let mut lexer = Lexer::from_js_string(&source);
    let tokens = lexer.tokenize_with_spans();
    let mut parser = Parser::new_with_spans(tokens);
    let statements = parser.parse();
    if parser.depth_exceeded {
        return Err(napi::Error::from_reason(
            "RangeError: Maximum parse depth exceeded",
        ));
    }
    Ok(format!("{:?}", statements))
}

// Exercise the production gate and detached arena without creating a Node
// environment or calling any N-API function. The same suite runs under Miri.
#[cfg(test)]
mod owner_migration_tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn owner_migration_empty_registry_reuse_is_isolated() {
        fn first_symbol(context: &mut crate::runtime::OwnerContext) -> u64 {
            let _lease = context.enter();
            let mut interp = Interpreter::with_builtins();
            let value = interp.eval_source("Symbol('first')").unwrap();
            let Value::Symbol(ref symbol) = value else {
                panic!("symbol")
            };
            let id = symbol.id;
            drop(value);
            interp.global.borrow_mut().clear_edges();
            drop(interp);
            assert_eq!(crate::heap::collect_after_interpreter_drop().skipped, None);
            id
        }
        // Initialize the ambient shape arena before taking its counters: a
        // first lease otherwise initializes TLS while installing the owner.
        let outer_shape = shape_identity();
        let outer_heap = crate::heap::counters();
        let outer_shapes = crate::shape::created_count();
        let expected = first_symbol(&mut crate::runtime::OwnerContext::default());
        assert_eq!(
            run_source(
                "var privateName=42;Map.prototype.extra=42;for(var i=0;i<50;i++)Symbol('old');42;",
                false
            )
            .unwrap(),
            "42"
        );
        let mut recycled = EMPTY_OWNER
            .with(|slot| slot.borrow_mut().take())
            .expect("empty registry retained");
        assert_eq!(first_symbol(&mut recycled), expected);
        assert_eq!(
            run_source("typeof privateName;", false).unwrap(),
            "undefined"
        );
        assert_eq!(
            run_source("Map.prototype.extra;", false).unwrap(),
            "undefined"
        );
        assert_eq!(crate::heap::counters(), outer_heap);
        assert_eq!(crate::shape::created_count(), outer_shapes);
        assert_eq!(shape_identity(), outer_shape);
    }

    #[test]
    fn owner_migration_prepared_templates_reset_feedback() {
        let source = "function f(o){return o.x;} var o={x:42}; f(o);f(o);f(o);";
        let mut cache = FreshProgramCache::default();
        let first = cache.prepare(source).unwrap();
        assert_eq!(first.tier(), crate::interpreter::ExecutionTier::Bytecode);
        let mut owner = crate::runtime::OwnerContext::default();
        {
            let _lease = owner.enter();
            let mut interp = Interpreter::with_builtins();
            interp.set_tier_tracking(crate::jit::TierTracking::CountersOnly);
            assert!(matches!(
                interp.execute(&first).unwrap(),
                Value::Number(42.)
            ));
            assert!(first.stats().unwrap().calls > 0);
            assert!(first.stats().unwrap().ic_hits > 0);
            interp.global.borrow_mut().clear_edges();
            drop(interp);
            assert_eq!(crate::heap::collect_after_interpreter_drop().skipped, None);
        }
        let second = cache.prepare(source).unwrap();
        let stats = second.stats().unwrap();
        assert_eq!(stats.calls, 0);
        assert_eq!(stats.loop_iters, 0);
        assert_eq!(stats.ic_hits, 0);
        assert_eq!(stats.ic_misses, 0);
        assert_eq!(stats.compiled, 0);
        assert_eq!(
            cache.programs.get(source).unwrap().0.stats().unwrap().calls,
            0
        );
        assert_eq!(
            run_source("Map.prototype.marker=42;42;", false).unwrap(),
            "42"
        );
        assert_eq!(
            run_source("Map.prototype.marker;", false).unwrap(),
            "undefined"
        );
        assert_eq!(run_source(source, false).unwrap(), "42");
        assert_eq!(run_source(source, false).unwrap(), "42");
        let scalar = "function f(){return 42;} f();";
        let shared = cache.prepare_for(scalar, true).unwrap();
        let tracked = cache.prepare_for(scalar, false).unwrap();
        {
            let _lease = owner.enter();
            let mut interp = Interpreter::with_builtins();
            assert!(interp.feedback_disabled());
            assert!(matches!(
                interp.execute(&shared).unwrap(),
                Value::Number(42.)
            ));
            assert_eq!(shared.stats().unwrap().calls, 0);
            interp.set_tier_tracking(crate::jit::TierTracking::CountersOnly);
            assert!(matches!(
                interp.execute(&tracked).unwrap(),
                Value::Number(42.)
            ));
            assert!(tracked.stats().unwrap().calls > 0);
            assert_eq!(shared.stats().unwrap().calls, 0);
            assert_eq!(
                cache.programs.get(scalar).unwrap().0.stats().unwrap().calls,
                0
            );
            interp.global.borrow_mut().clear_edges();
            drop(interp);
            assert_eq!(crate::heap::collect_after_interpreter_drop().skipped, None);
        }
        for index in 0..70 {
            cache.prepare(&format!("{index};")).unwrap();
        }
        assert_eq!(cache.programs.len(), 64);
        assert_eq!(cache.order.len(), 64);
        assert!(!cache.programs.contains_key(source));
        assert_eq!(
            cache.bytes,
            cache.order.iter().map(|s| s.len()).sum::<usize>()
        );
    }

    fn shape_identity() -> usize {
        crate::shape::root_identity()
    }
    fn arena_identity(runtime: &mut VmRuntime) -> (usize, usize, usize, usize) {
        let symbol = runtime
            .interp
            .eval_source("Symbol.for('owner-probe')")
            .unwrap();
        let Value::Symbol(ref symbol) = symbol else {
            panic!("symbol")
        };
        let proto = runtime
            .interp
            .eval_source("Object.getPrototypeOf(new Map())")
            .unwrap();
        let Value::Object { ref props } = proto else {
            panic!("prototype")
        };
        (
            shape_identity(),
            Rc::as_ptr(symbol) as usize,
            Rc::as_ptr(props) as usize,
            crate::heap::pin_count(),
        )
    }
    #[test]
    fn owner_migration_thread_handoffs_and_drop() {
        let outer_shape = shape_identity();
        let outer_shapes_created = crate::shape::created_count();
        let outer_heap = crate::heap::counters();
        let state = VM::new_state();
        let (expected, pinned) = state.with_runtime(|runtime| {
            let object = runtime
                .interp
                .eval_source("var rooted={answer:42};rooted.self=rooted;rooted;")
                .unwrap();
            let pinned = runtime.exports.insert(object);
            runtime.interp.eval_source("rooted=undefined;").unwrap();
            (arena_identity(runtime), pinned)
        });
        assert_eq!(shape_identity(), outer_shape);
        assert_eq!(crate::shape::created_count(), outer_shapes_created);
        assert_eq!(crate::heap::counters(), outer_heap);
        for _ in 0..8 {
            let transferred = state.clone();
            std::thread::spawn(move || {
                let outer = shape_identity();
                let outer_count = crate::shape::created_count();
                transferred.with_runtime(|runtime| {
                    assert_eq!(arena_identity(runtime), expected);
                    assert_eq!(runtime.interp.collect_cycles().skipped, None);
                    assert!(matches!(
                        runtime.export(pinned).unwrap().get_prop("answer"),
                        Some(Value::Number(42.))
                    ));
                });
                assert_eq!(shape_identity(), outer);
                assert_eq!(crate::shape::created_count(), outer_count);
            })
            .join()
            .unwrap();
            assert_eq!(state.with_runtime(arena_identity), expected);
            let transferred = state.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            state
                .dispatch_async(Box::new(move || {
                    tx.send(transferred.with_runtime(arena_identity)).unwrap();
                }))
                .unwrap();
            assert_eq!(rx.recv().unwrap(), expected);
        }
        // No bridge exists: destruction here exercises only Rust state.
        std::thread::spawn(move || drop(state)).join().unwrap();
        assert_eq!(shape_identity(), outer_shape);
        assert_eq!(crate::shape::created_count(), outer_shapes_created);
        assert_eq!(crate::heap::counters(), outer_heap);
    }
    #[test]
    fn owner_migration_panic_nested_contexts_restore_tls() {
        let mut ambient = crate::runtime::OwnerContext::default();
        let _ambient = ambient.enter();
        let ambient_shapes_created = crate::shape::created_count();
        let first = VM::new_state();
        let second = VM::new_state();
        assert_eq!(crate::shape::created_count(), ambient_shapes_created);
        let first_identity = first.with_runtime(arena_identity);
        let second_identity = second.with_runtime(arena_identity);
        assert_ne!(first_identity.0, second_identity.0);
        assert_ne!(first_identity.1, second_identity.1);
        assert_ne!(first_identity.2, second_identity.2);
        let ambient_shape = shape_identity();
        let ambient_heap = crate::heap::counters();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            first.with_runtime(|runtime| {
                assert_eq!(arena_identity(runtime), first_identity);
                second.with_runtime(|runtime| {
                    assert_eq!(arena_identity(runtime), second_identity);
                    panic!("lease probe");
                });
            });
        }));
        assert!(panic.is_err());
        assert_eq!(shape_identity(), ambient_shape);
        assert_eq!(crate::shape::created_count(), ambient_shapes_created);
        assert_eq!(crate::heap::counters(), ambient_heap);
        assert_eq!(first.with_runtime(arena_identity), first_identity);
        assert_eq!(second.with_runtime(arena_identity), second_identity);
    }
}

#[cfg(test)]
mod runtime_profile {
    use super::*;
    #[test]
    #[ignore = "serialized diagnostic profile; run with --ignored --nocapture"]
    fn runtime_phase_profile() {
        for (name, source) in [
            ("tiny", "function f(x,y){return x+y;}f(20,22);"),
            ("arithmetic", "var n=0;for(var i=0;i<1000;i++)n+=i;n;"),
        ] {
            let mut nanos = [0_u128; 6];
            for iteration in 0..1050 {
                let started = std::time::Instant::now();
                let mut context = crate::runtime::OwnerContext::default();
                let lease = context.enter();
                let owner = started.elapsed().as_nanos();
                let started = std::time::Instant::now();
                let mut interp = Interpreter::with_builtins();
                let builtins = started.elapsed().as_nanos();
                let started = std::time::Instant::now();
                let program = FRESH_PROGRAMS
                    .with(|cache| {
                        cache
                            .borrow_mut()
                            .prepare_for(source, interp.feedback_disabled())
                    })
                    .unwrap();
                let value = interp.execute(&program).unwrap();
                let execution = started.elapsed().as_nanos();
                let started = std::time::Instant::now();
                std::hint::black_box(try_to_string(&value).unwrap());
                drop(value);
                interp.global.borrow_mut().clear_edges();
                drop(interp);
                let teardown = started.elapsed().as_nanos();
                let started = std::time::Instant::now();
                crate::heap::collect_after_interpreter_drop();
                let collection = started.elapsed().as_nanos();
                let started = std::time::Instant::now();
                drop(lease);
                drop(context);
                let detach = started.elapsed().as_nanos();
                if iteration >= 50 {
                    for (total, sample) in nanos
                        .iter_mut()
                        .zip([owner, builtins, execution, teardown, collection, detach])
                    {
                        *total += sample;
                    }
                }
            }
            println!(
                "{}",
                serde_json::json!({"profile": "runCode", "workload":name,"operations":1000,"phase_ns":nanos})
            );
        }
        let state = VM::new_state();
        let started = std::time::Instant::now();
        for _ in 0..100_000 {
            state.with_runtime(|runtime| {
                std::hint::black_box(runtime.interp.prepared_cache_stats())
            });
        }
        println!(
            "{}",
            serde_json::json!({"profile":"runtime_lease", "operations":100000,"elapsed_ns":started.elapsed().as_nanos()})
        );
        let program = Interpreter::compile("var n=0;for(var i=0;i<1000;i++)n+=i;n;").unwrap();
        let started = std::time::Instant::now();
        for _ in 0..100_000 {
            std::hint::black_box(program.clone());
        }
        println!(
            "{}",
            serde_json::json!({"profile":"prepared_clone", "operations":100000,"elapsed_ns":started.elapsed().as_nanos()})
        );
    }
}

fn strict_host_text(text: &napi::bindgen_prelude::Utf16String) -> napi::Result<String> {
    String::from_utf16(text).map_err(|_| {
        napi::Error::from_reason(
            "UTF-8 module source and identifier contracts do not support unpaired surrogates",
        )
    })
}
