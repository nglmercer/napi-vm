//! Stress tests for the generator implementation.
//!
//! Generators run their body on a dedicated stack via a stackful coroutine
//! (`corosensei`), suspending at each `yield` and switching back to the caller
//! — all on one thread. This file exercises the paths where that lifecycle is
//! easiest to get wrong: completion, abandonment mid-suspension, nesting,
//! cloning, and closing.
//!
//! These tests were originally written against an OS-thread implementation
//! that moved `Rc`-backed state across a thread boundary under
//! `unsafe impl Send`. ThreadSanitizer measured ~7.6 data races per run of
//! this file against that design (~1.0 after threads were joined). The
//! coroutine implementation reports **zero**, because there is no second
//! thread for a non-atomic refcount to race on. Re-check with:
//!
//! ```text
//! RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test -Zbuild-std \
//!     --release --no-default-features --test generator_stress \
//!     --target x86_64-unknown-linux-gnu -- --test-threads=1
//! ```

#![cfg(stackful_coroutines)]

use napi_vm::interpreter::Interpreter;
use napi_vm::lexer::Lexer;
use napi_vm::parser::Parser;

/// Evaluate `source` in a fresh interpreter and return the result as a string.
fn run(source: &str) -> String {
    let mut interp = Interpreter::with_builtins();
    let toks = Lexer::new(source).tokenize_with_spans();
    let stmts = Parser::new_with_spans(toks).parse();
    let value = interp.run(&stmts).expect("execution failed");
    interp.vs(&value).unwrap_or_default()
}

/// Evaluate `source`, expecting it to fail.
fn run_expecting_error(source: &str) {
    let mut interp = Interpreter::with_builtins();
    let toks = Lexer::new(source).tokenize_with_spans();
    let stmts = Parser::new_with_spans(toks).parse();
    assert!(interp.run(&stmts).is_err(), "expected an error");
}

// ── completion ───────────────────────────────────────────────────────

#[test]
fn many_generators_run_to_completion() {
    // Each generator allocates and releases an 8 MiB coroutine stack. Running
    // a few hundred in one interpreter catches stack leaks and bodies that
    // never finish.
    let out = run(r#"
        function* range(n) { let i = 0; while (i < n) { yield i; i = i + 1; } }
        let total = 0;
        let g = 0;
        while (g < 200) {
            for (const v of range(5)) { total = total + v; }
            g = g + 1;
        }
        total;
    "#);
    assert_eq!(out, "2000");
}

#[test]
fn generators_sharing_one_closure_environment() {
    // The closure `Env` is an `Rc<RefCell<_>>` shared by the driver and every
    // generator body. Under the old thread design this was the refcount most
    // likely to be raced on teardown; it is now the one most likely to expose
    // a borrow or lifetime mistake in the coroutine handoff.
    let out = run(r#"
        let shared = 0;
        function* bump() { shared = shared + 1; yield shared; shared = shared + 1; yield shared; }
        let seen = 0;
        let i = 0;
        while (i < 200) {
            const g = bump();
            seen = seen + g.next().value;
            seen = seen + g.next().value;
            shared = shared + 1;
            i = i + 1;
        }
        shared;
    "#);
    assert_eq!(out, "600");
}

// ── abandonment ──────────────────────────────────────────────────────

#[test]
fn generators_abandoned_while_suspended() {
    // Dropped mid-body: the coroutine is suspended at a `yield` and its stack
    // is unwound from that point when it is dropped, running destructors on
    // the way out.
    let out = run(r#"
        function* forever() { let i = 0; while (true) { yield i; i = i + 1; } }
        let i = 0;
        let sum = 0;
        while (i < 200) {
            const g = forever();
            sum = sum + g.next().value;
            sum = sum + g.next().value;
            i = i + 1;
        }
        sum;
    "#);
    assert_eq!(out, "200");
}

/// The same abandonment as above, at two scales.
///
/// These exist to bisect a `STATUS_ACCESS_VIOLATION` that only Windows
/// x86_64 shows, in `cloned_generator_values_share_one_coroutine`. libtest
/// runs single-threaded tests in name order, and a fault kills the process,
/// so the last name printed is the answer: `a_few` faulting means the bug is
/// structural, only `many` faulting means it is cumulative, and both passing
/// means something specific to that test's aliasing is involved.
///
/// `tests/coroutine_backend.rs` already rules out the coroutine backend
/// itself: it survives 200 of these unwinds with no interpreter attached.
#[test]
fn abandoning_a_few_suspended_generators() {
    let out = run(r#"
        function* counter() { yield 1; yield 2; yield 3; }
        let sum = 0;
        let i = 0;
        while (i < 10) {
            const g = counter();
            sum = sum + g.next().value;
            sum = sum + g.next().value;
            sum = sum + g.next().value;
            i = i + 1;
        }
        sum;
    "#);
    assert_eq!(out, "60");
}

#[test]
fn abandoning_many_suspended_generators() {
    let out = run(r#"
        function* counter() { yield 1; yield 2; yield 3; }
        let sum = 0;
        let i = 0;
        while (i < 100) {
            const g = counter();
            sum = sum + g.next().value;
            sum = sum + g.next().value;
            sum = sum + g.next().value;
            i = i + 1;
        }
        sum;
    "#);
    assert_eq!(out, "600");
}

#[test]
fn generators_abandoned_by_breaking_out_of_for_of() {
    let out = run(r#"
        function* forever() { let i = 0; while (true) { yield i; i = i + 1; } }
        let sum = 0;
        let i = 0;
        while (i < 200) {
            for (const v of forever()) { if (v > 2) { break; } sum = sum + v; }
            i = i + 1;
        }
        sum;
    "#);
    assert_eq!(out, "600");
}

#[test]
fn generators_never_started_are_dropped_cleanly() {
    // No `next()` at all: the coroutine is never created, so teardown must
    // cope with a `None` body.
    let out = run(r#"
        function* g() { yield 1; }
        let i = 0;
        while (i < 500) { const unused = g(); i = i + 1; }
        i;
    "#);
    assert_eq!(out, "500");
}

// ── nesting ──────────────────────────────────────────────────────────

#[test]
fn nested_generators_drive_one_another() {
    // An outer generator body drives an inner one, so a coroutine is itself
    // the caller that resumes another coroutine -- nested stack switches.
    let out = run(r#"
        function* inner(n) { let i = 0; while (i < n) { yield i; i = i + 1; } }
        function* outer(n) {
            for (const v of inner(n)) { yield v * 2; }
        }
        let sum = 0;
        let r = 0;
        while (r < 50) {
            for (const v of outer(4)) { sum = sum + v; }
            r = r + 1;
        }
        sum;
    "#);
    assert_eq!(out, "600");
}

#[test]
fn deeply_nested_generators() {
    let out = run(r#"
        function* a() { yield 1; yield 2; }
        function* b() { for (const v of a()) { yield v + 1; } }
        function* c() { for (const v of b()) { yield v + 1; } }
        function* d() { for (const v of c()) { yield v + 1; } }
        let sum = 0;
        let i = 0;
        while (i < 50) { for (const v of d()) { sum = sum + v; } i = i + 1; }
        sum;
    "#);
    assert_eq!(out, "450");
}

#[test]
fn nested_generators_abandoned_mid_flight() {
    // Breaking out of the outer loop tears down both coroutines at once.
    let out = run(r#"
        function* inner() { let i = 0; while (true) { yield i; i = i + 1; } }
        function* outer() { for (const v of inner()) { yield v; } }
        let sum = 0;
        let i = 0;
        while (i < 100) {
            for (const v of outer()) { if (v > 1) { break; } sum = sum + v; }
            i = i + 1;
        }
        sum;
    "#);
    assert_eq!(out, "100");
}

// ── cloning ──────────────────────────────────────────────────────────

#[test]
fn cloned_generator_values_share_one_coroutine() {
    // `Value::Generator` is `Rc<RefCell<GeneratorInner>>`; clones must observe
    // the same progress, and the body must not be resumed twice at once.
    let out = run(r#"
        function* counter() { yield 1; yield 2; yield 3; }
        let sum = 0;
        let i = 0;
        while (i < 100) {
            const g = counter();
            const alias = g;
            sum = sum + g.next().value;
            sum = sum + alias.next().value;
            sum = sum + g.next().value;
            i = i + 1;
        }
        sum;
    "#);
    assert_eq!(out, "600");
}

#[test]
fn generators_stored_in_structures_outlive_their_scope() {
    let out = run(r#"
        function* g(n) { yield n; yield n + 1; }
        const held = [];
        let i = 0;
        while (i < 100) { held.push(g(i)); i = i + 1; }
        let sum = 0;
        let j = 0;
        while (j < 100) { sum = sum + held[j].next().value; j = j + 1; }
        sum;
    "#);
    assert_eq!(out, "4950");
}

// ── failure paths ────────────────────────────────────────────────────

#[test]
fn a_throwing_generator_body_unwinds_cleanly() {
    // The body returns `Threw` and its stack unwinds; teardown has to work on
    // the error path too, not just on a clean return.
    let mut i = 0;
    while i < 100 {
        run_expecting_error(
            r#"
            function* boom() { yield 1; throw new Error("boom"); }
            const g = boom();
            g.next();
            g.next();
        "#,
        );
        i += 1;
    }
}

#[test]
fn a_generator_exhausting_the_loop_budget_unwinds_cleanly() {
    let mut interp = Interpreter::with_builtins();
    interp.set_loop_budget(10_000);
    let source = r#"
        function* forever() { while (true) { yield 1; } }
        const g = forever();
        let n = 0;
        while (true) { g.next(); n = n + 1; }
    "#;
    let toks = Lexer::new(source).tokenize_with_spans();
    let stmts = Parser::new_with_spans(toks).parse();
    assert!(interp.run(&stmts).is_err(), "expected the budget to trip");
}

// ── iterator closing ─────────────────────────────────────────────────

/// Leaving a `for...of` early closes the generator, so its `finally` runs.
/// A generator that is merely dropped is *not* closed — matching JavaScript,
/// where a generator collected by the GC never resumes.
#[test]
fn leaving_a_for_of_early_runs_finally() {
    for (name, source, expected) in [
        ("break", r#"break;"#, "fin"),
        ("labeled break", r#"break outer;"#, "fin"),
        ("return from the loop body", r#"return;"#, "fin"),
    ] {
        let program = format!(
            r#"
            let log = "";
            function* g() {{ try {{ yield 1; yield 2; }} finally {{ log = "fin"; }} }}
            function drive() {{ outer: for (const v of g()) {{ {source} }} }}
            drive();
            log;
        "#
        );
        assert_eq!(run(&program), expected, "closing on {name}");
    }
}

#[test]
fn an_exhausted_for_of_runs_finally_once() {
    let out = run(r#"
        let count = 0;
        function* g() { try { yield 1; yield 2; } finally { count = count + 1; } }
        for (const v of g()) {}
        count;
    "#);
    assert_eq!(out, "1");
}

#[test]
fn throwing_out_of_a_for_of_body_still_closes() {
    let out = run(r#"
        let log = "";
        function* g() { try { yield 1; } finally { log = "fin"; } }
        try { for (const v of g()) { throw new Error("boom"); } } catch (e) {}
        log;
    "#);
    assert_eq!(out, "fin");
}

#[test]
fn closing_is_idempotent_across_many_loops() {
    let out = run(r#"
        let count = 0;
        function* g() { try { yield 1; yield 2; } finally { count = count + 1; } }
        let i = 0;
        while (i < 200) { for (const v of g()) { break; } i = i + 1; }
        count;
    "#);
    assert_eq!(out, "200");
}

// ── re-entrancy ──────────────────────────────────────────────────────

#[test]
fn a_generator_resuming_itself_is_an_error_not_a_hang() {
    // The OS-thread implementation deadlocked here. The coroutine detects the
    // absent-but-unfinished body and reports it the way JavaScript does.
    let mut interp = Interpreter::with_builtins();
    let source = r#"
        let it;
        function* selfDrive() { it.next(); yield 1; }
        it = selfDrive();
        it.next();
    "#;
    let toks = Lexer::new(source).tokenize_with_spans();
    let stmts = Parser::new_with_spans(toks).parse();
    let err = interp.run(&stmts).expect_err("expected a TypeError");
    assert!(
        err.to_string().contains("already running"),
        "unexpected error: {err}"
    );
}
