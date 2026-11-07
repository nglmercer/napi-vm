# Retained performance captures

Use [the measurement guide](../docs/performance.md) for current commands,
boundaries, interpretation, and limitations. Captures are dated observations;
raw samples and build provenance identify what was measured.

## Broad baseline: November 6, 2026

[baseline.json](baselines/2026-11-06/baseline.json) contains 41 workloads:
16 Criterion frontend/VM/plugin cases, four Node binding cases, two language
service cases, ten Rust scheduler cases, four Node async cases, and five trusted
Node plugin cases. It predates the subsequent optimizations.

The capture identifies a dirty source tree, toolchains/features, addon and
executable digests, per-workload units, samples, medians, and MAD. Some scheduler
and process-plugin entries contain medians of independent run summaries rather
than individual call latencies. Do not compare those units interchangeably.

## Optimization and rquickjs review

[performance-review-2026-11-06/](baselines/performance-review-2026-11-06/) retains:

| Record | Meaning |
| --- | --- |
| [rquickjs-before.json](baselines/performance-review-2026-11-06/rquickjs-before.json), [rquickjs-after.json](baselines/performance-review-2026-11-06/rquickjs-after.json) | Ten shared workloads, three modes, both engines |
| [rquickjs-interleaved.json](baselines/performance-review-2026-11-06/rquickjs-interleaved.json) | Repeated property/class comparisons |
| [criterion-paired-summary.json](baselines/performance-review-2026-11-06/criterion-paired-summary.json) | AST/bytecode optimization and controls |
| [typed-array-paired-summary.json](baselines/performance-review-2026-11-06/typed-array-paired-summary.json) | Native subview-copy optimization |
| [lsp-interleaved.json](baselines/performance-review-2026-11-06/lsp-interleaved.json) | Isolated position conversion |
| [plugin-event-queue.json](baselines/performance-review-2026-11-06/plugin-event-queue.json) | SDK event delivery, mutation regressions, and repeated samples |
| [validation.json](baselines/performance-review-2026-11-06/validation.json), [followup-audit.json](baselines/performance-review-2026-11-06/followup-audit.json) | Correctness, source equivalence, and artifact/log provenance |

Measurements continued into November 7; the directory date is the review's
start date. Ignored `artifacts/performance/` directories can hold local logs and
immutable binaries, but a clean checkout must rebuild rather than depend on
those paths. Historical source hashes in retained reports continue to identify
the original inputs even when documentation is subsequently rewritten.
