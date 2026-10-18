# Declaration completion and binding-resolution continuation

Continues the [named-evaluation continuation](test262-named-evaluation-continuation.md),
which left seven failures in its focused selection, from `f02bed3`.

Multi-declarator lists (`var a = 1, b = 2;`) are parsed as a
`Statement::Declarations` group, which was missing from the
`produces_completion_value` exclusion list. The group therefore overwrote the
running completion value with `undefined`, so `eval('11; var a = 12, b;')`
returned `undefined` instead of `11` in both tiers. The group only ever
contains `VarDecl`s (export desugarings pair a declaration with an already
non-completing export marker), so excluding it matches a single declaration
exactly. `tests/declaration_completion.rs` pins single, multi-declarator,
and `var`/`let`/`const` cases in both tiers.

Simple `var` declarations now resolve their binding before evaluating the
initializer, per VariableDeclaration steps 2–3. An initializer that deletes
the with-object property the declaration writes through (`with (obj) { var
test262id = delete obj.test262id; }`) therefore still writes to that object.
The bytecode compiler needs no change: it declines `with` bodies to the AST
evaluator. The same test file covers this case in both tiers.

Focused pinned-corpus measurement (revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, four workers, five-second
timeouts, no feature skips): the combined compound-assignment, arithmetic,
assignment, and `var`/`let`/`const` declaration selection passes
3,008 / 3,008 variants (100.0%) in
`artifacts/test262/named-evaluation-focused.json`, with zero failures, skips,
timeouts, crashes, or harness errors. This clears the last seven failures
from the preceding selection with no losses.

Full conformance is not achieved, and no new full-corpus measurement was run
for this continuation; `tools/test262/latest.json` still records
67,677 / 102,956 passes (65.7339%).

Rust verification: the new `declaration_completion` suite passes, along with
the `destructuring`, `named_evaluation`, `numeric_binary`,
`compound_assignments`, `plain_assignments`, `array_elisions`,
`object_spread`, `object_enumeration`, `iterator_protocol`,
`coroutine_backend`, and `engine_boundary` suites; `cargo fmt --check` is
clean. The `evaluation_modes::compact_property_cache_verification` failure
was reproduced on the untouched tree and is pre-existing.

Subsequent block, label, loop, switch, and abrupt-completion work is documented
in the [control-flow continuation](test262-control-flow-completion-continuation.md),
which includes AST/bytecode validation and current focused release evidence.
