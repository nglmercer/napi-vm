# Large JSON array follow-up — 2026-10-26

The pinned JSON, Array, Object, Reflect, and array-literal selection passes
**13,661 / 13,661** variants, gaining both large JSON-array variants with zero
losses against the latest full-report projection. The standard six-variant
timing slice passes. The latest complete score remains **102,839 / 102,956**
until a complete run confirms the focused gains.

JSON parsing can materialize arrays larger than the normal sparse-growth
threshold, with a four-million total parsed-array-element budget per document.
Already-materialized dense slots remain addressable above that threshold,
including through reads, writes, deletion, descriptors, and length truncation;
new distant indices retain sparse behavior. Source-path strings are produced
only when a reviver needs them. Differential boundary tests exercise these
operations in both execution paths.

Validation passed **416 core tests**, five worker tests, core all-target Clippy
with warnings denied, root all-target checking, formatting, whitespace, and a
locked release build. Exact source diff, worker identity, checks, reports,
projection, comparison, and timing are archived; source and worker hashes were
revalidated after the broad run.
