# Full Test262 pass campaign

The target is **102,956 / 102,956 passing variants** on pinned corpus
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, with no feature exclusions,
skips, timeouts, crashes, failures, or harness errors. Measurements use two
workers and five-second per-variant deadlines. Focused reports do not update
the full score.

## Completed baseline

Clean source `39f6a97` passes **102,669 / 102,956 (99.7212%)** in the completed
2026-10-25 full run. There are **224 failures and 63 timeouts**, with zero
other non-passing outcomes. The failures comprise 180 runtime and 44 parse
outcomes. Comparison with the previous complete full report records 691 new
passes and zero lost passes.

Worker SHA-256 is
`7300cfef39470c226f0879ed40baf247a3cf4b0235e86c1397a7bcb061da4dfb`.
It was built with Rust 1.99.0 using the repository release profile, from a
clean committed source tree. No checkpoint outcomes were restored.
[Archived evidence](../tools/test262/evidence/full-baseline-2026-10-25/)
contains the complete report, source/configuration identity, build log,
comparison, triage, and the exact 287 remaining variants.

## Candidate work outside the full score

Current implementation work addresses these reproduced causes:

- Compiled call update targets throw before numeric coercion, and parentheses
  preserve valid update references.
- Var iteration heads assign through the current binding chain, including
  catch parameters, instead of preparing a separate head binding.
- Dynamic function parameter text does not gain a leading line terminator.
- Async-from-sync throw closes the underlying iterator on value rejection.
- RegExp has standard constructor metadata, and lookbehind consumes backward
  with directional captures, quantifiers, backreferences, and Unicode boundaries.
- Unicode backreferences compare code points; the `space` property alias
  uses the existing Unicode White_Space data.
- String iterator next validates its receiver's own internal slots.
- The isolated host exposes `$DONE` only for tests with the async flag.

Native and focused conformance verification is in progress. These changes do
not establish a new full percentage.

## Remaining gates

1. Complete native and focused verification of the current candidate, compare
   every selected identity against the full baseline, and retain any losses.
2. Close the remaining semantic cases, including decorator/auto-accessor syntax,
   source text and function behavior, generator delegation, scope semantics,
   and RegExp cases listed in the inventory.
3. Profile and fix all remaining timeout workloads, including exhaustive Date,
   URI and RegExp loops, TypedArray copying/sorting, and large buffer fixtures.
4. Run the complete corpus on a clean committed candidate under the same gate,
   archive its outcomes and source/worker identities, and verify every variant
   passes before claiming completion.

Full AST/bytecode differential and wasm32 execution are additional coverage
gates; the standalone worker's full score does not establish those results.
