use napi_vm_core::{Interpreter, Value};
fn source(vm: &mut Interpreter, name: &str, text: &str) {
    vm.define_module(name, text.into());
}
fn number(vm: &mut Interpreter, text: &str, expected: f64) {
    assert!(
        matches!(vm.eval_source(text), Ok(Value::Number(n)) if n == expected),
        "{text}"
    );
}
#[test]
fn linking_is_side_effect_free_and_validates_the_whole_graph() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source("var effects=0").unwrap();
    source(&mut vm, "dep", "effects++; export const found=1;");
    source(&mut vm, "bad", "effects++; import {missing} from 'dep';");
    let error = vm.link_module("bad").unwrap_err();
    assert!(error.to_string().contains("SyntaxError"), "{error}");
    number(&mut vm, "effects", 0.);
    source(
        &mut vm,
        "good",
        "effects++; import {found} from 'dep'; export const answer=found+1;",
    );
    assert!(vm.link_module("good").unwrap());
    number(&mut vm, "effects", 0.);
    vm.load_module("good").unwrap();
    number(&mut vm, "effects", 2.);
    number(&mut vm, "import {answer} from 'good'; answer", 2.);
}
#[test]
fn dependencies_run_before_body_even_when_import_appears_last() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source("var trace=''").unwrap();
    source(&mut vm, "dep", "trace+='D'; export const value=2;");
    source(
        &mut vm,
        "main",
        "trace+='M'; export const answer=value; import {value} from 'dep';",
    );
    vm.load_module("main").unwrap();
    assert!(matches!(vm.eval_source("trace").unwrap(),Value::String(ref value) if value == "DM"));
    number(&mut vm, "import {answer} from 'main'; answer", 2.);
}
#[test]
fn cyclic_function_exports_are_hoisted_and_lexical_exports_have_tdz() {
    let mut vm = Interpreter::with_builtins();
    source(
        &mut vm,
        "a",
        "import {result} from 'b'; export function f(){return 7;} export const answer=result;",
    );
    source(
        &mut vm,
        "b",
        "import {f} from 'a'; export const result=f();",
    );
    vm.load_module("a").unwrap();
    number(&mut vm, "import {answer} from 'a'; answer", 7.);
    source(
        &mut vm,
        "c",
        "import {result} from 'd'; export let value=7;",
    );
    source(
        &mut vm,
        "d",
        "import {value} from 'c'; export const result=value;",
    );
    let error = vm.load_module("c").unwrap_err();
    assert!(error.to_string().contains("ReferenceError"), "{error}");
}
#[test]
fn live_default_and_local_reexported_imports_share_storage() {
    let mut vm = Interpreter::with_builtins();
    source(
        &mut vm,
        "a",
        "export let value=1; export {value as default}; export function bump(){value++;}",
    );
    source(&mut vm, "b", "import value from 'a'; export {value};");
    vm.load_module("b").unwrap();
    number(
        &mut vm,
        "import value from 'a'; import {bump} from 'a'; import {value as forwarded} from 'b'; bump(); value+forwarded",
        4.,
    );
}
#[test]
fn ambiguous_stars_fail_named_import_but_same_binding_is_valid() {
    let mut vm = Interpreter::with_builtins();
    source(&mut vm, "a", "export const v=1;");
    source(&mut vm, "b", "export const v=2;");
    source(&mut vm, "barrel", "export * from 'a'; export * from 'b';");
    source(&mut vm, "consumer", "import {v} from 'barrel';");
    assert!(
        vm.link_module("consumer")
            .unwrap_err()
            .to_string()
            .contains("unambiguous")
    );
    source(&mut vm, "same", "export * from 'a'; export {v} from 'a';");
    vm.load_module("same").unwrap();
    number(&mut vm, "import {v} from 'same'; v", 1.);
}
#[test]
fn dynamic_import_failure_rejects_and_runs_after_current_stack() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source("var trace='';var reason;var promise;var sync=false")
        .unwrap();
    source(&mut vm, "bad", "trace+='B';throw 42;");
    vm.eval_source("try {promise=import('bad');trace+='S';promise.catch(e=>{reason=e;trace+='R';});} catch(e){sync=true;}").unwrap();
    assert!(matches!(vm.eval_source("trace").unwrap(),Value::String(ref value) if value == "SBR"));
    assert!(matches!(
        vm.eval_source("sync").unwrap(),
        Value::Bool(false)
    ));
    number(&mut vm, "reason", 42.);
    vm.eval_source("import('bad').catch(e=>{reason=e;})")
        .unwrap();
    assert!(matches!(vm.eval_source("trace").unwrap(),Value::String(ref value) if value == "SBR"));
    vm.eval_source("import({toString(){throw 17;}}).catch(e=>{reason=e;})")
        .unwrap();
    number(&mut vm, "reason", 17.);
}
#[test]
fn module_async_functions_keep_referrer_and_import_meta() {
    let mut vm = Interpreter::with_builtins();
    source(&mut vm, "./dep.js", "export const value=9;");
    source(
        &mut vm,
        "./main.js",
        "export async function load(){await 0;return (await import('./dep.js')).value;}",
    );
    vm.define_module_file_url("./main.js", "file:///main.js".into());
    vm.load_module("./main.js").unwrap();
    let exports = vm.module("./main.js").unwrap();
    vm.modules.borrow_mut().insert("main".into(), exports);
    number(&mut vm, "import {load} from 'main'; await load()", 9.);
}
#[test]
fn module_top_level_await_assimilates_thenables() {
    let mut vm = Interpreter::with_builtins();
    source(
        &mut vm,
        "dep",
        "export const value=await {then(resolve){resolve(8);}};",
    );
    source(
        &mut vm,
        "main",
        "import {value} from 'dep';export const answer=value+1;",
    );
    vm.load_module("main").unwrap();
    number(&mut vm, "import {answer} from 'main';answer", 9.);
}
#[test]
#[cfg(any(
    target_arch = "x86_64",
    target_arch = "x86",
    all(
        not(target_os = "windows"),
        any(
            target_arch = "aarch64",
            target_arch = "riscv32",
            target_arch = "riscv64",
            target_arch = "loongarch64"
        )
    )
))]
fn async_module_graph_waits_for_pending_await_and_shares_evaluation() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source("var trace='';var release;var gate=new Promise(resolve=>{release=resolve;});var first;var second;").unwrap();
    source(
        &mut vm,
        "dep",
        "trace+='D';export const value=await gate;trace+='d';",
    );
    source(
        &mut vm,
        "main",
        "import {value} from 'dep';trace+='M';export const answer=value+1;",
    );
    let promise = vm.import_module("main").unwrap();
    let pin = napi_vm_core::heap::RootPin::new(promise.clone());
    vm.eval_source(
        "import('main').then(ns=>{first=ns.answer;});import('main').then(ns=>{second=ns.answer;});",
    )
    .unwrap();
    assert!(matches!(vm.global.borrow().get("trace"),Some(Value::String(ref value)) if value=="D"));
    assert_eq!(
        promise.as_promise().unwrap().borrow().state,
        napi_vm_core::value::PromiseState::Pending
    );
    vm.eval_source("release(41);").unwrap();
    number(&mut vm, "first+second", 84.);
    assert!(
        matches!(vm.global.borrow().get("trace"),Some(Value::String(ref value)) if value=="DdM")
    );
    assert_eq!(
        promise.as_promise().unwrap().borrow().state,
        napi_vm_core::value::PromiseState::Fulfilled
    );
    drop(pin);
    assert!(vm.collect_cycles().skipped.is_none());
}
#[test]
fn async_module_cycles_and_cached_rejection_preserve_original_reason() {
    let mut vm = Interpreter::with_builtins();
    source(
        &mut vm,
        "a",
        "import {result} from 'b';export function f(){return 7;}export const answer=result;",
    );
    source(&mut vm, "b", "import {f} from 'a';export const result=f();");
    vm.eval_source("var answer;import('a').then(ns=>{answer=ns.answer;});")
        .unwrap();
    number(&mut vm, "answer", 7.);
    vm.eval_source("var reason={tag:42};var caught;").unwrap();
    source(&mut vm, "bad", "await Promise.reject(reason);");
    vm.eval_source("import('bad').catch(error=>{caught=error;});")
        .unwrap();
    assert!(matches!(
        vm.eval_source("caught===reason").unwrap(),
        Value::Bool(true)
    ));
    vm.eval_source("caught=undefined;import('bad').catch(error=>{caught=error;});")
        .unwrap();
    assert!(matches!(
        vm.eval_source("caught===reason").unwrap(),
        Value::Bool(true)
    ));
}
#[test]
fn linked_function_identity_does_not_change_when_body_runs() {
    let mut vm = Interpreter::with_builtins();
    source(
        &mut vm,
        "a",
        "import {saved} from 'b';export function f(){}export const same=saved===f;",
    );
    source(&mut vm, "b", "import {f} from 'a';export const saved=f;");
    vm.load_module("a").unwrap();
    assert!(matches!(
        vm.eval_source("import {same} from 'a';same").unwrap(),
        Value::Bool(true)
    ));
}
#[test]
fn namespaces_are_sorted_nonextensible_and_cannot_replace_live_cells() {
    let mut vm = Interpreter::with_builtins();
    source(
        &mut vm,
        "a",
        "export let z=1;export let a=2;export default 3;export function bump(){z++;}",
    );
    vm.load_module("a").unwrap();
    assert!(matches!(vm.eval_source("import * as ns from 'a';Object.keys(ns).join(',')==='a,bump,default,z' && Object.getPrototypeOf(ns)===null && Object.isExtensible(ns)===false && Reflect.set(ns,'z',99)===false").unwrap(),Value::Bool(true)));
    vm.eval_source("function write(){ns.z=99;}write();write();write();Object.defineProperty(ns,'z',{value:1});ns.bump();").unwrap();
    number(&mut vm, "ns.z", 2.);
    assert!(
        vm.eval_source("Object.defineProperty(ns,'z',{value:99})")
            .is_err()
    );
}
#[test]
fn imported_names_cannot_collide_with_module_declarations() {
    let mut vm = Interpreter::with_builtins();
    source(&mut vm, "dep", "export let value=1;");
    for (id, text) in [
        ("collision", "import {value} from 'dep';let value=42;"),
        (
            "duplicate",
            "import {value as local,value as local} from 'dep';",
        ),
        ("namespace", "import * as ns from 'dep';const ns=42;"),
    ] {
        source(&mut vm, id, text);
        assert!(
            vm.link_module(id)
                .unwrap_err()
                .to_string()
                .contains("Duplicate module binding")
        );
    }
    vm.load_module("dep").unwrap();
    number(&mut vm, "import {value} from 'dep';value", 1.);
}
#[test]
fn named_default_function_is_hoisted_and_default_expression_is_a_snapshot() {
    let mut vm = Interpreter::with_builtins();
    source(
        &mut vm,
        "a",
        "import {result} from 'b';export default function f(){return 7;}export const answer=result;",
    );
    source(&mut vm, "b", "import f from 'a';export const result=f();");
    vm.load_module("a").unwrap();
    number(&mut vm, "import {answer} from 'a';answer", 7.);
    source(
        &mut vm,
        "snapshot",
        "let value=1;export default value;value=2;",
    );
    vm.load_module("snapshot").unwrap();
    number(&mut vm, "import snapshot from 'snapshot';snapshot", 1.);
}
#[test]
fn modules_have_undefined_this_and_do_not_create_implicit_globals() {
    let mut vm = Interpreter::with_builtins();
    source(
        &mut vm,
        "strict",
        "export const value=this;export function write(){unboundModuleName=1;}",
    );
    vm.load_module("strict").unwrap();
    assert!(matches!(
        vm.eval_source("import {value,write} from 'strict';value===undefined")
            .unwrap(),
        Value::Bool(true)
    ));
    let error = vm.eval_source("write()").unwrap_err();
    assert!(error.to_string().contains("ReferenceError"));
    assert!(vm.global_value("unboundModuleName").is_none());
}

#[test]
fn module_namespace_identity_is_shared_across_imports_and_collection() {
    let mut vm = Interpreter::with_builtins();
    source(&mut vm, "identity", "export let value=42;");
    vm.load_module("identity").unwrap();
    vm.eval_source("import * as first from 'identity'; import * as second from 'identity';")
        .unwrap();
    assert!(matches!(
        vm.eval_source("first === second").unwrap(),
        Value::Bool(true)
    ));
    vm.eval_source("var dynamic; import('identity').then(m => dynamic=m);")
        .unwrap();
    vm.drain_jobs().unwrap();
    assert!(matches!(
        vm.eval_source("first === dynamic").unwrap(),
        Value::Bool(true)
    ));
    vm.collect_cycles();
    assert!(matches!(
        vm.eval_source("dynamic.value === 42").unwrap(),
        Value::Bool(true)
    ));
}

#[test]
fn namespace_reexports_include_indirect_exports_and_share_identity() {
    let mut vm = Interpreter::with_builtins();
    source(&mut vm, "leaf", "export const answer=42;");
    source(&mut vm, "middle", "export {answer} from 'leaf';");
    source(&mut vm, "outer", "export * as view from 'middle';");
    vm.load_module("outer").unwrap();
    number(&mut vm, "import {view} from 'outer'; view.answer", 42.);
    assert!(matches!(
        vm.eval_source("import * as same from 'middle'; view === same")
            .unwrap(),
        Value::Bool(true)
    ));
}
