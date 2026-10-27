# Test262 full-pass plan — 2026-10-26

## Verified status

The latest complete pinned run on clean commit `9d23d08` passes **102,906 /
102,956 (99.9514%)**, with **four failures and 46 timeouts**, no skips, crashes,
or harness errors. Against the preceding full report it gains 71 passes and
loses four: two strict TypedArray prototype-chain assignments and two Proxy
ownKeys timeouts. [Full evidence](../tools/test262/evidence/full-clean-2026-10-26/)
contains every outcome, comparison, residual, and source/worker identity.

The four failures are two TypedArray regressions plus two staging Annex B
arguments cases whose expectations conflict with the pinned normative test;
see [Annex B evidence](../tools/test262/evidence/annex-arguments-labels-2026-10-26/).
All 46 timeouts remain in the full report. The run used the pinned revision,
two workers, five-second deadlines, no exclusions, and no resumed outcomes.

The [regression repair](../tools/test262/evidence/full-regressions-2026-10-26/)
passes **17,680 / 17,680** focused variants, recovering all four full-run losses
with zero new losses. Repeated timing checks pass; full-run confirmation remains.

## Ordered execution

1. **Confirm the four regression repairs in a full run.** TypedArray
   prototype-chain assignments and Proxy ownKeys now pass focused comparisons
   and repeated timing checks; the full score above remains unchanged.

2. **Resolve the two staging arguments conflicts.** The pinned staging tests
   require behavior opposite to the pinned normative Annex B arguments test.
   Keep both outcomes visible and pursue a corpus resolution rather than
   disguising either expectation.

3. **Remove the 46 timeouts.** These include 22 staging variants (16 Date,
   four TypedArray, and two Proxy), eight RegExp literals, eight URI cases,
   four Annex B RegExp cases, two RegExp cases, and two comment cases. Profile
   independent causes while preserving two workers and five-second deadlines.

4. **Run the final full gate.** Require **102,956 passes and zero failures,
   timeouts, skips, crashes, or harness errors** on clean committed source.
   Archive the complete report, every transition, source identity, worker hash,
   configuration, and build/check logs. Repeat formerly marginal timing cases.
   Any remaining nonpass reopens its cause. Keep the pinned corpus,
   denominator, worker count, and timeout fixed.

## Validation policy

After implementation changes, run formatting, core all-target Clippy with
warnings denied, core tests, and affected worker/integration selections.
Check root bindings when their shared types change. Broaden testing where the
change can affect neighboring semantics. Preserve every measured regression;
focused results never replace the latest full score. Full AST/bytecode
differential and wasm execution remain separate gates.

Historical complete evidence remains available for
[102,783 passes](../tools/test262/evidence/full-worktree-2026-10-26/),
[102,760 passes](../tools/test262/evidence/full-native-prototype-repair-2026-10-25/),
and the [d942b04 reference](../tools/test262/evidence/full-refresh-2026-10-25/).
The current evidence does not support a reliable completion date.
