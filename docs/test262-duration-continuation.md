# DurationFormat continuation toward the full-suite 98% goal

Historical continuation snapshot. The current complete pinned-corpus result is
maintained in [latest.json](../tools/test262/latest.json); the measurements
below describe the earlier implementation checkpoint.

Implemented `Intl.DurationFormat` with constructor/method descriptors, branded
slots, locale and numbering-system resolution, all base and unit styles,
display options, numeric style chaining, fractionalDigits, resolvedOptions,
format, and formatToParts. Numbering-system resolution is shared with
RelativeTimeFormat; its behavior and measured passes are preserved.

Guest duration records are read and converted in alphabetical field order.
Mixed signs and range limits are checked after reading the record. Exact
u128 arithmetic validates normalized nanoseconds below 2^53 seconds, preserving
unsafe Number integers' actual binary values during conversion. Decimal
formatting also keeps subsecond sums exact, rather than converting them back
to f64. This covers valid records whose microsecond/nanosecond counts exceed
u64, the adjacent valid/invalid nanosecond boundary, and fractional truncation.

ICU supplies unit labels, plural rules, digit/group parts, list patterns,
digital separators, and locale padding data. The VM combines exact decimal
values, controls unit visibility, and propagates one duration sign, including
leading negative-zero fields. No locale-specific duration patterns were copied
from tests. The pinned Temporal integration accepts duration strings and uses
branded Temporal.Duration slots without invoking prototype getters.

The general conversion/option algorithms were checked against
[ECMA-402](https://tc39.es/ecma402/#sec-getdurationunitoptions); the pinned corpus
also exercises the Temporal integration hooks.

## Verification

The final debug selection passes **138 / 220 DurationFormat** variants and
**158 / 160 RelativeTimeFormat** variants, with no skips, timeouts, crashes, or
harness errors. Evidence is in `artifacts/test262/duration/debug-final.json`,
worker SHA-256 `26ac97de68376c493e500dc1220679ef95fb8c5b521bb505d574459f3589606d`.
All 220 DurationFormat variants failed in the preceding full baseline.
The initial pre-Temporal-hook implementation passed 130 variants; its report
is retained as `debug-focused.json`.

The final optimized worker confirms the same 296 / 380 combined result,
including 138 DurationFormat and 158 RelativeTimeFormat passes, in
`artifacts/test262/duration/release-final.json`. There are no skips, timeouts,
crashes, or harness errors. Worker SHA-256 is
`3c163f6f2489047fec998eb95d44ee24c4f8251cdb4ae10e41d5c7d49d2b4d70`.
Source patch, new modules, test source, and source hashes are archived beside
that report.

The 82 remaining DurationFormat failures depend on missing Intl APIs:
78 fail when constructing Intl.NumberFormat (mostly the test oracle), two call
Intl.supportedValuesOf, and two inspect Intl.DateTimeFormat.prototype.
Those comparisons remain unverified; this is not a DurationFormat completion
claim. RelativeTimeFormat's two failures also require NumberFormat.

All 20 selected Intl integration tests pass across five suites. The four new
duration tests compare prepared execution and direct AST execution, checking
exact arithmetic, range boundaries, getter order, abrupt conversion, resolved
style chains, parts, signed leading zeros, Temporal strings, and tainted
Temporal prototype getters.

Additional small-value comparisons against Node pass all 90 visibility/sign/
parts cases and 165 formatting cases across eleven locales; evidence is in
`differential.json` and `locale-differential.json` beside the focused results.
These supplement the pinned corpus and do not affect its denominator.

At this checkpoint, the newest completed full-corpus baseline was 98,632 /
102,956 (95.8001%), and its live run excluded this DurationFormat work. That
full-run state is historical. The current overall result and its measured
source are recorded in [latest.json](../tools/test262/latest.json) and the
current [ShadowRealm/full-suite continuation](test262-shadow-realm-continuation.md).
No corpus, skips, worker count, timeout, or denominator changed for the
focused measurements above.
