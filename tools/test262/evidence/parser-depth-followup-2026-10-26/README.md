# Parser stack growth — 2026-10-26

All 1,438 selected function-statement, function-expression, and call variants
pass: two gains and zero losses against the complete-report projection.
Guarded recursive expression/statement entry and static validation grow the
native stack, with a bounded depth limit of 128. Both 32 nested functions and
rejection beyond the limit are covered on 512 KiB embedding threads.

An intermediate version omitted validation stack growth and the small-stack
regression aborted. The final version passes that regression, all 384 core
library tests, formatting, and all-target Clippy with warnings denied.

The exact worker/source snapshot and reports are retained here. This focused
result does not replace the complete-corpus score of 102,783 / 102,956.
