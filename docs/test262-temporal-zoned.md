# Temporal ZonedDateTime continuation

Adds Temporal.ZonedDateTime construction, private slots, property-bag and string
conversion, calendar/time/epoch/offset accessors, duration arithmetic,
differences, rounding, field replacement, withCalendar/withTimeZone/withPlainTime,
startOfDay, transitions, comparison, equality, and string/JSON serialization.
Conversions connect ZonedDateTime with Instant, PlainDate, PlainTime and
PlainDateTime; relativeTo also reads branded slots. Timezone, calendar, DST,
and nanosecond calculations use temporal_rs with the compiled TZ provider.
Constructor timezone strings use identifier-only parsing; conversion functions
retain ISO timezone-string support. Observable gets and coercions remain in
the VM, in the prescribed order.

The optimized complete built-ins/Temporal and intl402/Temporal selection passes
**12,928 / 13,268 (97.4374%)**, gaining **3,340 passes with zero losses** versus
the preceding all-Temporal year-month snapshot. There are no skips, timeouts,
crashes, or harness errors. The ZonedDateTime projection passes 2,926 / 2,968.
Remaining Temporal failures involve Intl formatting, supported timezone values,
Temporal.Now, or other unimplemented operations. This is a focused result.

The worker SHA-256 is
4b4d2ddf8eee5f3105b92dd87bb7dbf40960177169f999a5d7947ea0d6d4cb7a.
Evidence under tools/test262/evidence/temporal-zoned contains the compressed
report/transitions and exact source patch on source-base.txt. The corpus is
5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81; jobs=4, timeout=5 seconds, no skips.
Runner assertions, denominator, corpus and limits were not changed.

All 82 selected Rust tests pass (temporal, json_reviver, builtin_quick_wins,
utf16, test262_worker). Three new ZonedDateTime tests assert AST and verified
bytecode independently, covering exact epoch values, 23-hour DST calendar days
versus 24-hour durations, local-time gaps, disambiguation, cross-type conversions,
brands, constructor validation, and transition options. Strict Clippy passes
for library/worker/Temporal tests. Formatting, whitespace, and all 12 Python
Test262 tooling tests pass.

The latest completed full run is **84,728 / 102,956 (82.2953%)** for the snapshot
before YearMonth/MonthDay/ZonedDateTime. It includes three timeouts and no skips,
crashes or harness errors. Its comparison has 5,486 gains and one lost pass,
a Proxy timeout which passes on a later worker replay; the measured full
outcome is retained. latest.json records that completed run.

A new full optimized run including every current Temporal type is active in
exec session **17009**, output:

- artifacts/test262/temporal-zoned-release-full.checkpoint.jsonl
- artifacts/test262/temporal-zoned-release-full.json

Inspect live session/process state before attempting restart/resume. All earlier
Temporal build and focused-run sessions have completed. The Instant-only debug
full run remains deliberately interrupted, with its journal retained.

The original goal requires at least 92,661 / 102,956 passing variants. It remains
active and unverified. Do not present focused gains added to an earlier full
result as a measured overall percentage.

The full optimized run completed: **90,252 / 102,956 (87.6607%)**, 12,701
failures and three timeouts; zero skips, crashes, or harness errors. Comparison
against the preceding PlainDateTime full run has **5,524 gains, zero losses**.
Full report and transitions are archived in this batch's evidence directory.
latest.json now records this measurement. Session 17009 has exited with runner
status 1 for remaining failures. DataView/TypedArray changes are later than
this measured worker. The goal still needs 2,409 additional passing variants.
