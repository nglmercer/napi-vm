# Test262 full-pass plan — 2026-10-25

## Verified status

The best accepted complete run is still clean source `d942b04`: **102,745 /
102,956 (99.7951%)**, with 151 failures and 60 timeouts. Skips, crashes, and
harness errors were zero; it gained 76 passes with no losses against its
predecessor.

Evidence: [full refresh](../tools/test262/evidence/full-refresh-2026-10-25/summary.json)
and [exact remaining variants](../tools/test262/evidence/full-refresh-2026-10-25/remaining.json).

The most recent complete evaluation, on clean source `3816dcf`, passed
**102,661 / 102,956 (99.7135%)**, with 232 failures and 63 timeouts. It gained
18 passes and lost 102 against `d942b04`, so it is retained as a rejected
candidate rather than the accepted full baseline. The regressions came from
native methods not inheriting the realm's `Function.prototype` and function
property lookup using the wrong receiver/prototype chain. The complete report
and transition list are in
[regression evidence](../tools/test262/evidence/full-regression-function-binding-2026-10-25/summary.json).

The repair candidate now passes **120 / 315** variants in a recheck covering
all `d942b04` residuals and all status transitions from the rejected full run:
133 fail and 62 time out. It restores **100 passes with zero losses** against
the rejected candidate. Against `d942b04`, it gains 18 and loses two variants:
the script and strict variants of the exhaustive String case-mapping test.
Both variants also timed out in a fresh five-second diagnostic on the old
`d942b04` worker on this host, and pass with a 15-second diagnostic deadline.
This is focused evidence, not a new full score. The full projection of this
selection would be 102,761 passes, 133 failures, and 62 timeouts; a complete
run is required to confirm it.

The 211 non-passing variants break down into 44 parse failures, 107 runtime
failures, and 60 timeouts. The parse inventory is dominated by 38 class
decorator/element cases and four auto-accessor cases; two function syntax
cases remain. Runtime failures concentrate in 89 `staging/sm` variants, with
class and super semantics, RegExp, scope/eval, and built-in behavior as smaller
clusters. Timeouts concentrate in staging/SpiderMonkey (24), built-in URI
decoding (8), TypedArray (6), and RegExp/literal matching (10), with 12 spread
across Annex B, ArrayBuffer, URI encoding, and comment cases.

The earlier follow-up patch for inferred names, property lookup/prototype
behavior, and parser boundaries was compared on 212 variants drawn from the
residual files. It gained 18 passes with zero losses against that matching
full-report projection. The subsequent complete evaluation exposed function
binding regressions; the repair recheck described above restores them. Neither
focused result changes the accepted full score.

Subsequent committed fixes gained 35 passes in a 4,492-variant runtime/RegExp
selection and 18 passes in a 3,418-variant generator selection, with zero losses
in each comparison. These are focused results, not a replacement full score.
The source-text continuation on clean `d942b04` passes 1,006/1,020 selected
variants, gaining 16 passes with zero losses. Source preservation and native
syntax are implemented; compilation, native checks, and focused verification
pass. [Evidence](../tools/test262/evidence/function-source-2026-10-25/) retains
intermediate regressions and their resolution. The next gate is a full run on
the repaired, committed candidate.

## Ordered execution

1. **Repair native function inheritance and lookup — implemented and focused
   recheck complete.** Native methods now inherit the owning realm's
   `Function.prototype`, and property lookup follows the actual function value
   while respecting deleted prototype properties. The 315-variant recheck
   restores 100 passes with zero losses against the rejected full candidate.
   Focused evidence and native validation are archived; the full gate remains.

2. **Refresh the complete score on the repaired candidate.** Use the same
   pinned corpus, two workers, and five-second per-variant deadlines. Keep the
   rejected full evaluation in the history and compare every outcome with both
   `d942b04` and `3816dcf`. Triage the measured residual inventory.

3. **Close class syntax and decorators.** The last complete inventory has 38
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

5. **Remove the remaining timeouts.** The last accepted full inventory has 60;
   the repair projection has 62 because of the timing-sensitive case-mapping
   pair. Profile representative workloads from the
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

The next concrete milestone is a full pinned run on the clean committed repair
candidate. Use its exact residual inventory to order the remaining class,
runtime, and timeout work. The current evidence does not support a reliable
completion date.
