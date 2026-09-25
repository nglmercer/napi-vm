//! Criterion microbenchmarks for the VM, driving the full pipeline directly
//! (lexer → parser → interpreter) with no NAPI overhead. Run with `cargo bench`.
//!
//! Groups measured:
//! - `run`: end-to-end execution of representative JavaScript workloads.
//! - `frontend`: the lexer and parser in isolation over a large source, to show
//!   how much of the pipeline is parsing versus evaluation.
//! - `warm`: steady-state invocation on a reused VM.
//! - `plugin_call`: host-to-plugin call paths.
//! - `tiers`: the Phase H–J runtime machinery at steady state — inline-cache
//!   hits, megamorphic sites, call/tier-up counting, shape churn, and cycle
//!   collection — via compile-once/execute-many on a reused interpreter.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use napi_vm::{
    Interpreter, Lexer, Parser, RustPluginHost, RustPluginHostOptions, Statement, setup_builtins,
};

/// Lex + parse a source string into statements.
fn parse(src: &str) -> Vec<Statement> {
    let mut lex = Lexer::new(src);
    let toks = lex.tokenize();
    let mut parser = Parser::new(toks);
    parser.parse()
}

/// Evaluate pre-parsed statements on a fresh interpreter with builtins loaded.
fn run_stmts(stmts: &[Statement]) {
    let mut interp = Interpreter::new();
    setup_builtins(&interp.global);
    let _ = interp.run(stmts);
}

/// Full pipeline: parse then evaluate.
fn run(src: &str) {
    let stmts = parse(src);
    run_stmts(&stmts);
}

/// Representative workloads. Each is a self-contained program whose final
/// expression produces a value; sizes are tuned so a single run lands in the
/// tens-of-microseconds to low-milliseconds range that Criterion measures well.
const WORKLOADS: &[(&str, &str)] = &[
    (
        "arithmetic_loop",
        "let s = 0; for (let i = 0; i < 10000; i++) { s += i * 2 - 1; } s;",
    ),
    (
        "recursion_fib",
        // Keep the recursive benchmark below the debug harness's native stack
        // budget. The interpreter's crash-safety suite exercises the deeper
        // recursion boundary separately with subprocess isolation.
        "function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } fib(10);",
    ),
    (
        "array_chain",
        "let a = []; for (let i = 0; i < 1000; i++) { a.push(i); } \
         a.map(x => x * 2).filter(x => x % 3 === 0).reduce((s, x) => s + x, 0);",
    ),
    (
        "string_ops",
        "let parts = []; for (let i = 0; i < 1000; i++) { parts.push('item' + i); } \
         parts.join(',').split(',').length;",
    ),
    (
        "closures",
        "function counter() { let n = 0; return () => ++n; } \
         const c = counter(); for (let i = 0; i < 10000; i++) { c(); } c();",
    ),
    (
        "json_roundtrip",
        "const o = { a: 1, b: [1, 2, 3], c: { d: 'x', e: [true, null] } }; \
         let r; for (let i = 0; i < 200; i++) { r = JSON.parse(JSON.stringify(o)); } \
         r.c.e.length + r.b.length;",
    ),
];

fn bench_run(c: &mut Criterion) {
    let mut group = c.benchmark_group("run");
    for (name, src) in WORKLOADS {
        group.bench_with_input(BenchmarkId::from_parameter(name), src, |b, src| {
            b.iter(|| run(black_box(src)));
        });
    }
    group.finish();
}

fn bench_frontend(c: &mut Criterion) {
    // A sizeable program so lex/parse costs are well above noise.
    let big_src = "function f(x) { return x * 2 + 1; }\nconst v = f(10);\n".repeat(2000);

    let mut group = c.benchmark_group("frontend");
    group.bench_function("lex_big_source", |b| {
        b.iter(|| {
            let mut lex = Lexer::new(black_box(&big_src));
            black_box(lex.tokenize());
        });
    });
    group.bench_function("parse_big_source", |b| {
        b.iter(|| black_box(parse(black_box(&big_src))));
    });
    group.finish();
}

/// Reuse a prepared VM for class calls. Defining a fresh class in each
/// Criterion iteration creates the constructor/prototype cycle that napi-vm's
/// ref-counted heap intentionally cannot collect; benchmarking that cold
/// source repeatedly grows memory instead of measuring execution throughput.
fn bench_class_methods(c: &mut Criterion) {
    let setup = parse(
        "class P { constructor(x, y) { this.x = x; this.y = y; } \
         sum() { return this.x + this.y; } } \
         function class_methods() { let t = 0; \
         for (let i = 0; i < 1000; i++) { t += new P(i, i + 1).sum(); } \
         return t; }",
    );
    let invocation = parse("class_methods();");
    let mut interp = Interpreter::new();
    setup_builtins(&interp.global);
    interp
        .run_program_body(&setup)
        .expect("class benchmark setup should execute");

    let mut group = c.benchmark_group("warm");
    group.bench_function("class_methods_reused_vm", |b| {
        b.iter(|| {
            interp.begin_execution();
            black_box(
                interp
                    .run(black_box(&invocation))
                    .expect("class benchmark invocation should execute"),
            )
        });
    });
    group.finish();
}

/// Phase A comparison: the legacy per-call wrapper (generate JS source,
/// eval/parse it, `JSON.stringify` the result, re-parse the envelope)
/// against the direct `call_json` path (convert values, invoke the cached
/// guest function, convert back). Same fixture plugin, same payloads.
fn bench_plugin_call(c: &mut Criterion) {
    const GUEST: &str = r#"
export default {
  call(request, context) {
    return {
      echo: request,
      plugin: context.name,
      total: request.items.reduce((sum, item) => sum + item.n, 0),
    };
  },
};
"#;
    let root = std::env::temp_dir().join(format!("napi-vm-bench-plugin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("main.mjs"), GUEST).unwrap();
    std::fs::write(
        root.join("plugin.json"),
        r#"{"name":"bench-plugin","version":"1.0.0","apiVersion":1,"entry":"main.mjs","permissions":{}}"#,
    )
    .unwrap();

    let request = serde_json::json!({
        "type": "poll",
        "items": (0..32).map(|n| serde_json::json!({"n": n})).collect::<Vec<_>>(),
    });
    let context = serde_json::json!({"name": "bench-plugin", "version": "1.0.0"});
    let request_text = serde_json::to_string(&request).unwrap();
    let context_text = serde_json::to_string(&context).unwrap();

    let mut group = c.benchmark_group("plugin_call");
    group.bench_function("legacy_wrapper_eval", |b| {
        let mut host = RustPluginHost::new(RustPluginHostOptions::default());
        host.load(&root).unwrap();
        let plugin = host.get_mut("bench-plugin").unwrap();
        b.iter(|| {
            let source = format!(
                r#"await (async () => {{
  const request = {request_text};
  const context = {context_text};
  const value = await __pluginInstance.call(request, context);
  return JSON.stringify({{ serialized: JSON.stringify(value) }});
}})()"#
            );
            let value = plugin
                .interpreter_mut()
                .eval_source(black_box(&source))
                .unwrap();
            let napi_vm::Value::String(envelope) = &value else {
                panic!("envelope must be a string");
            };
            let envelope: serde_json::Value = serde_json::from_str(envelope).unwrap();
            black_box(envelope.get("serialized").unwrap().clone())
        });
    });
    group.bench_function("direct_call_json", |b| {
        let mut host = RustPluginHost::new(RustPluginHostOptions::default());
        host.load(&root).unwrap();
        let plugin = host.get_mut("bench-plugin").unwrap();
        b.iter(|| {
            black_box(
                plugin
                    .call_json(black_box(&request), black_box(&context))
                    .unwrap(),
            )
        });
    });
    group.finish();
    let _ = std::fs::remove_dir_all(&root);
}

/// Phase H–J steady-state suite. Each case compiles a setup program once
/// (fixtures hoisted to top-level globals, work in `bench()`), executes it
/// once, then invokes `bench()` per Criterion iteration on the same
/// interpreter. Setup asserts the tier under test: `bench` must be
/// bytecode-backed, so the suite can never silently measure the AST.
fn bench_tiers(c: &mut Criterion) {
    const CASES: &[(&str, &str)] = &[
        (
            "prop_mono_hot",
            "const O = {x: 1}; function bench() { let s = 0; \
             for (let i = 0; i < 2000; i++) { s += O.x; } return s; }",
        ),
        (
            "prop_mega_hot",
            "const OBJS = [{x:0},{x:1,a:1},{x:2,a:2,b:2},{x:3,a:3,b:3,c:3},\
             {x:4,a:4,b:4,c:4,d:4},{x:5,a:5,b:5,c:5,d:5,e:5},\
             {x:6,a:6,b:6,c:6,d:6,e:6,f:6},{x:7,a:7,b:7,c:7,d:7,e:7,f:7,g:7},\
             {x:8,a:8,b:8,c:8,d:8,e:8,f:8,g:8,h:8},\
             {x:9,a:9,b:9,c:9,d:9,e:9,f:9,g:9,h:9,i:9},\
             {x:10,a:10,b:10,c:10,d:10,e:10,f:10,g:10,h:10,i:10,j:10},\
             {x:11,a:11,b:11,c:11,d:11,e:11,f:11,g:11,h:11,i:11,j:11,k:11}]; \
             function bench() { let s = 0; \
             for (let i = 0; i < 2400; i++) { s += OBJS[i % 12].x; } return s; }",
        ),
        (
            "call_tiny_hot",
            "function add(a, b) { return a + b; } \
             function bench() { let s = 0; \
             for (let i = 0; i < 2000; i++) { s = add(s, i); } return s; }",
        ),
        (
            "shape_churn",
            "function bench() { let s = 0; \
             for (let i = 0; i < 300; i++) { let o = {a: i}; o['k' + i] = i; s += o.a; } \
             return s; }",
        ),
    ];
    let invoke = napi_vm::Interpreter::compile("bench();").unwrap();
    assert!(invoke.stats().is_some(), "invocation must be bytecode");

    let mut group = c.benchmark_group("tiers");
    for (name, setup_src) in CASES {
        let setup = napi_vm::Interpreter::compile(setup_src).unwrap();
        assert!(setup.stats().is_some(), "{name} setup must be bytecode");
        let mut interp = napi_vm::Interpreter::with_builtins();
        interp.execute(&setup).unwrap();
        let defined = interp.global.borrow().get("bench").expect("bench defined");
        assert!(
            matches!(&defined, napi_vm::Value::Function(f) if f.bytecode.is_some()),
            "{name} must be bytecode-backed"
        );
        // Warm the caches once so iterations measure steady state.
        interp.execute(&invoke).unwrap();
        group.bench_function(*name, |b| {
            b.iter(|| black_box(interp.execute(black_box(&invoke)).unwrap()));
        });
    }

    // Cycle collection over a builtin-rooted heap plus fresh garbage: each
    // iteration orphans 200 two-object cycles, then reclaims them.
    let garbage = napi_vm::Interpreter::compile(
        "function bench() { let arr = []; \
         for (let i = 0; i < 200; i++) { let a = {}; let b = {}; a.peer = b; b.peer = a; \
         arr.push(a); } return arr.length; }",
    )
    .unwrap();
    let mut interp = napi_vm::Interpreter::with_builtins();
    interp.execute(&garbage).unwrap();
    group.bench_function("gc_collect", |b| {
        b.iter(|| {
            black_box(interp.execute(black_box(&invoke)).unwrap());
            black_box(interp.collect_cycles())
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_run,
    bench_frontend,
    bench_class_methods,
    bench_plugin_call,
    bench_tiers
);
criterion_main!(benches);
