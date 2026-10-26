# Class strictness and source-phase regression selection — 2026-10-26

16,906 / 16,944 variants pass: five gains and zero losses against the matching
projection of the latest complete report. The gains are static super Proxy
receiver propagation (two), module-source prototype identity (two), and class
heritage/computed-name strict execution (one). All 38 remaining failures are
existing decorator/auto-accessor parse failures. No timeouts or worker errors.

The worker uses the active class scope's strict flag for cached property
writes, computed property assignments, and increments. Core validation passed
387 library tests, formatting, and all-target Clippy with warnings denied.
The source snapshot, reports, and exact comparison are retained here.
These focused results do not replace the full score of 102,783 / 102,956.
