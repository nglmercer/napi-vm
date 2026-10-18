# Temporal ZonedDateTime continuation

Historical focused continuation. The full-run handles and overall score below
belong to this earlier checkpoint; see [latest.json](../tools/test262/latest.json)
for the current full-corpus result.

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

At that earlier checkpoint, the latest completed full run was **84,728 /
102,956 (82.2953%)** for the snapshot before YearMonth/MonthDay/ZonedDateTime.
It includes three timeouts and no skips, crashes or harness errors. Its
comparison has 5,486 gains and one lost pass, a Proxy timeout which passes on a
later worker replay; the measured full outcome is retained.

The optimized full run including every then-current Temporal type completed at
the paths below:

- artifacts/test262/temporal-zoned-release-full.checkpoint.jsonl
- artifacts/test262/temporal-zoned-release-full.json

The Instant-only debug full run remains deliberately interrupted, with its
journal retained. These handles are historical and require no resumption.

At that point the 90% full-corpus threshold (92,661 / 102,956 passes) remained
unmet. Focused gains were not included in the full-run count.

The subsequent full optimized run completed: **90,252 / 102,956 (87.6607%)**, 12,701
failures and three timeouts; zero skips, crashes, or harness errors. Comparison
against the preceding PlainDateTime full run has **5,524 gains, zero losses**.
Full report and transitions are archived in this batch's evidence directory.
Session 17009 exited with runner status 1 for remaining failures. DataView/
TypedArray changes came after this worker. Later work surpassed that 90%
threshold; the current complete-corpus result is in
[latest.json](../tools/test262/latest.json).
