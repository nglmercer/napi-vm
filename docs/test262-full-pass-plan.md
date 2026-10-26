# Test262 full-pass plan — 2026-10-25

## Verified status

The latest complete run used clean source `259402f` and passed **102,760 /
102,956 (99.8096%)**. It recorded 133 failures and 63 timeouts, with zero
skips, crashes, or harness errors. It gains 18 passes and loses three versus
the `d942b04` zero-loss reference, for a net gain of 15. All three losses are
timeouts: script and strict variants of
`staging/sm/String/string-upper-lower-mapping.js`, and the script variant of
`staging/sm/regress/regress-610026.js`. Against the rejected `3816dcf` full
candidate it restores 99 passes with zero losses.

Evidence: [complete repaired run](../tools/test262/evidence/full-native-prototype-repair-2026-10-25/summary.json)
contains the report, both comparisons, triage, source identity, build log, and
the exact remaining variants.

The prior clean `d942b04` run remains the zero-loss reference at **102,745 /
102,956 (99.7951%)**, with 151 failures and 60 timeouts; it gained 76 passes
with zero losses against its predecessor. Its
[full evidence](../tools/test262/evidence/full-refresh-2026-10-25/) includes all
211 residual variants.

The intervening `3816dcf` complete candidate passed 102,661 variants, with 232
failures and 63 timeouts. It gained 18 and lost 102 versus `d942b04` because
native methods failed to inherit the realm's `Function.prototype`, and
function property lookup used the wrong receiver/prototype chain. Its
[regression evidence](../tools/test262/evidence/full-regression-function-binding-2026-10-25/)
is retained in full.

The current 196 residual variants comprise 44 parse failures, 89 runtime
failures, and 63 timeouts. Parse failures are 25 class statements, 15 class
expressions, and four staging decorator cases. Runtime failures include 71
`staging/sm`, six super, four RegExp, and smaller arguments/eval/built-in/module
clusters. Timeouts include 27 `staging/sm`, eight RegExp literal cases, six
TypedArray, eight URI decoding, and smaller Annex B, ArrayBuffer, RegExp, URI
encoding, and comment clusters.

The earlier follow-up patch for inferred names, property lookup/prototype
behavior, and parser boundaries was compared on 212 variants drawn from the
residual files. It gained 18 passes with zero losses against that matching
full-report projection. The subsequent complete evaluation exposed function
binding regressions; the repair recheck described above restores them. Neither
focused result changes the complete-corpus score.

Subsequent committed fixes gained 35 passes in a 4,492-variant runtime/RegExp
selection and 18 passes in a 3,418-variant generator selection, with zero losses
in each comparison. These are focused results, not a replacement full score.
The source-text continuation on clean `d942b04` passes 1,006/1,020 selected
variants, gaining 16 passes with zero losses. Source preservation and native
syntax are implemented; compilation, native checks, and focused verification
pass. [Evidence](../tools/test262/evidence/function-source-2026-10-25/) retains
intermediate regressions and their resolution.

## Ordered execution

1. **Stabilize the three lost passes.** The complete run lost two variants of
   `staging/sm/String/string-upper-lower-mapping.js` and one of
   `staging/sm/regress/regress-610026.js` to five-second timeouts. Repeat these
   exact variants on the current worker and the `d942b04` reference, then profile
   the slow paths and optimize only with reproducible timing evidence. Preserve
   all measured transitions.

2. **Close class syntax and decorators.** The current parse inventory has 25
   class-statement variants, 15 class-expression variants, and four staging
   decorator variants. Implement the required decorator grammar first, then
   decorator evaluation/application order, accessor storage, private brands,
   and initializer order required by runtime cases. Run valid and negative
   syntax groups and class behavior groups after each step.

3. **Close the remaining runtime semantics.** Start with the 71 failing
   `staging/sm` variants, then the six super, four RegExp, and smaller arguments,
   eval, built-in, and module clusters. Split cases by shared cause, add
   regressions, run neighboring groups, and compare against the full report
   without losing passes.

4. **Remove the remaining timeouts.** The current inventory has 63: 27
   `staging/sm`, eight RegExp literal, six TypedArray, eight URI decode, four
   Annex B, two ArrayBuffer, two RegExp, four URI encode, and two comment
   variants. Distinguish engine slowness from nontermination, optimize shared
   hot paths, and keep headroom under the five-second deadline. Longer
   diagnostic deadlines do not count as passes.

5. **Run the final full gate and archive reproducible evidence.** Require
   102,956 passes and zero failures, timeouts, skips, crashes, or harness errors
   on a clean committed source tree. Archive the full report, source identity,
   worker hash, configuration, build/check logs, and comparison. Recheck formerly
   timing-sensitive cases for stability. Any residual failure reopens its root
   cause; do not change the denominator, exclude staging tests, or relax timeouts.

## Validation policy

Run formatting, core all-target Clippy with warnings denied, core tests, and
affected integration/worker tests for implementation changes. Preserve every
measured lost pass in comparisons. Keep focused results separate from the most
recent completed full result. Full AST/bytecode differential and wasm execution
are separate coverage gates and must be reported separately from this worker's
Test262 result.

The next concrete milestone is to eliminate the three timing-sensitive losses
with reproducible five-second passes, then use zero-loss comparisons while
closing the 44 parse failures, 89 runtime failures, and 63 timeouts. The current
evidence does not support a reliable completion date.
