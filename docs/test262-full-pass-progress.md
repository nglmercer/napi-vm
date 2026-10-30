# Full Test262 pass campaign

The target is **102,956 / 102,956 passing variants** on pinned corpus
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, with no feature exclusions,
skips, timeouts, crashes, failures, or harness errors. Measurements use two
workers and five-second per-variant deadlines. Focused reports do not update
the full score.

## Interpreter overhead continuation: `85ba746` (revision `0b4e081`)

The current pinned revision is `0b4e0812022b18236b5276d2804948fdd9f71603`
(102,992 variants). Clean source `85ba746` passes **102,975 / 102,992
(99.9835%)**: one failure, 16 timeouts, no skips, crashes, or harness errors.
It gains 18 passes and loses none against the `d1d403d` full run. The 441 core
tests pass. [Full evidence](../tools/test262/evidence/full-interpreter-overhead-2026-10-30/)
holds the report, comparison, and worker identity.

Profiling with a frame-pointer build showed allocation and copying, not one
algorithm, behind the timeouts. The changes:

- Object metadata and environment records box rarely used payloads (Intl,
  Temporal, iterator, realm, and module slots) and replace usually empty hash
  collections with lazily allocated wrappers. Object and frame construction
  no longer copies or seeds kilobyte-sized records.
- Each realm caches its `Object`, `Array`, and `Date` allocation prototypes,
  guarded by an intrinsic-table generation and only for immutable
  `prototype` data properties.
- `new Date(...)` on a realm's own intrinsic, as its own newTarget, with no
  argument that can run guest code, allocates directly.
- Existing writable global data properties and numeric lexical bindings are
  read, written, and incremented in place. The AST tier's global assignment
  previously took the full descriptor path (about 12 µs per write).
- Calls cache each closure chain's global and module context, and bytecode
  dispatch is an inlined function instead of a per-instruction closure.

Measured alone, RegExp literal tests went from about 5.2 to 1.1 seconds, URI
decoding from 5.5 to 4.2, TypedArray counting sort from 5.6 to 4.8, and the
Date DST-caching tests from 10.3 to 6.5 seconds. The Date tests remain the only
timeouts. The one failure is the contradictory Annex B `arguments` test
described in [upstream issue 5113](https://github.com/tc39/test262/issues/5113);
it cannot pass together with the staging test that asserts the opposite.

## Latest complete evaluation: `259402f`

Clean source `259402f` passed **102,760 / 102,956 (99.8096%)** in the
2026-10-25 full run. It had 133 failures and 63 timeouts, with no skips,
crashes, or harness errors. Against the prior zero-loss reference `d942b04`, it
gained 18 passes and lost three; against the rejected `3816dcf` candidate it
gained 99 passes with zero losses.

The three losses versus `d942b04` are the script and strict variants of
`staging/sm/String/string-upper-lower-mapping.js` and the script variant of
`staging/sm/regress/regress-610026.js`; all timed out at five seconds. The
five-second rerun reproduced those same three timeouts while the strict
`regress-610026.js` variant passed. All four selected variants pass with a
15-second diagnostic deadline, confirming a performance-margin issue for these
cases rather than a semantic failure.

[Full evidence](../tools/test262/evidence/full-native-prototype-repair-2026-10-25/)
contains the report, comparisons, triage, source identity, build log, and exact
196 remaining variants.

The residual inventory has 44 parse failures (25 class statements, 15 class
expressions, four staging decorators), 89 runtime failures (71 in `staging/sm`
plus super, RegExp, arguments, eval, built-in, and module cases), and 63
timeouts. Timeouts include 27 `staging/sm`, eight RegExp literal, six
TypedArray, eight URI decode, and 14 Annex B/ArrayBuffer/RegExp/URI encode/
comment cases.

## Prior rejected full evaluation: `3816dcf`

Clean source `3816dcf` passed **102,661 / 102,956 (99.7135%)** with 232
failures and 63 timeouts. It gained 18 passes and lost 102 against `d942b04`.
The cause was native methods not inheriting their realm's `Function.prototype`
and function property lookup using the wrong receiver/prototype chain. The
[regression evidence](../tools/test262/evidence/full-regression-function-binding-2026-10-25/)
preserves the full report and transitions.

## Zero-loss full reference: `d942b04`

Clean source `d942b04` passes **102,745 / 102,956 (99.7951%)** in the completed
2026-10-25 full run. There are **151 failures and 60 timeouts**, with zero
skips, crashes, or harness errors. Against the previous complete full report,
the comparison records **76 new passes and zero lost passes**.

The pinned corpus is `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`; the run used
two workers, five-second deadlines, no exclusions, and no resumed outcomes.
Worker SHA-256 is
`364ea4a28f71ebf488f5c1a7ff78849e3bba8f3efee1f173366436bec3762b3d`.
[Archived evidence](../tools/test262/evidence/full-refresh-2026-10-25/)
contains every result, the comparison, triage, source/configuration identity,
and the 211 residual variants.

Those variants comprise 44 parse failures, 107 runtime failures, and 60
timeouts. The largest runtime cluster is 89 `staging/sm` failures. Parse
failures are mostly class decorators/elements and auto-accessors. The largest
timeout groups are staging/SpiderMonkey (24), URI decoding (8), TypedArray (6),
and RegExp/literal matching (10). See the
[ordered plan](test262-full-pass-plan.md) for the next steps.

## Previous full baseline

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
identities, implementation patch, and validation logs. At the time of this
focused measurement, the full score was 102,669; the later `d942b04` full
refresh is recorded above.

## Function source continuation

Clean source `d942b04` passes **1,006 / 1,020** selected Function and related
variants, up from 990: **16 new passes and zero lost passes**. Fourteen semantic
failures remain in that selection, with no timeouts or other outcomes. The
later `d942b04` full refresh is recorded above.

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

## Function binding and parser continuation

The follow-up patch was compared on **212 variants** from the latest residual
file inventory, gaining **18 passes and losing zero** against the matching
projection of the full refresh. Gains cover function name inference, async
property names, exponentiation grammar, anonymous function names in logical
assignment, method definitions, and primitive getter receivers. The later
`3816dcf` complete evaluation exposed native method inheritance and function
lookup regressions. The repaired `259402f` full run restores 99 passes with
zero losses against that rejected candidate and records 102,760 passes, 133
failures, and 63 timeouts. It gains 18 and loses three against the zero-loss
`d942b04` reference. The three losses are timing-sensitive five-second timeouts.

The patch also passes the 373 core tests, ten focused regressions, neighboring
integration suites, 23 worker tests, all-target Clippy with warnings denied,
and formatting. [Focused evidence](../tools/test262/evidence/function-binding-followup-2026-10-25/)
retains the report, no-loss comparison, source identity, and validation logs.
The subsequent [repair evidence](../tools/test262/evidence/native-prototype-repair-2026-10-25/)
contains the 315-variant recheck and its no-loss comparison against the rejected
candidate.

## Remaining gates

1. Remove the three five-second timeout regressions identified in the complete
   run. The same three time out in an exact five-second rerun, while all four
   selected variants pass at 15 seconds. Add runtime headroom under the original
   deadline without changing the denominator.
2. Implement the current 44 class and decorator parse cases, then close the 89
   runtime failures beginning with `staging/sm`, super ordering, and RegExp.
   Use focused regressions and neighboring groups after each implementation.
3. Profile and remove the remaining 63 timeouts under the same five-second
   limit, including `staging/sm`, RegExp literals, TypedArray, URI, Annex B,
   ArrayBuffer, and comment cases.
4. Run the complete corpus on a clean committed candidate and require 102,956
   passes with zero failures, timeouts, skips, crashes, or harness errors.

Full AST/bytecode differential and wasm32 execution are additional coverage
gates; the standalone worker's full score does not establish those results.
