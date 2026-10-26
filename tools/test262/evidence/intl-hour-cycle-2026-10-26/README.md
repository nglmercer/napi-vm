# DateTimeFormat hour-cycle validation — 2026-10-26

The pinned DateTimeFormat, Locale, and NUL-option selection passes **838 / 838**
variants, with **two gains and zero losses** against the latest full-report
projection. Both staging NUL-option variants now pass. The standard six-variant
timing slice also passes. The latest complete score remains **102,839 / 102,956**
until another complete run confirms the focused gains.

DateTimeFormat now rejects hourCycle values other than h11, h12, h23, and h24,
including embedded NULs. Differential tests exercise validation, valid values,
and observable option-read order in AST and bytecode execution.

Validation passed **415 core tests**, five worker tests, all-target core Clippy
with warnings denied, root all-target checking, formatting, whitespace, and a
locked release build. Source and worker hashes were revalidated after the broad
run. Source identity, raw checks, focused reports, projection, comparison, and
timing evidence are archived.
