# Large string limit follow-up — 2026-10-26

The pinned String, RegExp prototype, template, string literal, and large Unicode
escape selection passes **3,663 / 3,663** variants, gaining **six passes and
zero losses** against the latest full-report projection. Both variants of each
of three Unicode escape tests now pass, including their 16 MiB-plus source and
pattern inputs. The standard six-variant timing slice passes. The latest
complete score remains **102,839 / 102,956** until a full run confirms the
gains.

The bounded guest string limit rises from 16 Mi to 64 Mi units, allowing these
inputs and their enclosing syntax while continuing to bound individual string
allocations. Validation passed **415 core tests**, five worker tests, core
all-target Clippy with warnings denied, root all-target checking, formatting,
whitespace, and a locked release build. Exact source diff, worker identity,
checks, focused reports, projection, comparison, and timing are archived; source
and worker hashes were revalidated after the broad run.
