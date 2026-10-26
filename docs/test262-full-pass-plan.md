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

## Decorator follow-up

Auto-accessors clear eight variants, class decorators 26, and regular field
decorators another eight. The broad **32,596-variant** selection now passes
in full: **42 gains with zero losses** against the latest full-report
projection, or eight gains with zero losses in the 26,479-variant method-stage
projection. All previously nonpassing parse variants are clear in this focused
selection. These are focused gains; the complete score above is unchanged.

All 405 core tests, five worker tests, static checks, and the standard timing
slice pass. [Field evidence](../tools/test262/evidence/field-decorator-followup-2026-10-26/)
retains rejected array-field and pop-performance candidates plus corrected
comparisons. The 30,000-element pop diagnostic now finishes in about 1.62
seconds instead of timing out at five seconds. Context property creation
order is tested for methods and fields. Prior
[method](../tools/test262/evidence/method-decorator-followup-2026-10-26/),
[class](../tools/test262/evidence/class-decorator-followup-2026-10-26/), and
[auto-accessor](../tools/test262/evidence/auto-accessor-followup-2026-10-26/)
evidence remains preserved.

The subsequent [getter/setter follow-up](../tools/test262/evidence/accessor-decorator-followup-2026-10-26/)
retains **32,596 / 32,596** passes with zero losses and all 42 focused gains.
All **408 core tests**, five worker tests, and static checks pass. Canonical
accessor metadata preserves arbitrary callable identity across reflection,
reads/writes, caches, Node serialization, and exotic prototype inheritance.
Rejected core and broad candidates are archived. Auto-accessor decoration
remains next; this stage does not alter the complete score.

The [auto-accessor decorator follow-up](../tools/test262/evidence/auto-accessor-decorator-2026-10-26/)
retains **32,596 / 32,596** focused passes with zero losses, plus the six-case
timing slice and 409 core tests. It applies paired replacements and per-element
initializers in both tiers. These are focused results; the complete score is
unchanged until a new full run.

The [conversion and length follow-up](../tools/test262/evidence/conversion-length-2026-10-26/)
passes **18,247 / 18,247** focused variants, gaining three passes with zero
losses against the latest full projection: two Array ToLength and one object
ToPrimitive variant. All 412 core tests, five worker tests, static checks, and
the standard timing slice pass. These gains await full-run confirmation.

The [buffer follow-up](../tools/test262/evidence/buffer-cap-2026-10-26/)
gains both DataView variants with zero losses in 4,664 focused variants;
six existing TypedArray timeouts remain. All 414 core tests and the standard
timing slice pass. The byte storage limit is independent of dense arrays.

The [hour-cycle follow-up](../tools/test262/evidence/intl-hour-cycle-2026-10-26/)
passes **838 / 838** focused variants, gaining both NUL-option variants with
zero losses. All 415 core tests and the standard timing slice pass. The gains
need confirmation in another complete run.

The [large-string follow-up](../tools/test262/evidence/string-cap-2026-10-26/)
passes **3,663 / 3,663** focused variants, gaining six Unicode escape variants
with zero losses. All 415 core tests and the standard timing slice pass. The
string limit now accommodates the 16 MiB-plus test inputs within a bounded cap.

The [large JSON follow-up](../tools/test262/evidence/large-json-2026-10-26/)
passes **13,661 / 13,661** focused variants, gaining both large-array variants
with zero losses. All 416 core tests and the standard timing slice pass. JSON
uses a bounded total element budget while retaining sparse array growth.

The [Annex B follow-up](../tools/test262/evidence/annex-arguments-labels-2026-10-26/)
passes **2,947 / 2,949** focused variants, gaining two labeled-declaration
variants with zero losses. The remaining two staging arguments tests conflict
with the pinned normative arguments test; the alternate candidate and its
regression are archived. All 418 core tests and the standard timing slice pass.

The [Date parsing follow-up](../tools/test262/evidence/date-parsing-2026-10-26/)
passes **1,718 / 1,734** focused variants, gaining four passes with zero losses;
sixteen prior DST-caching timeouts remain. All 419 core tests and the standard
timing slice pass. The four Date gains need full-run confirmation.

## Ordered execution

1. **Confirm decorator gains in a full run.** Class, method, getter/setter,
   field, and auto-accessor decorators now have focused coverage, including
   arbitrary callable identities, private brands, source-order overrides, and
   per-element initializers. Confirm the 42 syntax gains on the full corpus;
   investigate any remaining conformance gaps and preserve replacement,
   binding, and static initialization semantics.

2. **Close the other runtime causes.** Two Annex B labeled-declaration variants now pass in focused runs. Two
   staging arguments variants conflict with the pinned normative test and need
   a corpus resolution or an explicit change to the acceptance scope. Four legacy Date variants now pass focused runs after adding two-digit-year
   heuristics, written months, compact timezone offsets, and strict ISO checks. The ten resource-limit variants now pass focused runs: six large-string,
   two DataView, and two large JSON-array variants. Confirm them in the next
   complete run. Array ToLength (two), object primitive conversion (one), and legacy NUL
   option validation (two) now pass in focused runs and require full-run
   confirmation. Each cause needs its neighboring comparison.

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
