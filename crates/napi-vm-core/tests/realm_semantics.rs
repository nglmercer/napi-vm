use napi_vm_core::{Interpreter, Value};

#[test]
fn thenable_jobs_retain_the_function_realm_for_resolver_allocation() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    child.eval_source("var saved;var then=function(resolve,reject){saved=[resolve,reject];resolve.call(null,42);};").unwrap();
    vm.set_global_checked("other", child.realm_global_object())
        .unwrap();
    vm.set_global_checked("foreignThen", child.eval_source("then").unwrap())
        .unwrap();
    drop(child);
    vm.eval_source_with_options(
        "var settled;Promise.resolve({then:foreignThen}).then(v=>{settled=v;});",
        napi_vm_core::interpreter::EvaluationOptions {
            drain: napi_vm_core::interpreter::DrainPolicy::None,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(vm.collect_cycles().skipped.is_none());
    vm.drain_jobs().unwrap();
    truth(
        &mut vm,
        "settled===42&&Object.getPrototypeOf(other.saved[0])===other.Function.prototype&&Object.getPrototypeOf(other.saved[1])===other.Function.prototype;",
    );
    truth(
        &mut vm,
        "Object.getOwnPropertyNames(other.saved[0]).length===2&&Object.prototype.toString.call(other.saved[0])==='[object Function]';",
    );
}

#[test]
fn string_and_regexp_brand_errors_belong_to_the_accessor_realm() {
    let mut vm = Interpreter::with_builtins();
    let child = vm.create_realm();
    vm.set_global_checked("other", child.realm_global_object())
        .unwrap();
    truth(
        &mut vm,
        "var caught;try{other.String.prototype.valueOf.call({valueOf(){return '';}});}catch(e){caught=e;}caught instanceof other.TypeError&&other.String.prototype.valueOf.call(Object('ok'))==='ok';",
    );
    for name in [
        "source",
        "global",
        "ignoreCase",
        "multiline",
        "dotAll",
        "sticky",
        "unicode",
        "unicodeSets",
        "hasIndices",
    ] {
        truth(
            &mut vm,
            &format!(
                "var getter=Object.getOwnPropertyDescriptor(other.RegExp.prototype,'{name}').get;var caught;try{{getter.call(RegExp.prototype);}}catch(e){{caught=e;}}caught instanceof other.TypeError&&Object.getPrototypeOf(getter)===other.Function.prototype;"
            ),
        );
    }
    truth(
        &mut vm,
        "other.RegExp.prototype.source==='(?:)'&&other.RegExp.prototype.global===undefined;",
    );
}

#[test]
fn shared_typed_elements_use_observable_numeric_and_bigint_conversion() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "var count=0;var b=new SharedArrayBuffer(16);var a=new BigInt64Array(b);a[0]={valueOf(){count++;return '42';}};a.fill(true,1);Atomics.add(a,0,'1')===42n&&a[0]===43n&&a[1]===1n&&count===1;",
    );
    truth(
        &mut vm,
        "var order='';var source={length:2,get 0(){order+='a';return {valueOf(){order+='b';return '7';}};},get 1(){order+='c';return '8';}};var copied=new BigInt64Array(source);order==='abc'&&copied[0]===7n&&copied[1]===8n;",
    );
    truth(
        &mut vm,
        "var calls=0;var source={get [Symbol.iterator](){calls++;return function(){return ['0','1'][Symbol.iterator]();};}};var a=new BigUint64Array(source);calls===1&&a[1]===1n;",
    );
    truth(
        &mut vm,
        "var view=new BigInt64Array(new ArrayBuffer(8));Atomics.add(view,0,'2')===0n&&view[0]===2n;",
    );
    truth(
        &mut vm,
        "var caught;try{new BigInt64Array(new Uint8Array(0));}catch(e){caught=e;}caught instanceof TypeError;",
    );
    truth(
        &mut vm,
        "new Uint8Array(2.9).length===2&&new Uint8Array('3').length===3;",
    );
}

#[test]
fn buffer_slice_observes_species_and_allocates_in_the_method_realm() {
    let mut vm = Interpreter::with_builtins();
    let child = vm.create_realm();
    vm.set_global_checked("other", child.realm_global_object())
        .unwrap();
    for name in ["ArrayBuffer", "SharedArrayBuffer"] {
        truth(
            &mut vm,
            &format!(
                "var C={name};var b=new C(8);var bytes=new Uint8Array(b);bytes[2]=42;var order='';var chosen;var ctor={{get [Symbol.species](){{order+='s';return function(n){{order+='c';return chosen=new C(n+1);}};}}}};Object.defineProperty(b,'constructor',{{get(){{order+='k';return ctor;}}}});var result=b.slice({{valueOf(){{order+='a';return 2;}}}},{{valueOf(){{order+='z';return 4;}}}});order==='azksc'&&result===chosen&&result.byteLength===3&&new Uint8Array(result)[0]===42;"
            ),
        );
        truth(
            &mut vm,
            &format!(
                "var fresh=new C(4);fresh.constructor=undefined;var foreign=other.{name}.prototype.slice.call(fresh);Object.getPrototypeOf(foreign)===other.{name}.prototype;"
            ),
        );
        truth(
            &mut vm,
            "var same=new C(4);same.constructor={[Symbol.species]:function(){return same;}};var caught;try{same.slice();}catch(e){caught=e;}caught instanceof TypeError;",
        );
        truth(
            &mut vm,
            "C[Symbol.species]===C&&Object.getOwnPropertyDescriptor(C,'length').value===1&&C.prototype.slice.length===2;",
        );
    }
    truth(
        &mut vm,
        "SharedArrayBuffer.prototype[Symbol.toStringTag]==='SharedArrayBuffer';",
    );
}

#[test]
fn implicit_derived_constructors_forward_arguments_without_iteration() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "Array.prototype[Symbol.iterator]=function(){throw 42;};class Base{constructor(a,b){this.sum=a+b;}}class Derived extends Base{field=7;}class Leaf extends Derived{}var instance=new Leaf(2,3);instance.sum===5&&instance.field===7&&instance instanceof Leaf;",
    );
}

#[test]
fn promise_resolving_functions_have_call_but_no_construct() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "var resolve,reject;new Promise(function(a,b){resolve=a;reject=b;});var count=0;for(var fn of [resolve,reject]){try{Reflect.construct(function(){},[],fn);}catch(e){if(e instanceof TypeError)count++;}try{new fn();}catch(e){if(e instanceof TypeError)count++;}}count===4;",
    );
    truth(
        &mut vm,
        "var settled=false;var p=new Promise(function(resolve){resolve(42);});p.then(function(v){settled=v===42;});true;",
    );
    truth(&mut vm, "settled;");
    truth(
        &mut vm,
        "resolve.name===''&&resolve.length===1&&reject.name===''&&reject.length===1&&Object.getPrototypeOf(resolve)===Function.prototype&&Object.getPrototypeOf(reject)===Function.prototype;",
    );
    truth(
        &mut vm,
        "var p=new Promise(function(resolve){resolve.call({unrelated:true},7);});var result;p.then(function(value){result=value;});true;",
    );
    truth(&mut vm, "result===7;");
    truth(
        &mut vm,
        "Promise.resolve(1).then(()=>Promise.resolve());var savedThen=Promise.prototype.then;var checked=0;Promise.prototype.then=function(resolve,reject){if(resolve.name!==''||resolve.length!==1||reject.name!==''||reject.length!==1)throw 42;checked++;return savedThen.call(this,resolve,reject);};true;",
    );
    truth(&mut vm, "checked>0;");
}

#[test]
fn promise_combinators_reject_iterator_step_errors_without_closing() {
    let mut vm = Interpreter::with_builtins();
    for method in ["all", "allSettled", "race", "any"] {
        truth(
            &mut vm,
            &format!(
                "var error={{}};var closed=0;var rejected=false;var iterable={{[Symbol.iterator](){{return {{next(){{return {{get done(){{throw error;}},get value(){{throw 42;}}}};}},return(){{closed++;return {{}};}}}};}}}};Promise.{method}(iterable).then(undefined,function(e){{rejected=e===error;}});closed===0;"
            ),
        );
        truth(&mut vm, "rejected===true;");
    }
}

#[test]
fn concrete_typed_array_metadata_identifies_each_realms_constructor() {
    let mut vm = Interpreter::with_builtins();
    let child = vm.create_realm();
    vm.set_global_checked("other", child.realm_global_object())
        .unwrap();
    for name in [
        "Int8Array",
        "Uint8Array",
        "Uint8ClampedArray",
        "Int16Array",
        "Uint16Array",
        "Int32Array",
        "Uint32Array",
        "Float32Array",
        "Float64Array",
        "BigInt64Array",
        "BigUint64Array",
    ] {
        truth(
            &mut vm,
            &format!(
                "var C={name};var d=Object.getOwnPropertyDescriptor(C,'name');C.name==='{name}'&&d.value===C.name&&!d.writable&&!d.enumerable&&d.configurable&&C.length===3&&Object.getPrototypeOf(C)===Object.getPrototypeOf(Uint8Array);"
            ),
        );
        truth(
            &mut vm,
            &format!(
                "var target=other.{name}.bind(null);target.prototype=undefined;Object.getPrototypeOf(Reflect.construct({name},[0],target))===other[C.name].prototype;"
            ),
        );
    }
}

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

#[test]
fn independent_primary_globals_keep_their_identity_and_storage() {
    let mut first = Interpreter::with_builtins();
    let mut second = Interpreter::with_builtins();
    first
        .eval_source("var tag=11;function receiver(){return this;}var array=[];")
        .unwrap();
    second.eval_source("var tag=22;").unwrap();
    second
        .set_global_checked("firstGlobal", first.realm_global_object())
        .unwrap();
    truth(
        &mut second,
        "firstGlobal!==globalThis&&firstGlobal.globalThis===firstGlobal&&firstGlobal.tag===11&&tag===22;",
    );
    truth(
        &mut second,
        "var f=firstGlobal.receiver;f()===firstGlobal&&Object.getPrototypeOf(firstGlobal.array)===firstGlobal.Array.prototype;",
    );
    truth(
        &mut second,
        "firstGlobal.tag=33;firstGlobal.tag===33&&tag===22;",
    );
    truth(&mut first, "tag===33&&this===globalThis;");
    drop(first);
    assert!(second.collect_cycles().skipped.is_none());
    truth(
        &mut second,
        "firstGlobal.tag===33&&firstGlobal.receiver()===firstGlobal&&Object.getPrototypeOf(firstGlobal)===firstGlobal.Object.prototype;",
    );
}

#[test]
fn weak_collections_keep_live_primary_globals_and_release_dead_realms() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(
        "var map=new WeakMap();map.set(globalThis,{answer:42});var ref=new WeakRef(globalThis);",
    )
    .unwrap();
    assert!(vm.collect_cycles().skipped.is_none());
    truth(
        &mut vm,
        "ref.deref()===globalThis&&map.get(globalThis).answer===42;",
    );
    let foreign = Interpreter::with_builtins();
    vm.set_global_checked("foreign", foreign.realm_global_object())
        .unwrap();
    vm.eval_source("var foreignRef=new WeakRef(foreign);map.set(foreign,{owner:foreign});")
        .unwrap();
    drop(foreign);
    assert!(vm.collect_cycles().skipped.is_none());
    truth(
        &mut vm,
        "foreignRef.deref()===foreign&&map.get(foreign).owner===foreign;",
    );
    vm.eval_source("foreign=undefined;").unwrap();
    assert!(vm.collect_cycles().skipped.is_none());
    truth(
        &mut vm,
        "foreignRef.deref()===undefined&&ref.deref()===globalThis;",
    );
}

#[test]
fn foreign_native_and_guest_errors_keep_their_originating_intrinsics() {
    let mut vm = Interpreter::with_builtins();
    let mut other = Interpreter::with_builtins();
    other.eval_source("function bad(){null.x;}function* generator(){null.x;}async function asynchronous(){await 0;null.x;}function F(){}").unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    drop(other);
    for source in [
        "var caught;try{other.bad();}catch(e){caught=e;}caught.constructor===other.TypeError&&Object.getPrototypeOf(caught)===other.TypeError.prototype&&!(caught instanceof TypeError);",
        "try{other.Object.getPrototypeOf(undefined);}catch(e){caught=e;}caught.constructor===other.TypeError&&Object.getPrototypeOf(caught)===other.TypeError.prototype;",
        "try{other.generator().next();}catch(e){caught=e;}caught.constructor===other.TypeError;",
        "try{await other.asynchronous();}catch(e){caught=e;}caught.constructor===other.TypeError;",
        "Object.getPrototypeOf(other.F.prototype)===other.Object.prototype;",
        "try{other.Function('return )');}catch(e){caught=e;}caught.constructor===other.SyntaxError;",
    ] {
        truth(&mut vm, source);
    }
    assert!(vm.collect_cycles().skipped.is_none());
    truth(&mut vm, "caught.constructor===other.SyntaxError;");
}

#[test]
fn iterators_have_shared_realm_owned_prototypes_and_methods() {
    let mut vm = Interpreter::with_builtins();
    let mut other = Interpreter::with_builtins();
    other.eval_source("function* generator(){yield 1;}var g=generator();var arrayIterator=[1].values();var stringIterator='x'[Symbol.iterator]();").unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    drop(other);
    truth(
        &mut vm,
        "Object.getPrototypeOf(other.g)===other.generator.prototype&&other.g.next===Object.getPrototypeOf(other.generator.prototype).next;",
    );
    truth(
        &mut vm,
        "Object.getPrototypeOf(other.arrayIterator)!==Object.getPrototypeOf([].values())&&Object.getPrototypeOf(other.arrayIterator.next)===other.Function.prototype;",
    );
    truth(
        &mut vm,
        "Object.getPrototypeOf(other.stringIterator)!==Object.getPrototypeOf(''[Symbol.iterator]())&&Object.getPrototypeOf(other.stringIterator.next)===other.Function.prototype;",
    );
    truth(
        &mut vm,
        "other.arrayIterator.next().value===1&&other.stringIterator.next().value==='x'&&other.g.next().value===1;",
    );
    // Finish the generator before the conservative opaque-stack collection boundary.
    truth(&mut vm, "other.g.next().done;");
    assert!(vm.collect_cycles().skipped.is_none());
    truth(
        &mut vm,
        "Object.getPrototypeOf(other.arrayIterator.next)===other.Function.prototype;",
    );
}

#[test]
fn constructor_post_return_errors_belong_to_the_constructing_caller() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    child.eval_source("var Primitive=class extends Object{constructor(){return null;}};var Missing=class extends Object{constructor(){}};var Thrown=class extends Object{constructor(){null.x;}};").unwrap();
    vm.set_global_checked("other", child.realm_global_object())
        .unwrap();
    for source in [
        "var caught;try{new other.Primitive();}catch(e){caught=e;}caught.constructor===TypeError;",
        "try{new other.Missing();}catch(e){caught=e;}caught.constructor===ReferenceError;",
        "try{new other.Thrown();}catch(e){caught=e;}caught.constructor===other.TypeError;",
        "try{Reflect.construct(other.Primitive,[]);}catch(e){caught=e;}caught.constructor===TypeError;",
        "try{Reflect.construct(other.Missing,[]);}catch(e){caught=e;}caught.constructor===ReferenceError;",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn cross_realm_instances_use_prototypes_for_brand_checks() {
    let mut vm = Interpreter::with_builtins();
    let other = vm.create_realm();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    for (kind, args) in [
        ("Object", ""),
        ("Array", ""),
        ("Date", "0"),
        ("RegExp", "'x'"),
        ("Map", ""),
        ("Set", ""),
        ("WeakMap", ""),
        ("WeakSet", ""),
        ("ArrayBuffer", "4"),
        ("SharedArrayBuffer", "4"),
        ("Int32Array", "4"),
        ("DataView", "new other.ArrayBuffer(4)"),
        ("WeakRef", "{}"),
        ("FinalizationRegistry", "()=>{}"),
        ("Error", "'x'"),
    ] {
        truth(
            &mut vm,
            &format!(
                "var instance=new other.{kind}({args});instance instanceof other.{kind}&&!(instance instanceof {kind});"
            ),
        );
    }
}

#[test]
fn error_prototype_chains_and_generic_to_string_preserve_realm_semantics() {
    let mut vm = Interpreter::with_builtins();
    let other = vm.create_realm();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    for source in [
        "Object.getPrototypeOf(other.Error.prototype)===other.Object.prototype;",
        "Object.getPrototypeOf(other.TypeError)===other.Error&&other.Error.length===1;",
        "var called=other.TypeError.call(null,'message');called instanceof other.TypeError&&called.message==='message'&&!Object.prototype.hasOwnProperty.call(called,'name');",
        "var noMessage=other.Error();!Object.prototype.hasOwnProperty.call(noMessage,'message');",
        "var caused=other.Error('x',{cause:42});caused.cause===42&&!Object.getOwnPropertyDescriptor(caused,'cause').enumerable;",
        "var Derived=class extends other.Error{};var derived=new Derived('x');derived instanceof Derived&&derived instanceof other.Error&&derived.message==='x';",
        "var error=new other.TypeError('message');error instanceof other.Object&&error instanceof other.Error&&!(error instanceof Object);",
        "Error.prototype.toString.call({name:'',message:'message'})==='message';",
        "Error.prototype.toString.call({name:12,message:34})==='12: 34';",
        "Error.prototype.toString.call({})==='Error';",
        "var order='';Error.prototype.toString.call({name:{toString(){order+='n';return 'N';}},get message(){order+='m';return 'M';}})==='N: M'&&order==='nm';",
        "var caught;try{other.Error.prototype.toString.call(1);}catch(e){caught=e;}caught instanceof other.TypeError;",
        "var marker={};try{Error.prototype.toString.call({name:Symbol(),get message(){throw marker;}});}catch(e){caught=e;}caught instanceof TypeError;",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn weak_intrinsics_and_iterator_aliases_have_owned_standard_metadata() {
    let mut vm = Interpreter::with_builtins();
    let other = vm.create_realm();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    for source in [
        "other.Array.prototype.values===other.Array.prototype[Symbol.iterator];",
        "Object.getPrototypeOf(other.WeakRef)===other.Function.prototype&&other.WeakRef.length===1&&other.WeakRef.name==='WeakRef';",
        "Object.getPrototypeOf(other.FinalizationRegistry)===other.Function.prototype&&other.FinalizationRegistry.length===1;",
        "Object.getPrototypeOf(other.WeakRef.prototype)===other.Object.prototype&&other.WeakRef.prototype[Symbol.toStringTag]==='WeakRef';",
        "other.FinalizationRegistry.prototype.register.length===2&&other.FinalizationRegistry.prototype.unregister.length===1;",
        "Object.getPrototypeOf(other.WeakRef.prototype.deref)===other.Function.prototype;",
        "var descriptor=Object.getOwnPropertyDescriptor(other.WeakRef,'prototype');!descriptor.writable&&!descriptor.enumerable&&!descriptor.configurable;",
        "!Object.getOwnPropertyDescriptor(other.FinalizationRegistry.prototype,'constructor').enumerable;",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn class_call_and_non_callable_apply_use_the_required_error_realms() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    child
        .eval_source("var C = class {}; var object = {};")
        .unwrap();
    vm.set_global_checked("other", child.realm_global_object())
        .unwrap();
    for source in [
        "var caught;try{other.C();}catch(error){caught=error;}caught instanceof other.TypeError;",
        "try{other.Function.prototype.apply.call({}, null, []);}catch(error){caught=error;}caught instanceof other.TypeError;",
        "try{Function.prototype.call.call(other.object);}catch(error){caught=error;}caught instanceof TypeError;",
        "try{other.Function.prototype.apply.call(function(){}, null, 42);}catch(error){caught=error;}caught instanceof other.TypeError;",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn aggregate_errors_and_error_branding_preserve_realm_and_identity() {
    let mut vm = Interpreter::with_builtins();
    let child = vm.create_realm();
    vm.set_global_checked("other", child.realm_global_object())
        .unwrap();
    for source in [
        "var error=other.AggregateError([1,2],'message',{cause:42});error instanceof other.AggregateError&&error instanceof other.Error&&error.message==='message'&&error.cause===42&&error.errors.join(',')==='1,2';",
        "Object.getPrototypeOf(error.errors)===other.Array.prototype&&Object.prototype.toString.call(error)==='[object Error]';",
        "Error.isError(error)&&other.Error.isError(new Error())&&!Error.isError(Object.create(Error.prototype))&&!Error.isError(new Proxy(error,{}));",
        "var descriptor=Object.getOwnPropertyDescriptor(error,'errors');descriptor.writable&&!descriptor.enumerable&&descriptor.configurable;",
        "var Target=new other.Function();Target.prototype=null;Object.getPrototypeOf(Reflect.construct(AggregateError,[[]],Target))===other.AggregateError.prototype;",
        "var order='';var errors={};errors[Symbol.iterator]=function(){order+='i';return [1][Symbol.iterator]();};var message={toString(){order+='m';return 'message';}};var options={get cause(){order+='c';return 42;}};AggregateError(errors,message,options);order==='mci';",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn apply_and_construct_share_array_like_property_access_order() {
    let mut vm = Interpreter::with_builtins();
    for source in [
        "var order='';var list={get length(){order+='l';return 2;},get 0(){order+='a';return 1;},get 1(){order+='b';return 2;}};function add(a,b){return a+b;}add.apply(null,list)===3&&order==='lab';",
        "order='';Reflect.apply(add,null,list)===3&&order==='lab';",
        "order='';function C(a,b){this.value=a+b;}Reflect.construct(C,list).value===3&&order==='lab';",
        "var caught;try{Reflect.apply(add,null,undefined);}catch(error){caught=error;}caught instanceof TypeError;",
        "order='';try{Function.prototype.apply.call({},null,list);}catch(error){caught=error;}caught instanceof TypeError&&order==='';",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn regexp_compile_checks_the_defining_realm_of_its_receiver() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    child.eval_source("var regex=/child/;").unwrap();
    vm.set_global_checked("other", child.realm_global_object())
        .unwrap();
    for source in [
        "var caught;try{RegExp.prototype.compile.call(other.regex,'main');}catch(error){caught=error;}caught instanceof TypeError&&other.regex.source==='child';",
        "try{other.RegExp.prototype.compile.call(/main/,'child');}catch(error){caught=error;}caught instanceof other.TypeError;",
        "other.regex.compile('changed')===other.regex&&other.regex.source==='changed';",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn primitive_property_references_use_the_current_execution_realm() {
    let mut vm = Interpreter::with_builtins();
    let mut other = vm.create_realm();
    other.eval_source("Number.prototype.realm='child';String.prototype.realm='child';Boolean.prototype.realm='child';Symbol.prototype.realm='child';BigInt.prototype.realm='child';var read=value=>value.realm;var iterator=value=>value[Symbol.iterator];").unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    for source in [
        "other.read(1)==='child'&&other.read('')==='child'&&other.read(true)==='child'&&other.read(Symbol())==='child'&&other.read(1n)==='child';",
        "(1).realm===undefined&&''.realm===undefined;",
        "other.iterator('')===other.String.prototype[Symbol.iterator];",
        "Symbol('description').description==='description'&&Object(Symbol('boxed')).description==='boxed';",
        "other.eval(\"String.prototype['01']='inherited';'abc'['01']==='inherited'&&'abc'[1]==='b'\");",
        "other.eval(\"Object.defineProperty(Number.prototype,'receiver',{get(){'use strict';return this;}});(42).receiver===42\");",
        "other.eval(\"Number=function(){};(1).realm==='child'\");",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn proxy_internal_errors_use_the_caller_realm_and_preserve_trap_abrupt_completions() {
    let mut vm = Interpreter::with_builtins();
    let mut other = vm.create_realm();
    other.eval_source("var target=function(){};var callable=new Proxy(target,{apply:1});var constructible=new Proxy(target,{construct:1});var Class=class{};var throwing=new Proxy(target,{get apply(){throw new TypeError('foreign getter');}});").unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    for source in [
        "var caught;try{other.callable();}catch(e){caught=e;}caught instanceof TypeError&&!(caught instanceof other.TypeError);",
        "try{new other.constructible();}catch(e){caught=e;}caught instanceof TypeError&&!(caught instanceof other.TypeError);",
        "try{other.Class();}catch(e){caught=e;}caught instanceof other.TypeError&&!(caught instanceof TypeError);",
        "try{other.throwing();}catch(e){caught=e;}caught instanceof other.TypeError;",
        "var apply=new Proxy(function(target,receiver,args){return args[0];},{});new Proxy(function(){},{apply})(42)===42;",
        "new Proxy({x:42},new Proxy({},{})).x===42;",
        "var handler=[];handler.get=function(){return 42;};new Proxy({},handler).x===42;",
        "try{other.Proxy({},{});}catch(e){caught=e;}caught instanceof other.TypeError;",
        "var reads=0;var newTarget=function(){}.bind(null);Object.defineProperty(newTarget,'prototype',{get(){reads++;throw 'read';}});Reflect.construct(Proxy,[{},{}],newTarget);reads===0;",
    ] {
        truth(&mut vm, source);
    }
    for (name, operation) in [
        ("get", "proxy.x"),
        ("set", "proxy.x=1"),
        ("has", "'x' in proxy"),
        ("deleteProperty", "delete proxy.x"),
        ("ownKeys", "Object.keys(proxy)"),
        ("getPrototypeOf", "Object.getPrototypeOf(proxy)"),
    ] {
        for trap in ["get(){throw marker;}", "value:1"] {
            truth(
                &mut vm,
                &format!(
                    "var marker={{}};var handler={{}};Object.defineProperty(handler,'{name}',{{{trap}}});var proxy=new Proxy({{}},handler);var caught;try{{{operation};}}catch(e){{caught=e;}}{};",
                    if trap.starts_with("get") {
                        "caught===marker"
                    } else {
                        "caught instanceof TypeError"
                    }
                ),
            );
        }
    }
}

#[test]
fn restricted_accessors_share_one_thrower_per_realm_and_keep_argument_identity() {
    let mut vm = Interpreter::with_builtins();
    let mut other = vm.create_realm();
    other.eval_source("var strictArgs=function(){'use strict';return arguments;};var defaults=function(a=0){return arguments;};var rest=function(...a){return arguments;};var ordinary=function(){return arguments;};").unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    for source in [
        "var foreign=Object.getOwnPropertyDescriptor(other.strictArgs(),'callee');var caller=Object.getOwnPropertyDescriptor(other.Function.prototype,'caller');foreign.get===foreign.set&&foreign.get===caller.get&&foreign.get===caller.set;",
        "Object.getOwnPropertyDescriptor(other.defaults(),'callee').get===foreign.get&&Object.getOwnPropertyDescriptor(other.rest(),'callee').set===foreign.get;",
        "var local=Object.getOwnPropertyDescriptor(Function.prototype,'caller').get;foreign.get!==local&&Object.getPrototypeOf(foreign.get)===other.Function.prototype;",
        "foreign.get.name===''&&foreign.get.length===0&&!Object.isExtensible(foreign.get)&&!Object.getOwnPropertyDescriptor(foreign.get,'name').configurable;",
        "var caught;try{other.strictArgs().callee;}catch(e){caught=e;}caught instanceof other.TypeError;",
        "try{other.Function.prototype.caller=1;}catch(e){caught=e;}caught instanceof other.TypeError;",
        "var arguments=other.ordinary(42);arguments.callee===other.ordinary&&arguments[0]===42&&arguments[Symbol.iterator]===other.Array.prototype.values;",
        "!Object.getOwnPropertyDescriptor(other.strictArgs(),'callee').configurable&&!Object.getOwnPropertyDescriptor(arguments,'length').enumerable;",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn async_generator_prototype_ownership_preserves_existing_iteration() {
    let mut vm = Interpreter::with_builtins();
    let mut other = vm.create_realm();
    other
        .eval_source("var generate=async function*(){yield 42;};")
        .unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    truth(
        &mut vm,
        "var generator=other.generate();Object.getPrototypeOf(generator)===other.generate.prototype&&Object.getPrototypeOf(generator.next)===other.Function.prototype;",
    );
    truth(
        &mut vm,
        "var sum=0;for await(var value of generator){sum+=value;}sum===42;",
    );
    truth(
        &mut vm,
        "var fallback=Object.getPrototypeOf(other.generate.prototype);other.generate.prototype=undefined;Object.getPrototypeOf(other.generate())===fallback;",
    );
    truth(
        &mut vm,
        "var caught;try{Error({toString:undefined,valueOf:undefined});}catch(e){caught=e;}caught instanceof TypeError;",
    );
    truth(
        &mut vm,
        "Number.prototype.split=String.prototype.split;try{(42).split({toString(){return /x/;}});}catch(e){caught=e;}caught instanceof TypeError;",
    );
}

#[test]
fn non_strict_ordinary_functions_keep_legacy_properties_without_exposing_callers() {
    let mut vm = Interpreter::with_builtins();
    for source in [
        "function ordinary(){return ordinary.caller;}function strict(){'use strict';return ordinary();}strict()===null;",
        "eval(\"'use strict';ordinary();\")===null;",
        "ordinary.arguments===null&&!Object.getOwnPropertyDescriptor(ordinary,'caller').configurable;",
        "var caught;try{strict.caller;}catch(e){caught=e;}caught instanceof TypeError;",
        "try{(()=>{}).caller;}catch(e){caught=e;}caught instanceof TypeError;",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn array_species_allocation_observes_custom_constructors_and_foreign_intrinsics() {
    let mut vm = Interpreter::with_builtins();
    let mut other = vm.create_realm();
    other.eval_source("var input=[2,0,4];delete input[1];var Result=function(n){this.size=n;};Object.defineProperty(Array,Symbol.species,{get(){throw new Error('foreign species observed');}});").unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    for source in [
        "var mapped=Array.prototype.map.call(other.input,x=>x*2);Object.getPrototypeOf(mapped)===Array.prototype;",
        "mapped.length===3;",
        "mapped[0]===4;",
        "!(1 in mapped);",
        "var log='';var input=[2,0,4];delete input[1];input.constructor={get [Symbol.species](){log+='s';return other.Result;}};var output=input.map(x=>{log+='m';return x+1;});log==='smm'&&Object.getPrototypeOf(output)===other.Result.prototype&&output.size===3&&output[0]===3&&!(1 in output)&&output[2]===5;",
        "var filtered=input.filter(x=>x>2);Object.getPrototypeOf(filtered)===other.Result.prototype&&filtered.size===0&&filtered[0]===4;",
        "var descriptor=Object.getOwnPropertyDescriptor(Array,Symbol.species);descriptor.get.call(other.Result)===other.Result&&!descriptor.enumerable&&descriptor.configurable&&descriptor.get.length===0;",
        "input.constructor={[Symbol.species]:null};Array.isArray(input.map(x=>x));",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn proxy_revocation_preserves_flags_and_captures_slots_before_trap_getters() {
    let mut vm = Interpreter::with_builtins();
    let mut other = vm.create_realm();
    other
        .eval_source("var revocable=Proxy.revocable;var Target=function(){};")
        .unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    for source in [
        "var pair=other.revocable(other.Target,{});var proxy=pair.proxy;Object.getPrototypeOf(pair.revoke)===other.Function.prototype&&pair.revoke.name===''&&pair.revoke.length===0&&!Object.prototype.hasOwnProperty.call(pair.revoke,'prototype');",
        "pair.revoke.call({});pair.revoke();typeof proxy==='function';",
        "var caught;caught=undefined;try{proxy.x;}catch(e){caught=e;}caught instanceof TypeError&&!(caught instanceof other.TypeError);",
        "caught=undefined;try{proxy();}catch(e){caught=e;}caught instanceof TypeError;",
        "caught=undefined;try{new proxy();}catch(e){caught=e;}caught instanceof TypeError;",
        "var pair=Proxy.revocable([1],{});pair.revoke();caught=undefined;try{Array.isArray(pair.proxy);}catch(e){caught=e;}caught instanceof TypeError;",
        "var state,seen;var handler={get get(){state.revoke();return function(target,key,receiver){seen=this===handler&&target.answer===42&&receiver===state.proxy;return target[key];};}};state=Proxy.revocable({answer:42},handler);state.proxy.answer===42&&seen;",
        "var state=Proxy.revocable(other.Target,{get get(){state.revoke();return ()=>null;}});caught=undefined;try{Reflect.construct(Object,[],state.proxy);}catch(e){caught=e;}caught instanceof TypeError;",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn proxy_extensibility_and_prototype_operations_share_invariant_checks() {
    let mut vm = Interpreter::with_builtins();
    for source in [
        "var target={};var proxy=new Proxy(target,{});Object.isExtensible(proxy)&&Reflect.preventExtensions(proxy)&&!Object.isExtensible(target);",
        "var caught;try{Object.isExtensible(new Proxy({}, {isExtensible(){return false;}}));}catch(e){caught=e;}caught instanceof TypeError;",
        "caught=undefined;try{Reflect.preventExtensions(new Proxy({}, {preventExtensions(){return true;}}));}catch(e){caught=e;}caught instanceof TypeError;",
        "!Reflect.preventExtensions(new Proxy({}, {preventExtensions(){return false;}}));",
        "var proto={};var target={};var proxy=new Proxy(target,{});Reflect.setPrototypeOf(proxy,proto)&&Object.getPrototypeOf(target)===proto;",
        "Object.preventExtensions(target);!Reflect.setPrototypeOf(proxy,{})&&Reflect.setPrototypeOf(proxy,proto);",
        "caught=undefined;try{Object.setPrototypeOf(new Proxy(target,{setPrototypeOf(){return true;}}),{});}catch(e){caught=e;}caught instanceof TypeError;",
        "var pair=Proxy.revocable({},{});pair.revoke();var operations=[()=>Reflect.has(pair.proxy,'x'),()=>Object.isExtensible(pair.proxy),()=>Object.preventExtensions(pair.proxy),()=>Object.setPrototypeOf(pair.proxy,null)];operations.every(operation=>{try{operation();return false;}catch(e){return e instanceof TypeError;}});",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn bound_functions_observe_target_prototypes_before_metadata_and_realm_lookup() {
    let mut vm = Interpreter::with_builtins();
    let mut other = vm.create_realm();
    other.eval_source("var target=function(a,b,c){};").unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    for source in [
        "Object.getPrototypeOf(Function.prototype.bind.call(other.target,null))===other.Function.prototype;",
        "var log='';var proto={};var target=new Proxy(other.target,{getPrototypeOf(){log+='p';return proto;},getOwnPropertyDescriptor(target,key){log+='d';return Reflect.getOwnPropertyDescriptor(target,key);},get(target,key){log+=key==='length'?'l':'n';return target[key];}});var bound=Function.prototype.bind.call(target,null,1);log==='pdln'&&bound.length===2&&Object.getPrototypeOf(bound)===proto;",
        "var target=function(){};Object.defineProperty(target,'length',{value:{valueOf(){throw new Error('length conversion');}}});target.bind(null).length===0;",
        "var pair=Proxy.revocable(other.target,{});var bound=Function.prototype.bind.call(pair.proxy,null);pair.revoke();var observed=false;Object.defineProperty(bound,Symbol.species,{get(){observed=true;return Array;}});var input=[1];input.constructor=bound;var caught;try{input.map(x=>x);}catch(e){caught=e;}caught instanceof TypeError&&!observed;",
        "caught=undefined;try{Function.prototype.bind.call(pair.proxy,null);}catch(e){caught=e;}caught instanceof TypeError;",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn array_from_and_of_use_constructor_realms_and_observable_initialization_order() {
    let mut vm = Interpreter::with_builtins();
    let mut other = vm.create_realm();
    other.eval_source("var Result=function(n){this.count=arguments.length;this.size=n;};Result.prototype=null;").unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    for source in [
        "var result=Array.of.call(other.Result,7,8);Object.getPrototypeOf(result)===other.Object.prototype&&result.size===2&&result.count===1&&result.length===2&&result[1]===8;",
        "var result=Array.from.call(other.Result,[7,8]);Object.getPrototypeOf(result)===other.Object.prototype&&result.count===0&&result.length===2&&result[0]===7;",
        "var result=Array.from.call(other.Result,{0:7,length:1});result.count===1&&result.size===1&&result[0]===7;",
        "var log='';function Result(n){log+='c';this.size=n;}var source={get [Symbol.iterator](){log+='i';return undefined;},get length(){log+='l';return 2;},get 0(){log+='a';return 7;},get 1(){log+='b';return 8;}};Array.from.call(Result,source,x=>{log+='m';return x;});log==='ilcambm';",
        "var closed=false;var sentinel={};var iterable={[Symbol.iterator](){return {next(){return {value:1,done:false};},return(){closed=true;throw new Error('close');}};}};var caught;try{Array.from(iterable,()=>{throw sentinel;});}catch(e){caught=e;}caught===sentinel&&closed;",
        "function Locked(){Object.defineProperty(this,'length',{value:0,writable:false});}caught=undefined;try{Array.of.call(Locked,1);}catch(e){caught=e;}caught instanceof TypeError;",
        "var stored=0;var locked=Object.freeze({x:0});function WithSetter(){Object.defineProperty(this,'length',{set(n){locked.x=1;stored=n;}});}Array.of.call(WithSetter,1,2);stored===2&&locked.x===0;",
        "var descriptor=Object.getOwnPropertyDescriptor(result,'0');descriptor.writable&&descriptor.enumerable&&descriptor.configurable;",
        "Array.isArray(Array.from.call(()=>{},[1]))&&Array.isArray(Array.of.call(()=>{},1));",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn array_species_methods_allocate_before_reads_and_preserve_sparse_generic_inputs() {
    let mut vm = Interpreter::with_builtins();
    let mut other = vm.create_realm();
    other
        .eval_source("var Result=function(n){this.size=n;};")
        .unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    for source in [
        "var input=[2,0,4];delete input[1];input.constructor={[Symbol.species]:other.Result};var result=input.slice(0);Object.getPrototypeOf(result)===other.Result.prototype&&result.size===3&&result.length===3&&result[2]===4&&!(1 in result);",
        "var result=input.concat([5]);Object.getPrototypeOf(result)===other.Result.prototype&&result.size===0&&result.length===4&&result[3]===5&&!(1 in result);",
        "var result=input.splice(1,2,7);Object.getPrototypeOf(result)===other.Result.prototype&&result.size===2&&result.length===2&&!(0 in result)&&result[1]===4&&input.length===2&&input[1]===7;",
        "var input=[[1],[2]];input.constructor={[Symbol.species]:other.Result};var result=input.flat();Object.getPrototypeOf(result)===other.Result.prototype&&result.size===0&&result[1]===2&&!('length' in result);",
        "var result=input.flatMap(x=>[x[0]*2]);Object.getPrototypeOf(result)===other.Result.prototype&&result[0]===2&&result[1]===4;",
        "var generic={0:'a',2:'c',length:3};var result=Array.prototype.splice.call(generic,1,1,'b','B');result.length===1&&!(0 in result)&&generic.length===4&&generic[1]==='b'&&generic[2]==='B'&&generic[3]==='c';",
        "var source={0:7,length:2,[Symbol.isConcatSpreadable]:true};var result=[1].concat(source);result.length===3&&result[1]===7&&!(2 in result);",
        "var proto={1:8};var source=[7,0,9];delete source[1];Object.setPrototypeOf(source,proto);Array.prototype.slice.call(source)[1]===8&&Reflect.has(source,'1');",
        "var typed=new Uint8Array([7,8]);typed[Symbol.isConcatSpreadable]=true;var result=[].concat(typed);result[0]===7&&result[1]===8&&!Reflect.has(typed,'-0');",
        "var boxed;Array.prototype.map.call('ab',(value,index,source)=>{boxed=source;return value;}).join('')==='ab'&&typeof boxed==='object';",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn shared_set_results_do_not_inherit_guest_assignment_strictness() {
    let mut vm = Interpreter::with_builtins();
    for source in [
        "var locked=Object.freeze([1]);!Reflect.set(locked,0,2)&&locked[0]===1;",
        "'use strict';!Reflect.set(locked,0,2)&&!Reflect.set(new Proxy({}, {set(){return false;}}),'x',1);",
        "var caught;try{(function(){'use strict';locked[0]=2;})();}catch(e){caught=e;}caught instanceof TypeError;",
        "var target={};Object.defineProperty(target,'x',{get(){return 1;}});!Reflect.set(target,'x',2);",
    ] {
        truth(&mut vm, source);
    }
}
