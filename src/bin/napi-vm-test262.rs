//! One test per process. Corpus orchestration applies a hard process timeout.
use napi_vm::interpreter::ExecutionBudget;
use napi_vm::{Interpreter, ModuleLoader, ModuleSource, Value, VirtualLoader, VmErr};
use serde::Deserialize;
use serde_json::{Value as Json, json};
use std::io::{self, Read};
use std::rc::Rc;
thread_local! { static GC_REQUESTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

#[derive(Deserialize)]
struct Request {
    source: String,
    #[serde(default)]
    harness: String,
    #[serde(default)]
    module: bool,
    #[serde(default)]
    asynchronous: bool,
    #[serde(default)]
    modules: std::collections::HashMap<String, String>,
    #[serde(default = "default_id")]
    id: String,
    /// Explicit host capability used only by this isolated conformance worker.
    #[serde(default)]
    corpus_root: Option<std::path::PathBuf>,
}

struct CorpusLoader {
    virtual_loader: VirtualLoader,
    root: Option<std::path::PathBuf>,
    entry: String,
}
impl CorpusLoader {
    fn path(&self, id: &str) -> Result<std::path::PathBuf, VmErr> {
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| VmErr::Msg("TypeError: Module not found".into()))?;
        let path = root
            .join(id)
            .canonicalize()
            .map_err(|_| VmErr::Msg(format!("TypeError: Module not found: {id}")))?;
        if !path.starts_with(root) || !path.is_file() {
            return Err(VmErr::Msg(
                "TypeError: Module escapes the test corpus".into(),
            ));
        }
        Ok(path)
    }
}
impl ModuleLoader for CorpusLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, VmErr> {
        let referrer = referrer.unwrap_or(&self.entry);
        if let Ok(id) = self.virtual_loader.resolve(specifier, Some(referrer)) {
            return Ok(id);
        }
        let id = if specifier.starts_with('.') {
            std::path::Path::new(referrer)
                .parent()
                .unwrap_or_else(|| std::path::Path::new(""))
                .join(specifier)
        } else {
            std::path::PathBuf::from(specifier)
        };
        let path = self.path(&id.to_string_lossy())?;
        Ok(path
            .strip_prefix(self.root.as_ref().expect("checked root"))
            .expect("checked containment")
            .to_string_lossy()
            .replace('\\', "/"))
    }
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
        if let Ok(source) = self.virtual_loader.load(id) {
            return Ok(source);
        }
        let path = self.path(id)?;
        let metadata = path.metadata().map_err(|e| VmErr::Msg(e.to_string()))?;
        if metadata.len() > 32 * 1024 * 1024 {
            return Err(VmErr::Msg(
                "ResourceLimit: Module source size exceeded".into(),
            ));
        }
        let source = std::fs::read_to_string(path).map_err(|e| VmErr::Msg(e.to_string()))?;
        Ok(ModuleSource {
            id: id.into(),
            source,
        })
    }
}
fn default_id() -> String {
    "test.js".into()
}
fn done(vm: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let count = vm
        .persistent_global
        .borrow()
        .get("__test262_done_count")
        .map(|v| v.to_number())
        .unwrap_or(0.0);
    vm.persistent_global
        .borrow_mut()
        .set("__test262_done_count", Value::Number(count + 1.0));
    if let Some(error) = args.first()
        && !matches!(error, Value::Undefined)
    {
        vm.persistent_global
            .borrow_mut()
            .set("__test262_done_error", error.clone());
        return Err(VmErr::Throw(error.clone()));
    }
    Ok(Value::Undefined)
}
fn detach_array_buffer(_: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let Some(Value::ArrayBuffer(buffer)) = args.first() else {
        return Err(VmErr::Msg(
            "TypeError: detachArrayBuffer requires an ArrayBuffer".into(),
        ));
    };
    if args
        .get(1)
        .is_some_and(|key| !matches!(key, Value::Undefined))
    {
        return Err(VmErr::Msg(
            "TypeError: ArrayBuffer detachment key mismatch".into(),
        ));
    }
    buffer.detach();
    Ok(Value::Undefined)
}

fn request_gc(_: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    // Collection is only safe at a host boundary: defer requests made while
    // guest frames are live until execute() has returned from evaluation.
    GC_REQUESTED.with(|requested| requested.set(true));
    Ok(Value::Undefined)
}

fn eval_realm_script(vm: &mut Interpreter, _: Value, args: Vec<Value>) -> Result<Value, VmErr> {
    let source = vm.vs(args.first().unwrap_or(&Value::Undefined))?;
    let global = vm.realm_global_object();
    vm.eval_in_realm(&global, &source)
}

fn create_realm(vm: &mut Interpreter, _: Value, _: Vec<Value>) -> Result<Value, VmErr> {
    let mut child = vm.create_realm();
    let host = realm_host(&mut child);
    child.global.borrow_mut().set("$262", host.clone());
    Ok(host)
}

fn realm_host(vm: &mut Interpreter) -> Value {
    let host = Value::object(vec![
        ("global".into(), vm.realm_global_object()),
        (
            "evalScript".into(),
            vm.native_function_in_realm("evalScript", eval_realm_script),
        ),
        (
            "createRealm".into(),
            vm.native_function_in_realm("createRealm", create_realm),
        ),
        (
            "detachArrayBuffer".into(),
            vm.native_function_in_realm("detachArrayBuffer", detach_array_buffer),
        ),
        ("gc".into(), vm.native_function_in_realm("gc", request_gc)),
    ]);
    let gc = vm.native_function_in_realm("gc", request_gc);
    vm.global.borrow_mut().set("gc", gc);
    host
}

fn error_type(error: &VmErr) -> String {
    if let VmErr::Throw(value) = error
        && let Some(Value::String(ref name)) = value.get_prop("name")
    {
        return name.to_string();
    }
    let message = error.to_string();
    for name in [
        "Test262Error",
        "TypeError",
        "RangeError",
        "ReferenceError",
        "SyntaxError",
        "Error",
    ] {
        if message.starts_with(name) {
            return name.into();
        }
    }
    "EngineError".into()
}
fn failure(phase: &str, error: &VmErr) -> Json {
    json!({"status": "error", "phase": phase, "error_type": error_type(error), "message": error.to_string()})
}
fn execute(request: Request) -> Json {
    // Parse test source separately: a harness failure cannot satisfy a negative test.
    let goal = if request.module {
        napi_vm::parser::ParseGoal::Module
    } else {
        napi_vm::parser::ParseGoal::Script
    };
    if let Err(error) = Interpreter::compile_with_goal(&request.source, goal) {
        return json!({"status":"error", "phase":"parse", "error_type":"SyntaxError", "message":error.to_string()});
    }
    let mut vm = Interpreter::with_builtins();
    vm.set_execution_budget(ExecutionBudget {
        fuel: 1_000_000,
        max_call_depth: 128,
        max_jobs: 10_000,
    });
    vm.set_loop_budget(100_000);
    let root = match request
        .corpus_root
        .as_ref()
        .map(|root| root.canonicalize())
        .transpose()
    {
        Ok(root) => root,
        Err(error) => {
            return json!({"status":"error", "phase":"driver", "message":error.to_string()});
        }
    };
    let loader = CorpusLoader {
        virtual_loader: VirtualLoader::new(),
        root,
        entry: request.id.clone(),
    };
    for (id, source) in request.modules {
        loader.virtual_loader.insert(id, source);
    }
    loader
        .virtual_loader
        .insert(request.id.clone(), request.source.clone());
    vm.set_module_loader(Rc::new(loader));
    vm.global.borrow_mut().set(
        "$DONE",
        Value::NativeFunction {
            name: "$DONE".into(),
            callable: done,
        },
    );
    vm.global
        .borrow_mut()
        .set("__test262_done_count", Value::Number(0.0));
    let host = realm_host(&mut vm);
    vm.global.borrow_mut().set("$262", host);
    if let Err(error) = vm.eval_source(&request.harness) {
        return failure("harness", &error);
    }
    let result = if request.module {
        match vm.link_module(&request.id) {
            Ok(true) => {}
            Ok(false) => {
                return failure(
                    "resolution",
                    &VmErr::Msg("SyntaxError: Module not found".into()),
                );
            }
            Err(error) => return failure("resolution", &error),
        }
        vm.load_module(&request.id)
            .and_then(|_| vm.drain_jobs())
            .map(|_| Value::Undefined)
    } else {
        vm.eval_source(&request.source)
    };
    if GC_REQUESTED.with(|requested| requested.replace(false)) {
        vm.collect_cycles();
    }
    if let Err(error) = result {
        return failure("runtime", &error);
    }
    if let Some(error) = vm.persistent_global.borrow().get("__test262_done_error") {
        return failure("runtime", &VmErr::Throw(error));
    }
    let count = vm
        .persistent_global
        .borrow()
        .get("__test262_done_count")
        .map(|v| v.to_number())
        .unwrap_or(0.0);
    if request.asynchronous && count != 1.0 {
        return json!({"status":"error", "phase":"runtime", "error_type":"Test262AsyncError", "message":format!("expected one $DONE call, got {count}")});
    }
    json!({"status":"ok", "phase":"runtime"})
}
fn main() {
    let mut input = String::new();
    let report = match io::stdin()
        .take(32 * 1024 * 1024)
        .read_to_string(&mut input)
    {
        Ok(_) => match serde_json::from_str::<Request>(&input) {
            Ok(request) => execute(request),
            Err(error) => json!({"status":"error", "phase":"driver", "message":error.to_string()}),
        },
        Err(error) => json!({"status":"error", "phase":"driver", "message":error.to_string()}),
    };
    println!("{report}");
}
