use napi_vm::{Interpreter, Value, VirtualLoader};
use std::rc::Rc;

#[test]
fn core_globals_are_language_only_even_with_all_features() {
    for mut vm in [Interpreter::new(), Interpreter::with_builtins()] {
        for name in [
            "console",
            "setTimeout",
            "setInterval",
            "queueMicrotask",
            "fetch",
            "WebSocket",
            "Request",
            "Response",
            "Headers",
            "AbortController",
            "process",
            "Buffer",
            "require",
            "TextEncoder",
            "window",
            "navigator",
            "crypto",
            "localStorage",
        ] {
            assert!(
                matches!(vm.eval_source(&format!("typeof {name}")).unwrap(), Value::String(ref s) if s == "undefined"),
                "ambient global {name}"
            );
        }
    }
    let mut vm = Interpreter::with_builtins();
    assert!(matches!(
        vm.eval_source("Promise.resolve(42)").unwrap(),
        Value::Promise(_)
    ));
}

#[test]
fn virtual_modules_support_relative_imports() {
    let loader = Rc::new(VirtualLoader::new());
    loader.insert(
        "pkg/main.js",
        "import { answer } from './value.js'; export const result = answer;",
    );
    loader.insert("pkg/value.js", "export const answer = 42;");
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(loader);
    vm.load_module("pkg/main.js").unwrap();
    let result = vm
        .eval_source("import { result } from 'pkg/main.js'; result")
        .unwrap();
    assert!(matches!(result, Value::Number(42.0)));
}

#[test]
fn static_deferred_import_evaluates_on_export_access() {
    let mut vm = Interpreter::with_builtins();
    vm.define_module(
        "deferred",
        "globalThis.deferredRuns = (globalThis.deferredRuns || 0) + 1; export const value = deferredRuns;"
            .into(),
    );
    let result = vm
        .eval_source(
            "import defer * as ns from 'deferred'; JSON.stringify([typeof globalThis.deferredRuns, ns.value, globalThis.deferredRuns])",
        )
        .unwrap();
    assert!(
        matches!(result, Value::String(ref value) if value == "[\"undefined\",1,1]"),
        "{result:?}"
    );
}

#[test]
fn deferred_namespace_reflection_evaluates_but_then_does_not() {
    let mut vm = Interpreter::with_builtins();
    vm.define_module(
        "deferred-reflection",
        "globalThis.reflectionRuns = (globalThis.reflectionRuns || 0) + 1; export const value = 7; export const then = 9;"
            .into(),
    );
    let result = vm
        .eval_source(
            "import defer * as ns from 'deferred-reflection'; var before = typeof globalThis.reflectionRuns; var thenValue = ns.then; var afterThen = typeof globalThis.reflectionRuns; var keys = Object.keys(ns); JSON.stringify([before, thenValue, afterThen, keys.join(), globalThis.reflectionRuns])",
        )
        .unwrap();
    assert!(
        matches!(result, Value::String(ref value) if value == "[\"undefined\",null,\"undefined\",\"value\",1]"),
        "{result:?}"
    );
}

#[test]
fn dynamic_deferred_import_resolves_to_a_lazy_namespace() {
    let mut vm = Interpreter::with_builtins();
    vm.define_module(
        "dynamic-deferred",
        "globalThis.dynamicDeferredRuns = (globalThis.dynamicDeferredRuns || 0) + 1; export const value = 11;"
            .into(),
    );
    let result = vm
        .eval_source(
            "var imported; import.defer('dynamic-deferred').then(ns => imported = ns); await 0; globalThis.dynamicDeferredRuns === undefined && imported.value === 11 && globalThis.dynamicDeferredRuns === 1",
        )
        .unwrap();
    assert!(matches!(result, Value::Bool(true)), "{result:?}");
}

#[test]
fn private_elements_reject_non_extensible_receivers() {
    let mut vm = Interpreter::with_builtins();
    let result = vm
        .eval_source(
            "class Base { constructor(target) { return target; } } class Derived extends Base { #value = 1; static has(value) { return #value in value; } } const extensible = new Derived({}); let rejected = false; try { new Derived(Object.preventExtensions({})); } catch (error) { rejected = error instanceof TypeError; } rejected && Derived.has(extensible)",
        )
        .unwrap();
    assert!(matches!(result, Value::Bool(true)), "{result:?}");
}

#[test]
fn addition_obeys_to_primitive_order_and_type_errors() {
    let mut vm = Interpreter::with_builtins();
    let result = vm.eval_source("var log = ''; var left = { [Symbol.toPrimitive](hint) { log += hint + 'L'; return 40; } }; var right = { [Symbol.toPrimitive](hint) { log += hint + 'R'; return 2; } }; var answer = left + right; answer === 42 && log === 'defaultLdefaultR'").unwrap();
    assert!(matches!(result, Value::Bool(true)));
    for source in [
        "try { 1n + 1; false; } catch (e) { e.constructor === TypeError; }",
        "try { '' + Symbol(); false; } catch (e) { e.constructor === TypeError; }",
        "try { 1 + {valueOf() { return {}; }, toString() { return {}; }}; false; } catch (e) { e.constructor === TypeError; }",
        "(new TypeError()).constructor === TypeError",
        "({[Symbol.toPrimitive]() {return 2n;}}) + 1n === 3n",
        "eval('1 + 2') === 3",
    ] {
        let result = vm.eval_source(source).unwrap();
        assert!(matches!(result, Value::Bool(true)), "{source}: {result:?}");
    }
}

#[test]
fn binary_objects_and_regexp_keep_reference_identity() {
    let mut engine = napi_vm::Interpreter::with_builtins();
    for source in [
        "const buffer=new ArrayBuffer(8); buffer===buffer && buffer!==new ArrayBuffer(8)",
        "const view=new Uint8Array(4); view===view && view!==new Uint8Array(view.buffer)",
        "const data=new DataView(new ArrayBuffer(8)); data===data",
        "const pattern=/test/; pattern===pattern && pattern!==/test/",
    ] {
        assert!(
            matches!(
                engine.eval_source(source).unwrap(),
                napi_vm::Value::Bool(true)
            ),
            "{source}"
        );
    }
}
