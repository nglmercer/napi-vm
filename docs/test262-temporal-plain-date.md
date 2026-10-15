# Temporal PlainDate continuation

Adds privately branded Temporal.PlainDate instances, construction, cloning,
calendar-aware property-bag conversion, ISO/calendar string parsing, field
accessors, with/withCalendar, arithmetic, differences, equality, comparison,
and serialization. Calendar operations use temporal_rs. Guest field, option,
and primitive conversion operations remain observable. Duration relativeTo
recognizes PlainDate slots without calling guest property getters.

The pinned built-ins/Temporal/PlainDate and intl402/Temporal/PlainDate selection
passes **2,034 / 2,290 variants (88.8210%)**. The preceding complete report
had 16 passes in this selection: this is **2,018 gained passes, zero losses**.
There are no skips, timeouts, crashes, or harness errors. Evidence is archived
in tools/test262/evidence/temporal-plain-date, including the exact source patch
on source-base.txt and compressed full individual focused outcomes.

Measured debug worker SHA-256:
761620038bf7729590f7260f0e91c3620ff8be5e25f7478ecdae3507aff72a56.
Corpus: 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81, four jobs, five-second
variant timeout. No runner assertions, limits, denominator, or corpus files
were modified.

Validation passes: 75 Rust tests across temporal, json_reviver,
builtin_quick_wins, utf16, and test262_worker. Three new PlainDate regressions
independently run AST and verified bytecode. They cover calendar arithmetic,
field/coercion order, bounds, brands, calendar strings, and non-era calendars.
All 12 Python Test262 tooling tests, strict Clippy for library/worker/Temporal
tests, formatting and diff whitespace checks pass.

Remaining failures largely require absent PlainDateTime, PlainYearMonth,
PlainMonthDay, ZonedDateTime conversions and locale-aware formatting. This
batch does not claim those operations are implemented.

The full Instant-only debug run remains active in exec session 11633; its
frozen worker does not include PlainDate. A release build including PlainDate
was started in exec session 41856 for the next complete measurement. Check
live sessions/processes before restarting work. Latest completed full result
remains 79,243 / 102,956 (76.9678%), before raw JSON/Instant/PlainDate. Focused
gains must not be added to that total and called a measured full result.
The overall goal remains at least 92,661 / 102,956 (90%) and is not complete.

Reproduce the focused run:

```sh
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/debug/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 \
  --path built-ins/Temporal/PlainDate --path intl402/Temporal/PlainDate \
  --output artifacts/test262/temporal-plain-date-focused-final.json
```
