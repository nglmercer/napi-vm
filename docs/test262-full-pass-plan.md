# Test262 full-pass plan — 2026-10-27

## Measured status

The latest completed full run used source commit `d1d403d`, Test262 revision
`0b4e0812022b18236b5276d2804948fdd9f71603`, two workers, and a five-second
deadline per variant. It passed **102,957 / 102,992**, with one failure and
34 timeouts, and no skips, crashes, or harness errors. The complete report,
worker identity, and comparison are in [the full-run evidence](../tools/test262/evidence/full-new-revision-2026-10-27/).
The run overlapped builds and diagnostics; all 34 timeouts repeated in a
subsequent isolated selection with the same limits. The previous revision's
full run passed 102,918 / 102,956.

The current implementation follows the current specification for Annex B
`arguments` block functions. Focused testing recovered the two staging failures,
but the older pinned Annex B test now fails because it asserts the opposite
result. [Upstream issue 5113](https://github.com/tc39/test262/issues/5113)
describes the contradiction, and [PR 5112](https://github.com/tc39/test262/pull/5112)
proposes a correction. The PR is open; simply updating Test262 does not by
itself establish that the conflict is fixed.

The new revision also expects DataView length and offset accessors to return
zero for detached or out-of-bounds views. Test262 adopted this ahead of the
corresponding [ECMA-262 proposal](https://github.com/tc39/ecma262/pull/3972),
which remains open; the published specification currently requires a
TypeError. The implementation follows the selected test revision, so this is
an explicit compatibility difference until the specification changes.

## Ordered work

1. **Move the target to a newer Test262 revision.** The user selected a newer
   revision. Upstream `main` was `0b4e0812022b18236b5276d2804948fdd9f71603`
   on 2026-10-27, six commits beyond the old pin. Both conflicting files are
   unchanged there. The commit is pinned and its full unmodified corpus has
   run. Preserve the old revision and report separately so the denominator and changes are explicit.
   If the contradictory tests remain, report the unresolved upstream issue;
   do not claim a literal full pass or silently change or exclude a test.
2. **Confirm timing-sensitive results.** The complete reports have been
   compared, and all 34 timeouts repeated without competing builds under the
   same two-worker, five-second configuration. Repeat marginal cases after
   performance changes.
3. **Remove remaining timeouts and semantic failures.** The new full run has
   16 Date, eight RegExp literal, four Annex B RegExp, four URI, and two
   TypedArray sort timeouts. Profile the unmodified cases
   and prioritize shared interpreter call, binding, property, and loop costs.
   For Date, profile the four-level guest-language sweep and repeated calls;
   its scaled 24-timestamp diagnostic still took 2.36–2.84 seconds after the
   prototype cache, so small local wins are unlikely to suffice. Evaluate
   bytecode dispatch, call overhead, and the existing JIT seam against
   representative unmodified tests, with guards for observable side effects,
   mutation, execution limits, and cancellation. For RegExp and URI, profile
   parsing, construction, calls, and repeated global bindings independently.
   Preserve observable semantics and compare broad related suites after each
   change. Reclassify residuals using the new revision's actual full report.
   A numeric-operand fast path passed 2,369 arithmetic and coercion variants
   and reduced a scaled URI diagnostic by about 24%; the original URI, Date,
   and TypedArray targets still timed out at five seconds. See the
   [focused evidence](../tools/test262/evidence/numeric-binary-fast-path-2026-10-28/).
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
