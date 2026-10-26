# Function activation follow-up — 2026-10-26

Candidate v3 passes 8,573 / 8,573 semantic variants, gaining thirteen passes
with zero losses in that selection. The first candidate gained thirteen but
lost the immutable caller-descriptor invariant. Candidate v2 restored it and
passed 1,783 variants, then failed a separate data-property getter-alias probe.
Candidate v3 uses key-specific legacy accessor metadata and restores that
probe; the broader semantic selection includes the whole Object suite. All
predecessor sources, outcomes, comparisons, and probes are retained.

A six-variant timing check on v3 passes four and times out both toSpliced-dense
variants. Only script is a lost pass versus the latest full report; strict
was already a timeout. A separate diagnostic with
two workers and a 15-second deadline passes those variants at about 5.06
seconds each. This is not a five-second pass; the losses remain recorded.
Candidate v4 adds guarded own-array indexed reads. It passes 14,690 semantic
variants with thirteen gains and zero losses, but still times out both dense
toSpliced variants. GDB samples identify repeated binding resolution and
property-key construction as additional hot paths.

Candidate v5 adds guarded reads and calls for ordinary binding records,
retaining the full reference algorithm for with records, global accessors,
and inherited globals. All 395 core tests, five worker tests, Clippy, the root
all-target check, formatting, and whitespace checks pass. Its six-variant
timing slice passes twice with two workers and five-second deadlines: one
gain and zero losses against the latest full report. The broader semantic selection passes **16,279 / 16,279**, with thirteen
gains and zero losses against the exact complete-report projection. The
138-variant selection covering all previously nonpassing files passes 21,
with **20 gains and zero losses**: thirteen activation cases, two ArrayBuffer
coercion cases, four URI-decode cases, and strict toSpliced. The remaining
selection has 65 failures and 52 timeouts; see v5-remaining.json. Exact source and worker hashes are
recorded in source-v5.json and source-v5.diff.

A subsequent [complete run](../full-activation-followup-2026-10-26/) confirms
102,839 / 102,956, with 20 gains and zero losses against the 102,819 baseline.
The focused comparisons here retain their original 102,819 baseline.
