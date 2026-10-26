# Super references and typed-array integrity — 2026-10-26

The 761-variant selection passes 758, gaining eight passes with zero losses
against the matching projection of the latest complete report. Six gains are
computed super references whose key conversion mutates the home prototype.
Two gains are Reflect.preventExtensions on variable-length typed arrays.
The three remaining variants were already failing: strictExecution.js script
and both superPropProxies.js variants. No timeouts, skips, or worker errors.

The full-corpus score remains 102,783 / 102,956. This focused selection uses
worker 805b1972…, two workers, and five-second deadlines. Reports, comparison,
source identity, and source.diff retain the measured snapshot. Core validation
passed 383 library tests, formatting, and all-target Clippy with warnings denied.
