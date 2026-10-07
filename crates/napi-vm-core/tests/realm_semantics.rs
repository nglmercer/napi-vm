use napi_vm_core::{Interpreter, Value};

fn truth(vm: &mut Interpreter, source: &str) {
    let result = vm.eval_source(source);
    assert!(
        matches!(result, Ok(Value::Bool(true))),
        "{source}: {result:?}"
    );
}

#[test]
fn exotic_instances_retain_their_defining_realm_after_collection() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    vm.set_global_checked("otherGlobal", child.realm_global_object())
        .unwrap();
    for (name, source) in [
        ("Date", "new Date(0)"),
        ("RegExp", "new RegExp('x')"),
        ("Promise", "Promise.resolve(42)"),
        ("ArrayBuffer", "new ArrayBuffer(4)"),
        ("SharedArrayBuffer", "new SharedArrayBuffer(4)"),
        ("Uint8Array", "new Uint8Array(4)"),
        ("DataView", "new DataView(new ArrayBuffer(4))"),
    ] {
        let value = child.eval_source(source).unwrap();
        vm.set_global_checked(name, value).unwrap();
    }
    drop(child);
    assert!(vm.collect_cycles().skipped.is_none());
    for name in [
        "Date",
        "RegExp",
        "Promise",
        "ArrayBuffer",
        "SharedArrayBuffer",
        "Uint8Array",
        "DataView",
    ] {
        truth(
            &mut vm,
            &format!("Object.getPrototypeOf({name})===otherGlobal.{name}.prototype;"),
        );
    }
}

#[test]
fn fresh_realms_isolate_globals_intrinsics_and_function_receivers() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    let global = child.realm_global_object();
    vm.set_global_checked("otherGlobal", global.clone())
        .unwrap();
    vm.eval_in_realm(
        &global,
        "var realmSecret=17; function realmFunction(){return this;}",
    )
    .unwrap();
    truth(
        &mut vm,
        "otherGlobal!==globalThis && otherGlobal.globalThis===otherGlobal && otherGlobal.realmSecret===17 && typeof realmSecret==='undefined';",
    );
    truth(
        &mut vm,
        "otherGlobal.Object!==Object && otherGlobal.Array!==Array && otherGlobal.Object.prototype!==Object.prototype;",
    );
    truth(
        &mut vm,
        "var f=otherGlobal.realmFunction;f()===otherGlobal && f.call(null)===otherGlobal;",
    );
    truth(
        &mut vm,
        "Object.getOwnPropertyDescriptor(otherGlobal,'realmSecret').value===17 && Object.prototype.hasOwnProperty.call(otherGlobal,'realmSecret');",
    );
    truth(
        &mut vm,
        "otherGlobal.realmSecret=19;otherGlobal.realmSecret===19;",
    );
    truth(&mut child, "realmSecret===19;");
    truth(
        &mut vm,
        "Object.getPrototypeOf(new otherGlobal.Array())===otherGlobal.Array.prototype;",
    );
    truth(
        &mut vm,
        "Object.getPrototypeOf(otherGlobal)===otherGlobal.Object.prototype;",
    );
    truth(
        &mut vm,
        "Object.getPrototypeOf(otherGlobal.Array.prototype)===otherGlobal.Object.prototype;",
    );
}

#[test]
fn escaped_realm_values_keep_their_intrinsics_after_interpreter_drop() {
    let mut vm = Interpreter::with_builtins();
    let global = {
        let mut child = vm.create_realm();
        child
            .eval_source("var object={x:1};var array=[];function f(){return [this,{}];}")
            .unwrap();
        child.realm_global_object()
    };
    vm.set_global_checked("otherGlobal", global).unwrap();
    vm.collect_cycles();
    truth(
        &mut vm,
        "Object.getPrototypeOf(otherGlobal.object)===otherGlobal.Object.prototype && Object.getPrototypeOf(otherGlobal.array)===otherGlobal.Array.prototype;",
    );
    truth(
        &mut vm,
        "var f=otherGlobal.f;var result=f();result[0]===otherGlobal && Object.getPrototypeOf(result)===otherGlobal.Array.prototype && Object.getPrototypeOf(result[1])===otherGlobal.Object.prototype;",
    );
    let result = vm.eval_in_realm(
        &vm.global_value("otherGlobal").unwrap(),
        "throw new Error('realm error');",
    );
    assert!(result.is_err());
    truth(
        &mut vm,
        "this===globalThis && Object.getPrototypeOf({})===Object.prototype;",
    );
}

#[test]
fn foreign_eval_keeps_its_realm_and_cannot_become_direct_eval() {
    let mut vm = Interpreter::with_builtins();
    let child = vm.create_realm();
    vm.set_global_checked("otherGlobal", child.realm_global_object())
        .unwrap();
    truth(
        &mut vm,
        "var eval=otherGlobal.eval;function f(){var local=17;return eval('typeof local');}f()==='undefined';",
    );
    truth(
        &mut vm,
        "eval('var otherSecret=42');otherGlobal.otherSecret===42 && typeof otherSecret==='undefined';",
    );
    truth(&mut vm, "eval('this')===otherGlobal;");
}

#[test]
fn constructor_default_prototypes_come_from_the_new_target_realm() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    child
        .eval_source("function F(){}F.prototype=undefined;")
        .unwrap();
    vm.set_global_checked("otherGlobal", child.realm_global_object())
        .unwrap();
    for kind in ["Map", "Set", "WeakMap", "WeakSet"] {
        truth(
            &mut vm,
            &format!(
                "Object.getPrototypeOf(Reflect.construct({kind},[],otherGlobal.F))===otherGlobal.{kind}.prototype;"
            ),
        );
    }
    truth(
        &mut vm,
        "function F(){}Object.getPrototypeOf(Reflect.construct(F,[],otherGlobal.F))===otherGlobal.Object.prototype;",
    );
    truth(
        &mut vm,
        "var bound=otherGlobal.F.bind(null);bound.prototype=undefined;Object.getPrototypeOf(Reflect.construct(F,[],bound))===otherGlobal.Object.prototype;",
    );
    truth(
        &mut vm,
        "function G(){}Object.defineProperty(G,'prototype',{value:1});Object.getPrototypeOf(Reflect.construct(WeakMap,[],G))===WeakMap.prototype;",
    );
}

#[test]
fn foreign_generators_and_async_resumes_allocate_in_the_defining_realm() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    child
        .eval_source("function* g(){yield {};yield [];}async function f(){await 0;return [];}")
        .unwrap();
    vm.set_global_checked("otherGlobal", child.realm_global_object())
        .unwrap();
    truth(
        &mut vm,
        "var g=otherGlobal.g();var a=g.next().value;var b=g.next().value;Object.getPrototypeOf(a)===otherGlobal.Object.prototype && Object.getPrototypeOf(b)===otherGlobal.Array.prototype;",
    );
    truth(
        &mut vm,
        "var result=await otherGlobal.f();Object.getPrototypeOf(result)===otherGlobal.Array.prototype;",
    );
}

#[test]
fn realm_script_sources_preserve_unpaired_utf16_surrogates() {
    let mut vm = Interpreter::with_builtins();
    let child = vm.create_realm();
    let global = child.realm_global_object();
    let source = napi_vm_core::JsString::from_units(vec![39, 0xd800, 39]);
    let result = vm.eval_in_realm_utf16(&global, &source).unwrap();
    assert!(matches!(result, Value::String(ref value) if value.units() == [0xd800]));
    let mut units: Vec<u16> = "import.meta; '".encode_utf16().collect();
    units.extend([0xd800, 39]);
    let source = napi_vm_core::JsString::from_units(units);
    assert!(vm.eval_in_realm_utf16(&global, &source).is_err());
    truth(&mut vm, "this===globalThis;");
}

#[test]
fn module_sources_are_copied_but_evaluation_and_namespace_identity_are_isolated() {
    let mut vm = Interpreter::with_builtins();
    vm.define_module("shared", "globalThis.runs=(globalThis.runs||0)+1;export const count=globalThis.runs;export const array=[];".into());
    vm.load_module("shared").unwrap();
    let mut child = vm.create_realm();
    assert!(child.module("shared").is_none());
    child.load_module("shared").unwrap();
    truth(&mut vm, "runs===1;");
    truth(&mut child, "runs===1;");
    let parent_exports = vm.module("shared").unwrap();
    let child_exports = child.module("shared").unwrap();
    assert!(!std::rc::Rc::ptr_eq(
        &parent_exports.namespace,
        &child_exports.namespace
    ));
    vm.set_global_checked("childArray", child_exports.exports["array"].deref_binding())
        .unwrap();
    vm.set_global_checked("childGlobal", child.realm_global_object())
        .unwrap();
    truth(
        &mut vm,
        "Object.getPrototypeOf(childArray)===childGlobal.Array.prototype;",
    );
    assert!(child.remove_module("shared"));
    child.define_module("shared", "export const count=99;".into());
    assert!(vm.module("shared").is_some());
    child.load_module("shared").unwrap();
    truth(&mut vm, "import {count} from 'shared';count===1;");
    truth(&mut child, "import {count} from 'shared';count===99;");
}

#[test]
fn escaped_bytecode_functions_import_from_their_own_realm_and_referrer() {
    let mut vm = Interpreter::with_builtins();
    vm.define_module(
        "pkg/main",
        "export function load(){return import('./dep');}export function fallback(p=1){return import('./dep');}".into(),
    );
    vm.define_module("dep", "export const value=10;".into());
    vm.define_module_alias("pkg/main", "./dep", "dep");
    let mut child = vm.create_realm();
    child.define_module("dep", "export const value=20;".into());
    child.load_module("pkg/main").unwrap();
    let load = child.module("pkg/main").unwrap().exports["load"].deref_binding();
    vm.set_global_checked("loadChild", load).unwrap();
    let fallback = child.module("pkg/main").unwrap().exports["fallback"].deref_binding();
    assert!(matches!(&fallback, Value::Function(function) if function.bytecode.is_none()));
    vm.set_global_checked("loadFallbackChild", fallback)
        .unwrap();
    drop(child);
    assert!(vm.collect_cycles().skipped.is_none());
    vm.eval_source("var answer=0;loadChild().then(ns=>{answer=ns.value;});")
        .unwrap();
    truth(&mut vm, "answer===20;");
    vm.eval_source("loadFallbackChild().then(ns=>{answer=ns.value+1;});")
        .unwrap();
    truth(&mut vm, "answer===21;");
    assert!(vm.module("dep").is_none());
    vm.load_module("dep").unwrap();
    truth(&mut vm, "import {value} from 'dep';value===10;");
}

#[test]
fn shared_scheduler_evaluates_each_pending_import_in_its_own_realm() {
    let mut vm = Interpreter::with_builtins();
    vm.define_module(
        "same",
        "export const value=globalThis.tag;export const array=[];".into(),
    );
    vm.eval_source("var tag=10;").unwrap();
    let mut child = vm.create_realm();
    child.eval_source("var tag=20;").unwrap();
    let parent_import = vm.import_module("same").unwrap();
    let child_import = child.import_module("same").unwrap();
    vm.set_global_checked("parentImport", parent_import)
        .unwrap();
    vm.set_global_checked("childImport", child_import).unwrap();
    vm.set_global_checked("childGlobal", child.realm_global_object())
        .unwrap();
    drop(child);
    assert!(vm.collect_cycles().skipped.is_none());
    vm.eval_source("var a,b;parentImport.then(ns=>{a=ns.value;});childImport.then(ns=>{b=ns.value;globalThis.childArray=ns.array;});").unwrap();
    truth(
        &mut vm,
        "a===10&&b===20&&Object.getPrototypeOf(childArray)===childGlobal.Array.prototype;",
    );
}

#[test]
fn suspended_child_module_resumes_on_the_parent_scheduler_after_child_drop() {
    let mut vm = Interpreter::with_builtins();
    vm.define_module("pending", "export const ready=await new Promise(resolve=>{globalThis.release=resolve;});export const value=globalThis.tag;".into());
    vm.eval_source("var tag=10;").unwrap();
    let mut child = vm.create_realm();
    child.eval_source("var tag=20;").unwrap();
    let completion = child.import_module("pending").unwrap();
    vm.set_global_checked("completion", completion).unwrap();
    vm.set_global_checked("childGlobal", child.realm_global_object())
        .unwrap();
    vm.eval_source("var ready,value;completion.then(ns=>{ready=ns.ready;value=ns.value;});")
        .unwrap();
    let release = child.global.borrow().get("release").unwrap();
    vm.set_global_checked("releaseChild", release).unwrap();
    drop(child);
    vm.eval_source("releaseChild(7);").unwrap();
    truth(
        &mut vm,
        "ready===7&&value===20&&typeof release==='undefined';",
    );
    assert!(vm.module("pending").is_none());
}

#[test]
fn commonjs_instances_and_escaped_require_use_the_defining_realm() {
    struct Loader;
    impl napi_vm_core::CommonJsModuleLoader for Loader {
        fn resolve(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> Result<napi_vm_core::ResolvedCommonJsModule, napi_vm_core::VmErr> {
            Ok(napi_vm_core::ResolvedCommonJsModule {
                id: "pkg".into(),filename:"/virtual/pkg.js".into(),
                format:napi_vm_core::CommonJsModuleFormat::JavaScript,
                source:Some("globalThis.cjsRuns=(globalThis.cjsRuns||0)+1;module.exports={tag:globalThis.tag};".into()),
            })
        }
    }
    let loader = std::rc::Rc::new(Loader);
    let mut vm = Interpreter::with_builtins();
    vm.set_commonjs_loader(loader.clone()).unwrap();
    vm.eval_source("var tag=10;var parentPackage=require('pkg');")
        .unwrap();
    let mut child = vm.create_realm();
    child.set_commonjs_loader(loader).unwrap();
    child
        .eval_source("var tag=20;var childPackage=require('pkg');")
        .unwrap();
    vm.set_global_checked(
        "childRequire",
        child.global.borrow().get("require").unwrap(),
    )
    .unwrap();
    drop(child);
    truth(
        &mut vm,
        "parentPackage.tag===10&&childRequire('pkg').tag===20&&childRequire('pkg')!==parentPackage&&cjsRuns===1;",
    );
}
