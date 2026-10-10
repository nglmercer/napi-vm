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

fn check(source: &str, expect_bytecode: bool) {
    check_with_modules(source, expect_bytecode, &[]);
}

/// [`check`] with `define_module` sources registered on both interpreters,
/// so imports resolve identically on each tier.
fn check_with_modules(source: &str, expect_bytecode: bool, modules: &[(&str, &str)]) {
    let statements = parse_cached(source).expect("test source must parse");
    let tier = compile_program(&statements);
    assert_eq!(
        tier.is_ok(),
        expect_bytecode,
        "tier decision for {source:?}: {}",
        tier.as_ref()
            .err()
            .map(|e| format!("{e:?}"))
            .unwrap_or_default(),
    );
    if let Ok(module) = &tier {
        verify_module(module).expect("compiler output must verify");
    }
    let program = Interpreter::compile(source).expect("compile");
    let mut vm = Interpreter::with_builtins();
    for (name, body) in modules {
        vm.define_module(name, body.to_string());
    }
    let vm_out = tag(&vm.execute(&program));
    let ast_out = tag(&run_ast_with_modules(source, modules));
    assert_eq!(vm_out, ast_out, "tier divergence for {source:?}");
}

fn run_ast_with_modules(source: &str, modules: &[(&str, &str)]) -> Result<Value, VmErr> {
    let statements = parse_cached(source).expect("test source must parse");
    let mut interp = Interpreter::with_builtins();
    for (name, body) in modules {
        interp.define_module(name, body.to_string());
    }
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
    check(
        "let s = 0; for (let i = 0; i < 5; i++) { if (i % 2) continue; s += i; } s",
        true,
    );
    check("hoisted(); function hoisted(){ return 9; }", true);
}

#[test]
fn functions_calls_and_this() {
    check("function f(a, b){ return a - b; } f(10, 4)", true);
    check(
        "function fib(n){ return n < 2 ? n : fib(n - 1) + fib(n - 2); } fib(15)",
        true,
    );
    check(
        "function isEven(n){ return n === 0 ? true : isOdd(n - 1); } \
         function isOdd(n){ return n === 0 ? false : isEven(n - 1); } isEven(10)",
        true,
    );
    check("const add = (a, b) => a + b; add(2, 3)", true);
    check("const f = () => { return 8; }; f()", true);
    check(
        "let c = 0; function inc(){ c += 1; return c; } inc(); inc(); c",
        true,
    );
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
    check(
        "function P(){ this.a = 1; this.b = 2; } let p = new P(); p.a + p.b",
        true,
    );
    check("function Q(){ return [1]; } (new Q())[0]", true);
    check("function Q(){ return 5; } (new Q()) instanceof Q", true);
    check(
        "function P(a){ this.a = a; } let p = new P(...[42]); p.a",
        true,
    );
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
    check(
        "async function f(){ return 1; } f() instanceof Promise",
        true,
    );
    check("function* g(){ yield 1; } typeof g().next", true);
    check("function r(...rest){ return rest.length; } r(1, 2)", true);
    // Named expressions resolve their own name outward (there is no
    // intermediate self-scope), identically in both tiers — including
    // across reassignment of the shared name.
    check("const f = function foo(){ return 1; }; f()", true);
    check(
        "const g = (function bar(){ return typeof bar; })(); g",
        true,
    );
    check(
        "let foo = function foo(){ return foo; }; let g = foo; foo = 5; typeof g()",
        true,
    );
}

#[test]
fn closures() {
    // The defining function boxes captured slots into its frame env.
    check(
        "function o(){ let v = 1; function i(){ return v; } return i(); } o()",
        true,
    );
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
    check(
        "function mk(a){ return function(){ return a * 2; }; } mk(21)()",
        true,
    );
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
    check(
        "function o(){ function f(n){ return n < 1 ? 0 : f(n - 1); } return f(3); } o()",
        true,
    );
    // Arrows capture slots (only `this`/`arguments` still decline).
    check(
        "function o(){ let x = 1; let f = () => x + 1; return f(); } o()",
        true,
    );
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
    check(
        "let ran = false; let o = {[{}]: (ran = true, 1)}; ran",
        true,
    );
    check(
        "let ran = false; let o = {[null]: (ran = true, 1)}; ran",
        true,
    );
    check(
        "let ran = false; let o = {[undefined]: (ran = true, 1)}; ran",
        true,
    );
    check(
        "let ran = false; let o = {[true]: (ran = true, 1)}; ran",
        true,
    );
    // Dedup: later wins, first position kept.
    check("let o = {a: 1, a: 2}; o.a", true);
    check("Object.keys({a: 1, b: 2, a: 3}).join(',')", true);
    // Methods are named, non-constructor, `this`-bound at call.
    check("let o = {m(){ return 42; }}; o.m()", true);
    check("let o = {m(){ return this.x; }, x: 5}; o.m()", true);
    check("let o = {m(){}}; o.m.name", true);
    check("let o = {m(){}}; new o.m()", true);
    check(
        "let o = {async m(){ return 1; }}; o.m() instanceof Promise",
        true,
    );
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
    check(
        "let o = {a: 1, ...{a: 2, b: 3}, a: 4}; [o.a, o.b].join(',')",
        true,
    );
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
    check(
        "let ran = false; let o = null; o?.[(ran = true, 'a')]; ran",
        true,
    );
    check("let o = {a: 1}; delete o?.a; o.a", true);
    check("let o = null; delete o?.a", true);
    check("function f(){ return null; } f()?.x", true);
    check("function f(){ return {x: 3}; } f()?.x", true);
    check(
        "let o = {m(){ return {n(){ return 9; }}; }}; o?.m()?.n()",
        true,
    );
    check("let o = {m(){ return null; }}; o?.m()?.n()", true);
}

#[test]
fn spreads() {
    // Call spread follows the iterator protocol, including strings and errors.
    check(
        "function f(a, b, c){ return a + b + c; } f(...[1, 2], 3)",
        true,
    );
    check("function f(...r){ return r.length; } f(...[1, 2])", true);
    check("Math.max(...[1, 5, 3])", true);
    check("function f(a){ return a; } f(...'ab')", true);
    check("function f(a){ return typeof a; } f(...5)", true);
    check("let o = {m(a, b){ return a * b; }}; o.m(...[6, 7])", true);
    check(
        "let a=[1,2]; a[Symbol.iterator]=function*(){yield 42;}; function f(x){return x;} f(...a)",
        true,
    );
    check(
        "let a=[1,2]; a[Symbol.iterator]=function*(){yield 42;}; [...a].join(',')",
        true,
    );
    check(
        "let n=0;let o={};o[Symbol.iterator]=function(){return {next(){return ++n===2?{done:true}:{value:42};}}};function f(x){return x;}f(...o)",
        true,
    );
    // Array spread follows the same iterator protocol.
    check("let a = [...[1, 2], 3]; a.join(',')", true);
    check("let a = [...'ab']; a.join(',')", true);
    check("let a = [0, ...[1, 2], ...[3]]; a.join(',')", true);
    check(
        "function* g(){ yield 1; yield 2; } let a = [...g()]; a.join(',')",
        true,
    );
    check("let a = [...5]; 1", true);
    check(
        "let log = ''; function t(v){ log += v; return [v]; } \
         let a = [...t('a'), ...t('b')]; log + a.join('')",
        true,
    );
}

#[test]
fn classes() {
    check("class C {} typeof C", true);
    check(
        "class C { constructor(x){ this.x = x; } get(){ return this.x; } } new C(7).get()",
        true,
    );
    check("class C { m(){ return 1; } } new C().m()", true);
    // Static members, fields, and blocks.
    check("class C { static x = 40 + 2; } C.x", true);
    check("class C { static a = 1; static b = C.a + 1; } C.b", true);
    check("class C { static { C.y = 9; } } C.y", true);
    check("class C { static m(){ return 's'; } } C.m()", true);
    check("class C { static get g(){ return 3; } } C.g", true);
    // Instance fields run in the constructor, in order, observing params.
    check("class C { a = 1; b = 2; } let o = new C(); o.a + o.b", true);
    check(
        "class C { x = this.y; constructor(){ this.y = 5; } } new C().x",
        true,
    );
    check("class C { v = p; constructor(p){} } new C(11).v", true);
    // Accessors and computed names.
    check(
        "class C { get v(){ return this._v; } set v(x){ this._v = x * 2; } } let o = new C(); o.v = 21; o.v",
        true,
    );
    check(
        "let k = 'dyn'; class C { [k](){ return 8; } } new C().dyn()",
        true,
    );
    check("let k = 'f'; class C { [k] = 5; } new C().f", true);
    check("class C { ['a' + 'b'] = 1; } new C().ab", true);
    // Inheritance: implicit and explicit derived constructors, super calls.
    check(
        "class B { constructor(){ this.t = 'b'; } } class D extends B {} new D().t",
        true,
    );
    check(
        "class B { who(){ return 'B'; } } class D extends B { who(){ return super.who() + 'D'; } } new D().who()",
        true,
    );
    check(
        "class B { constructor(x){ this.x = x; } } class D extends B { constructor(){ super(4); } } new D().x",
        true,
    );
    check(
        "class B { static s(){ return 1; } } class D extends B {} D.s()",
        true,
    );
    check("class B extends Object {} new B() instanceof Object", true);
    // Expressions: anonymous, named (name visible to methods only).
    check("let C = class { m(){ return 2; } }; new C().m()", true);
    check(
        "let C = class Named { m(){ return Named === C; } }; new C().m()",
        true,
    );
    check("let C = class { static n = 1; }; C.n", true);
    // Field initializers close over the defining scope; methods capture.
    check(
        "function f(){ let base = 100; return class { m(){ return base + 1; } }; } new (f())().m()",
        true,
    );
    check(
        "function f(){ let v = 9; return class { f = v; }; } new (f())().f",
        true,
    );
    // Errors agree across tiers.
    check("class C { m(){ return super.m(); } } new C().m()", true);
    check("class C extends null {} 1", true);
    assert!(parse_cached("class C { constructor(){ super(); } } new C()").is_err());
}

#[test]
fn modules() {
    // Static imports: default, named, aliased, namespace.
    check_with_modules("import d from 'm'; d", true, &[("m", "export default 42;")]);
    check_with_modules(
        "import { a, b as c } from 'm'; a + c",
        true,
        &[("m", "export const a = 1; export const b = 2;")],
    );
    check_with_modules(
        "import * as ns from 'm'; ns.x * 2",
        true,
        &[("m", "export const x = 21;")],
    );
    check_with_modules(
        "import d, { n } from 'm'; d + n",
        true,
        &[("m", "export default 10; export const n = 5;")],
    );
    // Live bindings: the importer observes later writes.
    check_with_modules(
        "import { v } from 'm'; globalThis.__seen = v; 0",
        true,
        &[("m", "export let v = 1; v = 2;")],
    );
    // Re-exports forward the other module's bindings.
    check_with_modules(
        "import { q } from 'mid'; q",
        true,
        &[
            ("mid", "export { q } from 'leaf';"),
            ("leaf", "export const q = 7;"),
        ],
    );
    check_with_modules(
        "import * as ns from 'mid'; ns.q",
        true,
        &[
            ("mid", "export * from 'leaf';"),
            ("leaf", "export const q = 8; export default 0;"),
        ],
    );
    check_with_modules(
        "import { ns } from 'mid'; ns.q",
        true,
        &[
            ("mid", "export * as ns from 'leaf';"),
            ("leaf", "export const q = 9;"),
        ],
    );
    // Exports from the main program publish into its record.
    check("export default 1 + 2;", true);
    check("let a = 1; export { a };", true);
    check("let a = 1; export { a as b };", true);
    // Dynamic import resolves to a namespace promise; import.meta works.
    check_with_modules(
        "let p = import('m'); typeof p.then",
        true,
        &[("m", "export const x = 1;")],
    );
    check("export {}; let m = import.meta; m.main === false", true);
    // Errors agree across tiers.
    check("import x from 'missing';", true);
    check("import { x } from 'missing';", true);
    check("export * from 'missing';", true);
    // Module declarations are only valid at module top level.
    for source in [
        "{ import x from 'm'; x }",
        "{ let y=1; import x from 'm'; }",
        "function f(){let x=1;export {x};}",
    ] {
        assert!(parse_cached(source).is_err(), "{source}");
    }
}

#[test]
fn captures() {
    // Straight blocks: each entry boxes its own cells.
    check("{ let y = 1; function f(){ return y; } f(); }", true);
    check(
        "function o(){ { let y = 2; function f(){ return y; } return f(); } } o()",
        true,
    );
    check("let f; { let y = 3; f = () => y; } f()", true);
    check("let f; { let y = 1; f = () => y; y = 2; } f()", true);
    check(
        "let a; { let x = 1; a = () => x; } let b; { let x = 2; b = () => x; } a() + b()",
        true,
    );
    // C-style heads box per iteration.
    check(
        "function o(){ let r = 0; for (let i = 0; i < 2; i++) { function f(){ return i; } r += f(); } return r; } o()",
        true,
    );
    check(
        "function f(){ let r = []; for (let i = 0; i < 3; i++) { r.push(() => i); } return r.map(g => g()).join(','); } f()",
        true,
    );
    check(
        "function f(){ let r = []; for (let i = 0; i < 3; i++) { r.push(() => i * 2); } return r.map(g => g()).join(','); } f()",
        true,
    );
    // Loop bodies box per entry; for-in/of heads share one cell, like the AST.
    check(
        "function f(o){ let r = []; for (let k in o) { r.push(() => k); } return r.map(g => g()).join(','); } f({a:1,b:2})",
        true,
    );
    check(
        "function f(o){ let r = []; for (let v of o) { r.push(() => v); } return r.map(g => g()).join(','); } f([1,2])",
        true,
    );
    check(
        "function f(){ let r = []; for (let i = 0; i < 2; i++) { let t = i * 10; r.push(() => t); } return r.map(g => g()).join(','); } f()",
        true,
    );
    // Catch params, switch bindings, and nested arrows.
    check(
        "let f; try { throw 7; } catch (e) { f = () => e; } f()",
        true,
    );
    check(
        "let f; switch (1) { case 1: let q = 9; f = () => q; } f()",
        true,
    );
    check(
        "function g(){ const f = () => this; return typeof f(); } g()",
        true,
    );
    check(
        "let o = { m(){ return () => this.n; }, n: 5 }; o.m()()",
        true,
    );
    check(
        "function f(a, b){ return () => arguments.length; } f(1, 2, 3)()",
        true,
    );
    check(
        "function f(){ return () => () => arguments[0]; } f(42)()()",
        true,
    );
    check(
        "function f(arguments){ return () => arguments; } f(9)()",
        true,
    );
    // Abrupt exits pop scopes exactly once.
    check(
        "let f; { let y = 1; f = () => y; try { throw 0; } catch (e) {} } f()",
        true,
    );
    check(
        "function f(){ for (let i = 0; i < 5; i++) { if (i > 1) break; } return i; } f()",
        true,
    );
    check(
        "let n = 0; for (let i = 0; i < 3; i++) { n += i; continue; } n",
        true,
    );
    check("function f(){ { let y = 1; return () => y; } } f()()", true);
    // Errors agree across tiers.
    check("{ let y = 1; y(); }", true);
    check("function f(){ { let y = y; } } f()", true);
}

#[test]
fn declined_units_stay_on_ast() {
    check(
        "function o(){ function i(){ return 1; } return i(); } o()",
        true,
    );
    // Lexical iteration heads box independently of surrounding block scopes.
    check(
        "function g(o) { let r = []; { let z = 1; { for (let k in o) { r.push(() => k + z); } } } return r; } g({})",
        true,
    );
}

#[test]
fn switch_statements() {
    check(
        "let r = 0; switch (2) { case 1: r = 1; break; case 2: r = 2; break; } r",
        true,
    );
    // Fallthrough without break runs subsequent bodies.
    check(
        "let r = ''; switch (1) { case 1: r += 'a'; case 2: r += 'b'; break; case 3: r += 'c'; } r",
        true,
    );
    // Default runs on no match, in position on fallthrough.
    check(
        "let r = 0; switch (9) { case 1: r = 1; break; default: r = 7; break; } r",
        true,
    );
    check(
        "let r = ''; switch (1) { case 1: r += 'a'; default: r += 'd'; case 3: r += 'c'; } r",
        true,
    );
    check(
        "let r = ''; switch (2) { case 1: r += 'a'; default: r += 'd'; case 3: r += 'c'; } r",
        true,
    );
    // No match and no default: nothing runs.
    check("let r = 5; switch (9) { case 1: r = 1; break; } r", true);
    // Strict equality: no coercion across types.
    check(
        "let r = 0; switch ('1') { case 1: r = 1; break; default: r = 2; } r",
        true,
    );
    check(
        "let r = 0; switch (1) { case '1': r = 1; break; default: r = 2; } r",
        true,
    );
    // First matching case wins; later tests do not run.
    check(
        "let n = 0; function t(v){ n++; return v; } switch (1) { case t(1): break; case t(2): break; } n",
        true,
    );
    // The switch value is the last executed case body value.
    check("switch (1) { case 1: 9; }", true);
    check("switch (9) { case 1: 9; }", true);
    check("switch (1) { case 1: 9; break; case 2: 3; }", true);
    // Cases share one scope: fallthrough sees earlier `let`.
    check(
        "let r = 0; switch (1) { case 1: let q = 4; case 2: r = q * 2; break; } r",
        true,
    );
    // `break` exits the switch; `continue` reaches past it to the loop.
    check(
        "let r = ''; for (let i = 0; i < 3; i++) { switch (i) { case 1: continue; default: r += i; } } r",
        true,
    );
    check(
        "let r = ''; for (let i = 0; i < 3; i++) { switch (i) { case 1: break; default: r += i; } r += '!'; } r",
        true,
    );
    // Lexical hoist before dispatch: TDZ throws like the evaluator.
    check("switch (1) { case 1: r; let r = 2; break; }", true);
    // Nested switches dispatch independently.
    check(
        "let r = ''; switch (1) { case 1: switch (2) { case 2: r = 'in'; break; } r += '!'; } r",
        true,
    );
}

#[test]
fn labeled_statements() {
    check(
        "let r = 0; outer: for (let i = 0; i < 5; i++) { if (i === 2) { break outer; } r = i; } r",
        true,
    );
    check(
        "let r = ''; outer: for (let i = 0; i < 3; i++) { for (let j = 0; j < 3; j++) { if (j === 1) { continue outer; } r += i + '' + j + ';'; } } r",
        true,
    );
    // Labeled non-loop block: break exits with undefined.
    check("let r = 1; blk: { r = 2; break blk; r = 3; } r", true);
    check("blk: { 1; break blk; 2; }", true);
    check("blk: { 1; 2; }", true);
    // Labeled loops keep the loop value on a taken labeled break.
    check(
        "outer: for (let i = 0; i < 3; i++) { if (i === 1) { break outer; } i * 10; }",
        true,
    );
    // Nested labels: breaking to the outer discards the loop value.
    check(
        "a: b: for (let i = 0; i < 3; i++) { if (i === 1) { break a; } 7; }",
        true,
    );
    check(
        "let r = 0; a: b: for (let i = 0; i < 3; i++) { if (i === 1) { break b; } r = i; } r",
        true,
    );
    // Plain break inside a labeled block still exits just the block.
    check(
        "let r = 0; for (let i = 0; i < 3; i++) { blk: { if (i === 1) { break blk; } r += 10; } r += 1; } r",
        true,
    );
    // Labeled while and do-while.
    check(
        "let i = 0; let r = 0; w: while (i < 5) { i++; if (i === 3) { break w; } r = i; } r",
        true,
    );
    check(
        "let i = 0; let r = ''; w: while (i < 4) { i++; if (i === 2) { continue w; } r += i; } r",
        true,
    );
    check(
        "let i = 0; d: do { i++; if (i === 2) { break d; } } while (i < 5); i",
        true,
    );
    // Invalid control-flow targets are early errors in every execution tier.
    assert!(parse_cached("outer: for (;;) { break missing; }").is_err());
    assert!(parse_cached("blk: { continue blk; }").is_err());
}

#[test]
fn tagged_templates() {
    check(
        "function tag(parts){ return parts.join('|'); } tag`a${1}b${2}c`",
        true,
    );
    check(
        "function tag(parts, a, b){ return parts.length + ':' + a + ':' + b; } tag`x${10}y${20}z`",
        true,
    );
    check(
        "function tag(parts){ return parts.length; } tag`plain`",
        true,
    );
    check(
        "function tag(parts, v){ return parts[0] + v + parts[1]; } tag`${40 + 2}`",
        true,
    );
    // Method tags keep their receiver.
    check(
        "let o = { p: 9, tag(parts, v){ return this.p + v; } }; o.tag`!${1}`",
        true,
    );
    // Evaluation order: substitutions, then tag, then call.
    check(
        "let log = ''; function s(v){ log += 's' + v; return v; } function t(p, v){ log += 't'; return log; } t`${s(1)}${s(2)}`",
        true,
    );
    // Fresh parts array per evaluation.
    check(
        "function tag(p){ p.push('x'); return p.length; } [tag`a`, tag`a`].join(',')",
        true,
    );
    // Empty template still passes one empty part.
    check(
        "function tag(p, v){ return p.length + ':' + (v === undefined); } tag``",
        true,
    );
}

#[test]
fn trailing_jumps_land_on_end_of_code() {
    // A `break`/`switch` exit as the last statement of a function body
    // jumps to exactly end-of-code; the VM treats that as fall-off.
    check("function f(){ switch (1) { case 1: 1; break; } } f()", true);
    check("function f(){ switch (9) { case 1: 1; break; } } f()", true);
    check("function f(){ while (1) { break; } } f()", true);
    check("function f(){ for (;;) { break; } } f()", true);
    check("function f(){ blk: { break blk; } } f()", true);
    check("switch (1) { case 1: 1; break; }", true);
    check("while (1) { break; }", true);
}

#[test]
fn destructuring_declarations() {
    check("let [a, b] = [1, 2]; a + b", true);
    check("const {x, y} = {x: 1, y: 2}; x * y", true);
    check("var [a, b] = [3, 4]; a * b", true);
    // Defaults apply on missing and nullish values only.
    check("let [a = 5] = []; a", true);
    check("let [a = 5] = [undefined]; a", true);
    check("let [a = 5] = [null]; a", true);
    check("let [a = 5] = [0]; a", true);
    check("let {a = 5} = {}; a", true);
    check("let {a = 5} = {a: null}; a", true);
    check(
        "let n = 0; function d(){ n++; return 9; } let [a = d()] = [1]; [a, n].join(',')",
        true,
    );
    // Nested patterns, renames, computed keys.
    check("let [a, [b, c]] = [1, [2, 3]]; a + b + c", true);
    check("let {a: {b}} = {a: {b: 7}}; b", true);
    check("let {a: b} = {a: 3}; b", true);
    check("let k = 'x'; let {[k]: v} = {x: 8}; v", true);
    check("let {a, b: {c}} = {a: 1, b: {c: 2}}; a + c", true);
    // Array rest slices from its position.
    check(
        "let [a, ...r] = [1, 2, 3]; [a, r.join(',')].join(':')",
        true,
    );
    check("let [...r] = [1, 2]; r.join(',')", true);
    check("let [a, ...r] = [1]; [a, r.length].join(',')", true);
    // Object rest takes what named keys did not.
    check(
        "let {a, ...r} = {a: 1, b: 2, c: 3}; [a, r.b, r.c].join(',')",
        true,
    );
    check(
        "let k = 'b'; let {[k]: v, ...r} = {a: 1, b: 2}; [v, r.a, r.b].join(',')",
        true,
    );
    // Strings split per character; objects are not array sources.
    check("let [a, b] = 'xy'; a + b", true);
    check("let [a, ...r] = 'xyz'; [a, r.join('')].join(',')", true);
    check("let [a] = {0: 9}; a === undefined", true);
    check("let [a] = 5; a === undefined", true);
    check("let [a] = null; a === undefined", true);
    // Object patterns reject nullish sources.
    check("let {a} = null; 1", true);
    check("let {a} = undefined; 1", true);
    // Declaration elisions consume a position without binding a name.
    check("let [, b] = [1, 2]; b", true);
    // Missing initializers destructure `undefined`.
    check("var [a]; a === undefined", true);
    check("var {a}; 1", true);
    // Pattern heads in `for` with trailing declarators.
    check(
        "let s = 0; for (let [a, b] = [1, 2], i = 0; i < 2; i++) { s += a + b; } s",
        true,
    );
    check(
        "let s = 0; for (var {x} = {x: 3}, i = 0; i < 2; i++) { s += x; } s",
        true,
    );
    // Destructured parameters lower to pattern declarations.
    check("function f([a, b = 2]) { return a + b; } f([10])", true);
    check("function f({x}) { return x; } f({x: 9})", true);
    check(
        "function f({x = 1, ...r}) { return x + (r.y || 0); } f({y: 4})",
        true,
    );
    // Reading a dead-zone name from the initializer throws on both tiers.
    check("let [a] = a; 1", true);
}

#[test]
fn destructuring_assignment() {
    check("let a = 1, b = 2; [a, b] = [b, a]; a * 10 + b", true);
    check("let a; ({x: a} = {x: 5}); a", true);
    check("let a = 0; [a] = [7]; a", true);
    // The assignment evaluates to the right-hand side.
    check("let a = 0; let r = [a] = [7]; r.length + a", true);
    // Member targets write through the object.
    check("let o = {}; [o.p] = [42]; o.p", true);
    check("let o = {}; ({a: o.q} = {a: 9}); o.q", true);
    check(
        "let o = {p: 1}; [o.p, o.q] = [2, 3]; [o.p, o.q].join(',')",
        true,
    );
    // Defaults, nesting, and rest in assignments.
    check("let a; [a = 4] = []; a", true);
    check("let a, b; [a, [b]] = [1, [2]]; a + b", true);
    check("let a, r; [a, ...r] = [1, 2, 3]; a + r.length", true);
    check("let a, r; ({a, ...r} = {a: 1, b: 2}); a + r.b", true);
    // Assignment elisions leave all bindings untouched.
    check("let a; [, a] = [1, 2]; a", true);
    // Nullish object sources throw; array sources tolerate anything.
    check("let a; ({a} = null); 1", true);
    check("let a; [a] = null; a === undefined", true);
    // These targets are syntax errors, before either execution tier.
    assert!(parse_cached("let a; [a] += [1]; 1").is_err());
    assert!(parse_cached("let a; [a()] = [1]; 1").is_err());
}

#[test]
fn try_catch_finally() {
    check("try { throw 5; } catch (e) { e * 2; }", true);
    check("let r = 0; try { r = 1; } catch (e) { r = 2; } r", true);
    // Runtime errors arrive as error objects with name and message.
    check("try { null.x; } catch (e) { e.name; }", true);
    check(
        "try { x_undefined; } catch (e) { e.name + ':' + typeof e.stack; }",
        true,
    );
    check("try { 1 + {}; } catch (e) { e.name; }", true);
    // The try value is the body or catch completion.
    check("try { 1; 2; } catch (e) {}", true);
    check("try { throw 0; } catch (e) { 3; }", true);
    // Catch binds its parameter in a fresh scope.
    check(
        "let e = 1; let r = 0; try { throw 2; } catch (e) { r = e; } [r, e].join(',')",
        true,
    );
    check("try { throw 1; } catch (e) { let q = e + 1; q; }", true);
    // Finally runs on every path; its value is discarded.
    check(
        "let r = ''; try { r += 't'; } finally { r += 'f'; } r",
        true,
    );
    check(
        "let r = ''; try { r += 't'; throw 1; } catch (e) { r += 'c'; } finally { r += 'f'; } r",
        true,
    );
    check("try { 1; } finally { 2; }", true);
    check("try { throw 1; } catch (e) { 2; } finally { 3; }", true);
    // Finally-only rethrows after cleanup.
    check(
        "let r = ''; try { try { throw 7; } finally { r += 'f'; } } catch (e) { r += e; } r",
        true,
    );
    check("try { throw 42; } finally {}", true);
    // Return runs finally, then proceeds; a finally return replaces it.
    check(
        "function f(){ try { return 1; } finally { r = 2; } } let r = 0; [f(), r].join(',')",
        true,
    );
    check(
        "function f(){ try { return 1; } finally { return 2; } } f()",
        true,
    );
    check(
        "function f(){ try { throw 0; } catch (e) { return 1; } finally { r = 5; } } let r = 0; [f(), r].join(',')",
        true,
    );
    check(
        "function f(){ try { return 1; } catch (e) { return 2; } } f()",
        true,
    );
    // A throw inside finally replaces the in-flight outcome.
    check(
        "try { try { throw 1; } finally { throw 2; } } catch (e) { e; }",
        true,
    );
    check(
        "function f(){ try { return 1; } finally { throw 3; } } try { f(); } catch (e) { e; }",
        true,
    );
    // Break and continue run finally, then proceed.
    check(
        "let r = ''; for (let i = 0; i < 3; i++) { try { break; } finally { r += 'f'; } r += 'x'; } r",
        true,
    );
    check(
        "let r = ''; for (let i = 0; i < 2; i++) { try { continue; } finally { r += 'f'; } r += 'x'; } r",
        true,
    );
    check(
        "let r = ''; outer: { try { break outer; } finally { r += 'f'; } r += 'x'; } r",
        true,
    );
    check(
        "let r = ''; for (let i = 0; i < 2; i++) { try { throw 1; } catch (e) { continue; } finally { r += 'f'; } r += 'x'; } r",
        true,
    );
    // Control-flow signals are not catchable.
    check(
        "let i = 0; for (; i < 5;) { try { break; } catch (e) {} } i",
        true,
    );
    // Nested handlers compose.
    check(
        "try { try { throw 'a'; } catch (e) { throw 'b'; } } catch (e) { e; }",
        true,
    );
    check(
        "let r = ''; try { try { throw 1; } finally { r += 'i'; } } catch (e) { r += 'c'; } finally { r += 'o'; } r",
        true,
    );
}

#[test]
fn for_in_loops() {
    check(
        "let r = ''; for (let k in {a: 1, b: 2}) { r += k; } r",
        true,
    );
    check(
        "let o = {a: 1, b: 2}; let s = 0; for (let k in o) { s += o[k]; } s",
        true,
    );
    // Lexical heads shadow outer bindings and restore them after the loop.
    check("let k = 9; for (let k in {a: 1}) {} k", true);
    check("for (var k in {a: 1}) {} k", true);
    check(
        "function f(){ let k = 9; for (let k in {a: 1}) {} return k; } f()",
        true,
    );
    // ...while a head inside a block shadows the outer binding.
    check(
        "function f(){ let k = 9; { for (let k in {a: 1}) {} } return k; } f()",
        true,
    );
    check(
        "function f(){ for (let k in {a: 1}) {} return k; } f()",
        true,
    );
    // Break, continue, and labels thread through.
    check(
        "let r = ''; for (let k in {a: 1, b: 2, c: 3}) { if (k === 'b') { continue; } r += k; } r",
        true,
    );
    check(
        "let r = ''; for (let k in {a: 1, b: 2, c: 3}) { if (k === 'b') { break; } r += k; } r",
        true,
    );
    check("outer: for (let k in {a: 1}) { break outer; }", true);
    check(
        "let r = ''; outer: for (let k in {a: 1}) { for (let j in {x: 1}) { continue outer; r += 'x'; } } r",
        true,
    );
    // Keys snapshot once; later mutations do not join the iteration.
    check(
        "let o = {a: 1}; let r = ''; for (let k in o) { o[k + 'z'] = 1; r += k; } r",
        true,
    );
    // Empty and non-object sources simply run zero times.
    check("let r = 0; for (let k in {}) { r = 1; } r", true);
    check("for (let k in 5) {}", true);
    check("for (let k in null) {}", true);
    // The loop value is the last body value.
    check("for (let k in {a: 1}) { 7; }", true);
    check("for (let k in {}) { 7; }", true);
    // Nested loops and captured heads.
    check(
        "let r = ''; for (let a in {x: 1}) { for (let b in {y: 1}) { r += a + b; } } r",
        true,
    );
    check(
        "function f(){ for (let k in {a: 1, b: 2}) {} function g(){ return k; } return g(); } f()",
        true,
    );
    // Pattern heads now retain their actual binding and use AST fallback.
    check("let o = {x: 1}; for (let [a] in o) {} typeof a", false);
}

#[test]
fn for_of_loops() {
    check("let s = 0; for (let v of [1, 2, 3]) { s += v; } s", true);
    check("let r = ''; for (let c of 'ab') { r += c; } r", true);
    // Lexical heads shadow like `for-in` heads do.
    check("let v = 9; for (let v of [1]) {} v", true);
    check(
        "function f(){ let v = 9; for (let v of [1]) {} return v; } f()",
        true,
    );
    check(
        "function f(){ let v = 9; { for (let v of [1]) {} } return v; } f()",
        true,
    );
    check(
        "let r = ''; for (let v of [1, 2, 3]) { if (v === 2) { continue; } r += v; } r",
        true,
    );
    check(
        "let r = ''; for (let v of [1, 2, 3]) { if (v === 2) { break; } r += v; } r",
        true,
    );
    // Pattern heads destructure per iteration.
    check(
        "let s = 0; for (let [a, b] of [[1, 2], [3, 4]]) { s += a + b; } s",
        true,
    );
    check(
        "let s = 0; for (let {x} of [{x: 1}, {x: 2}]) { s += x; } s",
        true,
    );
    check(
        "let s = 0; for (let [a = 9] of [[], [2]]) { s += a; } s",
        true,
    );
    // Early exits close the iterator; exhaustion and continue do not.
    check(
        "function* g(){ try { yield 1; yield 2; } finally { log += 'c'; } } let log = ''; for (let v of g()) { break; } log",
        true,
    );
    check(
        "function* g(){ try { yield 1; yield 2; } finally { log += 'c'; } } let log = ''; function f(){ for (let v of g()) { return v; } } f(); log",
        true,
    );
    check(
        "function* g(){ try { yield 1; yield 2; } finally { log += 'c'; } } let log = ''; try { for (let v of g()) { throw 9; } } catch (e) {} log",
        true,
    );
    check(
        "function* g(){ try { yield 1; } finally { log += 'c'; } } let log = ''; for (let v of g()) {} log",
        true,
    );
    check(
        "function* g(){ try { yield 1; yield 2; } finally { log += 'c'; } } let log = ''; for (let v of g()) { continue; } log",
        true,
    );
    // A destructuring failure also closes before propagating.
    check(
        "function* g(){ try { yield null; } finally { log += 'c'; } } let log = ''; try { for (let {a} of g()) {} } catch (e) { log += 'e'; } log",
        true,
    );
    check(
        "let e={},caught;let o={};o[Symbol.iterator]=function(){return {next(){throw e;},return(){throw 42;}};};try{for(let x of o){}}catch(x){caught=x;}caught===e",
        true,
    );
    check(
        "let e={},caught;let o={};o[Symbol.iterator]=function(){return {next(){return {done:false};},get return(){throw 42;}};};try{for(let x of o){throw e;}}catch(x){caught=x;}caught===e",
        true,
    );
    check(
        "let e={},caught;let o={};o[Symbol.iterator]=function(){return {next(){return {done:false};},return:42};};try{for(let x of o){throw e;}}catch(x){caught=x;}caught===e",
        true,
    );
    check(
        "let e={},caught;let o={};o[Symbol.iterator]=function(){return {next(){return {get done(){throw e;}};},return(){throw 42;}};};try{for(let x of o){}}catch(x){caught=x;}caught===e",
        true,
    );
    // Non-iterables and method-less iterators fail like the evaluator.
    check("for (let v of {}) {}", true);
    check(
        "let o = {}; o['__symbol_iterator__'] = function() { return {}; }; for (let v of o) {}",
        true,
    );
    // A missing `done` is false; a missing value is undefined. Bound the
    // consumer because this iterator deliberately never reports completion.
    check(
        "let o = {}; o['__symbol_iterator__'] = function() { return { next: function() { return {}; } }; }; let n = 0; for (let v of o) { n++; if (n === 2) break; } n",
        true,
    );
    check(
        "let o = {}; let calls = 0; o['__symbol_iterator__'] = function() { return { next: function() { calls++; return calls > 1 ? {done: true} : {done: false}; } }; }; let r = []; for (let v of o) { r.push(v); } [r.length, r[0] === undefined].join(',')",
        true,
    );
    // Loop value, nesting, captured heads.
    check("for (let v of [1]) { 7; }", true);
    check(
        "let r = ''; for (let a of [1]) { for (let b of [2]) { r += a + b; } } r",
        true,
    );
    check(
        "function f(){ let g; for (let v of [1, 2]) { g = function() { return v; }; } return g(); } f()",
        true,
    );
    check(
        "function f(){ for (let [a] of [[1], [2]]) {} function g(){ return a; } return g(); } f()",
        true,
    );
}

#[test]
fn raw_tagged_templates_and_export_mutation_remain_equivalent() {
    check("function tag(s){return s.raw[0];}tag`a\\nb`;", true);
    check("String.raw`a\\nb`;", true);
    check_with_modules(
        "import {n,inc} from 'counter';inc();n;",
        true,
        &[(
            "counter",
            "export let n=7;export function inc(){return ++n;}",
        )],
    );
}

#[test]
fn arrow_super_uses_lexical_receiver() {
    for source in [
        "class A { constructor(){ this.x=1; } } class B extends A { constructor(){ (()=>super())(); } } new B().x;",
        "class A { constructor(){ this.x=1; } } class B extends A { constructor(){ (()=>(()=>super())())(); } } new B().x;",
        "class A { method(){ return this.x; } } class B extends A { constructor(){super();this.x=7;} method(){return (()=>super.method())();} } new B().method();",
        "class A { method(){ return this.x; } } class B extends A { constructor(){super();this.x=7;} method(){return (()=>(()=>super.method())())();} } new B().method();",
        "class A { constructor(){this.x=1;} } class B extends A { constructor(){super();function inner(){return this;}this.y=inner();} } new B().y;",
    ] {
        check(source, true);
    }
    assert!(parse_cached("class A { method(){return this.x;} } class B extends A { method(){function inner(){return super.method();}return inner.call({x:9});} }").is_err());
}

#[test]
fn named_expression_bindings_are_lexical_and_immutable_in_both_tiers() {
    for source in [
        "var f=function self(n){return n ? self(n-1) : 7;};f(4)",
        "var f=function self(){self=1;return typeof self;};f()",
        "var f=function self(){'use strict';self=1;};f()",
        "var f=function self(){self+=1;return typeof self;};f()",
        "function f(){return f;}var old=f;f=7;old()",
        "var f=function self(){return self;};typeof self",
        "var f=async function self(){return typeof self;};f().then(v=>{if(v!=='function')throw 1;})",
    ] {
        check(source, true);
    }
}

#[test]
fn catch_identifier_bindings_remain_lexical_and_mutable_in_both_tiers() {
    for source in [
        "var e=7;try{throw 1;}catch(e){var e=3;}e;",
        "let f;try{throw 1;}catch(e){f=()=>++e;}f()+f();",
        "let e=7;try{throw 1;}catch{e=9;}e;",
        "function f(){let e=7;try{throw 1;}catch(e){e=3;}return e;}f();",
    ] {
        check(source, true);
    }
    check("try{throw {x:7};}catch({x}){x;}", false);
    // Direct eval still declines compilation until lexical-environment parity.
    check("var e=7;try{throw 1;}catch(e){eval('var e=3');}e;", false);
    check("try{throw 1;}catch(e){eval('function e(){}');e;}", false);
}

#[test]
fn iteration_assignment_forms_retain_fallback_and_declaration_tier_parity() {
    for source in [
        "var x;for(x of [1,2]){}x;",
        "let x;for([x] of [[7]]){}x;",
        "let o={x:0};for(o.x of [9]){}o.x;",
        "for(var x=7 in {a:1}){}x;",
    ] {
        check(source, false);
    }
    check("function f(){for(var x of []){}return x;}f();", true);
    check("function f(){for(var x in {}){}return x;}f();", true);
}

#[test]
fn literal_metadata_keeps_primitive_and_object_tier_parity() {
    for source in [
        "010+1;",
        r"'\012'.charCodeAt(0);",
        r"({'\1':3})['\1'];",
        r"var \u0061=6;a/2;",
        "var α=3;α+1;",
    ] {
        check(source, true);
    }
}

#[test]
fn parenthesized_references_keep_existing_tier_parity() {
    for source in [
        "var x=1;(x)++;(x)=4;x;",
        "typeof (missing);",
        "var o={x:1};delete (o.x);'x' in o;",
        "var o={x:3,f:function(){return this.x;}};(o.f)();",
    ] {
        check(source, true);
    }
}

#[test]
fn public_hash_properties_preserve_private_field_tier_parity() {
    check(
        "class C{#x=1;'#x'=2;m(){return this.#x+this['#x'];}}new C().m();",
        true,
    );
}

#[test]
fn native_buffer_growth_uses_the_same_semantics_in_both_tiers() {
    for source in [
        "var b=new SharedArrayBuffer(4,{maxByteLength:16});var a=new Int32Array(b);a[0]=7;b.grow(16);Atomics.store(a,3,42);a.length===4&&a[0]===7&&a[3]===42;",
        "var b=new ArrayBuffer(8,{maxByteLength:16});var a=new Uint8Array(b,4,4);b.resize(2);var empty=a.length===0;b.resize(16);empty&&a.length===4&&a[0]===0;",
    ] {
        check(source, true);
    }
}

#[test]
fn realm_intrinsic_arguments_and_proxy_abrupt_completions_share_tier_semantics() {
    for source in [
        "function f(){'use strict';return()=>arguments;}var args=f()();var getter=Object.getOwnPropertyDescriptor(args,'callee').get;getter===Object.getOwnPropertyDescriptor(Function.prototype,'caller').get;",
        "function f(){return arguments;}var args=f(42);args.callee===f&&args[0]===42&&args[Symbol.iterator]===Array.prototype.values;",
        "var proxy=new Proxy({},{get:1});var caught=false;try{proxy.x;}catch(e){caught=e instanceof TypeError;}caught;",
    ] {
        check(source, true);
    }
}

#[test]
fn shared_proxy_deletion_and_atomic_hints_match_both_tiers() {
    for source in [
        "'use strict';var o={};Object.defineProperty(o,'x',{value:1});delete o.x;",
        "var o={};Object.defineProperty(o,'x',{value:1});delete o.x;",
        "'use strict';var p=new Proxy(new String('x'),{});delete p[0];",
        "var p=new Proxy(new Uint8Array(1),{});Reflect.deleteProperty(p,'0');",
        "Atomics.pause(1)===undefined;",
    ] {
        check(source, true);
    }
}

#[test]
fn realm_global_records_use_shared_declaration_and_property_operations() {
    for source in [
        "let x=1;globalThis.x=2;x+globalThis.x;",
        "var x=1;Object.getOwnPropertyDescriptor(globalThis,'x').configurable;",
        "Object.defineProperty(globalThis,'f',{get(){throw 1;},configurable:true});eval('function f(){return 42;}');f();",
        "var receiver;Object.defineProperty(globalThis,'x',{get(){receiver=this;return 1;},set(v){}});x++;receiver===globalThis;",
        "let f=1;{function f(){}}f;",
    ] {
        check(source, true);
    }
}

#[test]
fn lexical_iteration_tdz_and_capture() {
    for source in [
        "let x = [1]; for (let x of x) {}",
        "let x = {}; for (let x in x) {}",
        "let a=[]; for(let x of [1,2]) { a.push(()=>x); } a[0]()+a[1]()",
        "let a=[]; for(const x of [1,2]) { a.push(()=>x); } a[0]()+a[1]()",
        "let a=[]; for(let x in {a:1,b:2}) { a.push(()=>x); } a[0]()+a[1]()",
        "for(const x of [1]) { x=2; }",
        "let x=9; try { for(let x of [1]) { throw x; } } catch(e) {} x",
        "let a=[]; for(let [x] of [[1],[2]]) { a.push(()=>x); } a[0]()+a[1]()",
    ] {
        check(source, true);
    }
}

#[test]
fn property_update_fast_paths_preserve_observable_operations() {
    for source in [
        "var calls=0,o={x:1},key={toString(){calls++;return 'x';}};var old=o[key]++;[calls,old,o.x]",
        "var reads=0,writes=0,value=1,o={get x(){reads++;return value;},set x(v){writes++;value=v;}};var old=o.x++;[reads,writes,old,value]",
        "var reads=0,writes=0,o=new Proxy({x:1},{get(t,k,r){reads++;return Reflect.get(t,k,r);},set(t,k,v,r){writes++;return Reflect.set(t,k,v,r);}});var old=o.x++;[reads,writes,old,o.x]",
        "var o={};Object.defineProperty(o,'x',{value:1,writable:false});var old=o.x++;[old,o.x]",
        "var value=1,reads=0,writes=0;Object.defineProperty(globalThis,'x',{get(){reads++;return value;},set(v){writes++;value=v;},configurable:true});var old=x++;[reads,writes,old,value]",
    ] {
        check(source, true);
    }
}

#[test]
fn empty_lexical_iteration_patterns_own_and_unwind_runtime_scopes() {
    for source in [
        "var n=0;for(let [] of [[],[]]){n++;}n",
        "var n=0;for(const {} of [{},{}]){n++;}n",
        "var n=0;for(let [[]] of [[[]],[[]]]){n++;}n",
        "try{for(let {} of [null]){}}catch(e){e instanceof TypeError;}",
        "try{for(const {} of [undefined]){}}catch(e){e instanceof TypeError;}",
        "var n=0;for(let [] of [[],[]]){if(++n===1)continue;break;}n",
        "var n=0;for(const [] of [[],[]]){try{n++;}finally{}}n",
    ] {
        check(source, true);
    }
}

#[test]
fn global_callable_data_and_update_reference_order_match_both_tiers() {
    for source in [
        "var calls=0;var f=Object.getOwnPropertyDescriptor({get f(){calls++;}},'f').get;typeof f==='function'&&calls===0&&Object.getOwnPropertyDescriptor(globalThis,'f').value===f;",
        "var count=0;var key={toString(){count++;throw 1;}};var caught;try{++null[key];}catch(e){caught=e;}caught instanceof TypeError&&count===0;",
        "var count=0;var key={toString(){count++;throw 1;}};var caught;try{null[key]--;}catch(e){caught=e;}caught instanceof TypeError&&count===0;",
        "var names=[];for(var key in new String('abc')){names.push(key);}names.join(',');",
        "var object=Object.create(new Int32Array(1));var caught;try{object.buffer;}catch(e){caught=e;}caught instanceof TypeError;",
    ] {
        check(source, true);
    }
}

#[test]
fn proxy_private_field_stamping_uses_shared_brand_storage_in_both_tiers() {
    for source in [
        "class Identity{constructor(object){return object;}}class Stamp extends Identity{#value=42;static read(object){return object.#value;}static write(object,value){object.#value=value;}}var target={};var traps=0;var proxy=new Proxy(target,{get(){traps++;throw 1;},set(){traps++;throw 2;}});new Stamp(proxy);Stamp.write(proxy,7);Stamp.read(proxy)===7&&traps===0;",
        "class Identity{constructor(object){return object;}}class Stamp extends Identity{#value=42;static read(object){return object.#value;}}var pair=Proxy.revocable({},{});pair.revoke();new Stamp(pair.proxy);Stamp.read(pair.proxy);",
        "class Identity{constructor(object){return object;}}class Stamp extends Identity{#value=42;}var proxy=new Proxy({},{});new Stamp(proxy);new Stamp(proxy);",
    ] {
        check(source, true);
    }
}

#[test]
fn private_field_brand_checks_use_lexical_slots_in_both_tiers() {
    for source in [
        "class A{#x;static has(o){return #x in o;}}var a=new A();A.has(a)&&!A.has({})&&!A.has(Object.create(a));",
        "class A{#x;static has(o){return #x in o;}}var caught;try{A.has(1);}catch(e){caught=e;}caught instanceof TypeError;",
        "class Identity{constructor(o){return o;}}class A extends Identity{#x;static has(o){return #x in o;}}var p=Proxy.revocable({},{});p.revoke();new A(p.proxy);A.has(p.proxy);",
        "class A{#x;static has(o){return #x in o;}}class B{#x;static has(o){return #x in o;}}var a=new A();A.has(a)&&!B.has(a);",
        "var calls=0;class A{#x;static has(){return #x in (++calls,new A());}}A.has()&&calls===1;",
    ] {
        check(source, true);
    }
}

#[test]
fn class_private_methods_accessors_and_static_order_share_semantics() {
    for source in [
        "class A{#value=this.#method();#method(){return 42;}read(){return this.#value;}}new A().read();",
        "class A{#method(){}static has(o){return #method in o;}write(){this.#method=1;}}var a=new A();var caught;try{a.write();}catch(e){caught=e;}A.has(a)&&caught instanceof TypeError;",
        "class A{#value=1;get #access(){return this.#value;}set #access(v){this.#value=v;}read(){return this.#access;}write(v){this.#access=v;}}var a=new A();a.write(42);a.read();",
        "class A{static #field=1;static #method(){return this.#field;}static get #access(){return this.#method();}static set #access(v){this.#field=v;}static read(){return this.#access;}static write(v){this.#access=v;}}A.write(42);A.read();",
        "var order='';class A{static x=(order+='a',1);static{order+='b';}static #y=(order+='c',this.x+1);static{order+='d';}static read(){return this.#y;}}order==='abcd'&&A.read()===2;",
        "function make(v){class A{static x=v;static #y=v+1;static read(){return this.#y;}}return A;}var A=make(41);A.x===41&&A.read()===42;",
        "class Base{static value=41;}class A extends Base{static x=super.value+1;static read(){return super.value;}}A.x===42&&A.read()===41;",
    ] {
        check(source, true);
    }
}

#[test]
fn legacy_regexp_state_and_descriptors_match_in_both_tiers() {
    for source in [
        "/(a)(b)/.exec('xabz');RegExp.$1==='a'&&RegExp.lastMatch==='ab'&&RegExp.leftContext==='x'&&RegExp.rightContext==='z';",
        "var caught;try{Reflect.get(RegExp,'lastMatch',{});}catch(e){caught=e;}caught instanceof TypeError;",
        "var calls=0;RegExp.input={toString(){calls++;return 'input';}};RegExp.input==='input'&&calls===1;",
        "class Derived extends RegExp{}new Derived('a').test('a');RegExp.input='restored';var caught;try{RegExp.lastMatch;}catch(e){caught=e;}RegExp.input==='restored'&&caught instanceof TypeError;",
    ] {
        check(source, true);
    }
}

#[test]
fn dictionary_growth_preserves_property_reads_and_key_order_in_both_tiers() {
    check(
        "var object={};for(var i=0;i<256;i++){object['key'+i]=i;}function read(){return object.key1;}read();read();object.extra=7;var keys=Object.keys(object);read()===1&&object.key255===255&&keys.length===257&&keys[256]==='extra';",
        true,
    );
}

#[test]
fn class_property_keys_preserve_symbols_and_observable_conversion_in_both_tiers() {
    for source in [
        "var log='';var key={};key[Symbol.toPrimitive]=function(hint){log+=hint;return 'method';};class A{[key](){return 42;}[log+='next'](){}}log==='stringnext'&&new A().method()===42;",
        "var key=Symbol('member');class A{[key](){return 42;}static [key](){return 7;}}var a=new A();a[key]()===42&&A[key]()===7&&Object.getOwnPropertySymbols(A.prototype)[0]===key&&Object.getOwnPropertySymbols(A)[0]===key&&a[key].name==='[member]';",
        "var key=Symbol('field');var calls=0;var converted={toString(){calls++;return key;}};class A{[converted]=42;static [converted]=7;}var a=new A();calls===2&&a[key]===42&&A[key]===7&&Reflect.ownKeys(a)[0]===key;",
        "var key=Symbol();class A{get [key](){return 42;}set [key](v){this.value=v;}}var a=new A();a[key]=7;var d=Object.getOwnPropertyDescriptor(A.prototype,key);a[key]===42&&a.value===7&&d.get.name==='get '&&d.set.name==='set ';",
    ] {
        check(source, true);
    }
}

#[test]
fn super_reads_follow_home_object_prototypes_in_both_tiers() {
    for source in [
        "class A{method(){return super.value;}}Object.setPrototypeOf(A.prototype,{value:42});var a=new A();var method=a.method;method.call({value:7})===42;",
        "class A{method(){return 1;}}class B extends A{method(){return super.method();}static method(){return super.value;}}Object.setPrototypeOf(B.prototype,{method(){return this.value;}});Object.setPrototypeOf(B,{value:7});var b=new B();b.value=42;b.method()===42&&B.method()===7;",
        "class A{method(){return ()=>super.value;}}Object.setPrototypeOf(A.prototype,{value:42});var f=new A().method();Object.setPrototypeOf(A.prototype,{value:7});f()===7;",
        "class A{constructor(){this.value=1;}}class B extends A{}function Other(){this.value=42;}Object.setPrototypeOf(B,Other);new B().value===42;",
    ] {
        check(source, true);
    }
}

#[test]
fn super_references_writes_calls_and_object_home_objects_share_semantics() {
    for source in [
        "var object={method(){return ()=>super.value;}};Object.setPrototypeOf(object,{value:42});var f=object.method();Object.setPrototypeOf(object,{value:7});f()===7;",
        "class A{get x(){return this.value;}set x(v){this.value=v;}}class B extends A{write(){return super.x=42;}update(){return super.x++;}compound(){return super.x+=2;}}var b=new B();b.value=1;b.write()===42&&b.update()===42&&b.compound()===45&&b.value===45;",
        "class A{get x(){return this.value;}set x(v){this.value=v;}}class B extends A{update(){return super.x++;}logical(){return super.x??=42n;}}var b=new B();b.value=1n;b.update()===1n&&b.value===2n&&b.logical()===2n;",
        "class A{constructor(){this.value=1;}}function Other(){this.value=42;}class B extends A{constructor(){super(Object.setPrototypeOf(B,Other));}}new B().value===1;",
        "var log='';class A{}class B extends A{method(){return super.method(log+='argument');}}Object.setPrototypeOf(B.prototype,{get method(){log+='get';return function(v){return this.value+v;};}});var b=new B();b.value='receiver';b.method()==='receivergetargument'&&log==='getargument';",
        "var log='';class A{set x(v){log+='old';this.value=v;}}class B extends A{write(){super.x=(Object.setPrototypeOf(B.prototype,{set x(v){log+='new';}}),42);}}var b=new B();b.write();log==='old'&&b.value===42;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "AST result for {source}"
        );
    }
}

#[test]
fn class_member_attributes_and_field_names_share_semantics() {
    for source in [
        "class A{method(){return 1;}method(){return 42;}get value(){return 1;}value(){return 7;}}var a=new A();var d=Object.getOwnPropertyDescriptor(A.prototype,'method');a.method()===42&&a.value()===7&&!d.enumerable&&d.writable&&d.configurable&&Object.keys(A.prototype).length===0;",
        "var key=Symbol('named');class A{[key]=()=>42;static field=function(){};value=class{static inferred=this.name;};}var a=new A();a[key].name==='[named]'&&A.field.name==='field'&&a.value.name==='value'&&a.value.inferred==='value';",
    ] {
        check(source, true);
        assert!(matches!(
            run_ast_with_modules(source, &[]),
            Ok(Value::Bool(true))
        ));
    }
}

#[test]
fn super_destructuring_tags_and_delete_keep_tier_parity() {
    for source in [
        "class A{set x(v){this.value=v;}tag(parts,v){return this.value+v;}}class B extends A{assign(){[super.x]=[42];}tagged(){return super.tag`value${7}`;}}var b=new B();b.assign();b.value===42&&b.tagged()===49;",
        "var calls=0;var key={toString(){calls++;return 'x';}};class A{method(){return delete super[key];}}var caught;try{new A().method();}catch(e){caught=e;}calls===1&&caught instanceof ReferenceError;",
    ] {
        check(source, true);
        assert!(matches!(
            run_ast_with_modules(source, &[]),
            Ok(Value::Bool(true))
        ));
    }
}
