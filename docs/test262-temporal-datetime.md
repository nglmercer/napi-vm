# Temporal PlainDateTime continuation

Historical feature continuation. Its overall pass target below describes that
earlier checkpoint; see [latest.json](../tools/test262/latest.json) for the
current full-corpus state.

Adds Temporal.PlainDateTime with private slots, constructor/subclass semantics,
cloning, field-bag/string conversion, date/time/calendar getters, arithmetic,
differences, rounding, equality, comparison, with/withPlainTime/withCalendar,
and string/JSON serialization. Conversions among PlainDate, PlainTime and
PlainDateTime use private slots. PlainDate.toPlainDateTime is implemented.
Date-field conversion is shared while preserving each type's receiver brand.
Field gets and primitive conversions occur in alphabetical order, with options
read afterwards. Calendar and nanosecond calculations use temporal_rs.

The combined selection of built-ins/Temporal/{PlainDateTime,PlainDate,PlainTime}
and intl402/Temporal/{PlainDateTime,PlainDate} passes **5,444 / 5,788 (94.0567%)**.
Against the preceding full Temporal report overlaid with the independently
measured PlainDate selection, there are **2,438 gains and zero losses**; 3,006
passes are retained. This is a focused comparison, not a full-suite count.
No skips, timeouts, crashes, or harness errors occurred. Remaining selected
failures depend largely on unimplemented Temporal types, ZonedDateTime
conversions, and locale formatting.

Focused debug worker SHA-256:
d734058a45b2c29e89e9d4160bd9ab040d19b8a585d1040b55fa01e8e6a503f9.
Evidence in tools/test262/evidence/temporal-datetime includes a compressed
report, comparison, source base, and measured/final source patches. Final source
differs from the measured snapshot only by three needless-borrow Clippy fixes.

Validation: all 77 selected Rust tests pass across temporal, json_reviver,
builtin_quick_wins, utf16, and test262_worker. The two new PlainDateTime tests
independently assert AST and verified bytecode results, covering nanosecond
carry over leap day, round/difference operations, private-slot conversions,
partial field replacement, and observable coercion order. Strict Clippy for
library/worker/Temporal tests passes, as do formatting and whitespace checks.

The Instant-only full debug run was deliberately interrupted with SIGINT after
verified debug-speed RegExp timeouts. Its journal is retained, but it is not a
completed full measurement. The optimized PlainDateTime full run completed at
the output paths below; its former session handle is historical:

- artifacts/test262/temporal-datetime-release-full.checkpoint.jsonl
- artifacts/test262/temporal-datetime-release-full.json

The corpus remains 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81, four jobs and a
five-second variant timeout, with no feature skips. At this checkpoint, the
latest completed full report was 79,243 / 102,956 (76.9678%) before raw JSON and
these Temporal extensions. The original 90% target has since been surpassed by
the current full-corpus result in [latest.json](../tools/test262/latest.json).

The optimized full run has completed: **84,728 / 102,956 (82.2953%)**, 18,225
failures and three timeouts, with zero skips, crashes, or harness errors.
Comparison against temporal-time-duration-full.json shows 5,486 gains and one
lost pass: staging/sm/Proxy/ownkeys-linear.js strict times out. Both variants
pass on a replay using the newer optimized year-month worker; this replay has
a different worker hash and does not replace the full-run outcome. Full report
and transitions are archived in this batch's evidence directory. latest.json
now records this full measurement. The running-session description above is
historical; session 27032 has exited normally with runner status 1 for failures.
