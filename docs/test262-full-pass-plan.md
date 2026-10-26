# Test262 full-pass plan — 2026-10-25

## Verified status

The latest complete run used clean source `d942b04`, pinned Test262 revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, two workers, and a five-second
timeout per variant. It passed **102,745 / 102,956 (99.7951%)**, with **151
failures and 60 timeouts**. Skips, crashes, and harness errors were all zero.
Compared with the preceding full report, it gained **76 passes with no losses**.

Evidence: [full refresh](../tools/test262/evidence/full-refresh-2026-10-25/summary.json)
and [exact remaining variants](../tools/test262/evidence/full-refresh-2026-10-25/remaining.json).

The 211 non-passing variants break down into 44 parse failures, 107 runtime
failures, and 60 timeouts. The parse inventory is dominated by 38 class
decorator/element cases and four auto-accessor cases; two function syntax
cases remain. Runtime failures concentrate in 89 `staging/sm` variants, with
class and super semantics, RegExp, scope/eval, and built-in behavior as smaller
clusters. Timeouts concentrate in staging/SpiderMonkey (24), built-in URI
decoding (8), TypedArray (6), and RegExp/literal matching (10), with 12 spread
across Annex B, ArrayBuffer, URI encoding, and comment cases.

The follow-up patch for inferred names, property lookup/prototype behavior, and
parser boundaries was compared on 212 variants drawn from the residual files.
It gained **18 passes with zero losses** against the matching full-report
projection. Native and worker checks also pass. This focused result does not
change the full score above; the next gate is a clean full run on the committed
candidate.

Subsequent committed fixes gained 35 passes in a 4,492-variant runtime/RegExp
selection and 18 passes in a 3,418-variant generator selection, with zero losses
in each comparison. These are focused results, not a replacement full score.
The source-text continuation on clean `d942b04` passes 1,006/1,020 selected
variants, gaining 16 passes with zero losses. Source preservation and native
syntax are implemented; compilation, native checks, and focused verification
pass. [Evidence](../tools/test262/evidence/function-source-2026-10-25/) retains
intermediate regressions and their resolution. The full refresh is complete;
the current gate is a full run on the follow-up candidate.

## Ordered execution

1. **Function source preservation — completed on `d942b04`.** Carry original
   UTF-16 source ranges through parsing, bytecode, AST execution, classes, eval,
   and dynamic Function constructors. Connect `Function.prototype.toString`,
   including native/bound/proxy behavior, without invoking guest name getters.
   Check static-method boundaries, comments, computed names, and lone surrogates.
   The saved focused baseline is 990/1,020, with 30 failures. Require exact-source
   regression tests in both execution paths, passing native checks, and a focused
   before/after comparison with zero lost passes before committing the candidate.

2. **Refresh the complete score on the follow-up candidate.** It has passed the
   residual-file comparison with 18 gains and no losses. Run the complete pinned
   corpus on the clean committed candidate, then triage its new remaining set.

3. **Close class syntax and decorators.** The refreshed inventory has 38
   language class/decorator parse variants and four staging auto-accessor
   variants, plus class runtime failures. Implement the accepted decorator
   grammar first, then decorator evaluation/application order, accessor storage,
   private brands, and initializer order required by the runtime cases. Run
   valid and negative syntax groups and class behavior groups after each step.

4. **Close the remaining runtime semantics.** Start with the 89 failing
   `staging/sm` variants and split by shared cause: function names and inferred
   names, Annex B/eval bindings, class/super evaluation order, RegExp parsing,
   Set methods, and TypedArray behavior. Then address the smaller arguments,
   direct-eval, and module-source prototype clusters. Each change needs a
   regression, neighboring-group run, and zero-loss comparison.

5. **Remove all 60 timeouts.** Profile representative workloads from the
   refreshed inventory: staging/SpiderMonkey (24), URI decode (8), TypedArray
   (6), RegExp and literal matching (10), plus Annex B, ArrayBuffer, URI encode,
   and comment cases (12). Distinguish engine slowness from nontermination,
   optimize shared hot paths, and keep headroom under the same five-second
   deadline. Longer diagnostic deadlines do not count as passes.

6. **Run the final full gate and archive reproducible evidence.** Require
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

The next concrete milestone is a full pinned run on the clean committed
follow-up candidate. Use its exact residual inventory to order the remaining
class, runtime, and timeout work. The current evidence does not support a
reliable completion date.
