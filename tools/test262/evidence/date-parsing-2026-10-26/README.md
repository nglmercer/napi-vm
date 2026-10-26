# Legacy Date parsing follow-up — 2026-10-26

The pinned Date, DateTimeFormat, and staging Date selection passes
**1,718 / 1,734** variants, gaining **four passes and zero losses** against
the latest full-report projection. Both variants of the non-ISO and two-digit
year tests pass; sixteen existing DST-caching timeouts remain. The standard
six-variant timing slice passes. The latest complete score remains
**102,839 / 102,956** until another complete run confirms the focused gains.

Legacy slash dates support month/day/year and year-first forms with two-digit
year mapping, and written-month forms accept the tested year/day orders.
Space-separated timestamps accept compact legacy timezone offsets; ISO
`T`-separated timestamps validate field widths, ranges, and timezone forms.
Differential tests cover valid and invalid inputs and non-ASCII safety. The
engine continues to interpret local-time forms as UTC, its documented model.

Validation passed **419 core tests**, five worker tests, core all-target Clippy
with warnings denied, root all-target checking, formatting, whitespace, and a
locked release build. The debug worker timed out on both exhaustive two-digit
year variants; the optimized worker passes both with two jobs and five-second
deadlines. Source and worker hashes were revalidated after the broad run. The
source, check logs, debug and optimized reports, full projection, comparison,
and timing evidence are retained.
