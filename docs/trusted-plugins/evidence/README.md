# Trusted-plugin evidence index

These files are retained outputs of earlier validation and measurement runs.
They describe their recorded artifacts/configurations; they are not live CI
status or proof that a newly built package passed.

| Record | Boundary |
| --- | --- |
| [interop-results.json](interop-results.json) | Host/plugin interoperability scenarios |
| [package-results.json](package-results.json) | Packaged/relocated artifact scenarios |
| [compatibility-summary.json](compatibility-summary.json) | Recorded runtime/target availability |
| [legacy-validation-report.json](legacy-validation-report.json) | Earlier plugin validation capture |
| [benchmark-node.json](benchmark-node.json), [benchmark-bun.json](benchmark-bun.json) | Process-plugin measurements under selected JS runtimes |
| [benchmark-rust-host.json](benchmark-rust-host.json) | Rust-host measurements |

Inspect each JSON record's commands, versions, units, samples, and limitations
before comparing results. Buildable, distributed, and actually tested targets
are different statuses. The SDK event-queue optimization has separate
[repeated raw evidence](../../../bench/baselines/performance-review-2026-11-06/plugin-event-queue.json).

Reproduce current checks using [testing](../testing.md), and use
[performance methodology](../../performance.md) before accepting a speed claim.
