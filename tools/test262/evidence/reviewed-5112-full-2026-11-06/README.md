# Reviewed-profile conformance capture

This directory retains a complete Test262 result for the explicitly selected
`reviewed-5112` profile. It covers 53,615 files and 102,992 unique variants,
all passing, with zero skips, failures, timeouts, crashes, or harness errors.
The run used two workers and a ten-second deadline.

| Artifact | Purpose |
| --- | --- |
| [report.json.gz](report.json.gz) | Full per-variant outcomes and corpus/engine provenance |
| [summary.json](summary.json) | Grouped counts and profile metadata |
| [audit.json](audit.json) | Variant-set, overlay, and outcome checks |
| [comparison.json](comparison.json) | Comparison against the recorded baseline |

The worker SHA-256 is
`cedda1803516ed6d0b3f9e46b624e36d7dc05aec459ff4561d63dc4b7a8dcad9`.
The corpus base is `0b4e0812022b18236b5276d2804948fdd9f71603`, with the
identified PR #5112 overlay recorded in the report. The original corpus is
unchanged. This run does not claim that the contradictory unmodified corpus,
every host API, or every platform passed.

Reproduce the profile using the [runner guide](../../README.md). A later
optimized-engine run is retained separately in the
[performance validation directory](../../../../bench/baselines/performance-review-2026-11-06/).
Compare the actual worker digest and source inputs before applying either
historical report to a new build.
