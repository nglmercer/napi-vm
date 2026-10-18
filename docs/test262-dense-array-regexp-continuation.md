# Dense array and regular expression continuation toward 98%

Historical continuation snapshot. Its full and focused measurements below are
from 2026-10-15; the current pinned-corpus status is maintained in
[latest.json](../tools/test262/latest.json) and the newer
[ShadowRealm/full-suite continuation](test262-shadow-realm-continuation.md).

The active goal is at least **98% of the complete pinned Test262 corpus**.
The newest completed full report inspected at the start of this continuation,
`artifacts/test262/full-results-2026-10-15d.json`, records 98,632 passes,
3,530 failures, and 794 timeouts out of 102,956 variants (95.8001476%).
It has no skips, crashes, or harness errors. This is below the requested goal.
The older `tools/test262/latest.json` records a preceding source snapshot.

## Changes

- Retain and validate the existing scalar-only regular expression repetition
  optimization: deterministic scalar classes use a position vector rather
  than allocating a layer and visited sets for every character.
- Avoid descriptor object allocation during ordinary array index writes when
  the target and receiver are identical. Existing own data indices use the
  array storage helper. New indices first prove that the ordinary prototype
  chain has no intercepting indexed property. Proxies, boxed primitives,
  module namespaces, inherited descriptors, and accessor arrays fall back to
  the general path. Storage helpers still enforce writability, extensibility,
  length restrictions, and hole presence.
- Represent lone-surrogate membership separately from Rust scalar Unicode
  property ranges. Support the Surrogate general category and its aliases;
  preserve complement/case-fold order in both `u` and `v` modes.
- Use the ECMAScript whitespace set for regexp `\s` and `\S`; U+0085 is not
  ECMAScript whitespace.

## Verification so far

- 44 root integration tests pass across `array_elisions`,
  `array_mutation_quick_wins`, `object_spread`, and `test262_worker`.
- All six core `regexp_legacy` integration tests pass.
- All eight regexp matcher unit tests pass, including bounds, greedy/lazy
  backtracking, scoped flags, million-character input, lone surrogates,
  surrogate pairs, complements, and whitespace.
- All twelve Python Test262 runner tests pass.
- New array tests compare prepared bytecode and direct AST execution and
  cover inherited setters, Proxy prototypes, boxed string prototypes,
  non-writable own indices, non-writable lengths, non-extensible receivers,
  holes, and appends.

With only the scalar optimization, the ASCII and Any generated property
tests time out in all four variants (`artifacts/test262/scalar-repeat-check.json`).
Adding the array fast path passes all four under the same four-worker,
five-second limits (`artifacts/test262/dense-array-scalar-check.json`, worker
SHA-256 `19d57ef0b1d45632f5953941606474c182871f4e661a5f42ccf386836409ca24`).
This focused measurement predates the surrogate and whitespace corrections
and is not an overall pass-rate claim.

## Remaining work

The final release build succeeded. Its worker SHA-256 is
`4de24a48a27bf504369ffb1d415179d33ed570ce69c5117c0fdd133bb76f28a2`.
A complete `built-ins/RegExp` run finished, writing
`artifacts/test262/dense-array-regexp/regexp-results.json` and the resumable
`regexp.checkpoint.jsonl` beside it. Tool session `80785` is terminal.
The run passes 3,441 / 3,756 variants, with 192 failures and 123 timeouts,
and no skips, crashes, or harness errors. Comparison with the full baseline's
RegExp projection proves 605 gains and zero regressions in
`regexp-baseline-comparison.json`. Source patch and hashes are saved beside
these artifacts. The run uses four workers and five seconds per variant.

Measure the latest release worker and run the full corpus. The full baseline contains 740 RegExp
timeouts; the speed changes need broader measurement. Unicode property data
also needs an update: local regex-syntax 0.8.11 tables identify themselves as
Unicode 16.0.0, while the pinned generated property tests use Unicode 17.0.0.
Missing Intl NumberFormat, DateTimeFormat, RelativeTimeFormat, DurationFormat,
and other remaining failures still contribute to the full denominator.

## Unicode data follow-up

While the preceding frozen-worker RegExp run continues, property lookup now
uses ICU 2.3's `CodePointSetData::new_for_ecma262`, strict general-category and
script parsers, and ScriptWithExtensions data. The dependency was already
present transitively and is now explicit. Scalar intervals and surrogate
membership remain separate. Case closures use equivalence classes from ICU
simple case folding instead of regex-syntax's Unicode 16 folding table.
Literal, range, and backreference equality use Unicode simple folding in
Unicode mode and the single-character uppercase Canonicalize rule in legacy
mode, including its non-ASCII-to-ASCII restriction.

All nine matcher unit tests pass. New checks cover Unicode 17 Alphabetic,
Assigned, Sidetic, currency-symbol additions, strict property names, new case
pairs, Greek case equivalence, and legacy/Unicode distinctions for long s,
Kelvin sign, dotless i, and sharp s. A new release build and focused corpus
measurement are required; the active frozen-worker run predates these edits.

The new release build succeeded with SHA-256
`eaabff882bcaaaaf8a41e5f8ea4bed4608abb11d80e8603c021dbc078902c2b6`.
The 14-variant Unicode-focused run records nine passes, five timeouts, and
zero semantic failures, skips, crashes, or harness errors in
`artifacts/test262/dense-array-regexp/unicode-icu-focused.json`. This ran
concurrently with the preceding RegExp measurement; repeat measurement after
that process finishes before interpreting the timeout rate. All 14 selected
variants were failures in the full baseline. The nine passes therefore prove
focused gains, but neither selection is a full-suite measurement.
The complete core library run has 363 passes and one failure in the existing
`interpreter::node_addon::tests::loads_and_invokes_a_real_node_api_addon`
fixture (pending Promise cannot settle). No regexp, property, or bytecode
parity test failed.

Source identities and the patch for this follow-up are saved beside the
artifacts as `unicode-icu-source.sha256`, `unicode-icu-source.patch`, and
`unicode-icu.rs` (the new source module).

No corpus content, feature skips, denominator, timeout, or worker count was
changed for these historical measurements. The statement above that the 98%
goal was unachieved describes the state at that checkpoint only; consult the
current result linked above for today's measured status.
