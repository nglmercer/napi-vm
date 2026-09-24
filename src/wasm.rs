//! Browser bindings for the VM: a `#[wasm_bindgen]` [`WasmVm`] that runs the
//! interpreter **in the page** and exposes the shared language services
//! (completion, diagnostics, symbols) from [`crate::lang`].
//!
//! This is the WASM analogue of the NAPI layer in [`crate::bindings`]: the same
//! pure-Rust core, the same marshalling shape, and the same host-bridge design,
//! but crossing the Rust ↔ browser boundary instead of Rust ↔ Node.js. Because
//! the language services live in the frontend-agnostic [`crate::lang`] module,
//! the in-browser playground, the LSP server, and native GUIs all get identical
//! completions without re-deriving them.
//!
//! Build with:
//! `--no-default-features --features wasm --target wasm32-unknown-unknown`
//!
//! The VM is single-threaded (`Rc`/`RefCell`), which matches the browser's
//! single-threaded wasm model exactly — no `Send`/`Sync` gymnastics needed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::error::VmErr;
use crate::host::HostBridge;
use crate::interpreter::Interpreter;
use crate::lang::{
    AnalysisContext, Completion, CompletionKind, DiagnosticSeverity, HostFunctionInfo,
    HostFunctionParameter, ModuleInfo, clamp_type_name,
};
use crate::lexer::Lexer;
use crate::parser::Parser;
use crate::value::{MAX_ARRAY_LEN, MAX_OBJECT_PROPS, MAX_STRING_LEN, Value, limit_err};

/// Maximum nesting marshalled across the wasm boundary in either direction.
/// Mirrors the NAPI layer: a structure deeper than this yields a catchable
/// error instead of overflowing the stack in the recursive walkers below.
const MAX_MARSHAL_DEPTH: usize = 512;

// ---------------------------------------------------------------------------
// Marshalling: Value <-> JsValue
// ---------------------------------------------------------------------------

/// Marshal a VM `Value` into a browser `JsValue`.
///
/// Functions, promises, generators and other VM-only values have no faithful
/// representation in this direction and are surfaced as `undefined` (the NAPI
/// layer makes the same choice).
fn value_to_js(v: &Value) -> Result<JsValue, VmErr> {
    value_to_js_d(v, 0)
}

fn value_to_js_d(v: &Value, depth: usize) -> Result<JsValue, VmErr> {
    if depth > MAX_MARSHAL_DEPTH {
        return Err(VmErr::Msg("value is too deep to marshal".to_string()));
    }
    Ok(match v {
        Value::Undefined => JsValue::UNDEFINED,
        Value::Null => JsValue::NULL,
        Value::Bool(b) => JsValue::from_bool(*b),
        Value::Number(n) => JsValue::from_f64(*n),
        Value::String(s) => JsValue::from_str(s),
        Value::Array(items) => {
            let arr = js_sys::Array::new();
            for item in items.borrow().iter() {
                arr.push(&value_to_js_d(item, depth + 1)?);
            }
            arr.into()
        }
        Value::Object { props, .. } => {
            // A null prototype prevents a guest-controlled `__proto__` key
            // from invoking Object.prototype's legacy setter while exporting
            // a VM object to browser JavaScript.
            let null_prototype: js_sys::Object = JsValue::NULL.unchecked_into();
            let obj = js_sys::Object::create(&null_prototype);
            for (k, val) in props.borrow().iter() {
                js_sys::Reflect::set(&obj, &JsValue::from_str(k), &value_to_js_d(val, depth + 1)?)
                    .map_err(|_| VmErr::Msg("failed to set object property".to_string()))?;
            }
            obj.into()
        }
        Value::Error(e) => {
            let null_prototype: js_sys::Object = JsValue::NULL.unchecked_into();
            let obj = js_sys::Object::create(&null_prototype);
            let _ = js_sys::Reflect::set(
                &obj,
                &JsValue::from_str("name"),
                &JsValue::from_str(&e.name),
            );
            let _ = js_sys::Reflect::set(
                &obj,
                &JsValue::from_str("message"),
                &JsValue::from_str(&e.message),
            );
            obj.into()
        }
        _ => JsValue::UNDEFINED,
    })
}

/// Marshal a browser `JsValue` into a VM `Value`.
///
/// JavaScript functions are not marshalled into callable VM values; use
/// [`WasmVm::expose_function`] to make a browser function callable from the VM.
fn js_to_value(j: &JsValue) -> Result<Value, VmErr> {
    js_to_value_d(j, 0)
}

fn js_to_value_d(j: &JsValue, depth: usize) -> Result<Value, VmErr> {
    if depth > MAX_MARSHAL_DEPTH {
        return Err(VmErr::Msg("value is too deep to marshal".to_string()));
    }
    if j.is_undefined() {
        return Ok(Value::Undefined);
    }
    if j.is_null() {
        return Ok(Value::Null);
    }
    if let Some(b) = j.as_bool() {
        return Ok(Value::Bool(b));
    }
    if let Some(n) = j.as_f64() {
        return Ok(Value::Number(n));
    }
    if let Some(s) = j.as_string() {
        if s.len() > MAX_STRING_LEN {
            return Err(limit_err("Maximum string length exceeded"));
        }
        return Ok(Value::String(s));
    }
    // A JS function has no callable VM representation in v1.
    if j.is_instance_of::<js_sys::Function>() {
        return Ok(Value::Undefined);
    }
    // A JS `Error` carries `message` non-enumerably, so surface it explicitly —
    // the same shape the VM's own errors use, so `catch (e) { e.message }` works.
    if j.is_instance_of::<js_sys::Error>() {
        let name = read_str_prop(j, "name").unwrap_or_else(|| "Error".to_string());
        let message = read_str_prop(j, "message").unwrap_or_default();
        return Value::checked_object(vec![
            ("name".to_string(), Value::String(name)),
            ("message".to_string(), Value::String(message)),
        ]);
    }
    if js_sys::Array::is_array(j) {
        let arr: &js_sys::Array = j.unchecked_ref();
        let len = arr.length() as usize;
        if len > MAX_ARRAY_LEN {
            return Err(limit_err("Maximum array length exceeded"));
        }
        let mut items = Vec::with_capacity(len);
        for i in 0..len {
            items.push(js_to_value_d(&arr.get(i as u32), depth + 1)?);
        }
        return Value::checked_array(items);
    }
    if j.is_instance_of::<js_sys::Object>() {
        let keys = js_sys::Object::keys(j.unchecked_ref::<js_sys::Object>());
        let n = keys.length();
        if n as usize > MAX_OBJECT_PROPS {
            return Err(limit_err("Maximum object property count exceeded"));
        }
        let mut props = Vec::with_capacity(n as usize);
        for i in 0..n {
            let k = keys.get(i);
            let key = k.as_string().unwrap_or_default();
            let val = js_sys::Reflect::get(j, &k)
                .map_err(|_| VmErr::Msg("failed to read object property".to_string()))?;
            props.push((key, js_to_value_d(&val, depth + 1)?));
        }
        return Value::checked_object(props);
    }
    Ok(Value::Undefined)
}

/// Read a string-valued property via `Reflect.get`, returning `None` when the
/// property is absent or not a string.
fn read_str_prop(obj: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.as_string())
}

fn host_function_info_from_js(name: &str, metadata: &JsValue) -> Result<HostFunctionInfo, JsValue> {
    if !metadata.is_object() {
        return Err(JsValue::from_str(
            "host function metadata must be an object",
        ));
    }

    let params_value = js_sys::Reflect::get(metadata, &JsValue::from_str("params"))
        .map_err(|_| JsValue::from_str("failed to read host function params"))?;
    let mut params = Vec::new();
    if !params_value.is_undefined() && !params_value.is_null() {
        let params_array = js_sys::Array::from(&params_value);
        for index in 0..params_array.length() {
            let parameter = params_array.get(index);
            let parameter_name = read_str_prop(&parameter, "name")
                .ok_or_else(|| JsValue::from_str("host function parameter needs a name"))?;
            let type_name = clamp_type_name(read_str_prop(&parameter, "type").as_deref());
            params.push(HostFunctionParameter {
                name: parameter_name,
                type_name,
            });
        }
    }

    let return_type = clamp_type_name(read_str_prop(metadata, "returns").as_deref());
    let documentation = read_str_prop(metadata, "documentation");
    let async_fn = js_sys::Reflect::get(metadata, &JsValue::from_str("async"))
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    Ok(HostFunctionInfo {
        name: name.into(),
        params,
        return_type,
        documentation,
        async_fn,
    })
}

/// Best-effort error message from a thrown `JsValue`.
fn js_error_message(e: &JsValue) -> String {
    if e.is_instance_of::<js_sys::Error>()
        && let Some(m) = read_str_prop(e, "message")
        && !m.is_empty()
    {
        return m;
    }
    e.as_string().unwrap_or_else(|| "unknown error".to_string())
}

/// Set a named property on an object, ignoring failure (keys are always valid
/// strings here, so this only fails on a frozen target — not our concern).
fn set_prop(obj: &js_sys::Object, key: &str, val: &JsValue) {
    let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), val);
}

// ---------------------------------------------------------------------------
// Host bridge
// ---------------------------------------------------------------------------

/// Bridge that lets the VM call back into browser (JavaScript) functions
/// exposed via [`WasmVm::expose_function`]. Holds persisted `js_sys::Function`
/// handles and invokes them synchronously on the same thread that drives the
/// VM — the browser wasm instance is single-threaded, so no TSFN/channel dance
/// is required (unlike the NAPI async path).
struct WasmBridge {
    funcs: RefCell<Vec<js_sys::Function>>,
}

impl WasmBridge {
    fn new() -> Self {
        Self {
            funcs: RefCell::new(Vec::new()),
        }
    }

    /// Persist a JS function and return the id the VM uses to reach it.
    fn register(&self, func: js_sys::Function) -> usize {
        let mut funcs = self.funcs.borrow_mut();
        funcs.push(func);
        funcs.len() - 1
    }
}

impl HostBridge for WasmBridge {
    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        let funcs = self.funcs.borrow();
        let func = funcs
            .get(id)
            .ok_or_else(|| VmErr::Msg(format!("no host function #{}", id)))?;
        let js_args = js_sys::Array::new();
        for a in &args {
            js_args.push(&value_to_js(a)?);
        }
        match func.apply(&JsValue::UNDEFINED, &js_args) {
            Ok(ret) => js_to_value(&ret),
            Err(e) => Err(VmErr::Msg(format!("Error: {}", js_error_message(&e)))),
        }
    }
}

// ---------------------------------------------------------------------------
// Console capture
// ---------------------------------------------------------------------------

/// Overrides the VM's `console` so `console.log` and friends route through the
/// exposed `__out(level, text)` host function instead of `println!` — which
/// traps on `wasm32-unknown-unknown` (there is no stdout). The formatter is
/// cycle-safe and depth-capped so logging a circular structure cannot hang or
/// overflow. Runs once at construction and again on [`WasmVm::reset`].
const CONSOLE_SETUP: &str = r#"
(function () {
  var seen = [];
  function fmt(v, d) {
    if (v === null) return 'null';
    if (v === undefined) return 'undefined';
    var t = typeof v;
    // Top-level string args print raw (like Node's console.log); nested strings
    // are quoted so `{ name: "alice" }` stays readable.
    if (t === 'string') return d === 0 ? v : JSON.stringify(v);
    // `String`/`Number` are namespace objects in this VM, not callable — coerce
    // with concatenation instead of `String(v)`.
    if (t === 'number' || t === 'boolean') return '' + v;
    if (t === 'function') return '[function]';
    if (d > 4) return '[object]';
    if (seen.indexOf(v) !== -1) return '[circular]';
    seen.push(v);
    var out;
    if (Array.isArray(v)) {
      out = '[' + v.map(function (x) { return fmt(x, d + 1); }).join(', ') + ']';
    } else {
      var ks = Object.keys(v);
      out = '{ ' + ks.map(function (k) { return k + ': ' + fmt(v[k], d + 1); }).join(', ') + ' }';
    }
    seen.pop();
    return out;
  }
  // Rest parameters collect the call's arguments into a real array; indexing
  // the `arguments` object across a call boundary does not resolve in this VM,
  // so we map over the rest array instead.
  function emit(level) {
    return (...a) => __out(level, a.map((x) => fmt(x, 0)).join(' '));
  }
  console.log = emit('log');
  console.info = emit('info');
  console.debug = emit('debug');
  console.warn = emit('warn');
  console.error = emit('error');
  console.dir = emit('dir');
})();
"#;

/// Run a snippet on the interpreter, ignoring its result. Used for console setup.
fn run_setup(interp: &mut Interpreter) {
    interp.begin_execution();
    let toks = Lexer::new(CONSOLE_SETUP).tokenize_with_spans();
    let mut parser = Parser::new_with_spans(toks);
    let stmts = parser.parse();
    let _ = interp.run_program_body(&stmts);
}

/// Build the `__out` sink: a Rust closure that appends `(level, text)` to the
/// shared log buffer, plus the `js_sys::Function` handle handed to the bridge.
fn make_out_closure(
    logs: Rc<RefCell<Vec<(String, String)>>>,
) -> (Closure<dyn FnMut(String, String)>, js_sys::Function) {
    let closure = Closure::new(move |level: String, text: String| {
        logs.borrow_mut().push((level, text));
    });
    let func: js_sys::Function = closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
    (closure, func)
}

// ---------------------------------------------------------------------------
// The exported VM
// ---------------------------------------------------------------------------

/// The VM, runnable in the browser. Construct with `new WasmVm()`.
///
/// `run` returns a plain object `{ ok, value, error, logs }` where `logs` is an
/// array of `{ level, text }` captured from `console.*` during that run.
#[wasm_bindgen]
pub struct WasmVm {
    interp: Interpreter,
    bridge: Rc<WasmBridge>,
    /// Functions exposed via [`WasmVm::expose_function`]; feeds completion and
    /// hover context.
    exposed_functions: Vec<HostFunctionInfo>,
    /// Registered modules and their exports; feeds completion context.
    module_infos: Vec<ModuleInfo>,
    /// UTF-8 module snapshots used by the language service to propagate types
    /// across imports during hover and diagnostics.
    module_sources: HashMap<String, String>,
    /// Captured `console.*` output for the current run.
    logs: Rc<RefCell<Vec<(String, String)>>>,
    /// The `__out` JS handle, kept so [`WasmVm::reset`] can re-register it.
    out_fn: js_sys::Function,
    /// Kept alive for the VM's lifetime: dropping it invalidates `out_fn`.
    _out_closure: Closure<dyn FnMut(String, String)>,
}

impl Default for WasmVm {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl WasmVm {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        let logs: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let (out_closure, out_fn) = make_out_closure(logs.clone());

        let bridge = Rc::new(WasmBridge::new());
        let mut interp = Interpreter::with_builtins();
        interp.host = Some(bridge.clone());

        // Wire the console sink as a callable VM global before overriding console.
        let id = bridge.register(out_fn.clone());
        interp
            .global
            .borrow_mut()
            .set("__out", Value::host_function("__out", id));
        run_setup(&mut interp);

        Self {
            interp,
            bridge,
            exposed_functions: Vec::new(),
            module_infos: Vec::new(),
            module_sources: HashMap::new(),
            logs,
            out_fn,
            _out_closure: out_closure,
        }
    }

    /// Execute a script. Returns `{ ok, value, error, logs }`. `value` is the
    /// pretty-printed result (empty on error); `error` is empty on success.
    pub fn run(&mut self, source: &str) -> JsValue {
        self.run_source(None, source)
    }

    /// Execute a source file with its workspace path as the module context.
    /// Relative imports are resolved from this path, without assuming a
    /// particular entry-file name.
    pub fn run_file(&mut self, name: &str, source: &str) -> JsValue {
        self.run_source(Some(name), source)
    }

    fn run_source(&mut self, module_name: Option<&str>, source: &str) -> JsValue {
        self.logs.borrow_mut().clear();
        self.interp.cur_mod = module_name.map(ToString::to_string);
        self.interp.set_source(source);
        self.interp.begin_execution();
        let toks = Lexer::new(source).tokenize_with_spans();
        let mut parser = Parser::new_with_spans(toks);
        let stmts = parser.parse();
        let result = if parser.depth_exceeded {
            Err(VmErr::Msg(
                "RangeError: Maximum parse depth exceeded".to_string(),
            ))
        } else {
            self.interp
                .run_program_body(&stmts)
                .and_then(|value| self.interp.drain_jobs().map(|()| value))
                .map_err(|e| self.interp.enrich_error(e, None))
        };
        self.interp.cur_mod = None;
        self.build_run_result(result)
    }

    /// Expose a browser function to the VM as a callable global. Arguments and
    /// the return value are marshalled across the boundary; a thrown error
    /// propagates into the VM as a catchable exception. The name also becomes a
    /// completion candidate.
    pub fn expose_function(&mut self, name: &str, func: js_sys::Function) -> Result<(), JsValue> {
        self.register_exposed_function(name, func, HostFunctionInfo::unknown(name))
    }

    /// Expose a browser function and provide language-service metadata.
    /// `metadata` is an object shaped like:
    /// `{ params: [{ name, type }], returns, documentation?, async? }`.
    pub fn expose_function_with_info(
        &mut self,
        name: &str,
        func: js_sys::Function,
        metadata: JsValue,
    ) -> Result<(), JsValue> {
        let info = host_function_info_from_js(name, &metadata)?;
        self.register_exposed_function(name, func, info)
    }

    fn register_exposed_function(
        &mut self,
        name: &str,
        func: js_sys::Function,
        info: HostFunctionInfo,
    ) -> Result<(), JsValue> {
        let id = self.bridge.register(func);
        if let Err(error) = self
            .interp
            .set_global_checked(name, Value::host_function(name, id))
        {
            return Err(JsValue::from_str(&error.to_string()));
        }
        if let Some(existing) = self
            .exposed_functions
            .iter_mut()
            .find(|function| function.name == name)
        {
            *existing = info;
        } else {
            self.exposed_functions.push(info);
        }
        Ok(())
    }

    /// Register an importable module. Its `export`s are recorded so that
    /// `import * as ns from 'name'` completions can offer them.
    pub fn register_module(&mut self, name: &str, source: &str) -> Result<(), JsValue> {
        self.interp.cur_mod = Some(name.to_string());
        self.interp.begin_execution();
        let toks = Lexer::new(source).tokenize_with_spans();
        let mut parser = Parser::new_with_spans(toks);
        let stmts = parser.parse();
        if parser.depth_exceeded {
            self.interp.cur_mod = None;
            return Err(JsValue::from_str(
                "RangeError: Maximum parse depth exceeded",
            ));
        }
        let result = self
            .interp
            .run_program_body(&stmts)
            .and_then(|value| self.interp.drain_jobs().map(|()| value));
        self.interp.cur_mod = None;
        result.map_err(|e| JsValue::from_str(&e.to_string()))?;

        let exports = self
            .interp
            .module(name)
            .as_ref()
            .map(|m| m.exports.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(mi) = self.module_infos.iter_mut().find(|m| m.name == name) {
            mi.exports = exports;
        } else {
            self.module_infos.push(ModuleInfo {
                name: name.to_string(),
                exports,
            });
        }
        self.module_sources
            .insert(name.to_string(), source.to_string());
        Ok(())
    }

    /// Cap loop iterations per execution (default 100M). When exhausted, the VM
    /// throws a catchable `RangeError` instead of freezing the page.
    pub fn set_loop_limit(&mut self, n: u32) {
        self.interp.set_loop_budget(n as u64);
    }

    /// Read a global as its pretty-printed string (`"undefined"` if absent).
    pub fn get_global(&self, name: &str) -> String {
        match self.interp.global_value(name) {
            Some(val) => {
                crate::format::try_to_string(&val).unwrap_or_else(|error| error.to_string())
            }
            None => "undefined".to_string(),
        }
    }

    /// Reset to a fresh VM: new interpreter and bridge, console re-captured,
    /// exposed-function/module tracking cleared. Callers re-expose what they need.
    pub fn reset(&mut self) {
        let bridge = Rc::new(WasmBridge::new());
        let mut interp = Interpreter::with_builtins();
        interp.host = Some(bridge.clone());
        let id = bridge.register(self.out_fn.clone());
        interp
            .global
            .borrow_mut()
            .set("__out", Value::host_function("__out", id));
        run_setup(&mut interp);

        self.interp = interp;
        self.bridge = bridge;
        self.exposed_functions.clear();
        self.module_infos.clear();
        self.module_sources.clear();
        self.logs.borrow_mut().clear();
    }

    // -- Language services (shared with the LSP and native GUIs) -------------

    /// Completion candidates at `offset` (a byte offset into `source`). Returns
    /// an array of `{ label, kind, detail }`.
    pub fn complete(&self, source: &str, offset: usize) -> JsValue {
        let ctx = self.analysis_context();
        let mut completions = crate::lang::complete(source, offset, &ctx);
        if let Some((receiver, prefix)) = crate::lang::member_trigger(source, offset) {
            let mut seen: std::collections::HashSet<String> =
                completions.iter().map(|item| item.label.clone()).collect();
            for (label, kind) in self.interp.completion_property_members(&receiver) {
                if label.starts_with(&prefix) && seen.insert(label.clone()) {
                    completions.push(Completion {
                        label,
                        kind,
                        detail: Some("runtime member".to_string()),
                    });
                }
            }
            completions.sort_by(|a, b| a.label.cmp(&b.label));
        }

        let arr = js_sys::Array::new();
        for c in completions {
            let o = js_sys::Object::new();
            set_prop(&o, "label", &JsValue::from_str(&c.label));
            set_prop(&o, "kind", &JsValue::from_str(kind_str(c.kind)));
            set_prop(
                &o,
                "detail",
                &JsValue::from_str(&c.detail.unwrap_or_default()),
            );
            arr.push(&o);
        }
        arr.into()
    }

    /// Hover information at a UTF-8 byte offset.
    pub fn hover(&self, source: &str, offset: usize) -> JsValue {
        match crate::lang::Document::parse_with_context(
            source,
            &self.module_sources,
            &self.exposed_functions,
        )
        .hover(offset)
        {
            Some(info) => {
                let object = js_sys::Object::new();
                set_prop(&object, "detail", &JsValue::from_str(&info.detail));
                if let Some(documentation) = info.documentation {
                    set_prop(&object, "documentation", &JsValue::from_str(&documentation));
                }
                object.into()
            }
            None => JsValue::NULL,
        }
    }

    /// Diagnostics for `source`: an array of `{ line, col, message, severity }`
    /// (line/col are 1-based).
    pub fn diagnose(&self, source: &str) -> JsValue {
        let arr = js_sys::Array::new();
        for d in crate::lang::diagnose(source) {
            let o = js_sys::Object::new();
            set_prop(&o, "line", &JsValue::from_f64(d.line as f64));
            set_prop(&o, "col", &JsValue::from_f64(d.col as f64));
            set_prop(&o, "message", &JsValue::from_str(&d.message));
            set_prop(&o, "severity", &JsValue::from_str(sev_str(d.severity)));
            arr.push(&o);
        }
        arr.into()
    }

    /// Top-level document symbols: an array of `{ name, kind, detail }`.
    pub fn symbols(&self, source: &str) -> JsValue {
        let arr = js_sys::Array::new();
        for s in crate::lang::symbols(source) {
            let o = js_sys::Object::new();
            set_prop(&o, "name", &JsValue::from_str(&s.name));
            set_prop(&o, "kind", &JsValue::from_str(kind_str(s.kind)));
            set_prop(
                &o,
                "detail",
                &JsValue::from_str(&s.detail.unwrap_or_default()),
            );
            arr.push(&o);
        }
        arr.into()
    }
}

impl WasmVm {
    fn analysis_context(&self) -> AnalysisContext {
        AnalysisContext {
            exposed_functions: self.exposed_functions.clone(),
            // The browser host has no static manifest layer.
            manifest_functions: Vec::new(),
            modules: self.module_infos.clone(),
            runtime_handlers: std::collections::HashMap::new(),
            runtime_globals: std::collections::HashMap::new(),
            manifest_globals: std::collections::HashMap::new(),
        }
    }

    fn build_run_result(&self, result: Result<Value, VmErr>) -> JsValue {
        let (ok, value, error) = match result {
            Ok(v) => match crate::format::try_to_string(&v) {
                Ok(value) => (true, value, String::new()),
                Err(error) => (false, String::new(), error.to_string()),
            },
            Err(e) => (false, String::new(), e.to_string()),
        };
        let obj = js_sys::Object::new();
        set_prop(&obj, "ok", &JsValue::from_bool(ok));
        set_prop(&obj, "value", &JsValue::from_str(&value));
        set_prop(&obj, "error", &JsValue::from_str(&error));
        set_prop(&obj, "logs", &self.logs_to_js());
        obj.into()
    }

    fn logs_to_js(&self) -> JsValue {
        let arr = js_sys::Array::new();
        for (level, text) in self.logs.borrow().iter() {
            let o = js_sys::Object::new();
            set_prop(&o, "level", &JsValue::from_str(level));
            set_prop(&o, "text", &JsValue::from_str(text));
            arr.push(&o);
        }
        arr.into()
    }
}

fn kind_str(k: CompletionKind) -> &'static str {
    match k {
        CompletionKind::Variable => "variable",
        CompletionKind::Function => "function",
        CompletionKind::Method => "method",
        CompletionKind::Property => "property",
        CompletionKind::Class => "class",
        CompletionKind::Module => "module",
        CompletionKind::Keyword => "keyword",
        CompletionKind::Global => "global",
        CompletionKind::ExposedFn => "exposed",
    }
}

fn sev_str(s: DiagnosticSeverity) -> &'static str {
    match s {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Hint => "hint",
    }
}
