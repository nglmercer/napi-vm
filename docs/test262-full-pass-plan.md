# Test262 full-pass plan — 2026-10-26

## Verified status

The latest complete pinned run passes **102,839 / 102,956 (99.8864%)**:
65 failures, 52 timeouts, and zero skips, crashes, or harness errors.
It gains **20 passes with zero losses** against the preceding full worker
`ef2a45bd…` (102,819 passes).

There are **117 remaining variants: 42 parse failures, 23 runtime failures,
and 52 timeouts**. Every outcome, comparison, triage, source identity, exact
source diff, and [residual inventory](../tools/test262/evidence/full-activation-followup-2026-10-26/remaining.json)
is retained in [full evidence](../tools/test262/evidence/full-activation-followup-2026-10-26/).

The measured source is an uncommitted snapshot based on `42ae715`. The worker
SHA-256 is `aacb94ea7e8fccf9e3ddd92adf9af09a8932e0b0ec6a678fffe2fb1daf8c39b5`.
The complete run used corpus `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, two
workers, five-second deadlines, no exclusions, and no resumed outcomes.
Source and worker hashes were revalidated after completion. The final
acceptance gate requires clean committed source.

The full run confirms thirteen caller/arguments gains, two ArrayBuffer
sliceToImmutable coercion gains, four URI-decode gains, and strict toSpliced.
The implementation tracks rooted guest activations and original argument
snapshots, uses stable accessor descriptors and native callback boundaries,
and avoids repeated property-key work for guarded ordinary array and binding
reads. All previous passes remain, including script toSpliced, BMP casing,
and regress-610026. The six-variant timing slice passed twice before the full
run. The broader 16,279-variant focused selection also passed without losses.

Validation passed **395 core library tests**, **five worker tests**,
all-target Clippy with warnings denied, root all-target checking, formatting,
and whitespace checks. Rejected descriptor and getter-alias candidates and
all measured timing losses remain in the
[activation evidence](../tools/test262/evidence/function-activation-followup-2026-10-26/).

## Decorator working-tree follow-up

Auto-accessors clear eight variants. Class decorators clear another 26.
The same 16,882-variant class selection now passes **16,874**, with **34 gains
and zero losses** against the latest full-report projection (26 gains and
zero losses against the preceding auto-accessor worker). The eight remaining
class variants require element decorators. The six-variant timing slice
passes. All 399 core tests, five worker tests, Clippy, root checking,
formatting, and whitespace checks pass.

[Class decorator evidence](../tools/test262/evidence/class-decorator-followup-2026-10-26/)
retains reports, both comparisons, exact source, and the residual class list.
The [auto-accessor evidence](../tools/test262/evidence/auto-accessor-followup-2026-10-26/)
also preserves the rejected naming-collision candidate. These focused gains
do not replace the complete score above.

## Method-decorator foundation

Method replacement, private/public context access, and initializer ordering
now pass direct AST/bytecode tests and six worker smoke probes. A broad
26,479-variant selection has **zero lost passes**, retaining the prior 34
focused gains and the same eight field-dependent failures. It adds no new
pinned passes yet. All 402 core tests, five worker tests, static checks, and
the timing slice pass. [Method evidence](../tools/test262/evidence/method-decorator-followup-2026-10-26/)
retains the rejected getter-name alias candidate and its data-descriptor fix.

## Ordered execution

1. **Implement element decorators and confirm the syntax gains in a full run.**
   Eight auto-accessor and 26 class-decorator variants are now clear in focused
   validation. The eight remaining class variants need element grammar,
   interleaved expression/key evaluation, method/getter/setter/field/accessor
   replacement validation, public/private context access operations, and
   per-element and extra initializer ordering. Method decoration is now in
   place. Finish field transforms/extras, getter/setter and auto-accessor
   decoration, context creation order, and arbitrary callable accessor identity.
   Preserve the class-decorator
   replacement, binding, and static-initialization behavior already tested.
   Run valid and negative syntax plus complete class selections after changes.
   Do not merely accept and discard decorators.

2. **Close the other runtime causes.** Four Annex B declaration variants need
   argument-shadowing, labeled declarations, and redeclaration ordering. Four
   legacy Date variants need two-digit-year heuristics, written months, compact
   timezone offsets, and strict ISO rejection. Ten resource-limit variants
   need legitimate large strings, a 20 MiB DataView buffer, and a flat JSON
   array with 2^21 + 1 entries. Decouple buffer limits from dense-array limits
   and preserve bounded allocation behavior. The five remaining runtime
   variants cover Array ToLength (two), legacy NUL quoting (two), and object
   primitive conversion (one). Each cause needs its neighboring comparison.

3. **Remove all 52 timeouts.** The inventory includes 22 staging/sm, eight
   RegExp-literal, six TypedArray, four URI-decode, four Annex B, two RegExp, four URI-encode,
   and two comment variants.
   Separate nontermination from slow execution. Several TypedArray cases copy
   buffers byte by byte through numeric indexed reads/writes; inspect redundant
   property-key conversion and descriptor work while preserving coercion,
   detachment, immutable-buffer, and Proxy semantics. Profile the URI, string,
   and regexp loops as separate causes. Keep headroom with two workers and
   five-second deadlines; longer diagnostics do not count as passes.

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
