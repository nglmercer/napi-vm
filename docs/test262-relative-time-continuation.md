# RelativeTimeFormat continuation toward the full-suite 98% goal

Historical continuation snapshot. The current complete pinned-corpus result is
maintained in [latest.json](../tools/test262/latest.json); the measurements
below describe the earlier implementation checkpoint.

Implemented `Intl.RelativeTimeFormat` with branded slots, constructor and method
metadata, inherited option reads in spec order, locale negotiation, supported
numbering system resolution, Unicode extension precedence, supportedLocalesOf,
receiver checks, ToNumber/ToString conversions, eight singular/plural units,
finite-number validation, signed zero, default decimal rounding, all styles,
numeric auto/always, resolvedOptions, format, and formatToParts.

ICU experimental 0.6.0 supplies CLDR relative-time patterns and plural selection;
ICU decimal 2.3.0 supplies grouping, localized digits and number parts. Both
dependencies and their data are locked in Cargo.lock. A provider wrapper
records the actual numbering system chosen by ICU, including fallback.
No language-specific patterns or digit tables were copied from the tests.
Fractional values select numeric formatting before rounding, so an auto
formatter renders 0.0001 seconds as numeric zero seconds rather than `now`.
ICU's relative pattern interpolation currently erases nested decimal spans;
parts reconstruction uses the identical decimal formatter to identify the
single numeric placeholder and preserve its integer/group/fraction spans.

## Measured results

The complete pinned RelativeTimeFormat selection passes **158 / 160** variants
with zero skips, timeouts, crashes, or harness errors. The two remaining
failures are the script/strict variants of `format/en-us-numbering-systems.js`,
which construct the still-missing `Intl.NumberFormat` for comparison. All 160
variants failed in the full baseline. The report is
`artifacts/test262/dense-array-regexp/relative-debug.json`, worker SHA-256
`c7874728b2529eb2f4cf9624a8043ee04eb598149035caccc6a2048a55bc6e95`.
This debug worker predates a follow-up empty `nu` extension fallback correction.

The final optimized worker confirms the same 158 passes and two failures,
without skips, timeouts, crashes, or harness errors, in
`artifacts/test262/relative-time/release-focused.json`. Its SHA-256 is
`d55d17873e7125bbbbf1a01f218215355985364b452309f9cce758ecb7628d15`.
The empty `nu` extension correction is included and its integration test passes.

All 16 selected Intl integration tests pass across `intl_relative_time`,
`intl_collation_plurals`, `intl_locale`, and `intl_segmenter_list`.
The three new tests compare prepared execution and direct AST execution.
They cover option and conversion order, Proxy branding, English and Polish
patterns, Arabic digits, extension/option precedence, signed zero, decimal
rounding, special relative words, and parts.

The preceding frozen-worker RegExp run also completed with 605 gains and zero
regressions against the full baseline projection. See the
[RegExp continuation](test262-dense-array-regexp-continuation.md).

These are focused results. At this checkpoint, the newest full-corpus baseline
was 98,632 / 102,956 (95.8001%). That historical full run did not include the
changes described here. The current overall result and its measured source are
recorded in [latest.json](../tools/test262/latest.json) and the current
[ShadowRealm/full-suite continuation](test262-shadow-realm-continuation.md).
No corpus or runner-budget changes were made for the recorded selections.
