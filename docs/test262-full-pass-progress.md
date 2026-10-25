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

## Verified continuation outside the full score

Clean source `d30e770` passes **4,486 / 4,492** selected variants, gaining
**35 passes with zero lost passes** against the matching projection of the
completed full baseline. Two Function source-text failures and four BMP RegExp
timeouts remain in this selection; all outcomes remain in the denominator.
[Continuation evidence](../tools/test262/evidence/runtime-regexp-2026-10-25/)
contains every outcome, the exact baseline projection, transitions, source and
worker identities, and validation logs.

The implementation addresses these reproduced causes:

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
- String-set classes preserve longest-alternative priority under lazy and
  greedy repetition, using iterative backtracking with failed-state memoization.
- String iterator next validates its receiver's own internal slots.
- The isolated host exposes `$DONE` only for tests with the async flag.

All 373 core unit tests, 78 focused integration tests, and 23 worker tests pass.
Formatting and core all-target Clippy with warnings denied also pass. The
Unicode property spelling checks follow the
[ECMAScript property table](https://tc39.es/ecma262/multipage/text-processing.html#table-binary-unicode-properties),
which lists `White_Space` and `space` rather than ICU's short name `WSpace`.
These focused results do not establish a new full percentage.

## Generator result continuation

Clean source `f3d2d61` passes **3,416 / 3,418** selected generator variants,
up from 3,398 on the preceding verified release worker: **18 new passes and
zero lost passes**. The selection includes synchronous and async generator
groups, yield expressions, and SpiderMonkey generator staging cases. Its only
remaining failures are both variants of `staging/sm/generators/runtime.js`,
which require preserved function source text. There are no timeouts, skips,
crashes, or harness errors in this selection.

Synchronous `yield*` now forwards each incomplete delegate result unchanged,
without eagerly reading its `value` property. A private suspension flag tells
the shared driver whether to forward that object or create a result for an
ordinary yield. Next, throw, and return share this handling. Async generators
retain their existing value/await path. Regressions cover object and proxy
identity, getter ordering, nested delegation, abrupt completion, and ordinary
yields after delegation.

All 373 core unit tests, 12 focused integration tests, 23 async generator
foundation tests, and 36 generator stress tests pass, as do formatting and core
all-target Clippy with warnings denied. The comparison uses the same pinned
corpus, two workers, five-second deadline, and selected variant identities.
[Archived evidence](../tools/test262/evidence/generator-result-2026-10-25/)
includes both full focused reports, every transition, exact source/worker
identities, implementation patch, and validation logs. The full score remains
the completed 102,669-pass baseline until a new full run completes.

## Function source continuation

Clean source `d942b04` passes **1,006 / 1,020** selected Function and related
variants, up from 990: **16 new passes and zero lost passes**. Fourteen semantic
failures remain in that selection, with no timeouts or other outcomes. The full
score remains the completed 102,669-pass baseline pending a fresh full run.

Functions, arrows, methods, accessors, classes, eval, and dynamic Function
constructors retain shared original UTF-16 source ranges through AST and bytecode
execution. Function toString returns exact source, including comments, original
line endings, computed names, and lone surrogates. Native functions use internal
display names and valid property syntax without reading guest name getters;
bound functions and proxies retain native syntax. Callable string conversion
uses the ordinary conversion hooks.

The runner now reads corpus and harness text without Python universal newline
translation. The initial comparison exposed eight losses: native symbol naming
and CR/CRLF fixtures. A second comparison left two punctuation-name losses.
Both intermediate reports are archived alongside the final zero-loss comparison;
no outcomes were filtered or reclassified. Native formatting follows the
[NativeFunction grammar](https://tc39.es/ecma262/multipage/fundamental-objects.html#sec-function.prototype.tostring).

All 373 core tests, 11 new source regressions in both execution paths, 99
neighboring integration tests, 23 worker integration tests, and seven runner
tests pass. Formatting and core all-target Clippy with warnings denied pass.
[Source evidence](../tools/test262/evidence/function-source-2026-10-25/)
contains all comparisons, exact source/worker identities, implementation patch,
and validation logs.

## Remaining gates

1. Preserve the verified continuation and use the archived baseline and focused
   transitions to drive the remaining work. Keep every measured loss visible.
2. Close the remaining semantic cases, including decorator/auto-accessor syntax,
   source text and function behavior, scope semantics,
   and RegExp cases listed in the inventory.
3. Profile and fix all remaining timeout workloads, including exhaustive Date,
   URI and RegExp loops, TypedArray copying/sorting, and large buffer fixtures.
4. Run the complete corpus on a clean committed candidate under the same gate,
   archive its outcomes and source/worker identities, and verify every variant
   passes before claiming completion.

Full AST/bytecode differential and wasm32 execution are additional coverage
gates; the standalone worker's full score does not establish those results.
