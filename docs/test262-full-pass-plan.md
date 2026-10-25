# Test262 full-pass plan — 2026-10-25

## Verified status

The latest complete run used clean source `39f6a97`, pinned Test262 revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, two workers, and a five-second
timeout per variant. It passed **102,669 / 102,956 (99.7212%)**, with **224
failures and 63 timeouts**. Skips, crashes, and harness errors were all zero.

Evidence: [full baseline](../tools/test262/evidence/full-baseline-2026-10-25/summary.json)
and [exact remaining variants](../tools/test262/evidence/full-baseline-2026-10-25/remaining.json).

Subsequent committed fixes gained 35 passes in a 4,492-variant runtime/RegExp
selection and 18 passes in a 3,418-variant generator selection, with zero losses
in each comparison. These are focused results, not a replacement full score.
Current HEAD is `63b4368`. Uncommitted function-source changes are unfinished;
the latest compiler check reports a missing `ClassAssembly.source_text` field.
Do not use the working tree as a verified release candidate yet.

## Ordered execution

1. **Finish function source preservation and restore the build.** Carry original
   UTF-16 source ranges through parsing, bytecode, AST execution, classes, eval,
   and dynamic Function constructors. Connect `Function.prototype.toString`,
   including native/bound/proxy behavior, without invoking guest name getters.
   Check static-method boundaries, comments, computed names, and lone surrogates.
   The saved focused baseline is 990/1,020, with 30 failures. Require exact-source
   regression tests in both execution paths, passing native checks, and a focused
   before/after comparison with zero lost passes before committing the candidate.

2. **Refresh the complete baseline on that clean candidate.** Build the release
   worker, record source commit and worker hash, and run the entire pinned corpus
   with the unchanged gate. Produce a fresh inventory grouped by root cause.
   This removes already-fixed cases from the backlog and detects regressions
   beyond the earlier focused selections.

3. **Implement remaining class syntax and semantics.** The old baseline has 44
   parse failures: 42 class/decorator/auto-accessor variants and two function
   variants. Reproduce the function pair separately. Implement decorator
   evaluation/application ordering, accessor storage, private brands, and
   initializer ordering as required by the pinned tests. Run complete related
   class/decorator groups, including negative syntax tests, after each change.

4. **Close residual semantic clusters from the refreshed inventory.** Prioritize
   shared causes: Annex B block functions and eval bindings; super/reference
   evaluation order; function/class behavior; RegExp nesting/resource limits;
   then remaining built-in coercion, descriptors, and module-source prototype
   cases. For each cause, add a targeted regression, run its neighboring groups,
   and compare every selected variant against the preceding clean candidate.

5. **Eliminate timeouts through measured performance fixes.** The old baseline's
   63 timeout variants include 16 Date cases, eight RegExp literal cases, six
   built-in TypedArray cases, six staging TypedArray cases, exhaustive URI and
   RegExp loops, and buffer/string/array fixtures. Profile representative cases
   individually to distinguish excessive work, allocation, and nontermination.
   Fix shared hot paths; preserve observable coercion, proxy, detachment, and
   iterator behavior. Verify under the same two-worker, five-second gate with
   useful timing headroom. Diagnostic longer runs do not count as passes.

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

The next concrete milestone is a clean, tested function-source commit followed
by a fresh full report. Estimate subsequent work from that refreshed inventory;
the present evidence does not support a reliable completion date.
