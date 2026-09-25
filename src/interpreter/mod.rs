pub(crate) mod async_fn;
pub(crate) mod call;
pub mod commonjs;
mod env;
mod eval;
pub mod jobs;
#[cfg(not(target_arch = "wasm32"))]
pub mod native_addon;
#[cfg(not(target_arch = "wasm32"))]
mod native_addon_binary;
#[cfg(not(target_arch = "wasm32"))]
pub mod node_addon;
mod ops;
mod promise;
mod resolve;
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub mod rust_node_api;

#[cfg(stackful_coroutines)]
pub use async_fn::AsyncTask;
pub use commonjs::{
    CommonJsModuleFormat, CommonJsModuleLoader, FileCommonJsLoader, NativeAddonLoader,
    ResolvedCommonJsModule,
};
pub use env::{AssignOutcome, BindKind, Env, Environment, Lookup, ModifyOutcome, Module};
#[cfg(not(target_arch = "wasm32"))]
pub use native_addon::{
    NativeAddonBackendHost, NativeAddonOptions, NativeAddonPolicy, NativeAddonRuntime,
};
#[cfg(not(target_arch = "wasm32"))]
pub use node_addon::{NodeAddonOptions, NodeAddonRuntimeInfo, NodeAddonSidecar};
pub(crate) use resolve::array_iter;
#[cfg(all(
    feature = "node-api-host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
pub use rust_node_api::{ReportedNodeVersion, RustNodeApiHost, RustNodeApiOptions};

/// The state a generator or async body must share with the interpreter that
/// started it: the one event loop, and the one module registry.
///
/// Those bodies run on their own stack with their own `Interpreter`, so
/// without an explicit hand-off a promise settled inside one would schedule
/// reactions nobody drains, and an `import` inside one would resolve against
/// an empty registry.
#[derive(Clone)]
pub struct Realm {
    jobs: Jobs,
    modules: Rc<RefCell<HashMap<String, Module>>>,
    module_sources: Rc<RefCell<HashMap<String, String>>>,
    module_aliases: Rc<RefCell<HashMap<(String, String), String>>>,
    module_file_urls: Rc<RefCell<HashMap<String, String>>>,
    evaluating: Rc<RefCell<std::collections::HashSet<String>>>,
    commonjs_loader: Option<Rc<dyn CommonJsModuleLoader>>,
    commonjs_cache: Rc<RefCell<HashMap<String, commonjs::CommonJsCacheEntry>>>,
    commonjs_entry: Option<String>,
}

impl Realm {
    pub fn of(interp: &Interpreter) -> Self {
        Self {
            jobs: interp.jobs.clone(),
            modules: interp.modules.clone(),
            module_sources: interp.module_sources.clone(),
            module_aliases: interp.module_aliases.clone(),
            module_file_urls: interp.module_file_urls.clone(),
            evaluating: interp.evaluating.clone(),
            commonjs_loader: interp.commonjs_loader.clone(),
            commonjs_cache: interp.commonjs_cache.clone(),
            commonjs_entry: interp.commonjs_entry.clone(),
        }
    }

    pub fn install(self, interp: &mut Interpreter) {
        interp.jobs = self.jobs;
        interp.modules = self.modules;
        interp.module_sources = self.module_sources;
        interp.module_aliases = self.module_aliases;
        interp.module_file_urls = self.module_file_urls;
        interp.evaluating = self.evaluating;
        interp.commonjs_loader = self.commonjs_loader;
        interp.commonjs_cache = self.commonjs_cache;
        interp.commonjs_entry = self.commonjs_entry;
    }
}
pub use jobs::{Job, JobQueue, Jobs};
pub use ops::{
    SYMBOL_ITERATOR_SLOT, is_internal_key, strict_equals, symbol_id_from_slot, symbol_slot_key,
};

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::error::{StackFrame, VmErr};
use crate::host::HostBridge;
use crate::parser::{Statement, VarKind, collect_var_names, pattern_names};
use crate::span::Span;
use crate::value::Value;

/// Maximum number of VM call frames (guest-visible recursion depth). Each VM
/// call maps onto several native Rust frames in the tree-walker. Recursive
/// calls grow the native stack in guarded segments before exhausting the
/// host-provided stack, while this cap bounds total guest recursion and turns
/// runaway calls into a catchable `RangeError`.
pub const MAX_CALL_DEPTH: usize = 256;

/// Maximum depth of *nested generator bodies* currently executing.
///
/// A generator body runs on its own coroutine stack with its own
/// `Interpreter`, so its call stack starts empty and `MAX_CALL_DEPTH` never
/// sees recursion that goes through generators. `function* g() { yield* g(); }`
/// therefore recursed until some other limit tripped, allocating an 8 MiB
/// stack per level on the way -- seconds of work for a program that should
/// fail immediately. This bounds that directly.
pub const MAX_GENERATOR_DEPTH: u32 = 64;

/// Default cap on loop iterations per top-level execution (`vm.run`,
/// `registerModule`, `callFunction`). The interpreter is synchronous and has
/// no preemption, so `while (true) {}` would otherwise freeze the host event
/// loop forever; this budget turns it into a catchable `RangeError`.
/// 100M iterations is far above any legitimate computation (the benchmark
/// workloads stay under a few million) while still stopping an empty
/// infinite loop within a couple of seconds.
pub const DEFAULT_LOOP_BUDGET: u64 = 100_000_000;

pub struct Interpreter {
    pub global: Env,
    /// Persistent user-global scope. `global` temporarily points at function
    /// and catch frames while those bodies execute, but `globalThis` aliases
    /// and top-level binding quotas must always target this frame.
    pub(crate) persistent_global: Env,
    /// Export records, shared with every generator and async body: those run
    /// on their own `Interpreter`, and an `import` inside one must resolve
    /// against the same registry as the code that started it.
    pub modules: Rc<RefCell<HashMap<String, Module>>>,
    /// Sources of modules that have been *defined* but not yet evaluated.
    /// `import` evaluates one on first use, which is what lets a cyclic graph
    /// link: whichever module is imported first runs, and its own import of
    /// the partner runs that one, whose import back is already in flight and
    /// so returns the partially-populated record.
    pub module_sources: Rc<RefCell<HashMap<String, String>>>,
    /// Import aliases scoped to the importing module. Package aliases need
    /// this context so two npm dependencies can use different versions of the
    /// same bare specifier without sharing a global name.
    pub(crate) module_aliases: Rc<RefCell<HashMap<(String, String), String>>>,
    module_file_urls: Rc<RefCell<HashMap<String, String>>>,
    /// Host-selected CommonJS source/native module resolver. No filesystem or
    /// native addon access is enabled unless an embedding host installs one.
    commonjs_loader: Option<Rc<dyn CommonJsModuleLoader>>,
    /// CommonJS module cache, including the partially initialized record used
    /// to make circular `require()` calls observable.
    commonjs_cache: Rc<RefCell<HashMap<String, commonjs::CommonJsCacheEntry>>>,
    /// Filename used to resolve a top-level `require()` call.
    commonjs_entry: Option<String>,
    /// Modules whose bodies are currently running, so a cycle is detected
    /// instead of recursing forever.
    evaluating: Rc<RefCell<std::collections::HashSet<String>>>,
    /// Optional bridge for calling host (Node.js) functions from inside the VM.
    /// Attached by the N-API layer when functions are exposed via
    /// `Vm.exposeFunction`; `None` for a standalone interpreter.
    pub host: Option<Rc<dyn HostBridge>>,
    pub cur_mod: Option<String>,
    pub is_main: bool,
    /// Label applied to the loop currently being entered, if any. A loop takes
    /// this on entry so nested unlabeled loops do not consume its signals.
    active_label: Option<String>,
    /// When executing inside a generator body, this is the handle used to
    /// suspend at a `yield`. `None` for every other interpreter, including the
    /// one that drives the generator.
    #[cfg(stackful_coroutines)]
    pub(crate) gen_yielder: Option<crate::value::GenYielder>,
    /// When executing inside an *async* function body, the handle used to
    /// suspend at an `await`. Distinct from `gen_yielder` so an async
    /// generator can suspend for either reason.
    #[cfg(stackful_coroutines)]
    pub(crate) await_yielder: Option<crate::value::GenYielder>,
    /// Where a `yield` sends its value on a target with no stack switching.
    /// `None` outside a generator body, and always `None` where generators
    /// suspend for real.
    #[cfg(not(stackful_coroutines))]
    pub(crate) yield_sink: Option<Rc<RefCell<Vec<Value>>>>,
    /// The one event loop, shared with every generator and async body so a
    /// promise settled on another stack schedules work the outer drain runs.
    pub jobs: Jobs,
    /// Call stack for error reporting. Pushed on function entry, popped on exit.
    call_stack: Vec<StackFrame>,
    /// Lazily interned names reused by ordinary function calls in this VM.
    this_binding_key: Option<Rc<str>>,
    anonymous_frame_name: Option<Rc<str>>,
    /// Active JavaScript constructor targets. A derived guest constructor
    /// inherits the original `new.target` when it calls a host constructor
    /// through `super()`.
    pub(crate) new_target_stack: Vec<Value>,
    /// The source code for the current module/script, used to extract
    /// source lines for error context. Stored as lines for efficient lookup.
    source_lines: Vec<String>,
    /// How many generator bodies are executing beneath this interpreter.
    /// Zero for the driver; one more than its parent inside a generator body.
    /// Unused where there are no coroutines to nest (see `build.rs`).
    #[cfg_attr(not(stackful_coroutines), expect(dead_code))]
    pub(crate) gen_depth: u32,
    /// Configured per-execution loop-iteration cap.
    loop_budget: u64,
    /// Remaining loop iterations in the current execution. Refilled by
    /// `begin_execution()` at each NAPI entry point; decremented by
    /// `consume_loop()` on every loop iteration.
    loops_remaining: u64,
    /// Active synchronous guest statement bodies. Node-API `make_callback`
    /// drains microtasks only when it is not nested inside guest JavaScript.
    guest_execution_depth: Rc<Cell<usize>>,
}

struct GuestExecutionGuard(Rc<Cell<usize>>);

impl Drop for GuestExecutionGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

/// Does this statement contribute to the enclosing script's completion value?
///
/// Declarations and the empty statement produce *empty* in the specification's
/// terms, which is not the same as producing `undefined`: an empty completion
/// leaves the previous statement's value in place.
pub(crate) fn produces_completion_value(statement: &Statement) -> bool {
    !matches!(
        statement,
        Statement::Empty
            | Statement::VarDecl { .. }
            | Statement::FnDecl { .. }
            | Statement::ClassDecl { .. }
            | Statement::Import { .. }
            | Statement::ExportNamed { .. }
            | Statement::ExportAll { .. }
            | Statement::ExportDefault(_)
    )
}

/// Whether a block introduces bindings that need a distinct environment.
///
/// `run_block` is used for every loop body and conditional branch. Most of
/// those bodies only contain expressions and `var` declarations, so allocating
/// and hoisting an otherwise empty scope on each iteration is unnecessary.
/// Keep this walk aligned with `hoist_lexical`: declarator groups are
/// transparent, while nested statements establish their own scopes when they
/// execute.
pub(crate) fn block_needs_lexical_scope(stmts: &[Statement]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Statement::VarDecl { kind, .. } => matches!(kind, VarKind::Let | VarKind::Const),
        Statement::FnDecl { .. } | Statement::ClassDecl { .. } => true,
        Statement::Declarations(inner) => block_needs_lexical_scope(inner),
        _ => false,
    })
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        let global = Rc::new(RefCell::new(Environment::global(None)));
        Self {
            global: global.clone(),
            persistent_global: global,
            modules: Rc::new(RefCell::new(HashMap::new())),
            module_sources: Rc::new(RefCell::new(HashMap::new())),
            module_aliases: Rc::new(RefCell::new(HashMap::new())),
            module_file_urls: Rc::new(RefCell::new(HashMap::new())),
            commonjs_loader: None,
            commonjs_cache: Rc::new(RefCell::new(HashMap::new())),
            commonjs_entry: None,
            evaluating: Rc::new(RefCell::new(std::collections::HashSet::new())),
            host: None,
            cur_mod: None,
            is_main: false,
            active_label: None,
            #[cfg(stackful_coroutines)]
            gen_yielder: None,
            #[cfg(stackful_coroutines)]
            await_yielder: None,
            #[cfg(not(stackful_coroutines))]
            yield_sink: None,
            jobs: Jobs::default(),
            call_stack: Vec::new(),
            this_binding_key: None,
            anonymous_frame_name: None,
            new_target_stack: Vec::new(),
            source_lines: Vec::new(),
            gen_depth: 0,
            loop_budget: DEFAULT_LOOP_BUDGET,
            loops_remaining: DEFAULT_LOOP_BUDGET,
            guest_execution_depth: Rc::new(Cell::new(0)),
        }
    }

    /// Create an interpreter whose global scope is a fresh *user* frame chained
    /// to a shared builtins frame. User declarations land in the small user
    /// frame, so hot-path variable lookups hit immediately instead of scanning
    /// the large builtins table; builtins still resolve via the parent chain.
    pub fn with_builtins() -> Self {
        let mut interp = Self::new();
        let builtins = Rc::new(RefCell::new(Environment::new()));
        crate::builtins::setup_builtins(&builtins);
        let global = Rc::new(RefCell::new(Environment::global(Some(builtins))));
        interp.global = global.clone();
        interp.persistent_global = global;
        interp
    }

    /// Attach a host bridge for values such as native addon exports.
    pub fn set_host_bridge(&mut self, bridge: Rc<dyn HostBridge>) {
        self.host = Some(bridge);
    }

    /// Register a wake notifier on the attached host bridge, if any. The
    /// bridge fires it from any thread when host-originated work arrives,
    /// so event-loop owners can sleep instead of polling. See
    /// [`HostBridge::set_wake_notifier`](crate::host::HostBridge::set_wake_notifier).
    pub fn set_host_wake_notifier(&self, notifier: crate::host::WakeNotifier) {
        if let Some(host) = &self.host {
            host.set_wake_notifier(notifier);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn install_native_addon_backend<T>(
        &mut self,
        loader: FileCommonJsLoader,
        backend: Rc<T>,
        entry: Option<PathBuf>,
    ) -> Result<Rc<T>, VmErr>
    where
        T: NativeAddonBackendHost + 'static,
    {
        let addon_loader: Rc<dyn NativeAddonLoader> = backend.clone();
        let host_bridge: Rc<dyn HostBridge> = backend.clone();
        self.set_commonjs_loader(Rc::new(loader.with_native_addon_loader(addon_loader)))?;
        self.set_host_bridge(host_bridge);
        if let Some(entry) = entry {
            self.set_commonjs_entry(entry.to_string_lossy().into_owned());
        }
        Ok(backend)
    }

    /// Install a host-controlled CommonJS loader. The interpreter itself does
    /// not read files or load native code unless the host provides a loader.
    pub fn set_commonjs_loader(
        &mut self,
        loader: Rc<dyn CommonJsModuleLoader>,
    ) -> Result<(), VmErr> {
        let require = commonjs::make_require(self, None)?;
        self.set_global_checked("require", require)?;
        self.commonjs_loader = Some(loader);
        self.commonjs_cache.borrow_mut().clear();
        self.define_module(
            "node:module",
            r#"
const createRequire = __napi_vm_module_create_require;
const isBuiltin = __napi_vm_module_is_builtin;
const builtinModules = ["fs", "module", "path"];
export { createRequire, isBuiltin, builtinModules };
export default { createRequire, isBuiltin, builtinModules };
"#
            .into(),
        );
        self.set_global_checked(
            "__napi_vm_module_create_require",
            Value::NativeFunction {
                name: "createRequire".into(),
                callable: |interp, _this, args| commonjs::create_require_builtin(interp, args),
            },
        )?;
        self.set_global_checked(
            "__napi_vm_module_is_builtin",
            Value::NativeFunction {
                name: "isBuiltin".into(),
                callable: |interp, _this, args| commonjs::is_builtin_builtin(interp, args),
            },
        )?;
        let installed = self.ensure_module("node:module");
        self.global
            .borrow_mut()
            .remove("__napi_vm_module_create_require");
        self.global
            .borrow_mut()
            .remove("__napi_vm_module_is_builtin");
        installed?;
        Ok(())
    }

    /// Enable filesystem-backed `require()` and explicitly allowlisted
    /// Node-API addons for a Rust-embedded runtime.
    ///
    /// JavaScript and JSON modules continue to execute in napi-vm. Native
    /// `.node` modules are initialized by a Node.js child process and their
    /// values cross the host bridge. Addons are trusted host code; the
    /// allowlist and digest pin prevent accidental or unapproved loading but
    /// do not sandbox addon behavior.
    ///
    /// This configures the CommonJS loader, host bridge, and optional entry
    /// path together so asynchronous addon callbacks and Promise settlements
    /// use the same event loop. Pump external events with
    /// [`Self::run_event_loop_once`]; the interpreter retains the bridge.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn enable_node_addons(
        &mut self,
        options: NodeAddonOptions,
    ) -> Result<Rc<NodeAddonSidecar>, VmErr> {
        let mut loader = FileCommonJsLoader::new(options.policy.roots().iter())?;
        for (addon, expected_sha256) in options.policy.allowed_addons() {
            loader = match expected_sha256 {
                Some(expected_sha256) => {
                    loader.allow_native_addon_with_sha256(addon, *expected_sha256)?
                }
                None => loader.allow_native_addon(addon)?,
            };
        }

        let entry = options
            .policy
            .entry_path()
            .map(Path::to_path_buf)
            .map(|entry| {
                let canonical = std::fs::canonicalize(&entry).map_err(|error| {
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
            .transpose()?;

        let bridge = Rc::new(NodeAddonSidecar::new_with_policy(
            &options.node_executable,
            loader.roots().to_vec(),
            loader.allowed_native_addon_digests().clone(),
        )?);
        if let Some(required_version) = options.minimum_napi_version
            && bridge.runtime_info().napi_version < required_version
        {
            return Err(VmErr::Msg(format!(
                "Node {} provides Node-API v{}, but Node-API v{} or newer is required",
                bridge.runtime_info().node_version,
                bridge.runtime_info().napi_version,
                required_version
            )));
        }
        self.install_native_addon_backend(loader, bridge, entry)
    }

    /// Configure one native addon backend while keeping guest `require()` and
    /// package resolution inside napi-vm.
    ///
    /// Pass [`NodeAddonOptions`] for a Node child process, or
    /// `RustNodeApiOptions` when the `node-api-host` feature is enabled.
    /// The returned handle reports which backend was selected. In both cases
    /// native loading remains explicit and every `.node` file must pass the
    /// selected backend's allowlist and integrity checks.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn enable_native_addons(
        &mut self,
        options: impl Into<NativeAddonOptions>,
    ) -> Result<NativeAddonRuntime, VmErr> {
        match options.into() {
            NativeAddonOptions::NodeSidecar(options) => self
                .enable_node_addons(options)
                .map(NativeAddonRuntime::NodeSidecar),
            #[cfg(all(
                feature = "node-api-host",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            NativeAddonOptions::RustNodeApi(options) => self
                .enable_rust_node_api_addons(options)
                .map(NativeAddonRuntime::RustNodeApi),
        }
    }

    /// Set the filename used to resolve `require()` in top-level source.
    /// Module-local `require()` calls retain their own filename automatically.
    pub fn set_commonjs_entry(&mut self, filename: impl Into<String>) {
        self.commonjs_entry = Some(filename.into());
    }

    /// Remove all cached CommonJS modules. A subsequent `require()` reloads
    /// source and reruns its wrapper.
    pub fn clear_commonjs_cache(&mut self) {
        self.commonjs_cache.borrow_mut().clear();
    }

    /// Load a CommonJS module using the configured host resolver.
    pub fn require_commonjs(
        &mut self,
        request: &str,
        parent: Option<&str>,
    ) -> Result<Value, VmErr> {
        commonjs::require_module(self, request, parent)
    }

    /// Lex + parse `source` without executing it. Identical sources share
    /// one cached AST; failures carry the same messages `eval_source`
    /// reports.
    pub fn compile(source: &str) -> Result<PreparedProgram, VmErr> {
        let statements =
            crate::parser::parse_cached(source).map_err(|failure| failure.into_vm_err())?;
        Ok(PreparedProgram {
            source: source.into(),
            statements,
        })
    }

    /// Execute a program compiled with [`Self::compile`]: fresh loop
    /// budget, full job drain, same as [`Self::eval_source`] but with no
    /// lexer or parser work.
    pub fn execute(&mut self, program: &PreparedProgram) -> Result<Value, VmErr> {
        self.begin_execution();
        self.set_source(&program.source);
        match self.run_program_body(&program.statements) {
            Ok(value) => self.drain_jobs().map(|()| value),
            Err(error) => {
                let _ = self.drain_jobs();
                Err(error)
            }
        }
    }
}

/// A lexed+parsed script, ready for repeated execution without touching
/// the lexer or parser again. Compile once with [`Interpreter::compile`],
/// run many times with [`Interpreter::execute`]. The AST is shared through
/// the process-wide parse cache, so identical sources compile once even
/// across interpreters; the source text is retained for diagnostics.
#[derive(Clone)]
pub struct PreparedProgram {
    source: std::sync::Arc<str>,
    statements: std::sync::Arc<Vec<Statement>>,
}

impl Interpreter {
    /// Parse and execute a complete JavaScript script, draining the existing
    /// promise/job queue before returning. This is the Rust embedding entry
    /// point; `require()` still needs an explicitly configured loader.
    /// Delegates to [`Self::compile`] + [`Self::execute`]: for repeated
    /// execution, compile once and execute many times instead.
    pub fn eval_source(&mut self, source: &str) -> Result<Value, VmErr> {
        match Self::compile(source) {
            Ok(program) => self.execute(&program),
            Err(error) => {
                let _ = self.drain_jobs();
                Err(error)
            }
        }
    }

    /// Drive one host-initiated guest function call exactly like a top-level
    /// entry point, without parsing anything: fresh loop budget, guest-depth
    /// guard (so a nested `napi_make_callback` defers its microtasks to our
    /// checkpoint, as it does inside `eval_source`), Promise-aware await,
    /// then a full job drain. Plugin hosts call this instead of evaluating
    /// generated wrapper source per invocation.
    pub(crate) fn call_host_function(
        &mut self,
        function: &Value,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        self.begin_execution();
        let depth = self.guest_execution_depth.clone();
        depth.set(depth.get().saturating_add(1));
        let _execution_guard = GuestExecutionGuard(depth);
        match self
            .call_this(function, receiver, args)
            .and_then(|value| self.perform_await(value))
        {
            Ok(value) => self.drain_jobs().map(|()| value),
            Err(error) => {
                let _ = self.drain_jobs();
                Err(error)
            }
        }
    }

    /// Execute one synchronous script body without a microtask checkpoint.
    /// Node-API's `napi_run_script` uses this when called from a native callback:
    /// its Promise jobs must wait until the host returns to the VM event loop.
    /// It also preserves the active source context and loop budget so a nested
    /// native call cannot reset execution limits or replace outer diagnostics.
    #[cfg(all(
        feature = "node-api-host",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    pub(crate) fn run_script_source(&mut self, source: &str) -> Result<Value, VmErr> {
        let previous_source = self.source_lines.clone();
        self.set_source(source);
        let result = match crate::parser::parse_cached(source) {
            Ok(statements) => self.run_program_body(&statements),
            Err(failure) => Err(failure.into_vm_err()),
        };
        self.source_lines = previous_source;
        result
    }

    /// Insert or replace a binding in the currently active scope. The
    /// persistent global frame is checked; local frames retain their fast
    /// infallible insertion path.
    pub(crate) fn set_binding(&mut self, name: &str, value: Value) -> Result<(), VmErr> {
        if Rc::ptr_eq(&self.global, &self.persistent_global) {
            self.global.borrow_mut().try_set(name, value)
        } else {
            self.global.borrow_mut().set(name, value);
            Ok(())
        }
    }

    /// Bind an imported name.
    ///
    /// When the export is a live cell the binding *shares* it, so a later
    /// write in the exporting module is visible through the imported name —
    /// which is the difference between an ES module import and a copy.
    pub(crate) fn bind_import(&mut self, name: &str, value: Value) -> Result<(), VmErr> {
        match &value {
            Value::Binding(cell) => {
                let mut scope = self.global.borrow_mut();
                // Importing a name that already denotes this very cell is a
                // no-op. It happens because module bodies share one scope, so
                // `import { n } from 'm'` inside the program that registered
                // `m` names the binding it is about to re-declare — and
                // re-declaring it `const` would make the exporting module's
                // own writes fail.
                if let Some(Value::Binding(existing)) = &scope.own_binding(name)
                    && Rc::ptr_eq(existing, cell)
                {
                    return Ok(());
                }
                scope.bind_cell(name, cell.clone(), crate::interpreter::BindKind::Const);
                Ok(())
            }
            _ => self.set_binding(name, value),
        }
    }

    /// The export record of the module being evaluated, created on first use.
    pub(crate) fn current_module(&mut self) -> std::cell::RefMut<'_, Module> {
        let name = self.cur_mod.clone().unwrap_or_default();
        let mut modules = self.modules.borrow_mut();
        if !modules.contains_key(&name) {
            modules.insert(
                name.clone(),
                Module {
                    exports: std::collections::HashMap::new(),
                    default: None,
                    scope: None,
                },
            );
        }
        std::cell::RefMut::map(modules, |m| m.get_mut(&name).expect("just inserted"))
    }

    /// Look up the export entries named by `specifiers` in another module,
    /// preserving their live cells so a re-export forwards the binding rather
    /// than a snapshot of its value.
    pub(crate) fn resolve_reexports(
        &mut self,
        source: &str,
        specifiers: &[(String, String)],
    ) -> Result<Vec<(String, Value)>, VmErr> {
        let resolved = self
            .resolve_module_name(source)
            .ok_or_else(|| VmErr::Msg(format!("Module not found: {}", source)))?;
        self.ensure_module(&resolved)?;
        let other = self
            .module(&resolved)
            .ok_or_else(|| VmErr::Msg(format!("Module not found: {}", source)))?;
        Ok(specifiers
            .iter()
            .map(|(local, exported)| {
                let value = if local == "default" {
                    other.default.clone()
                } else {
                    other.exports.get(local).cloned()
                };
                (exported.clone(), value.unwrap_or(Value::Undefined))
            })
            .collect())
    }

    /// Build a module namespace object: every named export, plus `default`
    /// when the module has one.
    ///
    /// Exports keep their live cells, so `ns.count` reflects the exporting
    /// module's current value rather than its value at import time.
    pub(crate) fn namespace_object(module: &Module) -> Result<Value, VmErr> {
        let mut props: Vec<(String, Value)> = module
            .exports
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // A namespace object's keys are sorted, not insertion-ordered.
        props.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(default) = &module.default {
            props.push(("default".to_string(), default.clone()));
        }
        Value::checked_object(props)
    }

    /// Assign an identifier, creating it in the active scope when it does not
    /// already exist. A new binding in the persistent global frame consumes
    /// one global quota entry; updates do not.
    pub(crate) fn assign_or_set_binding(&mut self, name: &str, value: Value) -> Result<(), VmErr> {
        let is_persistent_global = Rc::ptr_eq(&self.global, &self.persistent_global);
        let mut env = self.global.borrow_mut();
        match env.assign(name, value.clone()) {
            AssignOutcome::Assigned => Ok(()),
            AssignOutcome::Const => Err(VmErr::Msg(format!(
                "TypeError: Assignment to constant variable '{name}'"
            ))),
            AssignOutcome::Uninitialized => Err(VmErr::Msg(format!(
                "ReferenceError: Cannot access '{name}' before initialization"
            ))),
            // No such binding: an assignment to an undeclared name creates an
            // implicit `var`-like global, as sloppy-mode JavaScript does.
            AssignOutcome::Missing => {
                if is_persistent_global {
                    env.try_set(name, value)?;
                } else {
                    env.set(name, value);
                }
                Ok(())
            }
        }
    }

    /// Set a property through `globalThis`, `window`, or `self`. Reads and
    /// writes always target the persistent global frame, even when guest code
    /// is currently executing inside a function/catch environment.
    pub(crate) fn set_global_checked(&mut self, name: &str, value: Value) -> Result<(), VmErr> {
        let mut global = self.persistent_global.borrow_mut();
        // An explicit write through the global object creates or updates an
        // own user-global binding. Do not use `assign` here: it walks into the
        // trusted builtins parent and would mutate (for example) builtin
        // `Math` instead of creating a user shadow.
        global.try_set(name, value)?;
        Ok(())
    }

    pub(crate) fn global_value(&self, name: &str) -> Option<Value> {
        self.persistent_global.borrow().get(name)
    }

    /// Execute a statement list in the *current* scope, with no hoisting.
    ///
    /// This is the raw sequencer. Callers that introduce a scope should use
    /// [`Interpreter::run_block`]; callers that begin a function body or a
    /// program should use [`Interpreter::run_program_body`], which performs
    /// the hoisting JavaScript requires before the first statement runs.
    pub fn run(&mut self, stmts: &[Statement]) -> Result<Value, VmErr> {
        let mut r = Value::Undefined;
        for s in stmts {
            let value = self.eval_stmt(s)?;
            // A statement that produces no completion value leaves the
            // previous one standing: `1;;` evaluates to 1, not `undefined`.
            // Declarations and the empty statement are the cases that matter,
            // and getting this wrong made a trailing `;` erase a script's
            // result.
            if produces_completion_value(s) {
                r = value;
            }
        }
        Ok(r)
    }

    /// Execute a statement list in a fresh block scope.
    ///
    /// `let`, `const`, `class` and block-level function declarations become
    /// visible only inside this scope, and are hoisted into it before the
    /// first statement runs so a reference above the declaration reports a
    /// temporal dead zone rather than reaching an outer binding.
    pub fn run_block(&mut self, stmts: &[Statement]) -> Result<Value, VmErr> {
        self.run_block_with_lexical_scope(stmts, block_needs_lexical_scope(stmts))
    }

    /// Execute a block using a lexical-scope decision already computed by a
    /// surrounding loop. Loop bodies are immutable AST, so the check only
    /// needs to run once rather than on every iteration.
    pub(crate) fn run_block_with_lexical_scope(
        &mut self,
        stmts: &[Statement],
        needs_scope: bool,
    ) -> Result<Value, VmErr> {
        if !needs_scope {
            return self.run(stmts);
        }
        let outer = self.push_scope();
        let result = self.hoist_lexical(stmts).and_then(|()| self.run(stmts));
        self.pop_scope(outer);
        result
    }

    /// Execute a statement list as a block *in the current scope*, hoisting
    /// its lexical declarations but not creating a new frame.
    ///
    /// For constructs that already pushed a scope of their own -- a `catch`
    /// clause holding its parameter, a `switch` whose cases share one block --
    /// so their declarations land there rather than in a second, nested frame.
    pub(crate) fn run_hoisted_here(&mut self, stmts: &[Statement]) -> Result<Value, VmErr> {
        self.hoist_lexical(stmts)?;
        self.run(stmts)
    }

    /// Execute a function body or a whole program in the current scope,
    /// performing both halves of JavaScript hoisting first: `var` and function
    /// declarations (recursively, through blocks but not into nested
    /// functions), then this level's lexical declarations.
    pub fn run_program_body(&mut self, stmts: &[Statement]) -> Result<Value, VmErr> {
        let depth = self.guest_execution_depth.clone();
        depth.set(depth.get().saturating_add(1));
        let _execution_guard = GuestExecutionGuard(depth);
        self.hoist_vars(stmts)?;
        self.hoist_lexical(stmts)?;
        self.run(stmts)
    }

    /// Enter a new block scope, returning the scope to restore afterwards.
    pub(crate) fn push_scope(&mut self) -> Env {
        let outer = self.global.clone();
        self.global = Rc::new(RefCell::new(Environment::child(outer.clone())));
        outer
    }

    /// Leave a block scope. Always paired with `push_scope`, including on the
    /// error paths, so a `throw` cannot leave the interpreter in the block.
    pub(crate) fn pop_scope(&mut self, outer: Env) {
        self.global = outer;
    }

    /// Declare a name in the current scope, honouring the global frame's
    /// binding quota when that is where we are.
    pub(crate) fn declare_binding(
        &mut self,
        name: &str,
        value: Value,
        kind: BindKind,
        initialized: bool,
    ) -> Result<(), VmErr> {
        if Rc::ptr_eq(&self.global, &self.persistent_global) {
            self.global
                .borrow_mut()
                .declare_checked(name, value, kind, initialized)
        } else {
            self.global
                .borrow_mut()
                .declare(name, value, kind, initialized);
            Ok(())
        }
    }

    /// Hoist this level's lexical declarations into the current scope.
    ///
    /// `let` and `const` are created uninitialized, which is what makes a read
    /// above the declaration a `ReferenceError` instead of resolving to an
    /// outer binding. Function declarations are created *and* initialized,
    /// because calling a function above its declaration is legal.
    pub(crate) fn hoist_lexical_public(&mut self, stmts: &[Statement]) -> Result<(), VmErr> {
        self.hoist_lexical(stmts)
    }

    fn hoist_lexical(&mut self, stmts: &[Statement]) -> Result<(), VmErr> {
        for stmt in stmts {
            match stmt {
                Statement::VarDecl {
                    name,
                    destructuring,
                    kind,
                    ..
                } => {
                    let kind = match kind {
                        VarKind::Let => BindKind::Let,
                        VarKind::Const => BindKind::Const,
                        // `var` is hoisted by `hoist_vars`, to the function
                        // scope rather than this block.
                        VarKind::Var => continue,
                    };
                    match destructuring {
                        Some(pattern) => {
                            for name in pattern_names(pattern) {
                                self.declare_binding(&name, Value::Undefined, kind, false)?;
                            }
                        }
                        None => self.declare_binding(name, Value::Undefined, kind, false)?,
                    }
                }
                Statement::ClassDecl { name, .. } => {
                    // Classes are lexical and have a dead zone, like `let`.
                    self.declare_binding(name, Value::Undefined, BindKind::Let, false)?;
                }
                Statement::FnDecl { .. } => {
                    // Defined eagerly below so mutual recursion above the
                    // declarations works.
                }
                // Transparent: its declarators belong to this scope.
                Statement::Declarations(inner) => self.hoist_lexical(inner)?,
                _ => {}
            }
        }
        // Second pass: function declarations, after every lexical name exists,
        // so a hoisted function closing over a later `let` sees the binding.
        for stmt in stmts {
            if let Statement::FnDecl { name, .. } = stmt {
                let value = self.eval_stmt(stmt)?;
                let _ = value;
                let _ = name;
            }
        }
        Ok(())
    }

    /// Hoist `var` declarations to the current (function or program) scope.
    ///
    /// Recurses through blocks, loops, `if`, `try` and `switch` -- everywhere a
    /// `var` can hide -- but never into a nested function, which starts its own
    /// variable scope.
    fn hoist_vars(&mut self, stmts: &[Statement]) -> Result<(), VmErr> {
        let mut names = Vec::new();
        collect_var_names(stmts, &mut names);
        for name in names {
            // Only create the binding if nothing already provides it: a
            // parameter of the same name keeps its argument value, and a
            // repeated `var` must not erase an earlier assignment.
            if !self.global.borrow().has(&name) {
                self.declare_binding(&name, Value::Undefined, BindKind::Var, true)?;
            }
        }
        Ok(())
    }

    /// Install a fresh, empty export record for `name` and make it the module
    /// under evaluation, returning the record it displaced.
    ///
    /// Export statements merge into whatever record is already registered
    /// under the current module name (see `eval_stmt`), so a re-registration
    /// has to start from an empty one. Merging into the old record would let
    /// an export that the new source deliberately dropped stay importable —
    /// and when exports carry authority, that is a revoked capability that
    /// still answers. The displaced record is returned so a body that fails
    /// part-way through can be rolled back with `restore_module`.
    pub fn begin_module(&mut self, name: &str) -> Option<Module> {
        let scope = self.new_module_scope();
        let prior = self.modules.borrow_mut().insert(
            name.to_string(),
            Module {
                exports: HashMap::new(),
                default: None,
                scope: Some(scope),
            },
        );
        self.cur_mod = Some(name.to_string());
        prior
    }

    /// The live cell an importer should bind for `name` from `module`.
    ///
    /// When the module has already exported the name, that is its cell. When
    /// the module is still evaluating — a cycle — a *pending* cell is created
    /// and registered as the export, so the importer binds the storage the
    /// exporting module will fill in when its `export` finally runs. This is
    /// what makes two mutually recursive modules link.
    pub(crate) fn pending_export(&mut self, module: &str, name: &str) -> Option<Value> {
        if !self.evaluating.borrow().contains(module) {
            return None;
        }
        let cell = Rc::new(RefCell::new(Value::Undefined));
        let entry = Value::Binding(cell);
        self.modules
            .borrow_mut()
            .get_mut(module)?
            .exports
            .insert(name.to_string(), entry.clone());
        Some(entry)
    }

    /// A fresh module scope: a child of the *user global* frame.
    ///
    /// Chaining to the user global rather than to the builtins is what keeps
    /// host-exposed globals and anything written through `globalThis` visible
    /// inside a module, while the module's own declarations stay local to it.
    fn new_module_scope(&self) -> Env {
        Rc::new(RefCell::new(Environment::child(
            self.persistent_global.clone(),
        )))
    }

    /// Install `scope` as the current one, returning the scope it displaced.
    pub fn take_scope(&mut self, scope: Env) -> Env {
        std::mem::replace(&mut self.global, scope)
    }

    /// The scope a module body evaluates in, creating it if the module has
    /// not been entered yet.
    pub(crate) fn module_scope(&mut self, name: &str) -> Env {
        if let Some(scope) = self
            .modules
            .borrow()
            .get(name)
            .and_then(|module| module.scope.clone())
        {
            return scope;
        }
        let scope = self.new_module_scope();
        if let Some(module) = self.modules.borrow_mut().get_mut(name) {
            module.scope = Some(scope.clone());
        }
        scope
    }

    /// Leave module-evaluation context, keeping everything the body exported.
    pub fn commit_module(&mut self) {
        self.cur_mod = None;
    }

    /// Leave module-evaluation context and put `prior` back, discarding every
    /// export the failed body managed to write.
    ///
    /// This restores the *export table* only. A module body can also mutate
    /// globals before it throws, and those writes are not unwound here; see
    /// `registerModule` in the N-API layer for what that means for callers.
    pub fn restore_module(&mut self, name: &str, prior: Option<Module>) {
        match prior {
            Some(module) => {
                self.modules.borrow_mut().insert(name.to_string(), module);
            }
            None => {
                self.modules.borrow_mut().remove(name);
            }
        }
        self.cur_mod = None;
    }

    /// A module's export record, if it has been evaluated.
    pub fn module(&self, name: &str) -> Option<Module> {
        self.modules.borrow().get(name).cloned()
    }

    /// Record a module's source without running it. `import` evaluates it on
    /// first use.
    pub fn define_module(&mut self, name: &str, source: String) {
        self.module_sources
            .borrow_mut()
            .insert(name.to_string(), source);
    }

    /// Associate a file-backed ES module with its URL for `import.meta.url`.
    pub fn define_module_file_url(&mut self, name: &str, url: String) {
        self.module_file_urls.borrow_mut().insert(name.into(), url);
    }

    /// Resolve `specifier` to `target` only when it is imported by `importer`.
    /// This supports package graphs where the same bare npm specifier may
    /// resolve to different nested dependency versions.
    pub fn define_module_alias(&mut self, importer: &str, specifier: &str, target: &str) {
        self.module_aliases.borrow_mut().insert(
            (importer.to_string(), specifier.to_string()),
            target.to_string(),
        );
    }

    /// Make sure `name` has an export record, evaluating its deferred source
    /// if that is what it takes.
    ///
    /// A module already being evaluated returns immediately: that is a cycle,
    /// and the specification's answer is to let the importer see the record as
    /// far as it has been filled in. Live bindings are what make that useful —
    /// a function imported from a half-initialized module still sees the final
    /// value once the body finishes.
    pub fn ensure_module(&mut self, name: &str) -> Result<bool, VmErr> {
        if self.modules.borrow().contains_key(name) || self.evaluating.borrow().contains(name) {
            return Ok(true);
        }
        let Some(source) = self.module_sources.borrow().get(name).cloned() else {
            return Ok(false);
        };
        let outer = self.cur_mod.take();
        let displaced = self.begin_module(name);
        self.evaluating.borrow_mut().insert(name.to_string());
        let scope = self.module_scope(name);
        let outer_scope = std::mem::replace(&mut self.global, scope);
        let result = self.eval_module_source(&source);
        self.global = outer_scope;
        self.evaluating.borrow_mut().remove(name);
        match result {
            Ok(()) => {
                self.cur_mod = outer;
                Ok(true)
            }
            Err(error) => {
                self.restore_module(name, displaced);
                self.cur_mod = outer;
                Err(error)
            }
        }
    }

    /// Parse and run one module body. Kept beside `ensure_module` so deferred
    /// evaluation does not have to reach back into the N-API layer.
    fn eval_module_source(&mut self, source: &str) -> Result<(), VmErr> {
        let statements =
            crate::parser::parse_cached(source).map_err(|failure| failure.into_vm_err())?;
        // Modules hoist exactly like scripts: `var` and eagerly-defined
        // function declarations first, then lexical dead zones. Without this,
        // a module-level call above its function declaration fails to resolve.
        self.run_program_body(&statements)?;
        Ok(())
    }

    /// Drop a module's export record so `import` can no longer resolve it.
    ///
    /// This is the half that actually revokes reachability: the N-API layer's
    /// own source registry is bookkeeping, but `import` resolves through this
    /// map, so a module left here stays importable no matter what the public
    /// API reports.
    pub fn remove_module(&mut self, name: &str) -> bool {
        self.module_file_urls.borrow_mut().remove(name);
        let had_source = self.module_sources.borrow_mut().remove(name).is_some();
        let had_module = self.modules.borrow_mut().remove(name).is_some();
        self.module_aliases
            .borrow_mut()
            .retain(|(importer, _), target| importer != name && target != name);
        had_module || had_source
    }

    /// Whether `name` has an export record that `import` would resolve.
    pub fn has_module(&self, name: &str) -> bool {
        self.modules.borrow().contains_key(name) || self.module_sources.borrow().contains_key(name)
    }

    /// Resolve a relative import from the module currently being evaluated.
    /// Module names use browser-style POSIX paths so the same source behaves
    /// consistently in the native VM and in the browser playground.
    pub(crate) fn resolve_module_name(&self, module: &str) -> Option<String> {
        if let Some(importer) = self.cur_mod.as_deref()
            && let Some(target) = self
                .module_aliases
                .borrow()
                .get(&(importer.to_string(), module.to_string()))
        {
            return Some(target.clone());
        }
        if !module.starts_with('.') {
            return Some(module.to_string());
        }

        let current = self.cur_mod.as_deref()?;
        let current = current.strip_prefix("./").unwrap_or(current);
        let mut parts: Vec<&str> = current
            .rsplit_once('/')
            .map(|(base, _)| base.split('/').collect())
            .unwrap_or_default();
        for part in module.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                value => parts.push(value),
            }
        }
        Some(format!("./{}", parts.join("/")))
    }

    /// Refill the loop budget. Called at each NAPI entry point (`run`,
    /// `registerModule`, `callFunction`) so every top-level execution gets a
    /// full budget. Not called from `run` itself: block bodies and loop
    /// bodies re-enter it recursively and must not refill mid-execution.
    pub fn begin_execution(&mut self) {
        self.loops_remaining = self.loop_budget;
    }

    /// Change the loop-iteration cap (exposed to Node as `setLoopLimit`).
    pub fn set_loop_budget(&mut self, n: u64) {
        self.loop_budget = n;
        self.loops_remaining = n;
    }

    /// Account one loop iteration against the budget. Every loop construct
    /// calls this per iteration, so guest code can never spin forever.
    pub(crate) fn consume_loop(&mut self) -> Result<(), VmErr> {
        if self.loops_remaining == 0 {
            return Err(VmErr::Msg(
                "RangeError: Maximum loop iterations exceeded".to_string(),
            ));
        }
        self.loops_remaining -= 1;
        Ok(())
    }

    /// Set the source code for the current script/module. Used to extract
    /// source lines for error context.
    pub fn set_source(&mut self, source: &str) {
        self.source_lines = source.lines().map(String::from).collect();
    }

    /// Get a source line by 1-based line number, if available.
    pub fn get_source_line(&self, line: usize) -> Option<&str> {
        self.source_lines.get(line - 1).map(|s| s.as_str())
    }

    /// Push a frame onto the call stack. The name is shared (`Rc<str>`), so
    /// pushing a frame for a function call is a refcount bump, not a string
    /// allocation.
    pub(crate) fn push_frame(&mut self, name: std::rc::Rc<str>, span: Span) {
        self.call_stack.push(StackFrame { name, span });
    }

    /// Pop a frame from the call stack.
    pub(crate) fn pop_frame(&mut self) {
        self.call_stack.pop();
    }

    /// Get a snapshot of the current call stack.
    pub fn get_stack(&self) -> &[StackFrame] {
        &self.call_stack
    }

    /// Attach the current call stack and last span to an error.
    #[cfg_attr(not(feature = "napi"), allow(dead_code))]
    pub(crate) fn enrich_error(&self, err: VmErr, span: Option<Span>) -> VmErr {
        err.with_context(span, &self.call_stack)
    }

    /// Return all global variable names (user-defined + builtins). Used by
    /// `Object.getOwnPropertyNames(window)`.
    pub fn global_keys(&self) -> Vec<String> {
        self.persistent_global.borrow().all_keys()
    }

    /// Enumerate a proxy through its `ownKeys` trap when one is installed.
    pub(crate) fn keys_with_proxy_trap(&mut self, value: &Value) -> Result<Vec<String>, VmErr> {
        let Some(proxy) = value.as_proxy() else {
            return Ok(self.keys(value));
        };
        let target = proxy.target.clone();
        let Some(trap) = self.proxy_trap(&proxy, "ownKeys") else {
            return Ok(self.keys(&target));
        };
        let handler = proxy.handler.clone();
        let keys = self.call_this(&trap, handler, vec![target])?;
        let Value::Array(keys) = &keys else {
            return Err(VmErr::Msg(
                "TypeError: Proxy ownKeys trap must return an array".into(),
            ));
        };
        let keys = keys.borrow().clone();
        let mut names = Vec::with_capacity(keys.len());
        for key in &keys {
            match key {
                Value::String(name) => names.push(name.clone()),
                Value::Symbol(_) => {}
                _ => {
                    return Err(VmErr::Msg(
                        "TypeError: Proxy ownKeys trap returned a non-key".into(),
                    ));
                }
            }
        }
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Lexer;
    use crate::parser::Parser;

    fn eval(src: &str) -> Result<Value, VmErr> {
        let mut interp = Interpreter::with_builtins();
        let mut lex = Lexer::new(src);
        let toks = lex.tokenize_with_spans();
        let mut parser = Parser::new_with_spans(toks);
        let stmts = parser.parse();
        let completion = interp.run_program_body(&stmts);
        match completion {
            Ok(value) => interp.drain_jobs().map(|()| value),
            Err(error) => {
                let _ = interp.drain_jobs();
                Err(error)
            }
        }
    }

    fn eval_str(src: &str) -> String {
        let interp = Interpreter::new();
        match eval(src) {
            Ok(v) => interp.vs(&v).unwrap_or_else(|e| format!("ERROR: {}", e)),
            Err(e) => format!("ERROR: {}", e),
        }
    }

    #[test]
    fn test_arithmetic() {
        assert_eq!(eval_str("2 + 2;"), "4");
        assert_eq!(eval_str("10 - 3;"), "7");
        assert_eq!(eval_str("4 * 5;"), "20");
        assert_eq!(eval_str("15 / 3;"), "5");
        assert_eq!(eval_str("10 % 3;"), "1");
    }

    #[test]
    fn test_variables() {
        assert_eq!(eval_str("const x = 42; x;"), "42");
        assert_eq!(eval_str("let x = 1; x = 2; x;"), "2");
    }

    #[test]
    fn test_functions() {
        assert_eq!(
            eval_str("function add(a, b) { return a + b; } add(3, 4);"),
            "7"
        );
        assert_eq!(eval_str("const f = (x) => x * x; f(5);"), "25");
    }

    #[test]
    fn built_in_error_subclasses_inherit_from_error() {
        for source in [
            "new TypeError('x') instanceof Error;",
            "new RangeError('x') instanceof Error;",
            "new SyntaxError('x') instanceof Error;",
            "new ReferenceError('x') instanceof Error;",
        ] {
            assert!(matches!(eval(source), Ok(Value::Bool(true))), "{source}");
        }
        assert!(matches!(
            eval("new RangeError('x') instanceof TypeError;"),
            Ok(Value::Bool(false))
        ));
        assert!(matches!(
            eval("new RangeError('x').toString();"),
            Ok(Value::String(ref message)) if message == "RangeError: x"
        ));
    }

    #[test]
    fn test_closures() {
        assert_eq!(
            eval_str(
                "function counter() { let n = 0; return () => ++n; } const c = counter(); c(); c(); c();"
            ),
            "3"
        );
    }

    #[test]
    fn test_recursion() {
        assert_eq!(
            eval_str("function fib(n) { return n <= 1 ? n : fib(n-1) + fib(n-2); } fib(10);"),
            "55"
        );
    }

    #[test]
    fn test_strings() {
        assert_eq!(eval_str("'hello' + ' ' + 'world';"), "hello world");
        assert_eq!(eval_str("'hello'.length;"), "5");
    }

    #[test]
    fn test_arrays() {
        assert_eq!(eval_str("const a = [1,2,3]; a.length;"), "3");
        assert_eq!(eval_str("const a = [10,20,30]; a[1];"), "20");
    }

    #[test]
    fn sparse_arrays_preserve_holes_across_common_methods() {
        let value = eval(
            "const a = Array(4); a[1] = undefined; a[3] = 2; const mapped = a.map(x => x); const flat = [a].flat(); const flattened = a.flatMap(x => [x]); const sorted = Array(4); sorted[0] = 3; sorted[2] = 1; sorted[3] = undefined; sorted.sort(); ({keys: Object.keys(a).join(','), mapHole: Object.hasOwn(mapped, 0), mapUndefined: Object.hasOwn(mapped, 1), flatIndex0: Object.hasOwn(flat, 0), flatLength: flat.length, flatMapLength: flattened.length, reduced: a.reduceRight((count, value) => count + 1, 0), sorted: sorted.join(','), sortedHole: !Object.hasOwn(sorted, 3)});",
        )
        .unwrap();

        assert!(matches!(value.get_prop("keys"), Some(Value::String(ref text)) if text == "1,3"));
        assert!(matches!(
            value.get_prop("mapHole"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            value.get_prop("flatIndex0"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            value.get_prop("sortedHole"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            value.get_prop("mapUndefined"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            value.get_prop("flatLength"),
            Some(Value::Number(2.0))
        ));
        assert!(matches!(
            value.get_prop("flatMapLength"),
            Some(Value::Number(2.0))
        ));
        assert!(matches!(
            value.get_prop("reduced"),
            Some(Value::Number(2.0))
        ));
        assert!(
            matches!(value.get_prop("sorted"), Some(Value::String(ref text)) if text == "1,3,,")
        );
    }

    #[test]
    fn test_objects() {
        assert_eq!(eval_str("const o = {x: 1}; o.x;"), "1");
        assert_eq!(eval_str("const o = {x: 1}; o['x'];"), "1");
    }

    #[test]
    fn test_loops() {
        assert_eq!(
            eval_str("let s = 0; for (let i = 0; i < 10; i++) { s += i; } s;"),
            "45"
        );
        assert_eq!(eval_str("let i = 0; while (i < 5) { i++; } i;"), "5");
    }

    #[test]
    fn test_try_catch() {
        assert_eq!(
            eval_str("try { throw 'oops'; } catch(e) { 'caught: ' + e; }"),
            "caught: oops"
        );
    }

    #[test]
    fn test_typeof() {
        assert_eq!(eval_str("typeof 42;"), "number");
        assert_eq!(eval_str("typeof 'hi';"), "string");
        assert_eq!(eval_str("typeof true;"), "boolean");
        assert_eq!(eval_str("typeof undefined;"), "undefined");
        assert_eq!(eval_str("typeof null;"), "object");
    }

    #[test]
    fn test_comparison() {
        assert_eq!(eval_str("5 === 5;"), "true");
        assert_eq!(eval_str("5 !== 3;"), "true");
        assert_eq!(eval_str("5 == 5;"), "true");
        assert_eq!(eval_str("'5' === 5;"), "false");
        assert_eq!(
            eval_str("const error = new Error('same'); error === error;"),
            "true"
        );
        assert_eq!(
            eval_str("new Error('same') === new Error('same');"),
            "false"
        );
    }

    #[test]
    fn test_logical() {
        assert_eq!(eval_str("true && false;"), "false");
        assert_eq!(eval_str("true || false;"), "true");
        assert_eq!(eval_str("!true;"), "false");
    }

    #[test]
    fn test_ternary() {
        assert_eq!(eval_str("true ? 'yes' : 'no';"), "yes");
        assert_eq!(eval_str("false ? 'yes' : 'no';"), "no");
    }

    #[test]
    fn top_level_await_reports_a_promise_with_no_pending_work() {
        let mut interpreter = Interpreter::with_builtins();
        let error = interpreter
            .eval_source("await new Promise(() => {});")
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("cannot synchronously await a pending Promise")
        );
    }

    #[test]
    fn test_increment() {
        assert_eq!(eval_str("let i = 0; i++;"), "0");
        assert_eq!(eval_str("let i = 0; ++i;"), "1");
    }

    #[test]
    fn test_compound_assign() {
        assert_eq!(eval_str("let x = 5; x += 3; x;"), "8");
        assert_eq!(eval_str("let x = 10; x -= 4; x;"), "6");
        assert_eq!(eval_str("let x = 3; x *= 2; x;"), "6");
    }

    #[test]
    fn test_for_of() {
        assert_eq!(
            eval_str("let s = 0; for (const x of [1,2,3]) { s += x; } s;"),
            "6"
        );
    }

    #[test]
    fn test_for_in() {
        assert_eq!(
            eval_str("let r = ''; for (const k in {a: 1, b: 2}) { r += k; } r;"),
            "ab"
        );
    }

    #[test]
    fn test_switch() {
        assert_eq!(
            eval_str(
                "let r = ''; switch (2) { case 1: r = 'one'; break; case 2: r = 'two'; break; default: r = 'other'; } r;"
            ),
            "two"
        );
    }

    #[test]
    fn test_nested_functions() {
        assert_eq!(
            eval_str(
                "function outer() { function inner() { return 42; } return inner(); } outer();"
            ),
            "42"
        );
    }

    #[test]
    fn test_math_constants() {
        assert_eq!(eval_str("Math.PI;"), "3.141592653589793");
        assert_eq!(eval_str("Math.E;"), "2.718281828459045");
    }

    #[test]
    fn test_do_while() {
        assert_eq!(eval_str("let i = 0; do { i++; } while (i < 5); i;"), "5");
    }

    #[test]
    fn test_break_in_loops() {
        assert_eq!(
            eval_str("let i = 0; while (true) { if (i >= 3) { break; } i++; } i;"),
            "3"
        );
        assert_eq!(
            eval_str("let n = 0; for (let i = 0; i < 10; i++) { if (i === 4) { break; } n++; } n;"),
            "4"
        );
        assert_eq!(
            eval_str("let i = 0; do { if (i >= 2) { break; } i++; } while (true); i;"),
            "2"
        );
    }

    #[test]
    fn test_continue_in_loops() {
        assert_eq!(
            eval_str(
                "let s = 0; for (let i = 0; i < 5; i++) { if (i % 2) { continue; } s += i; } s;"
            ),
            "6"
        );
        assert_eq!(
            eval_str(
                "let s = 0; let i = 0; while (i < 5) { i++; if (i === 3) { continue; } s += i; } s;"
            ),
            "12"
        );
    }
}
