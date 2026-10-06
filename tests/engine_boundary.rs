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
