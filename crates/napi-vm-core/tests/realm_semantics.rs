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
        "try{other.Class();}catch(e){caught=e;}caught instanceof TypeError&&!(caught instanceof other.TypeError);",
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
