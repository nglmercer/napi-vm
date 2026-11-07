# Performance review and rquickjs comparison

This review follows the engine through parsing, AST and bytecode execution,
values, collection, embedding boundaries, runtime capabilities, editor tooling
and trusted plugins. The aim is to find avoidable work on real paths and keep
repeatable evidence for changes. This is an architectural performance review
with detailed inspection of the changed hot paths, not a line-by-line audit of
every builtin, generated data table, example and test. Repository-wide review
is not exhaustive benchmark coverage: filesystem/network latency, native addons, WASM throughput,
UI rendering and every builtin need their own workloads before speed claims.

## Review coverage

| Area | Inspection and decision |
| --- | --- |
| Lexer/parser and caches | Exact-source parse caches already bound entry count and source bytes; cold-source comparison must bypass them deliberately. Character-at-a-time temporary strings in literal/template scanning remain a candidate for a separately measured frontend change. |
| AST/bytecode, environments and calls | Inspect member-key conversion, property descriptors, verified slots, inline caches, activation and environment reuse. Preserve fallback and receiver semantics; benchmark AST and bytecode explicitly. |
| Values, strings, heap and builtins | Preserve UTF-16 keys and surrogate escaping. Retain cycle collection in fresh interpreter measurements. Guest case conversion already uses ICU with a single-scalar fast path; the allocation-heavy public `JsString::map_case` helper has no in-tree callers. Shape transition retention and dictionary hashing remain candidates; no unmeasured gain is claimed. |
| NAPI/host marshalling and WASM adapters | Inspect copy boundaries, typed-array offsets and shared-memory rejection. Measure small views over large buffers separately from guest allocation. Core changes are shared with WASM; a rebuilt WASM regression suite is required. |
| Optional runtime and schedulers | Existing baseline covers timer batches and legacy/persistent async calls. Permission checks and host I/O stay in the measured boundary. I/O operations cannot be represented by a core-engine comparison. |
| Language service, LSP and live sessions | Inspect document updates, position conversions, source rescans and transport. Isolated position conversion is different from whole-request latency. |
| Guest and trusted plugin hosts/SDKs | Existing direct-call versus wrapper-evaluation workloads and trusted-process startup/payload workloads cover different integration choices. Keep protocol validation, framing, timeouts and limits intact. |
| Browser playground and Zed extension | Playground already debounces diagnostics/completion and memoizes highlighting/tree derivation. It still persists the workspace and renders full documents on edits; profile large workspaces before changing these paths. Zed caches downloaded executables and prioritizes a configured/local binary. Neither is an rquickjs engine-throughput measurement. |
| Build, CI, generators, validation and benchmark tools | Build before measuring and identify the exact addon. Offline generators materialize archive members but are outside engine throughput. Test262 runs one isolated process per variant and preserves corpus/binary provenance. Legacy Node benchmarks now fail on wrong results, allow explicit addon selection, and use the Rust suite's fib(10) workload. |

## Follow-up targets

These are code-review findings, not measured speedups. They are retained so
future work can choose a workload and validate the relevant lifecycle first.

| Path | Avoidable work to investigate | Required measurement/check |
| --- | --- | --- |
| `crates/napi-vm-runtime/src/runtime/network.rs` | A new reqwest client and Tokio runtime per fetch prevent connection-pool reuse. | Repeated local HTTP requests, permission isolation, cancellation and runtime teardown. |
| `src/lsp/server.rs` | Rebuilding the service after a runtime snapshot reparses open documents. | Multiple documents and frequent metadata changes, checking invalidation and diagnostics. |
| `runtime/session.cjs` | Publishing metadata serializes the complete snapshot. | Subscriber count and payload-size latency; retain publication coalescing. |
| `src/bindings/marshal.rs` | Map/Set entry discovery is repeated before construction. Cyclic Maps currently hit the depth guard, and repeated collection references lose identity. | Large collections plus cycles and identity checks at the host boundary. |
| `crates/napi-vm-core/src/interpreter/call.rs` | Creating missing named properties still uses temporary descriptors; the measured class workload mostly takes this path. | Profile creation/prototype interception before extending the narrower numeric creation fast path. |
| `crates/napi-vm-core/src/lexer.rs` | Literal/template scanning constructs temporary strings per character. | Frontend workloads including lone surrogates and raw templates. |
| `crates/napi-vm-core/src/shape.rs` | Small-layout transitions retain strong child references. The 128-key depth threshold does not bound the number of distinct branches retained for an owner's lifetime. | Unique-key churn, retained memory, canonical sharing and inline-cache correctness before introducing bounded or weak transitions. |

## Reproducible comparison

The isolated comparison crate in [bench/rquickjs](../bench/rquickjs/Cargo.toml)
pins rquickjs and its lockfile without adding a QuickJS dependency to the
engine or changing the root dependency graph. See the
[baseline workflow](performance.md) for the existing broader layer measurements.

The retained reports below contain the comparison samples and provenance.

The comparison uses rquickjs **0.14.0**, with its locked `rquickjs-sys`
QuickJS-NG backend, and `napi-vm-core` **0.1.5** without NAPI, WASM or runtime
features. Both use release optimization, thin LTO and one Rust codegen unit;
QuickJS retains its default C build options. This compares the default Rust
embedding APIs rather than Node versus C or V8's native JIT loops.

```sh
python3 bench/rquickjs.py build --output artifacts/performance/quickjs-build
python3 bench/rquickjs.py run --build artifacts/performance/quickjs-build \
  --output artifacts/performance/quickjs-run.json --samples 15 --iterations 10
```

The build output directory must be new. The wrapper preserves an immutable
executable with input hashes and refuses an executable whose digest changes.
Each workload checks its expected numeric result and alternates engine order.
Samples are batch averages; reports retain the samples, median and MAD.
Both engines evaluate sloppy scripts. Sources and workload hashes are shared.

- **Fresh source** includes VM/runtime/context construction, builtins, source
  evaluation, destruction and explicit napi-vm cycle collection. Unique comments
  bypass the process-wide exact-source parser cache. The trivial workload is
  startup-dominated, not a pure constructor benchmark.
- **Warm source** reuses the VM and calls each source-evaluation API. napi-vm's
  source/prepared-program caches are part of this API measurement; rquickjs
  evaluates the invocation source afresh.
- **Warm call** retains the guest function and omits parsing, lookup and VM
  creation. napi-vm includes its empty job checkpoint; rquickjs's synchronous
  function call does not drain jobs. Workloads do not enqueue jobs.

The report records top-level and guest-function tiers. Nested callees may have
independent fallback behavior. Cycle collection between warm batches is outside
both timed regions. Do not combine the three boundaries into one engine score.
Desktop/background activity and lack of CPU isolation limit small comparisons.
The engines install different builtin sets: napi-vm's bare core includes Intl
and Temporal, which are not part of this default QuickJS context.
Fresh timings include that difference; they cannot isolate parser cost or
execution dispatch. The selected warm workloads use shared language features.

## Accepted changes

### LSP position conversion

`position_to_offset` now reuses the line start it already found instead of
rescanning the complete document prefix inside `line_text`. UTF-16 column
conversion, CRLF trimming, surrogate-pair clamping and out-of-range behavior
are unchanged and covered by the module tests.

The [interleaved report](../bench/baselines/performance-review-2026-11-06/lsp-interleaved.json)
uses one executable containing the exact baseline and candidate source modules,
alternates their order each sample, and records the linked URL dependency and
source/binary hashes. All 27 combined module tests passed. Results are isolated
conversions near the last line, not LSP request latency.

| Document | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| 200 ASCII lines | 4.761 µs | 2.464 µs | 48.3% |
| 20,000 ASCII lines | 474.532 µs | 245.510 µs | 48.3% |
| 4,000 Unicode/CRLF lines | 119.834 µs | 62.873 µs | 47.5% |

```sh
python3 bench/lsp-positions.py \
  --baseline artifacts/performance/lsp-before/text.rs \
  --output artifacts/performance/lsp-interleaved \
  --samples 21 --iterations 500
```

To reproduce a new comparison, save the pre-change `src/lsp/text.rs` and pass
that file as `--baseline`; the old ignored artifact directory is local evidence,
not a prerequisite supplied by a clean checkout.

### Property keys and existing own data writes

The interpreter now borrows encoded keys with `JsString::to_key_cow` where
callers only need `&str`, and reuses that conversion across a member update.
Lossless UTF-16 keys still allocate their escaped representation when required;
lone surrogates, replacement characters and the internal escape marker cannot
alias. Public string keys beginning with `#` retain their existing behavior.

For a same-object target and receiver, writing an existing writable ordinary
data property now reuses the existing direct-storage path. This avoids creating
temporary descriptor objects and invalidating a shape whose layout did not
change. Namespace/boxed objects, accessors, proxies, distinct receivers and
read-only properties retain the general path. Creating a missing named property
still uses complete prototype and receiver descriptor handling. This combines
two changes; timing gains must not be attributed to key borrowing alone.

### NAPI typed-array subviews

The native marshaller reads the visible byte range before copying it into a
host-owned ArrayBuffer, instead of cloning the entire guest backing buffer.
Offsets, element sizes, empty views, resizing, detachment and copy isolation are
covered by native regression cases. Shared backing stores remain rejected.

```sh
NAPI_BENCH_BINDING=/absolute/path/to/fresh-addon.node \
  node bench/performance/typed-array.cjs > typed-array.json
```

The workload uses `Vm.callFunction` (including its named function lookup) to
call a previously defined guest function returning the same 16-byte view over
64-byte, 8-MiB and 32-MiB backings. VM setup and correctness
assertions are outside the timed calls. Each report identifies the exact addon
and script; it does not measure backing-buffer construction or guest loops.

### Trusted-plugin SDK event queues

Production and harness subscriptions now retain the serialized byte count with
each queued event and subtract that count before invoking the listener. The
internal queue entry is shared across subscribers, avoiding a new allocation
per subscriber. Listeners still receive the same validated payload reference;
the change introduces no payload cloning or freezing.

Previously, dequeueing serialized the payload again for every subscriber.
Another listener could make it cyclic and prevent the next subscriber from
running, or shrink it and leave stale byte counts that eventually caused a
false `OVERLOADED` error on an empty queue. All four mutation regressions
(cycle and shrink, through both harness and real child process) fail on the
saved, freshly emitted pre-change SDK and pass on the new SDK. Four additional
regressions preserve count/byte limits and listener/error isolation. Clearing
a queue also resets its byte count on unsubscribe, overflow, error and shutdown.

The [event-queue report](../bench/baselines/performance-review-2026-11-06/plugin-event-queue.json)
retains three runs of 21 alternating before/after batches, each delivering 64
events to one or eight subscribers. Each batch checks listener counts, event
indices and payload contents. The timed boundary includes public
`createHarness.emitHostEvent` validation, enqueueing, Promise scheduling and all
listener counters; setup, shutdown and assertions stay outside. Both saved SDKs
use the same protocol module in the same Node process. Source/build metadata,
dependency fingerprints, raw samples and MAD are retained in the report.

| Text payload, eight subscribers | Before (µs/event) | After (µs/event) | After/before range across runs |
| --- | ---: | ---: | ---: |
| 64 bytes | 12.036 | 6.694 | 0.541–0.570 |
| 4 KiB | 31.029 | 7.677 | 0.223–0.249 |
| 64 KiB | 337.369 | 43.072 | 0.125–0.129 |

Times are medians of three run medians, per event delivered to **all eight**
subscribers. Single-subscriber cases are also retained and have smaller gains.
These measurements cover the in-process harness, without TCP transport or
child-process startup. They are independent of the rquickjs engine comparison.
No CPU isolation or forced GC is used; the report makes no significance claim.

```sh
node tools/plugins/benchmark-event-queue.mjs \
  --before artifacts/performance/plugin-event-queue-2026-11-07/before/index.js \
  --after artifacts/performance/plugin-event-queue-2026-11-07/after/index.js \
  --samples 21 --events 64 --output artifacts/performance/event-queue-new.json
```

The runner needs Node.js 22.14+ and a new output path. Save freshly emitted SDK
files with their source snapshots and `build.json` records before timing; the
retained report embeds the exact build commands and hashes. The ignored local
snapshot directories above are evidence from this run, not files supplied by a
clean checkout.

## Repeated optimization measurements

The [Criterion paired summary](../bench/baselines/performance-review-2026-11-06/criterion-paired-summary.json)
retains three alternating adjacent before/after pairs, 20 Criterion batch
samples per workload/run, both executable hashes and source provenance.
The [rquickjs interleaved report](../bench/baselines/performance-review-2026-11-06/rquickjs-interleaved.json)
retains three more pairs for property and class workloads, with 15 batches of
10 calls per mode. The Rust harness is unchanged; the engine changes are exactly
`interpreter/call.rs`, `eval.rs` and `resolve.rs`.

| Property-update boundary | Before | After | Approximate gain |
| --- | ---: | ---: | ---: |
| Fresh AST, including setup/parse/collection | 26.204 ms | 8.471 ms | 3.1× |
| Warm AST invocation | 22.731 ms | 4.371 ms | 5.2× |
| Warm bytecode host call | 14.422 ms | 2.265 ms | 6.4× |

These are medians of three run medians. Adjacent warm bytecode pair ratios
are 0.1536–0.1574; warm AST ratios are 0.1872–0.1935. The fresh AST case and
bytecode comparison use different sources and iteration counts and must not
be compared against each other as execution-tier speed ratios. Shape churn,
monomorphic reads, tiny calls, classes, arrays and JSON were retained as
controls; small changes near noise are not claimed as improvements.
Criterion uses the facade's default NAPI feature plus core test hooks; the
rquickjs comparison uses bare core. These rows describe separate workload and
feature configurations, rather than an isolated comparison of execution tiers.

The [typed-array paired summary](../bench/baselines/performance-review-2026-11-06/typed-array-paired-summary.json)
and six retained run reports retain three alternating native-addon pairs.
The old addon matches the earlier default-feature baseline; intervening commits
changed no `src/` or core engine code. The new addon came from
`npm run build:all`; both artifact hashes are recorded.

| Backing size, same 16-byte view | Before (µs/call) | After (µs/call) | Approximate gain |
| --- | ---: | ---: | ---: |
| 64 bytes | 3.036 | 3.013 | Unchanged control |
| 8 MiB | 318.424 | 2.853 | 112× |
| 32 MiB | 4,744.588 | 2.901 | 1,600× |

The improvement removes work proportional to the unused backing size. It is
specific to exporting a small subview; it is not a whole-VM speedup. All timed
calls passed the byte-content/type/bounds checks. The new focused native
regressions also passed against the rebuilt addon.

## rquickjs results

The [before](../bench/baselines/performance-review-2026-11-06/rquickjs-before.json)
and [after](../bench/baselines/performance-review-2026-11-06/rquickjs-after.json)
reports retain all 60 engine/mode/workload rows each, including fresh and warm
source measurements. The table below is **warm host-to-guest calls only**,
measured on the AMD BC-250 with 15 batches of 10 calls. A call executes the
complete workload, including its loop; these are not individual-property or
individual-arithmetic-operation timings.

| Workload | napi-vm after (µs/call) | rquickjs (µs/call) | napi-vm / rquickjs |
| --- | ---: | ---: | ---: |
| `startup_dominated` | 1.256 | 0.077 | 16.36× |
| `arithmetic_loop` | 1,925.437 | 952.588 | 2.02× |
| `recursion_fib` | 99.725 | 15.036 | 6.63× |
| `array_chain` | 8,201.077 | 274.457 | 29.88× |
| `string_ops` | 5,229.360 | 277.478 | 18.85× |
| `closures` | 451.967 | 80.075 | 5.64× |
| `json_roundtrip` | 4,470.968 | 1,143.286 | 3.91× |
| `property_reads` | 1,649.946 | 379.195 | 4.35× |
| `property_updates` | 2,198.581 | 244.086 | 9.01× |
| `class_methods` | 9,728.219 | 391.394 | 24.86× |

The property-update workload improves substantially; arithmetic is effectively
unchanged in this capture. Small differences in other workloads are not
accepted as gains without repeated evidence. rquickjs remains faster on most
measured execution workloads, including the optimized property case. These
results do not establish whole-application throughput or language equivalence.

## Correctness and final source audit

The [validation record](../bench/baselines/performance-review-2026-11-06/validation.json)
records the commands, feature selections, artifact/log digests and test results.

| Gate | Result |
| --- | --- |
| Complete `reviewed-5112` Test262 corpus | **102,992/102,992 pass**, zero skips, failures, timeouts, crashes or harness errors |
| Core Rust unit tests | 446 pass |
| Ten focused Rust integration suites | 95 pass |
| Node native suite | 77 pass, 8 existing runtime/timer capability skips |
| Bun suite | 1,427 pass, 32 existing optional capability skips |
| Rebuilt WASM regressions | 18 pass, zero skips; playground typecheck passed |
| Test262 runner tests | 11 unit + 1 documentation test pass |
| Full workspace Clippy | All targets/features pass with warnings denied |
| Other checks | Root/plugin TypeScript, Rust formatting including the standalone benchmark, plugin JavaScript syntax, benchmark Python syntax, whitespace and local documentation links pass |

The [complete retained report](../bench/baselines/performance-review-2026-11-06/test262-report.json.gz),
[summary](../bench/baselines/performance-review-2026-11-06/test262-summary.json) and
[comparison](../bench/baselines/performance-review-2026-11-06/test262-comparison.json)
prove exact equality with the previous 102,992-variant set and zero losses.
The run used the corrected, explicitly labelled `reviewed-5112` corpus at
`cff513bc172b79b97963046e83078e114b2a20ff`, two workers and a ten-second deadline.
It is separate from the contradictory original upstream corpus. The final
worker digest is `967f960ddf912454d9b3f64a77642facf7afa64cdd07b3a893fd744a4a4a0aa5`.

```sh
target/release/test262 run --profile reviewed-5112 \
  --engine target/test262-final/release/napi-vm-test262 \
  --jobs 2 --timeout 10 \
  --output artifacts/performance/reviewed-5112-performance-full.json
```

Build that worker with `cargo build --release --locked --no-default-features
--bin napi-vm-test262 --target-dir target/test262-final` before reproducing.
The default Node addon was rebuilt through `npm run build:all`; its generated
binding files were unchanged. WASM was rebuilt through `npm run test:wasm`,
so the browser regression suite exercised a new binary rather than skipping
an absent package or loading an old build. These tests load WASM under Node;
they are not browser UI timing or cross-platform CI results.

The full checks also found six stale Bun safety expectations, all reproduced
on the preserved pre-change addon. The
[guard validation](../bench/baselines/performance-review-2026-11-06/guard-validation.json)
records their correction: strict UTF-8 URI decoding, the current 64-Mi-unit
string limit, sparse array growth and the separate aggregate JSON array-element
budget. Shared-graph guards now cross the actual limit without millions of
unnecessary numeric conversions or huge failure logs. Both addons pass the
corrected focused cases; production resource limits were not changed.

After timing, an obsolete cache test was updated to require eight hits/two
misses after a layout addition, and a new same-slot write test checks fresh
values through a warm cache. Five narrowly scoped Clippy annotations explain
intentional cache/call layouts, and the runner's unchanged test module was
moved to the end of its file. The
[source audit](../bench/baselines/performance-review-2026-11-06/source-audit.json)
retains all 260 frozen Rust input hashes, the exact annotation-removal proofs
and the test-only exception. The
[runner audit](../bench/baselines/performance-review-2026-11-06/runner-test-module-source-audit.json)
proves its production text is unchanged. These changes alter no executable
engine bodies, types, layouts or signatures used by the measured and validated
artifacts.

The evidence directory is dated for the review's start on November 6; runs
continued past midnight locally on November 7. Individual reports retain UTC
timestamps, dirty source state and exact hashes. Raw logs and immutable binaries
remain in the ignored local `artifacts/performance/` directory; the retained
JSON/gzip evidence accompanies this review.

## Independent completion review

Three subagents independently reviewed the core changes, the comparison
method/evidence, and host/runtime/editor/plugin boundaries. Recomputing the
retained statistics reproduced the published ratios. The complete compressed
Test262 report contains 102,992 unique passing variants, exactly the baseline
variant set, and its worker digest matches the retained executable. The frozen
core inputs and the documented lint/test-only normalization were rechecked.

The comparison validator now rejects missing engine/mode rows, incorrect sample
counts or statistics, and changes to the timed Rust harness, manifest, lockfile
or mutually recorded dependency/configuration fingerprints. Its negative
regressions preserve the existing ratios. The older before report lacks the
separate dependency list and Cargo configuration fingerprint: matching lockfile
hashes identify dependencies, while configuration identity remains explicitly
unverified in the comparator's provenance warnings.

The boundary review also fixed empty/detached host-buffer imports, which could
construct a Rust slice from a null pointer at length zero. Host typed arrays
over shared backing stores now fail before raw-byte copying, including empty
views and Node Buffers. Ordinary buffers retain owned-copy behavior. The
[Node-API contract](https://nodejs.org/api/n-api.html#napi_get_typedarray_info)
permits both ordinary and shared backing stores; the bridge requires an
ordinary ArrayBuffer. These are NAPI-only import changes; core guest execution,
the timed Rust harness, WASM, LSP conversion and the measured export path are
unchanged. The original performance captures remain historical evidence for
their recorded binaries.

The [completion audit](../bench/baselines/performance-review-2026-11-06/completion-audit.json)
records the rechecked inputs, raw evidence and rebuilt addon. Final checks
passed: 80 Node tests (8 existing capability skips), 1,427 Bun tests (32 existing
capability skips), five benchmark-validator tests, workspace Clippy with warnings
denied, Rust formatting, unchanged generated bindings and local documentation
links. A fresh correctness smoke exercised all 60 cross-engine rows. A final
subview run on the rebuilt addon retained microsecond-scale copies at every
backing size; the original alternating runs remain the repeated speedup
evidence. Core/WASM production inputs are unchanged, so their complete retained
conformance and rebuilt WASM evidence remain applicable.

The [follow-up audit](../bench/baselines/performance-review-2026-11-06/followup-audit.json)
independently rechecks those hashes, every Test262 variant, and the paired
summaries. It also records the SDK queue fix and its additional eight
regressions, full plugin suites (142 Node passes with one existing Bun-specific
skip; 143 Bun passes), plugin typechecking/lint, and 26 fresh native/WASM
boundary passes with zero skips. The shared property fixture now asserts its
actual UTF-16 code units and covers both lone surrogate halves. Core execution
still matches the retained full conformance run; the new production change is
confined to the trusted-plugin SDK.
