# Retained lifecycle/runtime measurements

The JSONL files in this directory contain historical baseline/modified samples
for public async calls, call instrumentation, and timer queues. The paired run
configuration is in [measurement-context.json](measurement-context.json), the
analysis in [comparison-summary.json](comparison-summary.json), and associated
gates in [required-gates.json](required-gates.json).

The capture identifies its baseline/modified revisions and binaries,
instrumentation hashes, platform/toolchains, and 21 alternating process pairs.
Read each workload's timing/allocation unit and comparison method before using
its ratio. An instrumented call boundary is different from whole-application
throughput or a core-engine comparison.

These original records are preserved. Use the
[current performance guide](../../performance.md) and
[baseline index](../../../bench/BASELINE.md) for new measurements and the later
rquickjs/optimization captures.
