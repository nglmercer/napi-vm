# Named evaluation and exponentiation continuation

Continues the checkpoint in [test262-checkpoint-status.md](test262-checkpoint-status.md),
after destructuring validation at `8a3cf06`.

Simple `var`, `let`, and `const` declarations now apply NamedEvaluation to a
direct anonymous function, arrow, generator, async function, or class
initializer in both execution tiers: the AST evaluator reuses
`evaluate_class_field`, and the bytecode compiler reuses
`compile_named_assignment_value`. Named definitions keep their own name, and
values returned by calls keep their empty name, so only direct anonymous
definitions are renamed. `tests/named_evaluation.rs` covers both tiers.

Exponentiation now uses a shared `Number::exponentiate` helper in the `**`
operator (fast and coercing paths) and `Math.pow`: a NaN exponent, a NaN base
with a non-zero exponent, and a base with absolute value 1 with an infinite
exponent produce NaN, while a zero exponent produces 1. Coverage is in
`tests/numeric_binary.rs`.

Array destructuring assignment now propagates IteratorClose failures when a
generator `return()` resumes a `yield` inside the pattern: the close error (a
thrown error or a non-object `return` result) replaces the return completion
instead of being discarded, matching the for-of loop's existing behavior.
`tests/destructuring.rs` covers the throwing, non-object, clean-close, and
declaration cases; the new test was observed failing before the fix.

Focused pinned-corpus measurement (revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, four workers, five-second
timeouts, no feature skips): the combined compound-assignment, arithmetic,
assignment, and `var`/`let`/`const` declaration selection passes
3,001 / 3,008 variants (99.77%) in
`artifacts/test262/named-evaluation-focused.json`. This gains the 30
declaration `fn-name` variants, the four exponentiation variants, and the 16
generator-return IteratorClose variants that failed in the same selections
before this change, with no losses in those selections.

Remaining failures in the selection are unchanged, pre-existing clusters:
declaration statements do not yet produce an empty completion for `eval`
(6 variants), and one `with`-environment variable binding-resolution variant.

Full conformance is not achieved, and no new full-corpus measurement was run
for this continuation; `tools/test262/latest.json` still records
67,677 / 102,956 passes (65.7339%).

Rust verification: the focused integration suites pass
(`named_evaluation`, `numeric_binary`, `compound_assignments`,
`destructuring`, `plain_assignments`, `array_elisions`), all 80 bytecode
unit tests pass, `cargo fmt --check` is clean, and Clippy reports only the
pre-existing warnings in untouched files. The core library suite passes 353
tests with two failures (`shape::add_transition_matches_literal` and the
real-Node-API-addon load test) that were reproduced identically on the
untouched tree, so they are pre-existing and unrelated to this change.
