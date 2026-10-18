# ShadowRealm Test262 continuation

ShadowRealm now has constructor and prototype metadata, a fresh isolated
global per instance, `evaluate`, and asynchronous `importValue`. Evaluation
uses the realm's indirect `eval` scope: lexical declarations do not leak
between calls, parse-time syntax errors remain SyntaxErrors, and runtime
exceptions cross the boundary as TypeErrors. Primitive results pass through;
callable results become non-constructible wrapped functions. Calls translate
the receiver and arguments across the boundary, reject non-callable objects,
wrap callable arguments and return values, and convert target exceptions to
caller-realm TypeErrors. `importValue` loads in the shadow realm, returns a
promise, resolves exported primitives or wrapped callables, and rejects failed
imports or missing/unwrappable exports with TypeErrors.

The cycle collector traces the realm globals and wrapped targets retained by
these internal slots. This reuses the existing realm creation, module-source
sharing, scheduler, and callable invocation paths.

The complete pinned `built-ins/ShadowRealm` selection passes **124 / 124**
variants, including strict, script, and module cases. Together with the
`Object.prototype.isPrototypeOf` selection, the release worker passes **144 /
144** variants. The full corpus passes **101,488 / 102,956 (98.5741%)**, with
1,414 failures, 54 timeouts, zero skips/crashes/harness errors, and **134 gains
with zero losses** against the previous completed full run. The corpus,
denominator, worker count, and timeout are unchanged. See
[the current Test262 result](../tools/test262/latest.json); compressed full
report, focused report, source patch, comparison, and triage are in
[`tools/test262/evidence/shadow-realm-2026-10-18/`](../tools/test262/evidence/shadow-realm-2026-10-18/).

The 42 focused Rust integration tests pass across `builtin_quick_wins` and
`object_enumeration`. Workspace Clippy passes for all targets and features
with warnings denied, and `cargo fmt --all -- --check` passes.

The top remaining full-run clusters are `staging/sm` (220 non-passing
variants), `intl402/Temporal` (176), `language/import/import-defer` (95), and
`built-ins/Object` (79). Broader host realm, GC/finalization timing, and full
engine-wide realm integration remain separate work.

The full-corpus counts above describe the worker snapshot recorded in
`latest.json`; subsequent Object integrity and iterator fixes have not had a
full-corpus rerun. Their 622-variant focused selection is recorded in the
[Object integrity continuation](test262-object-integrity-continuation.md).
