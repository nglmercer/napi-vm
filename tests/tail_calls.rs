use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    let value = Interpreter::with_builtins().execute(&prepared).unwrap();
    assert!(
        matches!(&value, Value::String(s) if s == expected),
        "prepared: {value:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let value = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&value, Value::String(s) if s == expected),
        "AST: {value:?}"
    );
}

#[test]
fn strict_mutual_tail_calls_release_both_tiers_frames() {
    both(
        "'use strict'; function f(n) { return n ? g(n - 1) : 'done'; } function g(n) { return n ? f(n - 1) : 'done'; } f(20000)",
        "done",
    );
}

#[test]
fn tail_expression_paths_keep_receivers_spreads_and_evaluation_order() {
    both(
        "'use strict'; var o = { go(n) { return n ? (0, true && this.go(...[n-1])) : 'done'; } }; o.go(20000)",
        "done",
    );
    both(
        "'use strict'; function f(n) { if (!n) return 'done'; return null ?? f(n-1); } f(20000)",
        "done",
    );
}

#[test]
fn bound_and_proxy_tail_calls_do_not_build_trampoline_stacks() {
    both(
        "'use strict'; function f(n) { return n ? bound(n-1) : 'done'; } var bound = f.bind(null); f(20000)",
        "done",
    );
    both(
        "'use strict'; function f(n) { return n ? proxy(n-1) : 'done'; } var proxy = new Proxy(f, {}); f(20000)",
        "done",
    );
}

#[test]
fn finally_tail_calls_and_non_tail_cleanup_keep_their_order() {
    both(
        "'use strict'; function f(n) { if (!n) return 'done'; try {} finally { return f(n-1); } } f(20000)",
        "done",
    );
    both(
        "'use strict'; var log = []; function target() { log.push('target'); return 1; } function f() { try { return target(); } finally { log.push('finally'); } } f(); log.join(',')",
        "target,finally",
    );
    both(
        "'use strict'; var log = []; function target() { log.push('target'); return 1; } function f() { using x = { [Symbol.dispose]() { log.push('dispose'); } }; return target(); } f(); log.join(',')",
        "target,dispose",
    );
}

#[test]
fn non_tail_and_sloppy_calls_keep_the_call_depth_guard() {
    both(
        "'use strict'; function f(n) { for (var value of [0]) { return n ? f(n-1) : 1; } } try { f(20000); } catch(e) { e.name; }",
        "RangeError",
    );
    both(
        "function f(n) { return n ? f(n-1) : 1; } try { f(20000); } catch(e) { e.name; }",
        "RangeError",
    );
    both(
        "'use strict'; function f(n) { return n ? 1 + f(n-1) : 1; } try { f(20000); } catch(e) { e.name; }",
        "RangeError",
    );
}

#[test]
fn derived_constructor_return_constraints_survive_a_tail_call() {
    both(
        "'use strict'; function f() { return 1; } class C extends Object { constructor() { super(); return f(); } } try { new C(); } catch(e) { e.name; }",
        "TypeError",
    );
}

#[test]
fn call_apply_and_reflect_apply_forward_without_native_recursion() {
    for call in [
        "f.call(null, n-1)",
        "f.apply(null, [n-1])",
        "Reflect.apply(f, null, [n-1])",
    ] {
        both(
            &format!("'use strict'; function f(n) {{ return n ? {call} : 'done'; }} f(20000)"),
            "done",
        );
    }
}

#[test]
fn tail_calls_cross_ast_and_bytecode_function_boundaries() {
    let mut vm = Interpreter::with_builtins();
    let compiled =
        Interpreter::compile("'use strict'; function g(n) { return n ? f(n-1) : 'done'; }")
            .unwrap();
    assert_eq!(
        compiled.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    vm.execute(&compiled).unwrap();
    assert!(
        matches!(vm.global_value("g"), Some(Value::Function(ref function)) if function.bytecode.is_some())
    );
    let statements = Parser::new(
        Lexer::new("'use strict'; function f(n) { return n ? g(n-1) : 'done'; }").tokenize(),
    )
    .parse_program()
    .unwrap();
    vm.run_program_body(&statements).unwrap();
    assert!(
        matches!(vm.global_value("f"), Some(Value::Function(ref function)) if function.bytecode.is_none())
    );
    let value = vm.eval_source("f(20000)").unwrap();
    assert!(matches!(value, Value::String(ref value) if value == "done"));
}

#[test]
fn a_non_intrinsic_callee_named_eval_can_be_tail_called() {
    both(
        "'use strict'; function f(n) { return n ? eval(n-1) : 'done'; } globalThis.eval = f; f(20000)",
        "done",
    );
}

#[test]
fn using_null_still_prevents_a_call_from_being_in_tail_position() {
    both(
        "'use strict'; function f(n) { using x = null; return n ? f(n-1) : 1; } try { f(20000); } catch(e) { e.name; }",
        "RangeError",
    );
}

#[test]
fn native_tail_targets_retain_the_callers_module_referrer() {
    fn native_import(
        vm: &mut Interpreter,
        _: Value,
        _: Vec<Value>,
    ) -> Result<Value, napi_vm::VmErr> {
        vm.eval_source("import('./dep.js').then(ns => globalThis.tailModuleValue = ns.value)")
    }
    let loader = std::rc::Rc::new(napi_vm::VirtualLoader::new());
    loader.insert(
        "pkg/main.js",
        "export function run() { return nativeImport(); }",
    );
    loader.insert("pkg/dep.js", "export const value = 'done';");
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(loader);
    vm.set_global_checked(
        "nativeImport",
        Value::NativeFunction {
            name: "nativeImport".into(),
            callable: native_import,
        },
    )
    .unwrap();
    vm.eval_source("import { run } from 'pkg/main.js'; run();")
        .unwrap();
    vm.drain_jobs().unwrap();
    assert!(
        matches!(vm.global_value("tailModuleValue"), Some(Value::String(ref value)) if value == "done")
    );
}
