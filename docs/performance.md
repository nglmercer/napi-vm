# Measure performance and reproduce comparisons

Choose a boundary before choosing a benchmark. Parsing source, creating a VM,
calling a retained function, copying a host value, polling a timer, and invoking
a child process are different operations. Keep their results separate.

## Capture a broad baseline

The Linux baseline runner requires Python 3, Cargo, Node, npm, installed
workspace dependencies, and the native build toolchain:

```sh
python3 bench/performance.py run --output artifacts/performance/my-baseline
python3 bench/performance.py compare \
  bench/baselines/2026-11-06/baseline.json \
  artifacts/performance/my-baseline/baseline.json
```

Use a new output directory. The runner builds before timing and explicitly
loads the produced addon. Its report retains input hashes, Git state, features,
toolchain/host settings, artifact digests, samples, medians, and median absolute
deviation (MAD). The [baseline index](../bench/BASELINE.md) describes retained
captures and which measurements predate the optimizations.

The baseline covers frontend/VM workloads, Node calls, language-service work,
Rust/Node scheduling, and trusted-process calls. It does not time every builtin,
WASM throughput, filesystem/network latency, native package, browser UI, or LSP
transport. Add an appropriate workload before claiming a gain there.

## Compare with rquickjs

The [isolated comparison crate](../bench/rquickjs/README.md) pins rquickjs
0.14.0 and a standalone lockfile. It compares bare `napi-vm-core` with rquickjs's
default `std` build and full context. Build and retain an immutable executable:

```sh
python3 bench/rquickjs.py build --output artifacts/performance/engine-build
python3 bench/rquickjs.py run --build artifacts/performance/engine-build \
  --output artifacts/performance/engine-run.json --samples 15 --iterations 10
python3 bench/rquickjs-compare.py engines artifacts/performance/engine-run.json
```

Both sources and expected numeric outcomes are shared. Every timed batch checks
its result, and samples alternate engine order. The ten workloads cover a
trivial startup-dominated source, arithmetic, recursion, arrays, strings,
closures, JSON, property reads/writes, and classes.

| Mode | Included in the timed boundary |
| --- | --- |
| `fresh_source` | Unique-comment source, VM/context construction and builtins, evaluation, destruction, napi-vm cycle collection |
| `warm_source` | Source evaluation on reused VMs; napi-vm's public source caches remain active, while rquickjs reparses the invocation |
| `warm_call` | Retained guest function invocation; napi-vm includes an empty job checkpoint |

Warm batches collect cycles outside the timer. Workloads are synchronous and
enqueue no jobs. Both engines use sloppy scripts. Their default builtin sets
differ: fresh napi-vm includes Intl and Temporal, so fresh results do not isolate
parser or dispatch cost. Recorded outer tiers do not prove nested callees use
the same tier.

## Read the retained results

The [before](../bench/baselines/performance-review-2026-11-06/rquickjs-before.json)
and [after](../bench/baselines/performance-review-2026-11-06/rquickjs-after.json)
reports each retain 60 engine/mode/workload rows. For warm calls, napi-vm's
after/rquickjs time ratios include 2.02× for arithmetic, 4.35× for property
reads, 9.01× for property updates, 24.86× for classes, and 29.88× for the array
chain. A ratio above one means napi-vm took longer. rquickjs remains faster
across the retained warm-call workloads; these are not whole-application scores.

Accepted optimization evidence is retained separately:

| Change and measured boundary | Repeated result | Evidence |
| --- | --- | --- |
| Borrow member keys and update existing own data properties without temporary descriptors | Fresh AST 3.09×, warm AST 5.20×, warm bytecode property calls 6.37× faster | [Criterion pairs](../bench/baselines/performance-review-2026-11-06/criterion-paired-summary.json), [engine pairs](../bench/baselines/performance-review-2026-11-06/rquickjs-interleaved.json) |
| Copy only the visible 16-byte native typed-array subview | 8-MiB backing 111.6×; 32-MiB backing 1,635× faster; small-backing control unchanged | [Subview pairs](../bench/baselines/performance-review-2026-11-06/typed-array-paired-summary.json) |
| Reuse the already located line start in LSP position conversion | 47.5–48.3% less conversion time | [LSP pairs](../bench/baselines/performance-review-2026-11-06/lsp-interleaved.json) |
| Store enqueue byte counts in trusted SDK event queues | Eight subscribers/64-KiB payload: about 7.8× faster in-process harness delivery | [Queue samples](../bench/baselines/performance-review-2026-11-06/plugin-event-queue.json) |

The property result combines two changes; it does not attribute percentages to
key borrowing alone. The subview result excludes backing-buffer construction.
The LSP result is isolated conversion, not request latency. The queue result
excludes child-process startup and TCP transport. Queue mutation regressions
also prove that listeners cannot corrupt dequeue byte accounting by changing a
shared payload.

## Reproduce a change safely

Freeze the before build, make the change, and build into a second new directory.
Alternate retained before/after executables on the same host, with the same
workload, features, sources, dependency lockfile, flags, and sample settings.
Use `bench/rquickjs-compare.py changes BEFORE.json AFTER.json` to validate and
compare the reports. Targeted tools are `bench/lsp-positions.py`,
`bench/performance/typed-array.cjs`, and
`tools/plugins/benchmark-event-queue.mjs`; inspect their arguments and retain the
source snapshots/build records they require.

The validator rejects missing rows, inconsistent statistics, and changed timed
harness inputs. Older before-build reports lack separate dependency/config
fingerprints: matching lockfile hashes identify dependencies, while configuration
identity remains explicitly unverified. Preserve these provenance warnings.

## Limits and next candidates

Measurements came from an AMD BC-250 desktop without CPU isolation. Samples are
batch averages; medians/MAD and repeated ratios are descriptive, not formal
significance tests. Do not claim small changes inside the noise as gains.

Profile before changing the remaining candidates: per-fetch client/runtime
creation, metadata-triggered document reparsing, Map/Set conversion identity,
temporary descriptors for missing named properties, lexer literal allocations,
and shape-transition retention under unique-key churn. Each needs a workload
and lifecycle/correctness checks for its own boundary.

Run relevant Rust/Node/WASM tests and the appropriate complete Test262 gate for
engine changes. The [validation record](../bench/baselines/performance-review-2026-11-06/validation.json)
and [follow-up audit](../bench/baselines/performance-review-2026-11-06/followup-audit.json)
link retained correctness evidence and source-equivalence checks.
