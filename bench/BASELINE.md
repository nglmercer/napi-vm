# Retained performance captures

Use [the measurement guide](../docs/performance.md) for current commands,
boundaries, interpretation, and limitations. Captures are dated observations;
raw samples and build provenance identify what was measured.

## Filesystem/loaders and eight-group capture: November 7, 2026

[io-traversal-2026-11-07/validation.json](baselines/io-traversal-2026-11-07/validation.json)
retains immutable build metadata, eight captures, four pair comparisons,
production/test source snapshots, harnesses and validation logs. Descriptor
borrowing/component iteration is measured separately from the final bounded
read-buffer reservation. All captures use 15 samples × 50 iterations × three
processes; final order is candidate/baseline/baseline/candidate. There are 34
latency rows and 23 allocation rows per capture, with separate executables.

Flat 4-KiB/64-KiB/4-MiB reads fall from 12/16/22 allocation requests to three;
4-MiB cumulative requested bytes fall from about 16 MiB to 4 MiB. Small-read
latency improves in both final pairs, while control drift prevents attributing
all timing shifts to the patch, particularly for large reads. Full raw controls
and the noisier initial descriptor pair are retained. Counts do not describe
live heap/RSS, Node allocations, disk durability, or a universal engine score.
The [guide](../docs/performance.md#local-filesystem-and-source-loaders) explains
policy, page-cache, source-loading and TypeScript boundaries.

[full-suite-io-2026-11-07/suite.json](baselines/full-suite-io-2026-11-07/suite.json)
retains all eight current groups: 390 engine, 398 public API, 81 host, eight
memory, four HTTP, 104 async, 243 Intl/Temporal and 57 I/O rows. Each has its own
units and timed boundary. The operation inventory preserves remaining coverage.
All 43 broader runtime-feature tests, 34 Python validator tests and runtime
Clippy pass. Existing core/worker gate inputs are unchanged by this runtime fix.

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

## Expanded operation smoke capture: November 7, 2026

[operations-2026-11-07/rquickjs-expanded-smoke.json](baselines/operations-2026-11-07/rquickjs-expanded-smoke.json)
retains 25 workloads × three modes × two engines (150 checked timing rows),
with five samples and three iterations per sample.
[engine-ratios.json](baselines/operations-2026-11-07/engine-ratios.json) is generated
by the report validator. This capture verifies the expanded suite runs in both
engines. A separate release benchmark was compiling concurrently, so use it for
profiling priorities only, and repeat on a quiet host before claiming speedups.
The prior ten-workload reports remain unchanged.

The same directory retains [metadata-paired.json](baselines/operations-2026-11-07/metadata-paired.json)
for two interleaved before/after metadata refresh pairs, raw Criterion batch data,
logs, [provenance](baselines/operations-2026-11-07/metadata-provenance.json), and
[language-test output](baselines/operations-2026-11-07/language-tests.log). These
measure direct service APIs rather than LSP snapshot or transport latency.

## Array result allocation: November 7, 2026

[array-results-2026-11-07/](baselines/array-results-2026-11-07/)
retains two interleaved array-chain pairs (15 samples, 10 iterations), full
25-workload before/after smoke captures, compatible-report comparisons, focused
test logs, worker provenance, and [gate status](baselines/array-results-2026-11-07/validation-status.json).
The complete reviewed Test262 run passed 102,992/102,992 variants with an exact
variant-set match and zero outcome changes from the prior full gate. Its
compressed report, summary and comparison are retained in that directory.

## Native HTTP transport: November 7, 2026

[http-2026-11-07/](baselines/http-2026-11-07/) retains four interleaved before/after
pairs for four loopback HTTP cases, 15 samples and 10 iterations each. Both
builds use the same final harness, dependency/configuration inputs, and explicit
optimized profile. Only `runtime/network.rs` differs. GET controls vary and
redirect timings are inconclusive; repeated POST gains accompany removal of
full upload-body copies. See the [validation record](baselines/http-2026-11-07/validation.json)
for timing boundaries, concurrent host load, test counts, and limits.

## Process memory and GC: November 7, 2026

[memory-2026-11-07/](baselines/memory-2026-11-07/) retains three independent
process runs per engine/workload, with normal (8 × 2,000 objects) and stress
(32 × 4,000 objects) captures. Baseline, ready, allocated, collected, released,
and dropped phases have raw RSS/VmHWM and engine-specific counters. Timing
smoke checks verify the extended build still supports the existing comparison.
[Validation](baselines/memory-2026-11-07/validation.json) records scope and limits;
RSS and cumulative shape minting must not be treated as identical live-object
metrics. The unique-key workload deliberately performs repeated fixed-key reads
to exercise shape caching, rather than only constructing cold objects.

## Bounded shape-cache candidate: November 7, 2026

[shape-cache-2026-11-07/](baselines/shape-cache-2026-11-07/) retains the new
memory after-captures and comparisons against the prior memory baseline,
two interleaved full timing pairs, two longer focused property-read pairs,
core/Node tests, Clippy output and immutable worker provenance.
The [validation status](baselines/shape-cache-2026-11-07/validation.json)
records the full reviewed gates for both the original and selected headers,
plus the earlier property-read latency concern and layout controls.
The memory benefit is a measured unique-key stress result, not a universal
memory or speed claim.

## Unified all-group run: November 7, 2026

[full-suite-2026-11-07/suite.json](baselines/full-suite-2026-11-07/suite.json)
records a completed all-group run with verified child report digests: 150 engine
timing rows, eight memory rows, four HTTP rows, and 47 host rows. `bench/suite.py`
is the shared entry point. All-group completion covers the available finite
drivers; remaining operation-family coverage is listed in the measurement guide.

The bounded shape policy's original header also passed the full reviewed
102,992-variant gate. Its compressed report is retained in the shape-cache
capture directory. [Layout experiments](baselines/shape-cache-2026-11-07/layouts/selection.json)
record the later hot-header selection. Its own full gate also passed all
102,992 variants with an identical variant set and zero outcome changes;
immutable worker/source checks and the compressed report are retained.

## Outbound collections: November 7, 2026

[collections-2026-11-07/validation.json](baselines/collections-2026-11-07/validation.json)
retains three paired captures, explicit old/new identity observations, addon and
source hashes, build-profile limits, 50 passing Node tests, Clippy and 14 passing
benchmark-validator tests. The fix preserves outbound Map/Set aliases and cycles
and avoids repeated snapshots and temporary pair arrays. Large alias speedups
include correcting the old duplication behavior. `bench/performance.py` now
includes these four cases; the earlier full-suite capture predates the fix.

## Expanded all-group run: November 7, 2026

[full-suite-expanded-2026-11-07/suite.json](baselines/full-suite-expanded-2026-11-07/suite.json)
retains the current collection fix and 26 new persistent Node API cases:
**77 host rows**, 150 engine timing rows, eight memory rows and four HTTP rows.
All four groups and validators completed; report digests and the stable host
build-input fingerprint were verified. Raw public API/collection reports and
per-layer build/measurement logs are included. Sixteen benchmark-validator
tests pass. The initial API baseline measures finite cases and does not claim
that every public method or argument shape is covered.

## Indexed guest collection candidate: November 7, 2026

[collection-index-2026-11-07/validation.json](baselines/collection-index-2026-11-07/validation.json)
retains three before/after pairs for 24 collection-scaling cases, including one
full 49-workload pair, owner-memory probes, 913 passing core tests, six root
tests, 50 Node tests, Clippy and 18 benchmark-validator tests. Large collection
gains and index memory costs are measured. The new complete Test262 gate passed
all 102,992 variants, with matching inputs and an exact prior variant set.
Pinned and unpinned controls are retained; property-read latency neutrality
remains unproven. A fresh WASM package passes 19 tests with no skips, including
indexed collections; its input/package hashes and logs are retained.
The reference-engine audit
records a sequential-numeric-key collision case in the exact locked QuickJS
source; string/object controls are retained.

[full-suite-indexed-2026-11-07/suite.json](baselines/full-suite-indexed-2026-11-07/suite.json)
retains the completed current all-group run: 294 engine rows, 81 host rows,
eight bare-engine memory rows and four HTTP rows. Four host rows measure
owner RSS in bytes; report digests and the stable host build-input fingerprint
were verified. This does not convert finite coverage into an all-API claim.


## Public WASM boundary review: November 7, 2026

[platforms-2026-11-07/validation.json](baselines/platforms-2026-11-07/validation.json)
retains two interleaved public API pairs: 49 fixed guest fixtures, three processes,
seven samples and three iterations per batch. Both pairs have 302 aggregate
rows and all raw child samples. Only `src/wasm.rs` differs in compiler inputs;
native addon bytes match. Static helpers halve imported calls for a tiny guest
call (22→11), preserve independent result/log records and avoid inherited host
setters. Tiny-call latency ratios are 0.739/0.928; variation and moving controls
prevent a stable percentage gain claim. Instrumented counts are separate from
latency measurements. Twenty WASM tests pass, including from an extracted npm
archive; packaging tests, benchmark validators, WASM Clippy and the Vite build
pass. The wrapper includes linked JS snippets in the npm whitelist. Source
snapshots, build/staging provenance, pack file lists and validation logs are
retained. No core-worker input changed from the prior 102,992-variant gate.

## Five-group, 65-workload capture: November 7, 2026

[full-suite-platforms-2026-11-07/suite.json](baselines/full-suite-platforms-2026-11-07/suite.json)
retains the completed five-group suite: **390 bare-engine rows, 398 public
native/WASM/Node API rows, 81 host rows, eight memory rows and four HTTP rows**.
Sixteen new shared guest cases cover generators/iterators, proxies/Reflect,
Date, weak collections and more string/numeric/buffer operations. All 65 guest
sources and expected outcomes match across the embedded and public layers.
[Validation](baselines/full-suite-platforms-2026-11-07/validation.json) verifies
report digests and stable host build inputs; build records, logs, raw reports
and per-fixture ratios are retained. The earlier four-group captures describe
the driver inventory at their capture dates. Async comparisons, dedicated
Intl/Temporal timing and broader I/O remain open; no universal runtime score or
all-operation completion is claimed.


## Promise jobs and pending-cell candidate: November 7, 2026

[async-jobs-2026-11-07/validation.json](baselines/async-jobs-2026-11-07/validation.json)
retains 13 shared promise/async fixtures and two before/candidate pairs, each
with 104 rows, three processes, 15 samples and 10 iterations. Enqueue,
checkpoint, completed source and retained-call phases are separate. Every
invocation checks one numeric completion and no rejection. The candidate reuses
native species result promises while preserving custom-species fallback.
Pending fan-out has exactly one fewer tracked cell per native registration at
32/512/4,096 entries; the custom control is unchanged. Three process samples
per case/build retain RSS and release phases separately from latency. Repeated
fan-out/all-promises checkpoint gains accompany smaller, variable complete-call
changes; async-generator enqueue is slower in both pairs. These concerns are
retained. Core/runtime tests, 98 Node tests, 20 WASM tests and benchmark validators
pass. The candidate's own complete reviewed Test262 gate passed all 102,992
variants, with the exact prior variant set, unchanged configuration and current
worker/source hashes verified. Async-generator enqueue neutrality remains
unproven; no universal speedup is claimed.

[full-suite-async-2026-11-07/suite.json](baselines/full-suite-async-2026-11-07/suite.json)
retains all six completed groups: 390 embedded-engine, 398 public API, 81 host,
eight memory, four HTTP and 104 async rows. Reports and build records have
verified digests, matching shared synchronous sources and stable host build
fingerprints. `bench/suite.py` now includes the async group by default; selecting
the previous five groups records a partial selection. These finite cases do
not complete the broader operation-family audit.


## Intl/Temporal and seven-group capture: November 7, 2026

[international-2026-11-07/validation.json](baselines/international-2026-11-07/validation.json)
retains 23 fixed-input fixtures across three modes and three backends. The
three-process baseline uses 15 samples and 10 iterations, with 135 measured
rows, 69 unavailable-capability rows and three DisplayNames output-mismatch
rows. Non-measured rows have no timings or ratios. Complete-output checks for
parts/segmentation and representative Temporal results match in nine cases;
DisplayNames returns `US` versus Node's `United States`, and remains excluded.
Sources, compiler/helper/artifact fingerprints, process samples, preflight
results, Clippy and 28 passing validator tests are retained. The installed Node
is v26.13.0 (V8 14.6, ICU 78.3); configured rquickjs 0.14.0 lacks the required
Intl/Temporal constructors. These facts describe the captured configurations.

[full-suite-international-2026-11-07/suite.json](baselines/full-suite-international-2026-11-07/suite.json)
retains all seven available groups: 390 embedded-engine, 398 public API, 81
host, eight memory, four HTTP, 104 async and 207 international matrix rows.
Report digests, shared synchronous sources and stable host build fingerprints
are verified. Capability and semantic rows are distinct from latency samples.
The preceding promise candidate's own complete reviewed gate also passed all
102,992 variants; its retained source checks verify exact configuration/variant
identity. Getter slot cloning is a newly identified optimization candidate;
this capture does not attribute the whole Intl gap to that copying. The broader
operation-family audit remains active.


## Intl getter copy removal: November 7, 2026

[intl-getters-2026-11-07/validation.json](baselines/intl-getters-2026-11-07/validation.json)
retains two repeated before/candidate pairs for 27 shared fixtures, three
processes, 15 samples and 10 iterations. Brand-only checks avoid deep slot
copies in NumberFormat/DateTimeFormat unwrapping and all three bound getters.
Cached identity, receiver checks, observable legacy/proxy reads and liveness
are preserved; formatting operations retain their consumed slot snapshots.
Getter times fall 22–26% (NumberFormat), 18–21% (DateTimeFormat), and 6–10%
(Collator); the complete number-format fixture falls 10–14%. Parts/date-format
results are inconclusive; complete collation has smaller gains. All raw
controls and observed movement remain retained, without a global speedup claim.

Checks pass 912 core tests, 45 targeted Intl tests, 99 Node tests, 21 WASM tests
and 28 benchmark validators. The complete Intl402 selection passes all 6,764
variants with exact prior-set/outcome equality; source checks verify current
worker inputs. This is an explicit scoped gate. The preceding whole-corpus
102,992-variant report belongs to the pre-getter-change worker. Sources and
before-addon/Node reference checks for forged legacy wrappers are retained.

[full-suite-intl-getters-2026-11-07/suite.json](baselines/full-suite-intl-getters-2026-11-07/suite.json)
retains all seven groups after the change: 390 embedded-engine, 398 public API,
81 host, eight memory, four HTTP, 104 async and 243 Intl matrix rows. The Intl
matrix has 159 measured, 81 unsupported and three non-equivalent DisplayNames
rows. Earlier 23-case captures are historical; the optimization pairs use the
same 27-case fixtures/harness. Report digests, matching shared synchronous
sources and stable host build fingerprints are verified.
