# Test262 full-pass plan — 2026-10-27

## Measured status

The latest completed full run used clean commit `6416949`, Test262 revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, two workers, and a five-second
deadline per variant. It passed **102,918 / 102,956**, with two failures and
36 timeouts, and no skips, crashes, or harness errors. The complete report
and worker identity are in [the full-run evidence](../tools/test262/evidence/full-date-prototype-2026-10-27/).
Subsequent fixes have focused results but have not yet been confirmed by a
completed full run. A checkpointed run of committed source `7bcd549` against
the same revision is in progress.

The current implementation follows the current specification for Annex B
`arguments` block functions. Focused testing recovered the two staging failures,
but the older pinned Annex B test now fails because it asserts the opposite
result. [Upstream issue 5113](https://github.com/tc39/test262/issues/5113)
describes the contradiction, and [PR 5112](https://github.com/tc39/test262/pull/5112)
proposes a correction. The PR is open; simply updating Test262 does not by
itself establish that the conflict is fixed.

## Ordered work

1. **Refresh the baseline.** Finish the checkpointed pinned run and compare
   every variant against the previous full report. Keep timing-sensitive
   results visible and repeat marginal cases under the same two-worker,
   five-second configuration.
2. **Move the target to a newer Test262 revision.** The user selected a newer
   revision. Upstream `main` was `0b4e0812022b18236b5276d2804948fdd9f71603`
   on 2026-10-27, six commits beyond the old pin. Both conflicting files are
   unchanged there. Pin that commit and run its full unmodified corpus. Preserve the old
   revision and report separately so the denominator and changes are explicit.
   If the contradictory tests remain, report the unresolved upstream issue;
   do not claim a literal full pass or silently change or exclude a test.
3. **Remove remaining timeouts and semantic failures.** The last completed
   pinned run had 16 Date, eight RegExp literal, four Annex B RegExp, four URI,
   two RegExp whitespace, and two TypedArray sort timeouts. The whitespace
   variants subsequently passed focused checks. Profile the unmodified cases
   and prioritize shared interpreter call, binding, property, and loop costs;
   the Date sweep in particular needs a substantial speedup. Preserve observable
   semantics and compare broad related suites after each change. Reclassify
   residuals using the new revision's actual full report.
4. **Close the full gate.** On clean committed source, build the release
   worker and run every selected-revision variant with two workers, five-second
   deadlines, and no feature skips. Require zero failures, timeouts, skips,
   crashes, and harness errors. Archive the full report, corpus and source
   commits, worker hash, configuration, and check logs. Repeat marginal cases
   and investigate any regression before claiming completion.

## Validation policy

Use focused tests to diagnose and validate changes, not as a replacement for a
complete run. Run formatting, core tests, all-target core Clippy with warnings
denied, and affected worker and integration selections; broaden coverage for
shared semantics. Record failed experiments as well as improvements. Scaled
tests and longer deadlines are diagnostic only. Full AST/bytecode differential
and wasm execution remain separate gates. The available evidence does not
support a reliable completion date.
