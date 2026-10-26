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
    // Primitive and ordinary-object keys normalize before value evaluation.
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
    // Primitive prototype setters leave the default prototype and no own key.
    // Compare guest-visible primitives: dumping Object.prototype recursively
    // would traverse its constructor/prototype cycle in the test formatter.
    check(
        "let o = {__proto__: 5}; Object.getPrototypeOf(o)===Object.prototype&&!Object.hasOwn(o,'__proto__')",
        true,
    );
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
    check("let [a, b] = [1, 2]; a + b", false);
    check("const {x, y} = {x: 1, y: 2}; x * y", true);
    check("var [a, b] = [3, 4]; a * b", false);
    // Defaults apply only to undefined values.
    check("let [a = 5] = []; a", false);
    check("let [a = 5] = [undefined]; a", false);
    check("let [a = 5] = [null]; a", false);
    check("let [a = 5] = [0]; a", false);
    check("let {a = 5} = {}; a", true);
    check("let {a = 5} = {a: null}; a", true);
    check(
        "let n = 0; function d(){ n++; return 9; } let [a = d()] = [1]; [a, n].join(',')",
        false,
    );
    // Nested patterns, renames, computed keys.
    check("let [a, [b, c]] = [1, [2, 3]]; a + b + c", false);
    check("let {a: {b}} = {a: {b: 7}}; b", true);
    check("let {a: b} = {a: 3}; b", true);
    check("let k = 'x'; let {[k]: v} = {x: 8}; v", true);
    check("let {a, b: {c}} = {a: 1, b: {c: 2}}; a + c", true);
    // Array rest consumes the remaining iterator.
    check(
        "let [a, ...r] = [1, 2, 3]; [a, r.join(',')].join(':')",
        false,
    );
    check("let [...r] = [1, 2]; r.join(',')", false);
    check("let [a, ...r] = [1]; [a, r.length].join(',')", false);
    // Object rest takes what named keys did not.
    check(
        "let {a, ...r} = {a: 1, b: 2, c: 3}; [a, r.b, r.c].join(',')",
        true,
    );
    check(
        "let k = 'b'; let {[k]: v, ...r} = {a: 1, b: 2}; [v, r.a, r.b].join(',')",
        true,
    );
    // Strings iterate by code point; non-iterable sources throw.
    check("let [a, b] = 'xy'; a + b", false);
    check("let [a, ...r] = 'xyz'; [a, r.join('')].join(',')", false);
    check("let [a] = {0: 9}; a === undefined", false);
    check("let [a] = 5; a === undefined", false);
    check("let [a] = null; a === undefined", false);
    // Object patterns reject nullish sources.
    check("let {a} = null; 1", true);
    check("let {a} = undefined; 1", true);
    // Declaration elisions consume a position without binding a name.
    check("let [, b] = [1, 2]; b", false);
    // Missing initializers destructure `undefined`.
    check("var [a]; a === undefined", false);
    check("var {a}; 1", true);
    // Pattern heads in `for` with trailing declarators.
    check(
        "let s = 0; for (let [a, b] = [1, 2], i = 0; i < 2; i++) { s += a + b; } s",
        false,
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
    check("let [a] = a; 1", false);
}

#[test]
fn destructuring_assignment() {
    check("let a = 1, b = 2; [a, b] = [b, a]; a * 10 + b", false);
    check("let a; ({x: a} = {x: 5}); a", false);
    check("let a = 0; [a] = [7]; a", false);
    // The assignment evaluates to the right-hand side.
    check("let a = 0; let r = [a] = [7]; r.length + a", false);
    // Member targets write through the object.
    check("let o = {}; [o.p] = [42]; o.p", false);
    check("let o = {}; ({a: o.q} = {a: 9}); o.q", false);
    check(
        "let o = {p: 1}; [o.p, o.q] = [2, 3]; [o.p, o.q].join(',')",
        false,
    );
    // Defaults, nesting, and rest in assignments.
    check("let a; [a = 4] = []; a", false);
    check("let a, b; [a, [b]] = [1, [2]]; a + b", false);
    check("let a, r; [a, ...r] = [1, 2, 3]; a + r.length", false);
    check("let a, r; ({a, ...r} = {a: 1, b: 2}); a + r.b", false);
    // Assignment elisions leave all bindings untouched.
    check("let a; [, a] = [1, 2]; a", false);
    // Nullish object and array sources throw.
    check("let a; ({a} = null); 1", false);
    check("let a; [a] = null; a === undefined", false);
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
        false,
    );
    check(
        "let s = 0; for (let {x} of [{x: 1}, {x: 2}]) { s += x; } s",
        true,
    );
    check(
        "let s = 0; for (let [a = 9] of [[], [2]]) { s += a; } s",
        false,
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
fn direct_eval_closures_preserve_dynamic_class_bindings() {
    check(
        "{ let C = class { static field = () => eval('C'); }; C.field() === C ? 1 : 0; }",
        true,
    );
    check(
        "{ class C { static field = () => eval('C'); } const D = C; C = null; D.field() === D ? 1 : 0; }",
        true,
    );
}

#[test]
fn set_difference_uses_the_initial_receiver_snapshot() {
    let source = "let set; let seen = []; let other = { size: 100, has(value) { seen.push(value); if (seen.length === 1) { set.clear(); set.add(11); set.add(22); } return true; }, keys() { throw new Error('unexpected keys'); } }; set = new Set([1, 2, 3, 4]); let result = set.difference(other); if (seen.join(',') !== '1,2,3,4' || [...result].length !== 0 || [...set].join(',') !== '11,22') throw new Error('Set difference did not use the initial receiver snapshot'); 'pass';";
    check(source, true);
    let program = Interpreter::compile(source).expect("compile");
    let mut vm = Interpreter::with_builtins();
    assert!(matches!(vm.execute(&program), Ok(Value::String(ref value)) if value == "pass"));
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
    check(
        "let a=[]; for(let [x] of [[1],[2]]) { a.push(()=>x); } a[0]()+a[1]()",
        false,
    );
    for source in [
        "let x = [1]; for (let x of x) {}",
        "let x = {}; for (let x in x) {}",
        "let a=[]; for(let x of [1,2]) { a.push(()=>x); } a[0]()+a[1]()",
        "let a=[]; for(const x of [1,2]) { a.push(()=>x); } a[0]()+a[1]()",
        "let a=[]; for(let x in {a:1,b:2}) { a.push(()=>x); } a[0]()+a[1]()",
        "for(const x of [1]) { x=2; }",
        "let x=9; try { for(let x of [1]) { throw x; } } catch(e) {} x",
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
fn call_update_targets_throw_before_numeric_coercion_and_parentheses_keep_references() {
    for source in [
        "var n=0;function f(){n++;return {valueOf(){throw 'coerced'}}}try{f()++}catch(e){e instanceof ReferenceError&&n===1}",
        "var n=0;function f(){n++;throw 42}try{--f()}catch(e){e===42&&n===1}",
        "var x=1,o={x:2};var old=(x)++;var next=++(o.x);old===1&&x===2&&next===3&&o.x===3;",
        "var err='outer',during,after;try{throw 'exception'}catch(err){for(var err of ['key']){during=err}after=err}during==='key'&&after==='key'&&err==='outer';",
        "function test(){var err='outer',during;try{throw 'exception'}catch(err){{for(var err in {key:0}){during=err}}}return during==='key'&&err==='outer'}test();",
    ] {
        check(source, true);
    }
}

#[test]
fn empty_lexical_iteration_patterns_own_and_unwind_runtime_scopes() {
    for (source, expect_bytecode) in [
        ("var n=0;for(let [] of [[],[]]){n++;}n", false),
        ("var n=0;for(const {} of [{},{}]){n++;}n", true),
        ("var n=0;for(let [[]] of [[[]],[[]]]){n++;}n", false),
        (
            "try{for(let {} of [null]){}}catch(e){e instanceof TypeError;}",
            true,
        ),
        (
            "try{for(const {} of [undefined]){}}catch(e){e instanceof TypeError;}",
            true,
        ),
        (
            "var n=0;for(let [] of [[],[]]){if(++n===1)continue;break;}n",
            false,
        ),
        (
            "var n=0;for(const [] of [[],[]]){try{n++;}finally{}}n",
            false,
        ),
    ] {
        check(source, expect_bytecode);
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
fn plain_binding_reads_retain_environment_record_observability() {
    for (source, bytecode) in [
        (
            "var calls=0;Object.defineProperty(globalThis,'guarded',{get(){calls++;return 42;},configurable:true});guarded===42&&calls===1;",
            true,
        ),
        (
            "var proto=Object.create(Object.getPrototypeOf(globalThis));Object.defineProperty(proto,'inheritedGuarded',{get(){return 42;}});Object.setPrototypeOf(globalThis,proto);inheritedGuarded===42;",
            true,
        ),
        (
            "var guarded=42;var object={guarded:1};object[Symbol.unscopables]={guarded:true};var result;with(object){result=guarded;}result===42;",
            false,
        ),
        (
            "var guarded=42;var object={guarded:1};Object.defineProperty(object,Symbol.unscopables,{get(){delete object.guarded;return {};}});var result;with(object){result=guarded;}result===undefined&&guarded===42;",
            false,
        ),
        (
            "var guarded=42;var threw=false;try{{guarded;let guarded=1;}}catch(e){threw=e instanceof ReferenceError;}threw&&guarded===42;",
            true,
        ),
        (
            "var receiver={method:function(){'use strict';return this===receiver;}};var result;with(receiver){result=method();}result===true;",
            false,
        ),
    ] {
        check(source, bytecode);
        assert!(matches!(
            run_ast_with_modules(source, &[]),
            Ok(Value::Bool(true))
        ));
    }
}

#[test]
fn ordinary_array_numeric_reads_preserve_full_property_semantics() {
    for source in [
        "var a=[42];a[-0]===42&&a[0]===42;",
        "var a=[1];var calls=0;Object.defineProperty(a,'0',{get(){calls++;return this.tag;}});a.tag=42;a[0]===42&&calls===1;",
        "var proto=Object.create(Array.prototype);Object.defineProperty(proto,'0',{get(){return this.tag;}});var a=[];a.length=1;a.tag=42;Object.setPrototypeOf(a,proto);a[0]===42;",
        "var calls=0;var a=new Proxy([42],{get(t,k,r){calls++;return Reflect.get(t,k,r);}});a[0]===42&&calls===1;",
        "var a=[1];var key={toString(){a[0]=42;return '0';}};a[key]===42;",
        "var a=[];a.NaN=42;a[NaN]===42&&a[0]===undefined;",
    ] {
        check(source, true);
        assert!(matches!(
            run_ast_with_modules(source, &[]),
            Ok(Value::Bool(true))
        ));
    }
}

#[test]
fn completed_function_activations_release_argument_cycles() {
    let mut vm = Interpreter::with_builtins();
    let payload = vm
        .eval_source("var payload={child:42};payload.self=payload;payload;")
        .unwrap();
    let Value::Object { ref props } = payload else {
        panic!("object")
    };
    let weak = std::rc::Rc::downgrade(props);
    drop(payload);
    let result = vm.eval_source("function f(v){v=null;throw f.arguments[0].child===42;}var answer;try{f(payload);}catch(e){answer=e;}payload=null;answer;").unwrap();
    assert!(matches!(result, Value::Bool(true)));
    let stats = vm.collect_cycles();
    assert!(stats.skipped.is_none(), "{stats:?}");
    assert!(weak.upgrade().is_none());
}

#[test]
fn legacy_function_activation_properties_follow_calls_and_unwind() {
    for source in [
        "function child(){return child.caller;}function parent(){return eval('child()');}parent()===parent&&child.caller===null;",
        "function child(){return child.caller;}function parent(){'use strict';return child();}parent()===null;",
        "function child(){return child.caller;}var parent=()=>child();parent()===parent;",
        "function child(){return child.caller;}function parent(){return [0].map(child)[0];}parent()===null;",
        "function child(){return child.caller;}function parent(){return child.call(null)===parent&&child.apply(null,[])===parent&&Reflect.apply(child,null,[])===parent&&child.bind(null)()===parent;}parent();",
        "var seen;function child(){return child.caller;}function* parent(value=(seen=child())){}parent();seen===parent;",
        "function child(){return child.caller;}function* parent(){yield child();}parent().next().value===null;",
        "function child(){return child.caller;}function parent(){return Reflect.construct(child,[])===parent;}parent();",
        "function f(a){var first=f.arguments;a=9;return first!==f.arguments&&f.arguments[0]===1&&f.arguments[1]===2&&f.arguments.length===2&&f.arguments.callee===f;}f(1,2)&&f.arguments===null;",
        "function child(){return Object.getOwnPropertyDescriptor(child,'caller').get.call(child);}function parent(){return child();}var before=Object.getOwnPropertyDescriptor(child,'caller');parent()===parent&&before.get===Object.getOwnPropertyDescriptor(child,'caller').get;",
        "function f(n){if(n)return f(n-1);return f.caller===f;}f(2)&&f.caller===null;",
        "function f(){throw 1;}try{f();}catch(e){}f.arguments===null&&f.caller===null;",
        "var getter=Object.getOwnPropertyDescriptor({get x(){return 42;}},'x').get;function f(){}Object.defineProperty(f,'caller',{configurable:false});f.x=getter;var same=f.x===getter&&Object.getOwnPropertyDescriptor(f,'x').value===getter;f.x=7;same&&f.x===7;",
    ] {
        check(source, true);
        assert!(matches!(
            run_ast_with_modules(source, &[]),
            Ok(Value::Bool(true))
        ));
    }
}

#[test]
fn direct_eval_new_local_bindings_are_deletable_without_redeclaring_bare_vars() {
    for source in [
        "var read;function outer(){eval('var temporary=1;delete temporary;var temporary;read=function(){return temporary;};');}outer();var threw=false;try{read();}catch(e){threw=e instanceof ReferenceError;}threw;",
        "var read;function outer(){eval('delete temporary;read=function(){return temporary;};function temporary(){}');}outer();var threw=false;try{read();}catch(e){threw=e instanceof ReferenceError;}threw;",
        "function outer(){var existing=1;return eval('var existing;delete existing')===false&&existing===1;}outer();",
    ] {
        check(source, true);
        assert!(matches!(
            run_ast_with_modules(source, &[]),
            Ok(Value::Bool(true))
        ));
    }
}

#[test]
fn float16_arrays_share_binary_storage_and_round_without_double_rounding() {
    let source = "var a=new Float16Array([1.00048828125,1.0004882812500002,65520,-0,Math.pow(2,-24)]);var bits=new Uint16Array(a.buffer);var rounded=a[0]===1&&a[1]===1.0009765625&&a[2]===Infinity&&Object.is(a[3],-0)&&a[4]===Math.pow(2,-24);bits[0]=0xc000;var rejected=false;try{Atomics.add(a,0,1);}catch(e){rejected=e instanceof TypeError;}rounded&&a[0]===-2&&bits[3]===0x8000&&a.BYTES_PER_ELEMENT===2&&Float16Array.from([2]).map(x=>x/2)[0]===1&&rejected;";
    check(source, true);
    assert!(matches!(
        run_ast_with_modules(source, &[]),
        Ok(Value::Bool(true))
    ));
}

#[test]
fn regexp_allocation_precedes_string_coercion_and_source_round_trips() {
    for source in [
        "var log='';var pattern={get source(){log+='S';return {toString(){log+='s';return 'a';}};},get flags(){log+='F';return {toString(){log+='f';return 'g';}};}};Object.defineProperty(pattern,Symbol.match,{get:function(){log+='m';return true;}});var target=Object.defineProperty(function(){}.bind(null),'prototype',{get(){log+='p';return RegExp.prototype;}});var r=Reflect.construct(RegExp,[pattern],target);log==='mSFpsf'&&r.source==='a'&&r.global;",
        "var pattern=/a/;var calls=0;var target=Object.defineProperty(function(){}.bind(null),'prototype',{get(){calls++;pattern.compile('b');return RegExp.prototype;}});var r=Reflect.construct(RegExp,[pattern],target);r.source==='a'&&pattern.source==='b'&&calls===1;",
        r#"var r=new RegExp(String.fromCharCode(92,10));r.source==='\\n'&&new RegExp(r.source).test('\n')&&new RegExp('[/]').source==='[/]'&&new RegExp('[[]/').source==='[[]\\/';"#,
    ] {
        check(source, true);
        assert!(matches!(
            run_ast_with_modules(source, &[]),
            Ok(Value::Bool(true))
        ));
    }
}

#[test]
fn typed_array_prevent_extensions_reports_rejection_to_the_caller() {
    let source = "var buffer=new ArrayBuffer(4,{maxByteLength:8});var view=new Uint8Array(buffer);var reflected=Reflect.preventExtensions(view);var threw=false;try{Object.preventExtensions(view);}catch(e){threw=e instanceof TypeError;}reflected===false&&threw&&Object.isExtensible(view);";
    check(source, true);
    assert!(matches!(
        run_ast_with_modules(source, &[]),
        Ok(Value::Bool(true))
    ));
}

#[test]
fn class_heritage_and_computed_names_use_strict_property_writes() {
    for source in [
        "var caught=false;try{class C{[Object.preventExtensions({}).x=1](){}}}catch(e){caught=e instanceof TypeError;}caught;",
        "var caught=false;try{var C=class extends (Object.preventExtensions({}).x=1,Object){};}catch(e){caught=e instanceof TypeError;}caught;",
        "var caught=false;try{class C{[Object.freeze({x:1}).x++](){}}}catch(e){caught=e instanceof TypeError;}caught;",
        "var target=Object.preventExtensions({});var key={toString(){return 'x';}};var caught=false;try{class C{[target[key]=1](){}}}catch(e){caught=e instanceof TypeError;}caught;",
    ] {
        check(source, true);
        assert!(matches!(
            run_ast_with_modules(source, &[]),
            Ok(Value::Bool(true))
        ));
    }
}

#[test]
fn static_super_proxy_reads_preserve_the_original_receiver() {
    let source = "var seen=[];var key=Symbol();var base=new Proxy(function(){},{get(target,p,receiver){if(p==='x'||p===key){seen.push(receiver);return 42;}return Reflect.get(target,p,receiver);}});class A extends base{}class B extends A{static read(){return super.x+super[key];}}B.read()===84&&seen.length===2&&seen[0]===B&&seen[1]===B;";
    check(source, true);
    assert!(matches!(
        run_ast_with_modules(source, &[]),
        Ok(Value::Bool(true))
    ));
}

#[test]
fn super_references_capture_the_base_before_key_conversion() {
    for operation in [
        "super[key]",
        "super[key]()",
        "super[key]+=1",
        "++super[key]",
    ] {
        let callable = operation == "super[key]()";
        let old = if callable {
            "function(){return 1;}"
        } else {
            "1"
        };
        let expected = if operation.contains("+=") || operation.contains("++") {
            2
        } else {
            1
        };
        let source = format!(
            "var key={{toString(){{Object.setPrototypeOf(object,{{p:-1}});return 'p';}}}};var object={{__proto__:{{p:{old}}},method(){{return {operation};}}}};object.method()==={expected};"
        );
        check(&source, true);
        assert!(matches!(
            run_ast_with_modules(&source, &[]),
            Ok(Value::Bool(true))
        ));
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
    // Delete rejects super references before ToPropertyKey (ECMA-262 13.5.1.2).
    for source in [
        "class A{set x(v){this.value=v;}tag(parts,v){return this.value+v;}}class B extends A{assign(){[super.x]=[42];}tagged(){return super.tag`value${7}`;}}var b=new B();b.assign();b.value===42&&b.tagged()===49;",
        "var calls=0;var key={toString(){calls++;return 'x';}};class A{method(){return delete super[key];}}var caught;try{new A().method();}catch(e){caught=e;}calls===0&&caught instanceof ReferenceError;",
    ] {
        check(source, true);
        assert!(matches!(
            run_ast_with_modules(source, &[]),
            Ok(Value::Bool(true))
        ));
    }
}

#[test]
fn object_literal_prototypes_and_computed_keys_share_semantics() {
    for source in [
        "var parent={value:42};var o={__proto__:parent,method(){return super.value;}};Object.getPrototypeOf(o)===parent&&o.method()===42&&!Object.hasOwn(o,'__proto__');",
        "var o={'__proto__':null};Object.getPrototypeOf(o)===null&&!Object.hasOwn(o,'__proto__');",
        "var o={__proto__:7};Object.getPrototypeOf(o)===Object.prototype&&!Object.hasOwn(o,'__proto__');",
        "var parent={value:42};var o={__proto__:parent,['__proto__']:7};Object.getPrototypeOf(o)===parent&&Object.hasOwn(o,'__proto__')&&o.__proto__===7;",
        "var __proto__=42;var o={__proto__};Object.getPrototypeOf(o)===Object.prototype&&Object.hasOwn(o,'__proto__')&&o.__proto__===42;",
        "var log='';var key={toString(){log+='key';return 'x';}};var o={[key]:(log+='value',42)};log==='keyvalue'&&o.x===42;",
        "class A{[function(){}](){return 42;}}var a=new A();a[function(){}]()===42&&a[String(function(){})]()===42;",
        "var calls=0;var key={toString(){calls++;return 'x';}};var o={x:42};o[key]===42&&calls===1;",
        "var symbol=Symbol('x');var calls=0;var key={};key[Symbol.toPrimitive]=function(hint){calls++;return symbol;};var o={[key]:42};calls===1&&o[symbol]===42&&Reflect.ownKeys(o)[0]===symbol;",
        "var o={[null]:1,[true]:2,[1e21]:3};o.null===1&&o.true===2&&o['1e+21']===3;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "AST result for {source}"
        );
    }
}

#[test]
fn call_references_optional_receivers_and_argument_order_share_semantics() {
    for source in [
        "var log='';var object={value:'receiver',get method(){log+='get';return function(x){return this.value+x;};}};object.method(log+='argument')==='receivergetargument'&&log==='getargument';",
        "function old(){return 42;}function other(){return 7;}var f=old;f(f=other)===42&&f()===7;",
        "var calls=0;var object={get method(){throw 42;}};var caught;try{object.method(calls++);}catch(e){caught=e;}caught===42&&calls===0;",
        "var calls=0;var object=null;object?.method(calls++)===undefined&&calls===0;",
        "var calls=0;var f=null;f?.(calls++)===undefined&&calls===0;",
        "var object={value:42,method(){return this.value;}};object.method?.()===42;",
        "var calls=0;var object={method:null};object.method?.(calls++)===undefined&&calls===0;",
        "var object={value:42,method(){return this.value;}};object?.method?.()===42;",
        "var calls=0;var object=null;object?.method?.(calls++)===undefined&&calls===0;",
        "function C(){this.value=42;}function Other(){this.value=7;}var holder={C:C};new holder.C(holder.C=Other).value===42;",
        "var log='';function f(a,b){return a+b;}var source={};source[Symbol.iterator]=function(){log+='iterator';var count=0;return {next(){log+='next';return count++===0?{value:35,done:false}:{done:true};}};};f(...source,(log+='tail',7))===42&&log==='iteratornextnexttail';",
        "var log='';function C(a,b){this.value=a+b;}var source={};source[Symbol.iterator]=function(){log+='iterator';var count=0;return {next(){log+='next';return count++===0?{value:35,done:false}:{done:true};}};};new C(...source,(log+='tail',7)).value===42&&log==='iteratornextnexttail';",
        "var log='';var source={};source[Symbol.iterator]=function(){log+='iterator';var count=0;return {next(){log+='next';return count++===0?{value:35,done:false}:{done:true};}};};var array=[...source,(log+='tail',7)];array[0]+array[1]===42&&log==='iteratornextnexttail';",
        "var calls=0;var source={};source[Symbol.iterator]=function(){throw 42;};var caught;try{function f(){}f(...source,calls++);}catch(e){caught=e;}caught===42&&calls===0;",
        "var calls=0;var object={method:7};var caught;try{object.method(calls++);}catch(e){caught=e;}caught instanceof TypeError&&calls===1;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "AST result for {source}"
        );
    }
}

#[test]
fn null_receiver_checks_and_immutable_prototypes_share_internal_operations() {
    for source in [
        "var calls=0;var key={toString(){calls++;throw 42;}};var caught;try{null[key];}catch(e){caught=e;}caught instanceof TypeError&&calls===0;",
        "var prototype=Object.prototype;var other={__proto__:null};Reflect.setPrototypeOf(prototype,null)&&!Reflect.setPrototypeOf(prototype,other)&&Object.getPrototypeOf(prototype)===null;",
        "var caught;try{Object.setPrototypeOf(Object.prototype,{__proto__:null});}catch(e){caught=e;}caught instanceof TypeError;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "AST result for {source}"
        );
    }
}

#[test]
fn compound_members_convert_property_keys_once_in_both_tiers() {
    for operator in [
        "*=", "/=", "%=", "+=", "-=", "<<=", ">>=", ">>>=", "&=", "^=", "|=",
    ] {
        let source = format!(
            "var calls=0;var key={{toString(){{calls++;return 'x';}}}};var object={{x:8}};object[key]{operator}2;calls===1;"
        );
        check(&source, true);
        assert!(matches!(
            run_ast_with_modules(&source, &[]),
            Ok(Value::Bool(true))
        ));
    }
}

#[test]
fn abstract_iterator_and_intrinsic_accessors_share_execution_tiers() {
    for source in [
        "class Derived extends Iterator{}var object=new Derived();Object.getPrototypeOf(object)===Derived.prototype&&object instanceof Iterator;",
        "var errors=0;try{Iterator();}catch(e){if(e instanceof TypeError)errors++;}try{new Iterator();}catch(e){if(e instanceof TypeError)errors++;}errors===2;",
        "var d=Object.getOwnPropertyDescriptor(Iterator.prototype,'constructor');var object=Object.create(Iterator.prototype);d.set.call(object,42);object.constructor===42&&d.get.call()===Iterator;",
        "var d=Object.getOwnPropertyDescriptor(Iterator.prototype,Symbol.toStringTag);var object={};d.set.call(object,'custom');object[Symbol.toStringTag]==='custom'&&d.get.call()==='Iterator';",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn utf16_module_requests_reach_shared_loader_validation_in_both_tiers() {
    for source in [r"import '\uD800';", r"export * from '\uDC00';"] {
        check(source, true);
        let error = run_ast_with_modules(source, &[]).unwrap_err().to_string();
        assert!(
            error.contains("TypeError") && error.contains("UTF-8 host loader"),
            "{source}: {error}"
        );
    }
}

#[test]
fn json_stringification_uses_shared_semantics_in_both_execution_tiers() {
    for source in [
        r#"var input={a:1,b:2};JSON.stringify(input,function(k,v){return k==='b'?undefined:v;})==='{"a":1}';"#,
        r#"JSON.stringify({a:1,b:2},['b','a','b'])==='{"b":2,"a":1}';"#,
        r#"var p=new Proxy({a:1},{get(t,k){return k==='a'?42:Reflect.get(t,k);}});JSON.stringify(p)==='{"a":42}';"#,
        r#"var p=Proxy.revocable([],{});p.revoke();var ok=false;try{JSON.stringify({},p.proxy);}catch(e){ok=e instanceof TypeError;}ok;"#,
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn typed_array_static_inheritance_uses_the_same_native_helpers_in_both_tiers() {
    for source in [
        "var T=Object.getPrototypeOf(Int8Array);T.from===Uint8Array.from&&!Object.hasOwn(Int8Array,'from');",
        "class Derived extends Int8Array{}var value=Derived.from([1,2]);value instanceof Derived&&value[1]===2;",
        "var T=Object.getPrototypeOf(Int8Array);var value=T.of.call(Uint8Array,3,4);value instanceof Uint8Array&&value[0]===3;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn auto_accessors_share_storage_and_brand_semantics_in_both_tiers() {
    for source in [
        "class C{accessor x=1;}var a=new C(),b=new C();a.x=2;var d=Object.getOwnPropertyDescriptor(C.prototype,'x');a.x===2&&b.x===1&&!Object.hasOwn(a,'x')&&!d.enumerable&&d.configurable&&d.get.length===0&&d.set.length===1;",
        "var count=0;var key={toString(){count++;return 'x';}};class C{accessor [key]=3;}var a=new C();var b=new C();count===1&&a.x===3&&b.x===3;",
        "class C{accessor x=1;accessor x=2;}class D extends C{accessor x=3;}var c=new C(),d=new D();c.x===2&&d.x===3&&Object.getOwnPropertyDescriptor(C.prototype,'x').get.call(d)===2;",
        "class C{static accessor x=1;}class D extends C{}var errors=0;try{D.x;}catch(e){if(e instanceof TypeError)errors++;}try{D.x=2;}catch(e){if(e instanceof TypeError)errors++;}errors===2&&C.x===1;",
        "class C{accessor #x=3;get(){return this.#x;}set(v){this.#x=v;}}var c=new C();c.set(4);var errors=0;try{C.prototype.get.call({});}catch(e){if(e instanceof TypeError)errors++;}c.get()===4&&errors===1;",
        "class C{static accessor #x=3;static get(){return this.#x;}static set(v){this.#x=v;}}C.set(4);C.get()===4;",
        "class C{accessor x=function(){};static accessor y=class{};}new C().x.name==='x'&&C.y.name==='y';",
        "var symbol=Symbol('s');class C{accessor [symbol]=()=>0;}new C()[symbol].name==='[s]';",
        "class C{accessor x=1;get x(){return 2;}}new C().x===2;",
        r##"class C{accessor x;["#\0auto-accessor:0"]=function(){};}new C()["#\0auto-accessor:0"].name==="#\0auto-accessor:0";"##,
        r##"class C{static accessor x;static ["#\0auto-accessor:0"]=function(){};}C["#\0auto-accessor:0"].name==="#\0auto-accessor:0";"##,
        "var order='';class C{accessor x=(order+='a',1);y=(order+='b',this.x);accessor z=(order+='c',this.y);constructor(){order+='d';}}var c=new C();order==='abcd'&&c.z===1;",
        "class C{accessor x=this.y;accessor y=1;}var ok=false;try{new C();}catch(e){ok=e instanceof TypeError;}ok;",
        "class C{static accessor x=this.f();static f(){return 3;}}C.x===3;",
        "class C{accessor #x=function(){};get(){return this.#x.name;}}new C().get()==='#x';",
        "var s=Symbol('s');class C{static accessor [s]=function(){};}C[s].name==='[s]';",
        "class C{accessor x=1;}var get=Object.getOwnPropertyDescriptor(C.prototype,'x').get;var errors=0;try{get.call(new Proxy(new C(),{}));}catch(e){if(e instanceof TypeError)errors++;}try{new get();}catch(e){if(e instanceof TypeError)errors++;}errors===2;",
        "class C{accessor() {return 1;}accessor=2;}new C().accessor===2;",
        "class C{accessor\nx=3;}var c=new C();Object.hasOwn(c,'accessor')&&c.x===3;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn auto_accessor_modifiers_retain_class_early_errors() {
    for source in [
        "class C{accessor #x;accessor #x;}",
        "class C{accessor #x;get #x(){}}",
        "class C{accessor #x;static #x;}",
        "class C{accessor constructor;}",
        "class C{static accessor prototype;}",
        "class C{accessor x(){}}",
        "class C{accessor *x(){}}",
        "class C{accessor get x(){}}",
    ] {
        assert!(parse_cached(source).is_err(), "{source}");
    }
}

#[test]
fn class_decorators_preserve_evaluation_replacement_and_initializer_order() {
    for source in [
        "var log='';function dec(n){log+='e'+n;return function(C,c){log+='d'+n;c.addInitializer(function(){log+='i'+n;this.ready=true;});};}@dec('a') @dec('b') class C extends (log+='h',Object){static x=(log+='s',1);[log+='k'](){}}log==='eaebhkdbdasibia'&&C.ready&&C.x===1;",
        "var original;function dec(C,c){original=C;return class Replacement extends C{};}@dec class C{static x=3;static self(){return C;}}C!==original&&C.x===3&&!Object.hasOwn(original,'x')&&original.self()===C;",
        "var seen;var receiver={dec(C,c){seen=this;}};@receiver.dec class C{}seen===receiver;",
        "function d(){}var get=d,let=d,async=d;@get @let @async class C{}true;",
        "var C=function(value){return class extends value{m(){return 4;}};};var D=@C class C{};new D().m()===4;",
        "var seen;class Base{static dec(){seen=this;}}class Outer extends Base{static make(){return @(super.dec) class{};}}Outer.make();seen===Outer;",
        "var seen;var receiver={dec(C,c){seen=this;}};@(receiver.dec) class C{}seen===receiver;",
        "function dec(C,c){if(c.kind!=='class'||c.name!=='C'||'static' in c||'private' in c||'access' in c)throw 42;if(c.addInitializer.name!=='addInitializer'||c.addInitializer.length!==1)throw 43;}@dec class C{}true;",
        "var saved;function dec(C,c){saved=c.addInitializer;c.addInitializer.call({},function(){this.x=2;});}@dec class C{}var ok=false;try{saved(function(){});}catch(e){ok=e instanceof TypeError;}ok&&C.x===2;",
        "var saved;function dec(C,c){saved=c.addInitializer;throw 42;}var caught;try{var C=@dec class{};}catch(e){caught=e;}var ok=false;try{saved(function(){});}catch(e){ok=e instanceof TypeError;}ok&&caught===42;",
        "function dec(C,c){c.addInitializer(1);}var ok=false;try{var C=@dec class{};}catch(e){ok=e instanceof TypeError;}ok;",
        "function dec(){return null;}var ok=false;try{var C=@dec class{};}catch(e){ok=e instanceof TypeError;}ok;",
        "var replacement=()=>42;function dec(){return replacement;}var C=@dec class{};C===replacement&&C()===42;",
        "var seen;function dec(C,c){seen=c.name;}var C=@dec class{};seen==='C';",
        "class Base{static get x(){return this.tag;}}class Other{}function dec(){return class extends Other{static tag=9;};}@dec class C extends Base{static x=super.x;}C.x===9;",
        "function outer(){var dec=function(C){return class extends C{m(){return 3;}};};return @dec class{};}new (outer())().m()===3;",
        "class Outer{static #dec(C){return class extends C{m(){return 3;}};}static make(){return @Outer.#dec class{};}}new (Outer.make())().m()===3;",
        "var ok=false;function dec(C){try{C.self();}catch(e){ok=e instanceof ReferenceError;}}@dec class C{static self(){return C;}}ok;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn class_decorator_grammar_rejects_uncovered_expressions_and_exports() {
    for source in [
        "@obj['dec'] class C{}",
        "@d()() class C{}",
        "@d().x class C{}",
        "@1 class C{}",
        "@d function f(){}",
        "@d;class C{}",
        "@d export @d class C{}",
        "if(true) @d class C{}",
    ] {
        assert!(parse_cached(source).is_err(), "{source}");
    }
    check_with_modules(
        "function dec(C){return class extends C{m(){return 2;}};}@dec export default class C{}",
        true,
        &[],
    );
    check_with_modules(
        "function dec(C){return class extends C{m(){return 2;}};}export default @dec class C{}",
        true,
        &[],
    );
}

#[test]
fn method_decorators_preserve_replacement_access_and_initializer_order() {
    for source in [
        "function dec(f,c){return function wrapped(){return f.call(this)+1;};}class C{@dec m(){return 2;}}var c=new C();c.m()===3&&c.m.name==='wrapped';",
        "var access,original;var s=Symbol('m');function dec(f,c){if(c.kind!=='method'||c.name!==s||c.static||c.private)throw 42;access=c.access;original=f;return function(){return 3;};}class C{@dec [s](){return 2;}}var c=new C();access.has(c)&&access.get(c)===c[s]&&access.get(c)!==original&&c[s]()===3&&!('set' in access);",
        "var access;function dec(f,c){if(c.name!=='#m'||!c.private||c.static)throw 42;access=c.access;return function(){return 4;};}class C{@dec #m(){return 2;}get(){return this.#m();}}var c=new C(),bad=0;try{access.get({});}catch(e){if(e instanceof TypeError)bad++;}try{access.has(1);}catch(e){if(e instanceof TypeError)bad++;}c.get()===4&&access.has(c)&&!access.has({})&&!access.has(new Proxy(c,{}))&&access.get(c).call(c)===4&&bad===2;",
        "var access;function dec(f,c){access=c.access;c.addInitializer(function(){this.x=access.get(this)();});return function(){return 5;};}class C{@dec static #m(){return 1;}static y=this.x;}C.x===5&&C.y===5&&access.has(C);",
        "var log='';function dec(f,c){c.addInitializer(function(){log+='i';this.ready=true;});}class C{@dec m(){}x=(log+='f',this.ready);}var a=new C(),b=new C();log==='ifif'&&a.x&&b.x;",
        "var log='';function dec(f,c){c.addInitializer(function(){log+='i';});}class C{@dec m(){}x=(log+='b',1);}class D extends C{@dec n(){}y=(log+='d',1);}new D();log==='ibid';",
        "var log='';function cls(C){log+='c';return C;}function dec(f,c){c.addInitializer(function(){log+='i';});}@cls class C{@dec static m(){}static x=(log+='f',1);}log==='cif';",
        "var log='';function dec(n){log+='e'+n;return function(f,c){log+='d'+n;c.addInitializer(function(){log+='i'+n;});};}class C{@dec('a') [(log+='k','m')](){}@dec('b') static n(){}}new C();log==='eakebdbdaibia';",
        "var calls=0;function dec(f){calls++;return function(){return 9;};}class C{@dec m(){return 1;}m(){return 2;}}calls===1&&new C().m()===2;",
        "function dec(f){return function(){return 9;};}class C{@dec m(){return 1;}get m(){return 2;}}new C().m===2;",
        "function dec(f){return function(){return 9;};}class C{@dec m(){return 1;}accessor m=3;}new C().m===3;",
        "function dec(f){return function(){return 9;};}class C{accessor m=3;@dec m(){return 1;}}new C().m()===9;",
        "function dec(f){return new Proxy(f,{apply(t,r,a){return 6;}});}class C{@dec m(){return 1;}}new C().m()===6;",
        "var getter=Object.getOwnPropertyDescriptor({get m(){return 7;}},'m').get;function dec(){return getter;}class C{@dec m(){}get other(){return 1;}}var c=new C();c.m===getter&&c.m()===7&&Object.getOwnPropertyDescriptor(C.prototype,'m').value===getter;",
        "var late;function dec(f,c){late=c.addInitializer;}class C{@dec m(){}}var ok=false;try{late(function(){});}catch(e){ok=e instanceof TypeError;}ok;",
        "var late;function dec(f,c){late=c.addInitializer;throw 42;}var caught;try{var C=class{@dec m(){}};}catch(e){caught=e;}var ok=false;try{late(function(){});}catch(e){ok=e instanceof TypeError;}caught===42&&ok;",
        "function dec(){return 1;}var ok=false;try{var C=class{@dec m(){}};}catch(e){ok=e instanceof TypeError;}ok;",
        "function dec(f,c){c.addInitializer(1);}var ok=false;try{var C=class{@dec m(){}};}catch(e){ok=e instanceof TypeError;}ok;",
        "var C={dec(){}};var ok=false;try{var X=class C{@C.dec m(){}};}catch(e){ok=e instanceof ReferenceError;}ok;",
        "var count=0;function dec(f,c){c.addInitializer(function(){count++;});}class C{@dec m(){}make(){return class D{x=1;};}}var c=new C();var D=c.make();new D();count===1;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn method_decorator_early_errors_preserve_class_context() {
    for source in [
        "function d(){}class C{@d constructor(){}}",
        "function d(){}class C{@d static{}}",
        "var yield=function(){};class C{@yield m(){}}",
    ] {
        assert!(parse_cached(source).is_err(), "{source}");
    }
}

#[test]
fn explicit_data_descriptors_keep_getter_named_functions_as_data() {
    for source in [
        "var f=Object.getOwnPropertyDescriptor({get x(){throw 42;}},'x').get;var o={get other(){return 1;}};Object.defineProperty(o,'x',{value:f,writable:true,configurable:true});o.x===f&&Object.getOwnPropertyDescriptor(o,'x').value===f;",
        "var calls=0;var f=Object.getOwnPropertyDescriptor({set x(v){calls++;}},'x').set;var p={get other(){return 1;}};Object.defineProperty(p,'x',{value:f,writable:true,configurable:true});var o=Object.create(p);o.x=3;calls===0&&Object.hasOwn(o,'x')&&o.x===3;",
        "var s=Symbol('x');class Source{get [s](){return 9;}}var f=Object.getOwnPropertyDescriptor(Source.prototype,s).get;function dec(){return f;}class C{@dec [s](){}get other(){return 1;}}var c=new C();c[s]===f&&c[s]()===9&&Object.getOwnPropertyDescriptor(C.prototype,s).value===f;",
        "var f=Object.getOwnPropertyDescriptor({get x(){return 1;}},'x').get;var o={get other(){return 2;}};Object.defineProperty(o,'x',{value:f,configurable:true});delete o.x;Object.defineProperty(o,'x',{get(){return 3;},configurable:true});o.x===3;",
        "var f=Object.getOwnPropertyDescriptor({get x(){return 1;}},'x').get;var o={get other(){return 2;}};Object.defineProperty(o,'x',{value:f,writable:true,configurable:true});Object.freeze(o);o.x===f&&Object.getOwnPropertyDescriptor(o,'x').value===f&&!Object.getOwnPropertyDescriptor(o,'x').writable;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn field_decorators_transform_values_and_run_extras_after_storage() {
    for source in [
        "var log='';function d(n){log+='e'+n;return function(v,c){if(v!==undefined||c.kind!=='field')throw 42;log+='d'+n;c.addInitializer(function(){log+='x'+n;if(!c.access.has(this))throw 43;});return function(v){log+='i'+n;return n==='a'?v+1:v*2;};};}class C{@d('a') @d('b') x=(log+='r',3);y=(log+='y',this.x);}var c=new C();log==='eaebdbdariaibxbxay'&&c.x===8&&c.y===8;",
        "var access;function d(v,c){access=c.access;return function(v){return v+1;};}class C{@d #x=2;get(){return this.#x;}}var c=new C();access.set(c,5);var errors=0;try{access.get({});}catch(e){if(e instanceof TypeError)errors++;}try{access.set(new Proxy(c,{}),1);}catch(e){if(e instanceof TypeError)errors++;}c.get()===5&&access.has(c)&&!access.has({})&&!access.has(new Proxy(c,{}))&&errors===2;",
        "var access;function d(v,c){access=c.access;if(!c.private||!c.static||c.name!=='#x')throw 42;c.addInitializer(function(){this.y=access.get(this);});return function(v){return v+2;};}class C{@d static #x=3;static z=this.y;}access.get(C)===5&&C.y===5&&C.z===5;",
        "var symbol=Symbol('x'),calls=0;var key={toString(){calls++;return 'x';}};function d(v,c){return function(v){return v+1;};}class C{@d [key]=1;@d [symbol]=2;}var a=new C(),b=new C();calls===1&&a.x===2&&b.x===2&&a[symbol]===3;",
        "var log='';function d(n){return function(v,c){log+='d'+n;return function(v){log+='i'+n;return v+1;};};}class C{@d('a') x=1;@d('b') x=3;}var c=new C();log==='dadbiaib'&&c.x===4;",
        "function d(v,c){return function(v){return v===undefined?4:v;};}class C{a=1;@d x;b=2;accessor z=3;}new C().x===4;",
        "var access;var s=Symbol('x');function d(v,c){if(c.name!==s||c.private||c.static)throw 42;access=c.access;}class C{@d [s]=2;}var c=new C();access.set(c,5);access.get(c)===5&&access.has(c)&&!access.has({});",
        "var count=0;function d(v,c){c.addInitializer(function(){count++;});}class C{@d x=1;make(){return class D{x=2;};}}var c=new C();var D=c.make();new D();count===1;",
        "var log='';function d(v,c){c.addInitializer(function(){log+='x';});return function(v){log+='i';return v+1;};}class C{@d x=(log+='b',1);}class D extends C{@d y=(log+='d',2);}var o=new D();log==='bixdix'&&o.x===2&&o.y===3;",
        "var log='';function d(v,c){return function(v){return v+1;};}function cls(C){return class extends C{};}@cls class C{@d static x=2;static y=this.x;}C.x===3&&C.y===3&&!Object.hasOwn(Object.getPrototypeOf(C),'x');",
        "var name;function d(v,c){return function(v){name=v.name;return function(){};};}class C{@d x=function(){};}var c=new C();name==='x'&&c.x.name==='';",
        "var saved;function d(v,c){saved=c.addInitializer;throw 42;}var caught;try{var C=class{@d x;};}catch(e){caught=e;}var ok=false;try{saved(function(){});}catch(e){ok=e instanceof TypeError;}caught===42&&ok;",
        "function d(){return 1;}var ok=false;try{var C=class{@d x;};}catch(e){ok=e instanceof TypeError;}ok;",
        "var log='';function d(v,c){c.addInitializer(function(){log+='x';});return function(v){throw 42;};}class C{@d x=(log+='r',1);y=(log+='y',2);}var caught;try{new C();}catch(e){caught=e;}caught===42&&log==='r';",
        "var log='';function d(v,c){log+=c.static?'s':'i';}class C{@d a;@d static b;}log==='si';",
        "var saved,log='';function d(v,c){c.addInitializer(function(){saved=this;log+='x';throw 42;});return function(v){return v+1;};}class C{@d x=2;y=(log+='y',1);}var caught;try{new C();}catch(e){caught=e;}caught===42&&saved.x===3&&!Object.hasOwn(saved,'y')&&log==='x';",
        "var log='';function d(v,c){c.addInitializer(function(){log+='x';});return function(v){log+='i';return v;};}class C{@d x=(()=>{log+='r';throw 42;})();}var caught;try{new C();}catch(e){caught=e;}caught===42&&log==='r';",
        "var before,after;function d(v,c){c.addInitializer(function(){after=c.access.has(this);});return function(v){before=c.access.has(this);return v;};}class C{@d #x=1;}new C();before===false&&after===true;",
        "function make(d){class C{@(arguments[0]) x=1;}return new C().x;}make(function(){return function(v){return v+1;};})===2;",
        "var f=Object.getOwnPropertyDescriptor({get x(){return 7;}},'x').get;function d(v,c){c.addInitializer(function(){Object.defineProperty(this,'other',{get(){return 1;}});});return function(){return f;};}class C extends Array{@d x=1;}var c=new C();c.x===f&&c.x()===7&&Object.getOwnPropertyDescriptor(c,'x').value===f;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn element_decorator_context_creation_order_matches_its_visible_shape() {
    for source in [
        "var keys,accessKeys;function d(v,c){keys=Object.keys(c).join(',');accessKeys=Object.keys(c.access).join(',');}class C{@d x;}keys==='kind,access,static,private,name,addInitializer'&&accessKeys==='get,set,has';",
        "var keys,accessKeys;function d(v,c){keys=Object.keys(c).join(',');accessKeys=Object.keys(c.access).join(',');}class C{@d m(){}}keys==='kind,access,static,private,name,addInitializer'&&accessKeys==='get,has';",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn array_data_descriptor_tags_survive_writes_and_clear_on_deletion_or_truncation() {
    for source in [
        "var f=Object.getOwnPropertyDescriptor({get x(){return 7;}},'x').get;var a=[];Object.defineProperty(a,'x',{value:f,writable:true,configurable:true});Object.defineProperty(a,'other',{get(){return 1;}});var old=a.x;var desc=Object.getOwnPropertyDescriptor(a,'x');a.x=4;old===f&&desc.value===f&&a.x===4;",
        "var f=Object.getOwnPropertyDescriptor({get x(){return 7;}},'x').get;var a=[];Object.defineProperty(a,'x',{value:f,writable:true,configurable:true});delete a.x;Object.defineProperty(a,'x',{get(){return 3;},configurable:true});a.x===3;",
        "class Source{get 0(){return 7;}}var f=Object.getOwnPropertyDescriptor(Source.prototype,'0').get;var a=[];Object.defineProperty(a,'0',{value:f,writable:true,configurable:true});var popped=a.pop();Object.defineProperty(a,'0',{get(){return 4;},configurable:true});popped===f&&a.length===1&&a[0]===4;",
        "class Source{get 0(){return 7;}}var f=Object.getOwnPropertyDescriptor(Source.prototype,'0').get;var a=[];Object.defineProperty(a,'0',{value:f,writable:true,configurable:true});var before=a['0'];a.length=0;Object.defineProperty(a,'0',{get(){return 4;},configurable:true});before===f&&a[0]===4;",
        "var s=Symbol('x');class Source{get [s](){return 7;}}var f=Object.getOwnPropertyDescriptor(Source.prototype,s).get;var a=[];Object.defineProperty(a,s,{value:f,writable:true,configurable:true});a[s]===f&&a[s]()===7&&Object.getOwnPropertyDescriptor(a,s).value===f;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn explicit_accessors_preserve_arbitrary_callable_identity_and_receiver() {
    for source in [
        "function read(){return this.y;}function write(v){this.y=v;}var pg=new Proxy(read,{}),ps=new Proxy(write,{});var p={};Object.defineProperty(p,'x',{get:pg,set:ps,configurable:true});var o=Object.create(p);o.x=4;var d=Object.getOwnPropertyDescriptor(p,'x');o.x===4&&d.get===pg&&d.set===ps&&read.name==='read'&&write.name==='write'&&Reflect.get(p,'x',{y:5})===5;",
        "function read(){return 3;}var o={};Object.defineProperty(o,'x',{get:read,configurable:true});o.x===3&&read.name==='read'&&Object.getOwnPropertyDescriptor(o,'x').get===read;",
        "class C{}var o={};Object.defineProperty(o,'x',{get:C,configurable:true});var ok=false;try{o.x;}catch(e){ok=e instanceof TypeError;}ok&&Object.getOwnPropertyDescriptor(o,'x').get===C;",
        "var o={};Object.defineProperty(o,'x',{get:String,configurable:true});o.x===''&&Object.getOwnPropertyDescriptor(o,'x').get===String;",
        "var key=Symbol('x');function read(){return this.y;}function write(v){this.y=v;}var o=[];var g=new Proxy(read,{}),s=new Proxy(write,{});Object.defineProperty(o,key,{get:g,set:s,configurable:true});o[key]=6;var d=Object.getOwnPropertyDescriptor(o,key);o[key]===6&&d.get===g&&d.set===s;",
        "var o=[];var g=new Proxy(function read(){return 7;},{});Object.defineProperty(o,'0',{get:g,configurable:true});var before=o[0];o.length=0;o[0]=4;var d=Object.getOwnPropertyDescriptor(o,'0');before===7&&o[0]===4&&d.value===4&&d.writable&&d.configurable&&d.enumerable;",
        "var g=new Proxy(function read(){return 7;},{});var o=[];Object.defineProperty(o,'0',{get:g,configurable:true});delete o[0];o[0]=4;var d=Object.getOwnPropertyDescriptor(o,'0');o[0]===4&&d.value===4&&d.writable;",
        "var o={};var g=new Proxy(function read(){return 7;},{});Object.defineProperty(o,'x',{get:g});Object.defineProperty(o,'x',{get:g});o.x===7&&Object.getOwnPropertyDescriptor(o,'x').get===g;",
        "var y=0;var o=[];var s=new Proxy(function write(v){y=v;},{});Object.defineProperty(o,'0',{set:s,configurable:true});o[0]=4;o[0]===undefined&&y===4&&Object.getOwnPropertyDescriptor(o,'0').set===s;",
        "var g=Proxy.revocable(function read(){return 7;},{});var o={};Object.defineProperty(o,'x',{get:g.proxy});g.revoke();var ok=false;try{o.x;}catch(e){ok=e instanceof TypeError;}ok&&Object.getOwnPropertyDescriptor(o,'x').get===g.proxy;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn getter_setter_decorators_preserve_pairs_private_brands_and_initializer_order() {
    for source in [
        "var g,s;function dg(f,c){if(c.kind!=='getter'||'set' in c.access)throw 42;g=function read(){return f.call(this)+1;};return g;}function ds(f,c){if(c.kind!=='setter'||'get' in c.access)throw 42;s=function write(v){f.call(this,v*2);};return s;}class C{v=1;@dg get x(){return this.v;}@ds set x(v){this.v=v;}}var c=new C();c.x=3;var d=Object.getOwnPropertyDescriptor(C.prototype,'x');c.x===7&&d.get===g&&d.set===s&&g.name==='read'&&s.name==='write';",
        "function dg(f){return new Proxy(f,{apply(t,r,a){return Reflect.apply(t,r,a)+2;}});}function ds(f){return new Proxy(f,{apply(t,r,a){return Reflect.apply(t,r,[a[0]+1]);}});}class C{v=1;@dg get x(){return this.v;}@ds set x(v){this.v=v;}}var c=new C();c.x=3;c.x===6;",
        "var getAccess,setAccess;function dg(f,c){if(!c.private||c.name!=='#x')throw 42;getAccess=c.access;return function(){return f.call(this)+1;};}function ds(f,c){setAccess=c.access;return function(v){f.call(this,v+2);};}class C{#v=1;@dg get #x(){return this.#v;}@ds set #x(v){this.#v=v;}}var c=new C();setAccess.set(c,3);var bad=0;try{getAccess.get({});}catch(e){if(e instanceof TypeError)bad++;}getAccess.get(c)===6&&getAccess.has(c)&&!getAccess.has(new Proxy(c,{}))&&bad===1;",
        "function dg(f){return function(){return f.call(this)+1;};}class C{#v=2;@dg get #x(){return this.#v;}set #x(v){this.#v=v;}use(){this.#x=4;return this.#x;}}new C().use()===5;",
        "function ds(f){return function(v){f.call(this,v+1);};}class C{#v=2;get #x(){return this.#v;}@ds set #x(v){this.#v=v;}use(){this.#x=4;return this.#x;}}new C().use()===5;",
        "var ga,sa;function dg(f,c){if(!c.static||!c.private)throw 42;ga=c.access;return function(){return f.call(this)+1;};}function ds(f,c){sa=c.access;return function(v){f.call(this,v+2);};}class C{static #v=1;@dg static get #x(){return this.#v;}@ds static set #x(v){this.#v=v;}}sa.set(C,3);ga.get(C)===6&&ga.has(C);",
        "var log='';function d(f,c){log+='d'+c.kind;c.addInitializer(function(){log+='i'+c.kind;});}class C{@d get x(){return 1;}@d set x(v){}y=(log+='f',2);}new C();log==='dgetterdsetterigetterisetterf';",
        "var log='';function cls(C){log+='c';}function d(f,c){c.addInitializer(function(){log+='i';});}@cls class C{@d static get x(){return 1;}static y=(log+='f',2);}log==='cif';",
        "var symbol=Symbol('x'),access;function d(f,c){if(c.name!==symbol)throw 42;access=c.access;return function(){return 4;};}class C{@d get [symbol](){return 1;}}var c=new C();access.has(c)&&access.get(c)===4&&c[symbol]===4;",
        "function d(f){return function(){return 4;};}class C{@d get x(){return 1;}x(){return 2;}}new C().x()===2;",
        "function d(f){return function(){return 4;};}class C{x(){return 2;}@d get x(){return 1;}}new C().x===4;",
        "function d(f){return function(){return 4;};}class C{@d get x(){return 1;}accessor x=3;}new C().x===3;",
        "var saved;function d(f,c){saved=c.addInitializer;}class C{@d get x(){return 1;}}var ok=false;try{saved(function(){});}catch(e){ok=e instanceof TypeError;}ok;",
        "function d(){return 1;}var ok=false;try{var C=class{@d get x(){return 1;}};}catch(e){ok=e instanceof TypeError;}ok;",
        "function d(){return undefined;}class C{@d get x(){return 1;}}var c=new C();!Reflect.set(c,'x',2)&&c.x===1;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn inherited_explicit_accessors_keep_exotic_receivers_and_shadowing() {
    for source in [
        "let get=new Proxy(function read(){return this.marker},{}); for(let object of [[],function(){},(function(){}).bind(null),new Date(),new Number(1),/x/,new Map()]){let proto=Object.getPrototypeOf(object); Object.defineProperty(proto,'probe',{get,configurable:true});object.marker=42;if(object.probe!==42)throw Error('receiver');delete proto.probe;} true;",
        "let object=[];let key=Symbol();let proto=Object.create(Array.prototype);Object.setPrototypeOf(object,proto);Object.defineProperty(proto,key,{get:new Proxy(function(){return this.marker},{}),configurable:true});object.marker=42;object[key]===42;",
        "let proto=Object.create(Function.prototype);Object.defineProperty(proto,'probe',{set:function(v){this.marker=v},configurable:true});let f=function(){};Object.setPrototypeOf(f,proto);f.probe=42;f.marker===42&&f.probe===undefined;",
        "let base={};Object.defineProperty(base,'probe',{get:function(){return 7},configurable:true});let middle=Object.create(base);Object.defineProperty(middle,'probe',{value:19});let object=[];Object.setPrototypeOf(object,middle);object.probe===19;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn auto_accessor_decorators_apply_pairs_transforms_and_extra_initializers() {
    for source in [
        "var log='';function a(v,c){log+='a';c.addInitializer(function(){log+='A';});return {init(x){log+='i';return x+1;},get(){return v.get.call(this)+10;},set(x){v.set.call(this,x+2);}};}function b(v,c){log+='b';c.addInitializer(function(){log+='B';});return {init(x){log+='j';return x*2;}};}class C{@a @b accessor x=(log+='f',3);y=(log+='y',0);}var c=new C();var before=c.x;c.x=4;log==='bafijBAy'&&before===18&&c.x===16;",
        "var access;function d(v,c){if(c.kind!=='accessor'||!c.private||c.name!=='#x')throw 1;access=c.access;return {get(){return v.get.call(this)+1;},set(n){v.set.call(this,n+2);},init(n){return n*2;}};}class C{@d accessor #x=3;}var c=new C();var before=access.get(c);access.set(c,4);var bad=false;try{access.get({});}catch(e){bad=e instanceof TypeError;}before===7&&access.get(c)===7&&access.has(c)&&!access.has({})&&bad;",
        "var log='';function cls(C){log+='c';}function d(v,c){log+='d';c.addInitializer(function(){log+='e';});return {init(n){log+='i';return n+1;}};}@cls class C{@d static accessor x=(log+='f',4);static y=(log+='y',0);}C.x===5&&log==='dcfiey';",
        "var s=Symbol('x'),seen;function d(v,c){seen=c;return {get:new Proxy(v.get,{}),set:new Proxy(v.set,{})};}class C{@d accessor [s]=7;}var c=new C();c[s]=8;seen.name===s&&seen.access.get(c)===8&&seen.access.has(c);",
        "var log='';function d(v,c){return {get get(){log+='g';return undefined;},get set(){log+='s';return undefined;},get init(){log+='i';return undefined;}};}class C{@d accessor x=3;}new C().x===3&&log==='gsi';",
        "var n=0;for(var result of [null,1,{get:1},{set:null},{init:false}]){try{let r=result;class C{@(function(){return r;}) accessor x=1;}}catch(e){if(e instanceof TypeError)n++;}}n===5;",
        "var saved;function d(v,c){saved=c.addInitializer;}class C{@d accessor x=3;}var ok=false;try{saved(function(){});}catch(e){ok=e instanceof TypeError;}ok;",
        "var log='';function m(v,c){log+='m';}function a(v,c){log+='a';}function f(v,c){log+='f';}class C{@f x=1;@a accessor y=2;@m m(){}}log==='amf';",
        "function d(v,c){return {get(){return 9;}};}class C{@d accessor x=1;x(){return 2;}}new C().x()===2;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn template_interpolation_converts_in_order_and_rejects_symbols() {
    for source in [
        "let sym=Symbol('x');let ok=false;try{`${sym}`;}catch(e){ok=e instanceof TypeError;}ok;",
        "let log='';let a={};a[Symbol.toPrimitive]=function(hint){log+='a'+hint;return 'A';};let b=()=>{log+='b';return 'B';};let s=`${a}${b()}`;s==='AB'&&log==='astringb';",
        "let log='';let a={};a[Symbol.toPrimitive]=function(){log+='a';return Symbol();};try{`${a}${(log+='b')}`;}catch(e){if(!(e instanceof TypeError))throw e;}log==='a';",
        "let sym=Symbol('ok');delete Symbol.prototype[Symbol.toPrimitive];`${Object(sym)}`==='Symbol(ok)';",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn array_from_uses_full_to_length_before_constructing() {
    for source in [
        "let ok=false;try{Array.from({length:Infinity});}catch(e){ok=e instanceof RangeError;}ok;",
        "let got;function C(n){got=n;throw 42;}let ok=false;try{Array.from.call(C,{length:Infinity});}catch(e){ok=e===42;}ok&&got===Number.MAX_SAFE_INTEGER;",
        "let got;function C(n){got=n;}let x=Array.from.call(C,{length:3,0:'a',2:'c'});got===3&&x.length===3&&x[0]==='a'&&x[1]===undefined&&x[2]==='c';",
        "let called=false;function C(n){called=true;throw 42;}let ok=false;try{Array.from.call(C,{get length(){throw 13;}});}catch(e){ok=e===13;}ok&&!called;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn date_default_conversion_honors_removed_or_replaced_to_primitive() {
    for source in [
        "let d=new Date(0);let a=0+d;delete Date.prototype[Symbol.toPrimitive];let b=0+d;typeof a==='string'&&b===0;",
        "let d=new Date(5);Object.defineProperty(Date.prototype,Symbol.toPrimitive,{value:undefined,configurable:true});0+d===5;",
        "let d=new Date(5);Object.defineProperty(Date.prototype,Symbol.toPrimitive,{value:function(hint){return hint;},configurable:true});0+d==='0default';",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn buffer_byte_limit_is_independent_of_dense_array_capacity() {
    for source in [
        "let b=new ArrayBuffer(20*1024*1024);let v=new DataView(b);v.setUint32(b.byteLength-4,0x12345678);b.byteLength===20*1024*1024&&v.getUint32(b.byteLength-4)===0x12345678;",
        "let b=new ArrayBuffer(20*1024*1024,{maxByteLength:24*1024*1024});b.resize(21*1024*1024);b.byteLength===21*1024*1024&&b.maxByteLength===24*1024*1024;",
        "let b=new SharedArrayBuffer(20*1024*1024);new DataView(b).byteLength===20*1024*1024;",
        "let ok=0;try{new ArrayBuffer(64*1024*1024+1);}catch(e){if(e instanceof RangeError)ok++;}try{new SharedArrayBuffer(64*1024*1024+1);}catch(e){if(e instanceof RangeError)ok++;}ok===2;",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn typed_array_numeric_reads_do_not_fall_through_to_prototype() {
    for source in [
        "let b=new ArrayBuffer(16,{maxByteLength:40});let a=new Int8Array(b,0,4);a.__proto__={'2':'wrong','-1':'wrong','NaN':'wrong'};a[2]=10;let before=a[2];b.resize(0);before===10&&a[2]===undefined&&a[-1]===undefined&&a.NaN===undefined&&!(2 in a);",
        "let a=new Uint8Array(1);a.__proto__={'1':'wrong','1.5':'wrong','-0':'wrong','01':'named'};a[1]===undefined&&a['1.5']===undefined&&a['-0']===undefined&&a['01']==='named';",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}

#[test]
fn date_time_format_validates_hour_cycle_before_use() {
    for source in [
        "let ok=0;for(let x of ['h24\0cookie','invalid','H12']){try{new Intl.DateTimeFormat('en',{hourCycle:x});}catch(e){if(e instanceof RangeError)ok++;}}ok===3;",
        "let log='';let o={get hour12(){log+='a';return true;},get hourCycle(){log+='b';return 'invalid';},get timeZone(){log+='c';return 'UTC';}};try{new Intl.DateTimeFormat('en',o);}catch(e){if(!(e instanceof RangeError))throw e;}log==='ab';",
        "let d=new Intl.DateTimeFormat('en',{hour:'numeric',hourCycle:'h24'});d.resolvedOptions().hourCycle==='h24';",
    ] {
        check(source, true);
        assert!(
            matches!(run_ast_with_modules(source, &[]), Ok(Value::Bool(true))),
            "{source}"
        );
    }
}
