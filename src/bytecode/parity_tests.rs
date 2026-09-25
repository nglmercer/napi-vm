//! Differential parity: the register VM must agree with the AST evaluator.
//!
//! Every case runs twice — once through [`Interpreter::execute`] (which
//! selects the bytecode tier for supported programs) and once through the
//! AST evaluator directly — and the observable outcomes (primitive results
//! and error messages) must be identical. Cases also pin the tier decision
//! itself, so a test can never pass vacuously by running the AST twice.

use crate::bytecode::{compile_program, verify_module};
use crate::error::VmErr;
use crate::interpreter::Interpreter;
use crate::parser::parse_cached;
use crate::value::Value;

fn canon(value: &Value) -> String {
    match value {
        Value::Number(n) => format!("num:{n:?}"),
        Value::String(s) => format!("str:{s:?}"),
        Value::Bool(b) => format!("bool:{b}"),
        Value::Null => "null".to_string(),
        Value::Undefined => "undef".to_string(),
        other => format!("other:{other:?}"),
    }
}

fn tag(result: &Result<Value, VmErr>) -> String {
    match result {
        Ok(value) => format!("ok {}", canon(value)),
        Err(VmErr::Msg(message)) => format!("Msg:{message}"),
        Err(VmErr::RuntimeError(data)) => format!("Rt:{}", data.message),
        Err(VmErr::Throw(value)) => format!("Throw:{}", canon(value)),
        Err(VmErr::Ret(value)) => format!("Ret:{}", canon(value)),
        Err(other) => format!("{other:?}"),
    }
}

fn run_ast(source: &str) -> Result<Value, VmErr> {
    let statements = parse_cached(source).expect("test source must parse");
    let mut interp = Interpreter::with_builtins();
    interp.begin_execution();
    interp.set_source(source);
    match interp.run_program_body(&statements) {
        Ok(value) => interp.drain_jobs().map(|()| value),
        Err(error) => {
            let _ = interp.drain_jobs();
            Err(error)
        }
    }
}

fn check(source: &str, expect_bytecode: bool) {
    let statements = parse_cached(source).expect("test source must parse");
    let tier = compile_program(&statements);
    assert_eq!(
        tier.is_ok(),
        expect_bytecode,
        "tier decision for {source:?}: {}",
        tier.as_ref().err().map(|e| format!("{e:?}")).unwrap_or_default(),
    );
    if let Ok(module) = &tier {
        verify_module(module).expect("compiler output must verify");
    }
    let program = Interpreter::compile(source).expect("compile");
    let mut vm = Interpreter::with_builtins();
    let vm_out = tag(&vm.execute(&program));
    let ast_out = tag(&run_ast(source));
    assert_eq!(vm_out, ast_out, "tier divergence for {source:?}");
}

#[test]
fn arithmetic_and_strings() {
    check("1 + 2 * 3", true);
    check("(10 - 4) / 2", true);
    check("17 % 5", true);
    check("2 ** 10", true);
    check("'a' + 'b' + 1", true);
    check("let s = 'n:'; s += 5; s", true);
    check("`a${1 + 1}b${'x'}c`", true);
    check("`no holes`", true);
    check("typeof 42", true);
    check("typeof foo", true);
    check("typeof undefined", true);
    check("undefined", true);
    check("void 42", true);
    check("(1, 2, 3)", true);
    check("1 > 2 ? 'a' : 'b'", true);
    check("false && nope", true);
    check("true || nope", true);
    check("null ?? 7", true);
    check("0 in [5]", true);
    check("[] instanceof Array", true);
}

#[test]
fn variables_and_assignment() {
    check("let x = 5; x += 3; x", true);
    check("var a = 1; var a; a", true);
    check("var h; typeof h", true);
    check("typeof hv; var hv = 1;", true);
    check("x = 41; x + 1", true);
    check("let n = 5; n++ + ++n", true);
    check("let n = 5; --n - n--", true);
    check("const k = 1; k = 2;", true);
    check("x; let x;", true);
    check("nope;", true);
    check("var q = 1; delete q", true);
    check("delete neverDeclared", true);
    check("let a = [1]; delete a[0]; a.length", true);
}

#[test]
fn control_flow() {
    check("let s = 0; for (let i = 1; i <= 10; i++) s += i; s", true);
    check("let i = 0; while (i < 3) i++; i", true);
    check("let i = 0; do { i++; } while (i < 3); i", true);
    check("let i = 0; while (true) { i++; if (i > 2) break; } i", true);
    check("let s = 0; for (let i = 0; i < 5; i++) { if (i % 2) continue; s += i; } s", true);
    check("hoisted(); function hoisted(){ return 9; }", true);
}

#[test]
fn functions_calls_and_this() {
    check("function f(a, b){ return a - b; } f(10, 4)", true);
    check("function fib(n){ return n < 2 ? n : fib(n - 1) + fib(n - 2); } fib(15)", true);
    check(
        "function isEven(n){ return n === 0 ? true : isOdd(n - 1); } \
         function isOdd(n){ return n === 0 ? false : isEven(n - 1); } isEven(10)",
        true,
    );
    check("const add = (a, b) => a + b; add(2, 3)", true);
    check("const f = () => { return 8; }; f()", true);
    check("let c = 0; function inc(){ c += 1; return c; } inc(); inc(); c", true);
    // Lexical, not dynamic: `get` sees the top-level `x`, not the caller's.
    check(
        "let x = 0; function get(){ return x; } \
         function caller(){ let x = 99; return get(); } caller()",
        true,
    );
    check("function f(a, b){ return b; } f(1)", true);
    check("function f(a){ return a; } f(1, 2, 3)", true);
    check("let o = [1, 2]; o.push(3); o.length", true);
    check("typeof this", true);
}

#[test]
fn constructors() {
    check("function P(n){ this.n = n; } let p = new P(7); p.n", true);
    check("function P(){ this.a = 1; this.b = 2; } let p = new P(); p.a + p.b", true);
    check("function Q(){ return [1]; } (new Q())[0]", true);
    check("function Q(){ return 5; } (new Q()) instanceof Q", true);
    check("function P(a){ this.a = a; } let p = new P(...[42]); p.a", true);
    check("function P(){ this.x = 1; } new P() instanceof P", true);
    // The evaluator collapses every constructor outcome — including a
    // throw — to the fresh instance; both tiers must agree.
    check("function T(){ throw 9; } (new T()).constructor === T", true);
    check("const A = () => 1; new A()", true);
}

#[test]
fn arrays() {
    check("let a = [1, 2, 3]; a[1]", true);
    check("let a = [10]; a[0] += 5; a[0]", true);
    check("let a = [1]; a[0]++; a[0]", true);
    check("[1, 2, 3].length", true);
    check("let a = []; a.length", true);
}

#[test]
fn fallback_functions_inside_bytecode_units() {
    // The unit compiles; only the unsupported function falls back to AST.
    check("function a(){ return arguments.length; } a(1, 2, 3)", true);
    check("async function f(){ return 1; } f() instanceof Promise", true);
    check("function* g(){ yield 1; } typeof g().next", true);
    check("function r(...rest){ return rest.length; } r(1, 2)", true);
    // Named expressions resolve their own name outward (there is no
    // intermediate self-scope), identically in both tiers — including
    // across reassignment of the shared name.
    check("const f = function foo(){ return 1; }; f()", true);
    check("const g = (function bar(){ return typeof bar; })(); g", true);
    check(
        "let foo = function foo(){ return foo; }; let g = foo; foo = 5; typeof g()",
        true,
    );
}

#[test]
fn closures() {
    // The defining function boxes captured slots into its frame env.
    check("function o(){ let v = 1; function i(){ return v; } return i(); } o()", true);
    // Mutation through the shared cell, both directions.
    check(
        "function mk(){ let n = 0; function inc(){ n += 1; return n; } return inc; } \
         let f = mk(); f(); f();",
        true,
    );
    check(
        "function mk(){ let n = 0; function get(){ return n; } function set(v){ n = v; } \
         set(41); get() + 1; } mk()",
        true,
    );
    // Parameters capture like any other function-level binding.
    check("function mk(a){ return function(){ return a * 2; }; } mk(21)()", true);
    // Transitive capture through an intermediate frame.
    check(
        "function o(){ let x = 5; function m(){ function leaf(){ return x + 1; } return leaf(); } \
         return m(); } o()",
        true,
    );
    // Independent calls get independent cells.
    check(
        "function mk(){ let n = 0; return function(){ n += 1; return n; }; } \
         let a = mk(); let b = mk(); a(); a(); b();",
        true,
    );
    // Capture over a hoisted function declaration.
    check(
        "function o(){ function f(){ return 7; } function g(){ return f() + 1; } return g(); } o()",
        true,
    );
    // Nested declarations recurse through the chain.
    check("function o(){ function f(n){ return n < 1 ? 0 : f(n - 1); } return f(3); } o()", true);
    // Arrows capture slots (only `this`/`arguments` still decline).
    check("function o(){ let x = 1; let f = () => x + 1; return f(); } o()", true);
    // Shadowed names stay direct slots; only the outer cell boxes.
    check(
        "function o(){ let x = 1; function i(){ let x = 2; return x; } return i() + x; } o()",
        true,
    );
    // Captured `var` keeps hoisting semantics.
    check(
        "function o(){ function i(){ return v; } var v = 9; return i(); } o()",
        true,
    );
}

#[test]
fn objects() {
    check("let o = {a: 1}; o.a", true);
    check("let o = {a: 1, b: 2}; o.a + o.b", true);
    check("let o = {}; typeof o", true);
    check("let x = 41; let o = {x}; o.x + 1", true);
    // Shorthand tolerates missing and dead-zone bindings.
    check("let o = {missing}; typeof o.missing", true);
    check("let o = {tdz}; let tdz = 1; typeof o.tdz", true);
    // Computed keys: folded statics and dynamic normalization.
    check("let o = {['lit']: 1}; o.lit", true);
    check("let o = {[42]: 1}; o['42']", true);
    check("let k = 'dyn'; let o = {[k]: 7}; o.dyn", true);
    check("let k = 8; let o = {[k]: 1}; o['8']", true);
    check("let s = Symbol('x'); let o = {[s]: 9}; o[s]", true);
    // Bad computed keys skip the value evaluation entirely.
    check("let ran = false; let o = {[{}]: (ran = true, 1)}; ran", true);
    check("let ran = false; let o = {[null]: (ran = true, 1)}; ran", true);
    check("let ran = false; let o = {[undefined]: (ran = true, 1)}; ran", true);
    check("let ran = false; let o = {[true]: (ran = true, 1)}; ran", true);
    // Dedup: later wins, first position kept.
    check("let o = {a: 1, a: 2}; o.a", true);
    check("Object.keys({a: 1, b: 2, a: 3}).join(',')", true);
    // Methods are named, non-constructor, `this`-bound at call.
    check("let o = {m(){ return 42; }}; o.m()", true);
    check("let o = {m(){ return this.x; }, x: 5}; o.m()", true);
    check("let o = {m(){}}; o.m.name", true);
    check("let o = {m(){}}; new o.m()", true);
    check("let o = {async m(){ return 1; }}; o.m() instanceof Promise", true);
    // Getters and setters pair, replace, and read like the evaluator's.
    check("let o = {get x(){ return 11; }}; o.x", true);
    check(
        "let o = {set x(v){ globalThis.sv = v; }, get x(){ return globalThis.sv + 1; }}; \
         o.x = 10; o.x",
        true,
    );
    check("let o = {set x(v){}}; typeof o.x", true);
    check("let o = {x: 1, get x(){ return 2; }}; o.x", true);
    check("let o = {get x(){ return 2; }, x: 1}; o.x", true);
    // Spread copies objects, ignores the rest, fires getters in order.
    check("let o = {...{a: 1}, b: 2}; o.a + o.b", true);
    check("let o = {...null, ...42, a: 1}; o.a", true);
    check("let o = {...[1, 2]}; typeof o[0]", true);
    check(
        "let log = ''; let s = {get a(){ log += 'a'; return 1; }, get b(){ log += 'b'; return 2; }}; \
         let o = {...s}; log + o.a + o.b",
        true,
    );
    check("let o = {a: 1, ...{a: 2, b: 3}, a: 4}; [o.a, o.b].join(',')", true);
    // `__proto__` is ordinary data: no prototype switching.
    check("let o = {__proto__: 5}; o.__proto__", true);
    // Methods close over slots like any nested function.
    check(
        "function mk(){ let n = 0; return {inc(){ n += 1; return n; }}; } \
         let o = mk(); o.inc(); o.inc()",
        true,
    );
}

#[test]
fn bigint_and_regex_literals() {
    check("typeof 10n", true);
    check("10n + 5n === 15n", true);
    check("10n", true);
    check("typeof /ab+c/", true);
    check("/ab+c/.test('xxabcxx')", true);
    check("/ab+c/.test('xyz')", true);
    check("let r = /a/g; r.lastIndex", true);
    check("let r = /a/g; r.test('a'); r.lastIndex", true);
}

#[test]
fn optional_chaining() {
    check("let o = {a: 1}; o?.a", true);
    check("let o = null; typeof o?.a", true);
    check("let o = {a: {b: 2}}; o?.a?.b", true);
    check("let o = {a: null}; o?.a?.b", true);
    check("let o = null; o?.a.b", true);
    check("let o = {m(){ return 7; }}; o?.m()", true);
    check("let o = null; o?.m()", true);
    check("let f = null; f?.(1)", true);
    check("let f = (x) => x * 2; f?.(21)", true);
    // Arguments evaluate before the nullish check, like the evaluator.
    check("let ran = false; let o = null; o?.m(ran = true); ran", true);
    check("let o = {m(a, b){ return a + b; }}; o?.m(1, 2)", true);
    check("let o = {x: 5, m(){ return this.x; }}; o?.m()", true);
    check("let o = {a: 1}; let k = 'a'; o?.[k]", true);
    check("let o = null; let k = 'a'; o?.[k]", true);
    // The key is not evaluated when short-circuited.
    check("let ran = false; let o = null; o?.[(ran = true, 'a')]; ran", true);
    check("let o = {a: 1}; delete o?.a; o.a", true);
    check("let o = null; delete o?.a", true);
    check("function f(){ return null; } f()?.x", true);
    check("function f(){ return {x: 3}; } f()?.x", true);
    check("let o = {m(){ return {n(){ return 9; }}; }}; o?.m()?.n()", true);
    check("let o = {m(){ return null; }}; o?.m()?.n()", true);
}

#[test]
fn spreads() {
    // Call spread: arrays splice, anything else is one argument.
    check("function f(a, b, c){ return a + b + c; } f(...[1, 2], 3)", true);
    check("function f(...r){ return r.length; } f(...[1, 2])", true);
    check("Math.max(...[1, 5, 3])", true);
    check("function f(a){ return a; } f(...'ab')", true);
    check("function f(a){ return typeof a; } f(...5)", true);
    check("let o = {m(a, b){ return a * b; }}; o.m(...[6, 7])", true);
    // Array spread: arrays splice, strings per character, else iterables.
    check("let a = [...[1, 2], 3]; a.join(',')", true);
    check("let a = [...'ab']; a.join(',')", true);
    check("let a = [0, ...[1, 2], ...[3]]; a.join(',')", true);
    check("function* g(){ yield 1; yield 2; } let a = [...g()]; a.join(',')", true);
    check("let a = [...5]; 1", true);
    check(
        "let log = ''; function t(v){ log += v; return [v]; } \
         let a = [...t('a'), ...t('b')]; log + a.join('')",
        true,
    );
}

#[test]
fn declined_units_stay_on_ast() {
    check("try { throw 5; } catch (e) { e * 2; }", false);
    check("class C {} typeof C", false);
    check("let r = 0; switch (2) { case 1: r = 1; break; case 2: r = 2; break; } r", false);
    check("function o(){ function i(){ return 1; } return i(); } o()", true);
    // Block slots have no frame for the chain to serve: still declined.
    check("{ let y = 1; function f(){ return y; } f(); }", false);
    check("function o(){ { let y = 2; function f(){ return y; } return f(); } } o()", false);
    check(
        "function o(){ let r = 0; for (let i = 0; i < 2; i++) { function f(){ return i; } r += f(); } return r; } o()",
        false,
    );
    check("function g(){ const f = () => this; return f; }", false);
}
