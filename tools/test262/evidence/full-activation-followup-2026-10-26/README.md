# Full activation follow-up — 2026-10-26

Complete pinned run: **102,839 / 102,956 (99.886359%)**, with 65 failures,
52 timeouts, zero skips, crashes, or harness errors. It gains **20 passes
with zero losses** against worker ef2a45bd… (102,819 passes).

The 117 residual variants comprise 42 parse failures, 23 runtime failures,
and 52 timeouts. Every outcome is in full.json.gz; every transition is in
comparison-vs-ef2a.json.gz. Triage and the residual inventory are retained.
The exact uncommitted source snapshot, worker hash, check results, and run
command are in source.json and source.diff. Source and binary hashes were
revalidated after completion. The run used the complete pinned corpus, two
workers, five-second deadlines, no skips/exclusions, and no resumed outcomes.

Gains cover thirteen legacy caller/arguments variants, two ArrayBuffer
sliceToImmutable coercion variants, four URI-decode variants, and strict
toSpliced-dense. All previous passes, including script toSpliced-dense,
BMP casing, and regress-610026, remain passing. The six-variant timing slice
also passed twice before this full run. Focused activation evidence preserves
the rejected candidates and their measured regressions.

Core validation: 395 library tests, five worker tests, all-target Clippy with
warnings denied, root all-target check, formatting, and diff whitespace.
The goal is incomplete: all 102,956 variants must pass on clean committed
source, with the same corpus and execution configuration.
