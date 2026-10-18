# Temporal PlainYearMonth and PlainMonthDay continuation

Historical feature continuation. Its overall pass target below describes that
earlier checkpoint; see [latest.json](../tools/test262/latest.json) for the
current full-corpus state.

Adds privately branded Temporal.PlainYearMonth and Temporal.PlainMonthDay
objects with constructor/subclass behavior, intrinsic factories, string and
property-bag conversion, getters, partial replacement, equality, serialization,
and conversion to PlainDate. PlainYearMonth also has comparison, duration
arithmetic and differences. PlainDate converts to both types. Calendar-bearing
objects are recognized through private slots. PlainYearMonth preparation omits
the day field and preserves ordered field access/coercion. Reference ISO dates
and calendar arithmetic are delegated to temporal_rs.

The pinned built-ins and intl402 selections for these two types pass
**2,188 / 2,250 variants (97.2444%)**. Comparison against the preceding complete
Temporal report records **2,154 gains, zero losses**, and 34 retained passes.
No skips, timeouts, crashes, or harness errors occurred. Remaining selected
failures depend on absent ZonedDateTime or locale-aware formatting.

Focused debug worker SHA-256:
0ec1bd87ec3c6b81ec37936ba5726789c556edf73b8cba0541b0f8273664bdd9.
Corpus revision: 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81; four jobs,
five-second timeout. Runner limits, denominator, corpus and skips are unchanged.
Evidence is in tools/test262/evidence/temporal-yearmonth-monthday, with source
base, measured patch, comparison, and compressed individual focused results.

All 79 selected Rust tests pass across temporal, json_reviver,
builtin_quick_wins, utf16, and test262_worker. Two new regressions independently
assert AST and verified bytecode execution. They cover reference dates, leap
month-day conversion, arithmetic, calendar conversions, receiver brands,
invalid constructor fields, empty partial fields, and observable property and
option order. Strict Clippy for library/worker/Temporal tests passes.

Additional running measurements at checkpoint writing:

- Full optimized PlainDateTime snapshot: exec session 27032,
  artifacts/test262/temporal-datetime-release-full.json and its .checkpoint.jsonl.
  This frozen worker predates PlainYearMonth/PlainMonthDay.
- All built-ins/Temporal and intl402/Temporal with the new types: exec session
  67778, artifacts/test262/temporal-yearmonth-all-focused.json.
- Optimized build of this snapshot: exec session 84888.

Inspect live process/session state before deciding whether any needs resumption.
The Instant-only debug full run was interrupted due to verified debug-speed
RegExp timeouts and must not be represented as completed.

The latest completed full report at this checkpoint was 79,243 / 102,956
(76.9678%) before raw JSON and these Temporal extensions. Focused percentages
and gains do not replace a complete full measurement. The original 90% target
was later surpassed by the current full-corpus result in
[latest.json](../tools/test262/latest.json).

The all-Temporal run completed with **9,588 / 13,268 passes**, zero skips,
timeouts, crashes, or harness errors. Against the full PlainDateTime worker's
Temporal projection it gains **2,184 passes and loses zero**. The additional
30 gains outside the two type selections are inter-type conversions. This is
still a focused measurement. Its compressed report is archived as
all-temporal.json.gz. Session 67778 has exited with runner status 1 for the
remaining failures; the optimized build session 84888 also completed.
