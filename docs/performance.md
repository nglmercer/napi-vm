# Performance baseline and optimization workflow

Measure before changing a hot path. A useful result identifies the source,
binary, workload, unit, machine, build flags and noise. Conformance counts
and historical speed tables are separate evidence.

## Capture a baseline

The finite baseline runner is currently implemented for Linux. It requires
Python 3, Cargo, Node.js 22+, npm, the workspace dependencies, and the native
build tools needed by this repository. It builds artifacts before measuring.

```sh
npm ci --ignore-scripts
python3 bench/performance.py run --output artifacts/performance/my-baseline
```

The output directory must be new. The runner records source and build-input
fingerprints, Git revision and dirty state, toolchain versions, host details,
settings, artifact digests, samples and summaries in `baseline.json`. Raw logs,
Criterion samples and the explicitly loaded addon copy live beside it.
A dirty run is labelled as such; its Git commit alone does not identify its inputs.

The Node measurements load a copy of the newly built native library by its
explicit path. This avoids accidentally timing an old platform `.node` file.
Trusted-plugin measurements build and hash the emitted JavaScript, including
any local source edits. Missing or invalid timing fields fail collection.
The library uses the default NAPI feature set; Criterion additionally enables
the core test hooks through its dev-dependency. The Rust scheduler example
uses the runtime feature explicitly. Compare each backend with the same
feature configuration rather than treating these artifacts as one build.

## Coverage

| Layer | Measured surface |
| --- | --- |
| Frontend | Lexer and parser on a generated larger source |
| Fresh VM / AST | Arithmetic, recursion, arrays, strings, classes, closures and JSON |
| Warm VM and bytecode | Reused class methods, asserted execution tiers, properties, calls and cycle collection |
| Node binding | Fresh `runCode` calls through the selected addon |
| Language service | Completion and update/diagnostics through the selected addon |
| Scheduler | Rust turns and Node legacy/persistent async paths |
| Trusted processes | Node plugin startup and request/response payload sizes |

Fresh AST measurements include program declaration setup and collection after
discarding the temporary interpreter. Scheduler VM cases explicitly install
timer capabilities and assert callback counts. These corrected harnesses have
different boundaries from older historical tables; establish a new baseline
before making direct performance comparisons.

This is representative coverage for choosing optimization targets, not timing
of every code path. The baseline metadata lists omitted surfaces: WASM/Bun,
filesystem/network runtime operations, native trusted-process payloads and LSP
transport are not currently measured. The LSP executable is built and hashed
but that fingerprint is not an LSP latency result. Add a correctness-checked
workload before claiming a gain for an uncovered path.

## Compare the same workload

```sh
python3 bench/performance.py compare \
  bench/baselines/2026-11-06/baseline.json \
  artifacts/performance/my-baseline/baseline.json
```

Read metadata differences and added/removed workloads before interpreting
ratios. A ratio below one means less time per recorded unit. Units are part
of the contract: per-call latency, per-batch latency and medians of whole-run
summaries must not be treated as interchangeable measurements.

The summaries report median, median absolute deviation (MAD), minimum,
maximum and sample count. Criterion records here are per-iteration batch
averages. Scheduler and trusted-process entries contain independent repeated
whole-run summary medians. These are descriptive comparisons, not automatic
statistical-significance claims.

Use the same machine, toolchain, features and workload settings. Run on a quiet
host without competing builds or benchmarks. CPU affinity and machine isolation
are not enforced by the runner, so host load remains a limitation. Repeat and
interleave promising before/after measurements; small differences within run
noise are not a basis for a speed claim.

## Retain evidence

[The baseline index](../bench/BASELINE.md) points to the latest retained compact
snapshot and older measurements. Keep raw results in the run directory;
archive relevant samples and the compact report when accepting an optimization.
The original historical tables remain in versioned history with their dates,
source revisions and caveats.

Record the affected layer, mechanism, correctness checks, repeated samples,
and practical limitations with a performance change. Optimize a measured
bottleneck rather than changing unrelated code. Revert experiments that slow
the workload or add complexity without a demonstrated gain.

## Correctness gates

Engine changes need relevant Rust/Node/WASM regressions and an appropriate
complete [Test262 gate](../tools/test262/README.md). Host/runtime changes need
their own boundary tests. A local faster run does not replace these checks,
and a Test262 pass does not prove that the Node binding or plugin transport
became faster. See [development](development.md) and the
[implementation map](roadmap.md) for ownership and targeted checks.
