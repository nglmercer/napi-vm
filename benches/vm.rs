//! Criterion microbenchmarks for the VM, driving the full pipeline directly
//! (lexer → parser → interpreter) with no NAPI overhead. Run with `cargo bench`.
//!
//! Two groups are measured:
//! - `run`: end-to-end execution of representative JavaScript workloads.
//! - `frontend`: the lexer and parser in isolation over a large source, to show
//!   how much of the pipeline is parsing versus evaluation.

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use napi_vm::{Interpreter, Lexer, Parser, Statement, setup_builtins};

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

criterion_group!(benches, bench_run, bench_frontend, bench_class_methods);
criterion_main!(benches);
