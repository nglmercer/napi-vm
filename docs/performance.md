# Measure performance and reproduce comparisons

Choose a boundary before choosing a benchmark. Parsing source, creating a VM,
calling a retained function, copying a host value, polling a timer, and invoking
a child process are different operations. Keep their results separate.

## Run the layered suite together

```sh
python3 bench/suite.py --output artifacts/performance/full-suite \
  --samples 15 --iterations 10 --tmp-dir artifacts/performance/build-tmp
```

The default selection runs all eight available groups: embedded engine timings,
process memory/GC, native HTTP transport, the broad Rust/Node host suite, and
public native/WASM/Node API timings, explicit promise/job phases, and Intl/Temporal
capability and result-checked timing, and local filesystem/loaders. Python,
Cargo, the native toolchain, Node/npm,
installed workspace dependencies, wasm-pack and the wasm32-unknown-unknown Rust
target are required; the memory probes currently require Linux. Choose a new
output directory. `--platform-processes 3` controls independent public-API runs;
`--async-processes 3` controls independent async engine runs;
`--international-processes 3` controls independent Intl/Temporal runs;
`--io-processes 3` controls independent filesystem/loader runs.

`suite.json` records the requested groups, commands, logs, validated report paths,
SHA-256 digests, row counts and terminal status. A partial selection such as
`--groups engines,memory,http` records the omitted groups and keeps
`all_groups_complete` false. A failed build or report validator leaves a failed
manifest and exits unsuccessfully. Build and measurement steps run sequentially.
Each layer retains its own boundary and units; the runner computes no universal
runtime score. All-group completion means these finite drivers completed, not
that every public API or builtin was timed. The operation inventory below lists
remaining coverage explicitly.

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
filesystem/network latency, native package, browser UI, or LSP transport.
WASM source timing has its own driver below. Add an appropriate workload before
claiming a gain there.

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
its result, and samples alternate engine order. The original 25 workloads cover startup, arithmetic and bitwise operations, recursion,
array chains/index updates/sorting, strings and Unicode, closures, JSON,
property reads/writes/getters/key churn, classes, Map/Set, typed arrays, regular
expressions, exceptions, spread/destructuring, Math, BigInt, and cyclic allocation.
Another 24 cases measure Map/Set construction and prepopulated lookup at 16,
64, 256 and 1,024 entries, including numeric/string/object Map keys and Set
hits/misses. Sixteen more cases cover generators/custom iterators, proxies and
Reflect, Date UTC fields/ISO parsing, BigInt division/remainder, replacement
callbacks, normalization, DataView, typed-array bulk copies, WeakMap/WeakSet,
ArrayBuffer slicing and sparse-array mapping. There are now **65 workloads**,
producing 390 engine rows. Lookup fixtures are populated outside warm timers;
fresh-source mode includes that population.
All produce checked numeric outcomes. Async jobs and I/O need separate drivers.

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
creation, document reparsing after changed metadata,
temporary descriptors for missing named properties, lexer literal allocations,
and additional shape-transition costs under unique-key churn. The memory probes below
retain the bounded-cache candidate and its hot-layout/lifecycle checks. Outbound
Map/Set identity now has a measured fix below. The other candidates need workloads and
correctness checks for their own boundaries.

Run relevant Rust/Node/WASM tests and the appropriate complete Test262 gate for
engine changes. The [validation record](../bench/baselines/performance-review-2026-11-06/validation.json)
and [follow-up audit](../bench/baselines/performance-review-2026-11-06/followup-audit.json)
link retained correctness evidence and source-equivalence checks.

## Choosing comparison engines

Use [rquickjs](https://docs.rs/rquickjs/0.14.0/rquickjs/) as the first embedded
engine comparison: its Rust Runtime/Context/function APIs let this suite keep
host boundaries explicit. [Boa](https://github.com/boa-dev/boa) is a useful
additional comparison when a Rust engine implementation matters. Node/V8 is
useful for application-level comparisons, but keep process startup, host APIs,
and JIT warmup separate from retained embedded function calls. These are
comparison roles. Boa is unmeasured; the public API and Intl layers below
include Node/V8 results for their stated boundaries.

The `language_metadata` Criterion group measures changed and unchanged host
metadata refreshes with 1 and 20 open documents (50 declarations each and 100
registered host functions). Run it separately while profiling editor workloads:

```sh
cargo bench --locked --bench vm --no-default-features -- language_metadata
```

The retained November 6 engine reports still contain ten workloads. Rebuild to
measure the expanded suite; compare matching workload sources and harnesses.

## Metadata refresh follow-up

Identical host/global declarations now preserve document analysis. A changed
refresh resolves the shared metadata context once and replaces documents in
place, avoiding temporary URI/source copies. In two interleaved pairs with 20
open documents, repeated identical host declarations fell from 2.95–3.40 ms to
0.24–0.25 µs per registration (including a hover lookup). The changed-declaration
control had variable 1.03×/1.34× ratios; no stable percentage gain is claimed.
This measures embedding `LanguageService` APIs. The LSP server constructs a new
service per snapshot, so these numbers do not establish a server-latency gain.

[Raw paired samples](../bench/baselines/operations-2026-11-07/metadata-paired.json),
[build provenance](../bench/baselines/operations-2026-11-07/metadata-provenance.json),
and [84 passing language tests](../bench/baselines/operations-2026-11-07/language-tests.log)
are retained. The tests include replacement across two documents and restoration
of manifest metadata after clearing runtime declarations. Reproduce with the
`language_metadata` Criterion group, preserving separate before/after binaries.

## Array result allocation

The array-chain workload exposed descriptor allocation on every `map`/`filter`
result element. A guarded direct dense-array definition now avoids those
records, rechecking array metadata after every callback. Proxies, accessors,
protected attributes, named properties, sparse indices, and rejected extensions
keep the generic path. Species construction, callback order, and loop budgets
are unchanged.

Two interleaved before/after pairs measured **1.27–1.36× faster retained calls**,
1.28–1.37× warm source, and 1.27–1.33× fresh source. The rquickjs retained-call
controls stayed within roughly 1%. The only changed fingerprinted comparison
build input was `builtins/array.rs`. The full 25-workload before/after smoke run
completed all 150 rows per build; small control variations are not claimed as
gains. See [paired reports and validation status](../bench/baselines/array-results-2026-11-07/validation-status.json).

The 35 focused Rust array tests pass, including eight new scripts checked
against Node and executed through AST and bytecode. The complete reviewed-5112
Test262 gate passed **102,992/102,992 variants**, with no failures, skips,
timeouts, crashes, or harness errors. Its variant set, corpus and selection
settings match the prior full run exactly; the runner reports zero losses or
other outcome changes. The status file links the compressed full report,
summary, comparison, source checks and immutable-worker digest. This gate is
ECMAScript coverage, not a Node bridge/I/O/WASM/application performance claim.

## Measure the native HTTP transport

The loopback suite measures empty/64-KiB GETs, 4-MiB POSTs, and 4-MiB POSTs with
three 307 redirects. It compiles the production HTTP worker into a native probe
and checks status, redirect state, response bytes, and every uploaded byte.
Builds use the explicit `ci-optimized` profile and `runtime-net` feature.

```sh
python3 bench/http.py build --output artifacts/performance/http-build
python3 bench/http.py run --build artifacts/performance/http-build \
  --output artifacts/performance/http-run.json --samples 15 --iterations 10
python3 bench/http.py compare BEFORE.json AFTER.json
```

Choose new output paths. Build records retain input/executable hashes, compiler,
features, flags and Git state. Comparisons validate raw timings/statistics and
reject changed harness, dependency lock, configuration, compiler, or host inputs.
Production runtime source may change and is listed in the comparison output.

The timer excludes input allocation and response checks, and includes per-request
client/runtime construction, HTTP transport, redirect handling, and base64/JSON
response creation. The server drains and validates requests and closes each
connection. This excludes guest VM/bridge calls, owner-thread scheduling, TLS,
external DNS/network latency, and connection-pool benefits. rquickjs has no host
HTTP implementation in the embedded comparison, so these are separate transport
measurements rather than an engine ranking.

HTTP uploads now move their input Vec into shared immutable `bytes::Bytes`,
avoiding a full payload copy on every hop. 301/302/303 POST redirects discard the
body; 307/308 reuse it. This adds a direct optional reference to the already
locked `bytes` package without changing package versions or checksums.

Four interleaved pairs with the final harness measured **1.13–1.35× faster
4-MiB POSTs**. Redirect timings showed no stable gain; GET controls varied under
host load. Desktop activity and the two-worker Test262 run were concurrent, so
these are descriptive observations. No universal HTTP speedup is claimed.
[Raw reports, comparisons and validation](../bench/baselines/http-2026-11-07/validation.json)
retain those limitations. The 24 runtime tests pass, including replay/discard
checks for all five redirect statuses and existing permissions, cancellation,
body-limit, and binary response cases. The benchmark passes Clippy with warnings
denied; seven benchmark-validator tests pass.

## Compare process memory and explicit collection

The Linux memory probes instantiate one engine and workload per fresh process.
They cover ordinary objects, cyclic garbage, unique property keys with repeated
reads, and cycles retained by a guest global. Each invocation checks its numeric
result. Every batch records allocation and post-GC snapshots, followed by release
of the guest root and destruction of the engine.

```sh
python3 bench/rquickjs.py build --output artifacts/performance/memory-build
python3 bench/rquickjs.py memory --build artifacts/performance/memory-build \
  --output artifacts/performance/memory.json --samples 3 --iterations 8 --objects 2000
python3 bench/memory_report.py BEFORE.json AFTER.json
```

The same build also provides the existing timing executable. Memory reports
retain per-process samples, source hashes, tiers, RSS, observed VmHWM, explicit
GC durations, and engine-specific counters. Before/after comparisons reject
changed workloads, harnesses, dependencies, configurations, report helpers,
settings or host identity. Napi counters report tracked containers, reclaimed
containers and cumulative minted shapes. QuickJS counters report its own
allocation sizes and object/shape/atom counts. These counters have different
definitions and must not be compared as equivalent object counts.

[Baseline and stress captures](../bench/baselines/memory-2026-11-07/validation.json)
retain three process repetitions per engine/workload. The stress run uses 32
calls of 4,000 objects. Unique-key churn with repeated reads grew napi-vm's
post-GC RSS by about **64.8 MiB**, versus **1.26 MiB** for this rquickjs setup.
Napi's tracked-container count returned to its ready value, while roughly
128,000 additional shapes were minted. At that baseline revision, strong cached transitions retained these distinct
small layouts. This identifies cache retention; it does not establish that all
RSS growth is live shapes. The bounded-cache candidate below now has lifecycle
and hot-layout controls, and remains under review.

RSS includes allocator arenas, resident code/data, shared pages, builtins,
caches, and probe allocations. Builtin sets differ. RSS staying high after GC or
engine destruction does not alone prove a live-object leak. Linux documents
[VmRSS and VmHWM as approximate](https://man7.org/linux/man-pages/man5/proc_pid_status.5.html);
the diagnostic capture even records decreases in reported VmHWM. Summaries use
the maximum observed reading and do not enforce monotonic kernel readings.
Explicit GC times describe the configured collector calls; automatic collection
may also occur during allocations. These probes do not measure NAPI owner
contexts, WASM, host-bridge copies, or whole-application memory.

## Local filesystem and source loaders

```sh
python3 bench/io.py build --output artifacts/performance/io-build
python3 bench/io.py run --build artifacts/performance/io-build \
  --output artifacts/performance/io.json --samples 15 --iterations 10 --processes 3
python3 bench/io.py compare artifacts/performance/io.json artifacts/performance/io.json
```

The Unix driver freezes a normal System-allocator latency executable, a separate
counting-allocator diagnostic executable, the Node helper, and content-hashed
fixtures. Every capture checks these digests and the harness, alternates native
and Node process order, and retains raw process samples plus median/MAD. The
validator requires all **34 latency rows and 23 allocation diagnostic rows**;
it checks raw/summary consistency and rejects comparisons with different
fixtures, harness, compiler, flags, Node binary, or run settings. Production
source differences are listed explicitly.

Filesystem cases read empty, 4-KiB, 64-KiB, 4-MiB and nested 4-KiB files, decode
64-KiB ASCII text, overwrite 4 KiB, and traverse an existing directory tree.
Permission-denied access is a separate native case. Permission-checked calls
traverse granted directory handles with no-follow flags and reject hardlinks
and non-regular files; Rust std and Node filesystem references have different
policy checks. Their difference is a host API observation, not an engine score.
Each process performs five warmups per case. These are warm-page-cache tests,
without fsync, cold-disk claims, or concurrent load. Payload/grant setup is
outside timers; path processing, buffer allocation, reading, UTF-8 validation
and Rust buffer destruction remain inside. Node may defer reclamation to GC.

Loader cases separate file URL resolution from a 64-KiB source load. Package
cases resolve an import target backed by a 4-MiB file and resolve/read a 64-KiB
CommonJS source. The current npm resolver reads candidate contents while
probing; the import-resolution case intentionally exposes that cost. The Node
CommonJS reference includes a fresh source read after its warmed resolution
cache, whereas the Rust resolver returns source on every call. Oxc cases
transform 100 and 1,000 typed declarations with semantic analysis and separate
source-map generation. Generated JavaScript is evaluated and source maps are
checked before timing; no equivalent Node/rquickjs transform is claimed.

Allocation diagnostics count allocation/reallocation requests and cumulative
requested bytes per operation. They do not measure live heap, RSS, or Node
allocations and their instrumented executable supplies no latency samples.
This layer does not yet measure guest filesystem bridges, async file I/O,
executed module graphs, package installation, sockets, or cold-cache behavior.

### Permission traversal and read buffers

Traversal now borrows the granted root descriptor and owns only opened child
descriptors. It also iterates path components without collecting a temporary
vector. The regular-file/link validation already obtains the file length; reads
reuse that length as a bounded buffer-capacity hint, with room for an EOF probe.
Zero-length files allocate lazily, and reads still enforce the byte limit if
contents grow or a virtual file reports an inaccurate length. No-follow flags,
grant selection and regular-file/link checks remain in place.

Separate allocation diagnostics give stable request counts for these fixtures:

| Permission-checked operation | Before requests/call | After requests/call |
| --- | ---: | ---: |
| Flat 4-KiB binary read | 12 | 3 |
| Flat 64-KiB binary/text read | 16 | 3 |
| Flat 4-MiB binary read | 22 | 3 |
| Nested 4-KiB binary read | 17 | 7 |
| Empty read or 4-KiB overwrite | 3 | 2 |

For the 4-MiB read, cumulative requested bytes fall from **16,777,362 to
4,194,412** in the retained builds. These include path processing and realloc
requests, not simultaneous live heap or resident memory. Other build-directory
path lengths can change the small path-allocation totals.

[Eight retained captures](../bench/baselines/io-traversal-2026-11-07/validation.json)
separate the descriptor experiment from the final descriptor/buffer change.
Each uses 15 samples, 50 iterations and three processes. The final four captures
run in candidate/baseline/baseline/candidate order. Small-read latency improves
in both pairs: 4-KiB after/before medians are 0.435×/0.542× and 64-KiB medians
are 0.424×/0.585×. Rust std read controls are 0.832×/1.060× and 0.726×/0.967×,
respectively. Controls therefore prevent attributing every absolute shift to
the patch. The 4-MiB permission and std controls move almost together, so these
captures do not isolate a 4-MiB latency gain. Raw controls, first-pair drift and
all allocation samples remain available; there is no universal speedup claim.
Native cases share an allocator within each process, so earlier cases can also
affect later allocation state.

All **43 runtime tests** pass with Node compatibility, filesystem, TypeScript,
network and web features. Five traversal regressions cover anchored specific
grants after directory replacement, flat/nested operations, symlinks, hardlinks,
FIFOs, byte limits, UTF-8 errors and a zero-length-metadata Linux virtual file.
Runtime Clippy passes with warnings denied, and all **34 benchmark-validator
tests** pass. The existing Intl gate's 166 core/worker inputs remain unchanged;
this runtime change makes no new whole-Test262-corpus claim.

The [eight-group capture](../bench/baselines/full-suite-io-2026-11-07/suite.json)
retains 390 engine, 398 public API, 81 host, eight memory, four HTTP, 104 async,
243 Intl/Temporal and 57 I/O rows. The I/O rows comprise 34 latency measurements
and 23 allocation diagnostics with distinct units. Its completion covers these
finite drivers; the remaining operation families below still need measurement.

## Operation coverage and remaining audit work

The available drivers measure separate boundaries. A passing correctness test
is not a timing result; an unmeasured family below still needs a workload before
claiming a gain or an engine ranking for it.

| Operation family | Current measurement | Remaining coverage |
| --- | --- | --- |
| Lexing, parsing, AST execution, bytecode and property caches | `benches/vm.rs`; retained Criterion/broad baselines | More source sizes and module compilation/cache invalidation |
| Common synchronous guest operations | 65 shared rquickjs cases × fresh source/warm source/retained call, including collection scaling, generators, reflection/proxies, Date and buffers | More sizes, argument shapes and builtin methods; async work has a separate driver |
| Process memory, cyclic GC and layout churn | Bare-engine probes; Node-owner Map/Set replacement/release probes | Sustained mixed NAPI owners, more graph sizes, allocator/GC profiles |
| Node entry points | Fresh `runCode`; 26 persistent/public API cases including run/get/set/call/expose, modules and lifecycle | More source/argument/module sizes and async command latency |
| Host values and buffers | Typed-array copy pairs; outbound Map/Set aliases; plain objects with 1/128 properties and 1/64-KiB buffers | Incoming graph identity, broader callback/argument size scaling and WASM marshaling |
| Scheduling | Rust/Node scheduling drivers; 13 shared promise/async fixtures with enqueue, checkpoint and completed-call phases | Sustained mixed timer/I/O load, more fan-out/chain sizes, public async bridge scaling |
| HTTP transport | `http.py` GET/upload/redirect probes | Client/runtime setup attribution and lifecycle-safe pooling; TLS and external latency are separate measurements |
| Files, sockets, package and TypeScript loaders | `io.py`: checked local filesystem operations, file/package source resolution/loading, Oxc transformation and separate allocation counts | Guest bridges, async file I/O, WebSocket/TCP, executed module graphs, package installation and cold-cache behavior |
| Trusted plugins | Node process calls, Criterion Rust host calls and event queue pairs | Complete native/process/language/payload/event coverage under one driver |
| Language service | Completion/update diagnostics, metadata refresh and isolated position conversion | Hover/symbol scaling, changed-metadata parsing, and LSP transport latency |
| WASM and browser execution | Shared-source native/WASM/Node API timings, independent module/init components, VM lifecycle and result construction diagnostics | Browser UI, callbacks/value-graph scaling, worker loading and browser-specific startup |
| Intl and Temporal | 27 shared fixtures with API/result preflights; construction, formatting/parts, collation, locale and Temporal arithmetic/parsing | More locales, calendars, zones, numeric options, data coverage and buffer/bridge costs |

This inventory preserves the broad audit scope. The existing finite suites and
accepted fixes do not establish that every operation has already been optimized.

## Bounded shape cache

The bounded cache limits each shape-root family to **4,096 memoized
transitions** across the entire tree. It periodically prunes cache-only leaves,
retains ancestors of externally held layouts, and permits uncached new layouts
when the budget is full. Existing memoized live layouts keep their IDs and sharing; evicted layouts
receive fresh IDs if rebuilt. Owner contexts carry independent budgets. Large
uniquely owned dictionaries still grow in place, and pruning releases oversized
bucket allocations on live prefixes.

With the same stress workload and immutable before/after builds, post-GC RSS
growth fell from **64.8 MiB to 8.7 MiB** (about 86.5% less growth). Ordinary-object
and cycle controls remained close to baseline. This is the bare-engine
unique-key probe, not an 86.5% reduction in whole-application memory.

The first bounded layout had a latency concern: whole-suite retained
property-read ratios were 1.049×/1.078× after/before. Assembly showed the same
shape-ID guard instructions with a changed field offset. Explicitly grouping
the ID and slot index ahead of cold layout/transition fields reduced that
observed penalty. In two counterbalanced process captures per layout, selected
hot-header medians were about 1.02× original reads, 0.99× updates, and 1.02× class
calls. These small differences are descriptive under host noise; no general
speedup or statistically established latency neutrality is claimed.
[Layout experiments and selection](../bench/baselines/shape-cache-2026-11-07/layouts/selection.json)
retain alternatives and raw reports rather than discarding the slower captures.

All **452 core tests** pass with the NAPI feature, including budget saturation,
live-ancestor sharing, eviction/cache invalidation, dictionary growth, owner
isolation, bucket reclamation and destruction accounting. Core/test Clippy passes
with warnings denied. Default-addon owner checks pass 26 tests with eight timer
cases skipped; the runtime-enabled rerun passes **34/34 tests with no skips**,
including new sync/async owner-pressure and cleanup cases. The bounded policy
with its original header passed all 102,992 reviewed variants with an exact
variant-set match and zero outcome changes. The selected hot-header source also
passed its own complete gate: **102,992/102,992 variants**, no skips or other
non-passing outcomes. Its variant set, corpus and settings match the earlier
full gate exactly; the immutable worker digest and all recorded core inputs
match the selected source. [Reports and current validation status](../bench/baselines/shape-cache-2026-11-07/validation.json)
retain both timing pairs, memory data, tests and immutable worker metadata.

The first all-group run completed successfully and is retained at
[full-suite-2026-11-07](../bench/baselines/full-suite-2026-11-07/suite.json): 150 engine
timing rows, eight memory rows, four HTTP rows, and 47 host rows. Its manifests
and child reports retain per-layer settings and report digests. These counts
are report rows with different units, not an aggregate benchmark score. The
all-group runner and validators had 12 passing regression tests at that capture.

## Outbound Map and Set conversion

Collection conversion now consults the object-identity memo before taking one
entry snapshot. It creates and memoizes an empty native collection before
following entries, then calls `set`/`add` directly. This preserves aliases and
cycles and removes the temporary seed and per-entry pair arrays. Previously,
repeated references produced distinct host collections and self-references hit
the depth limit. The existing depth limit still rejects deeply nested acyclic
graphs. Incoming host-to-guest conversion is outside this change.

```sh
NAPI_BENCH_BINDING=/absolute/path/to/verified-addon.node \
  node bench/performance/collections.cjs > collections.json
python3 bench/collection_report.py BEFORE.json AFTER.json
```

The benchmark covers one or 64 references to a 256-entry Map or Set. Setup and
assertions are untimed; the timer includes the retained guest call and outbound
conversion. Reports record addon/harness hashes, host settings, raw samples,
median/MAD and identity observations. Comparisons reject changed harness,
workloads, host or sample settings, and explicitly show the old identity defect.
The broad host driver now includes these four rows and requires preserved
identity. The earlier 47-row all-group capture predates this binding change.

[Three interleaved pairs and validation](../bench/baselines/collections-2026-11-07/validation.json)
measured **110–182× faster Map alias conversion** and **58–84× faster Set alias
conversion**. Single-reference controls measured 2.02–2.92× for Map and
1.12–1.59× for Set. These are descriptive under concurrent Test262 and desktop
activity; the first pair also overlapped Node checks. Alias results changed
semantics to preserve identity, so these ratios are not equivalent-behavior
engine rankings. Both addons use the explicit `ci-optimized` profile; the broad
host suite uses `release`. The record states the earlier build-manifest limits.

All **50 selected Node tests pass with no skips**, including six new collection
graph/depth tests and existing owner, async, buffer and UTF-16 cases. Binding
Clippy passes with warnings denied. The benchmark validators now have **14
passing tests**, including rejection of corrupt/incompatible collection reports.

## Persistent Node API coverage

`bench/performance/public-api.cjs` adds **26 cases** covering construction and
disposal, warmed persistent `run`, scalar global lookup/update/removal, zero/eight
argument calls, guest addition and host callback controls, callback replacement,
plain incoming/outgoing objects, typed-array copies, module validation and
deferred/eager registration, module lookup/list/removal, and idle explicit GC.
These rows are included in the broad host driver alongside the four collection
cases. Each timed operation has an untimed correctness check.

```sh
NAPI_BENCH_BINDING=/absolute/path/to/verified-addon.node \
  node bench/performance/public-api.cjs > public-api.json
python3 bench/public_api_report.py BEFORE.json AFTER.json
```

Preparation and checks can execute guest code between timed calls, warm caches
or trigger collection. They are excluded from the timer. Sources and owners are
reused where the case says so; eager module registration includes its export
transaction and execution, while deferred registration excludes import
evaluation. The lifecycle case includes both creation and explicit disposal.
Idle GC measures owner/API overhead without a deliberately created garbage
workload. Object cases use one or 128 numeric properties; buffer cases use
1 KiB or 64 KiB. These sizes do not establish arbitrary graph or payload scaling.

The comparator requires the full 26-row matrix, unchanged harness, host,
sample settings and timing boundaries, and verifies raw samples and stored
median/MAD. Build manifests must also be inspected when comparing binaries.
The expanded benchmark-validator suite has **16 passing tests**. These public
Node operations have no equivalent rquickjs host adapter in this suite, so they
are separate API measurements rather than an embedded-engine ranking.

The [expanded all-group capture](../bench/baselines/full-suite-expanded-2026-11-07/suite.json)
completed with **77 host rows**, 150 engine timing rows, eight memory rows and
four HTTP rows. Report digests, the unchanged host build-input fingerprint,
explicit addon selection, raw public API/collection samples and build logs are
retained. This is an initial API baseline; adding coverage alone does not
establish an optimization. The earlier 47-row capture remains historical.

## Indexed guest collections under review

The scaling cases reproduced a growing linear scan for every Map/Set lookup
and insertion. The candidate retains the ordered, tombstoned entry list and
builds a hash index at 32 positions. Fingerprints use SameValueZero-compatible
primitive values and reference identities; collisions still check actual keys.
The index stores hashes and positions without retaining additional guest
values. A live count removes repeated scans for `size`. Updates, deletion,
reinsertion, clear, live iterators, set composition and groupBy retain ordering
and graph edges.

[Three paired captures and validation](../bench/baselines/collection-index-2026-11-07/validation.json)
retain the candidate's results. At 1,024 entries, retained construction measured
8.8–11.3× faster, Map lookup 11.2–14.4× faster across numeric/string/object keys,
and Set hits/misses 20.1–21.1× faster. These are descriptive observations.
One full pair also showed **4–9% slower non-collection controls**. Two longer
counterbalanced pairs restricted to one CPU measured after/before ratios of
1.102/0.999 for property reads, 1.020/1.011 for class calls, and 0.976/0.993 for
indexed-array updates. Property-read latency neutrality remains unproven;
the slower captures are retained. The candidate's own complete reviewed gate
passed **102,992/102,992 variants**, with the same corpus/configuration and exact
previous variant set, zero outcome changes, and matching worker/source hashes.

All **913 core unit/integration/doctests**, six root collection tests, 50 selected
Node tests and 18 benchmark-validator tests pass. Core Clippy passes with
warnings denied. A fresh release WASM package with `wasm,runtime` also passes
**19 tests with no skips**, including indexed mixed-key collections on wasm32.
Two pre-existing integration-test expectations were corrected
after both immutable before/after workers and stock Node agreed: class
prototype `constructor` precedes declared methods, and a bound typed-array
constructor with an undefined prototype falls back to the foreign intrinsic
TypedArray prototype. Retained reference checks show those diagnostics;
no production class or typed-array behavior changed for those corrections.

The pinned rquickjs-sys source has a numeric-key caveat: its double-word hash
puts these sequential small integers in one bucket at the tested power-of-two
sizes. The [source/hash audit](../bench/baselines/collection-index-2026-11-07/rquickjs-hash-audit.json)
matches the local source to the locked crate archive and records the derived
bucket distributions. String/object cases remain necessary controls; neither
runtime should be selected from a numeric-only Map/Set score.

## Native owner collection memory

The Node-owner probe runs one owner per fresh process, retaining either 1,000
collections with eight entries or four with 4,096 entries. It replaces roots
three times, explicitly collects, releases roots, and disposes the owner.
Each round checks contents and sizes. The broad host driver retains this raw
report and adds four process-RSS rows with **bytes** as their unit.

```sh
python3 bench/collection_memory.py run --addon /absolute/path/to/addon.node \
  --output artifacts/performance/owner-memory.json --samples 3
python3 bench/collection_memory.py compare BEFORE.json AFTER.json
```

[Paired owner captures](../bench/baselines/collection-index-2026-11-07/owner-memory-comparison.json)
show the index's cost: median post-GC RSS growth increased about 0.2–0.3 MiB
for many small collections and 0.8–1.0 MiB for a few large ones. All four cases
returned native tracked-container counts exactly to the ready baseline after
releasing roots. This supports release behavior for these graphs; it does not
establish a whole-application memory improvement.

RSS includes Node, native/V8 allocator retention, builtins, resident code and
probe buffers. Kernel readings are approximate; comparisons retain raw phases,
median/MAD, hashes, host settings and maximum observed high-water readings.
Small differences can be host/allocator noise. Native tracked counts omit
index allocations and are not equivalent to QuickJS object counts.

The [current all-group capture](../bench/baselines/full-suite-indexed-2026-11-07/suite.json)
passed with **294 engine timing rows, 81 host rows, eight bare-engine memory
rows and four HTTP rows**. Four of the host rows are owner RSS in bytes; the
other 77 retain their per-case timing units. Child report digests and the
unchanged host build-input fingerprint were verified. Completion covers these
finite drivers, not every operation family in the inventory.

Use the supplemental Linux control driver to compare immutable builds on one
allowed CPU. It reverses build order in the second pair, retains process-tree
CPU time/context switches, and records the inherited affinity and helper hash:

```sh
python3 bench/latency_controls.py --before BEFORE_BUILD --after AFTER_BUILD \
  --output artifacts/performance/latency-controls
```

It defaults to the last CPU allowed to the current process; `--cpu N` selects
another allowed CPU. Affinity prevents migrations across CPUs, not competition
with other work, frequency changes or cache/code-placement effects. These
controls are descriptive and do not establish statistical significance.

The WASM correctness tests can select an immutable package explicitly instead
of using the default playground output. A missing explicit package fails:

```sh
NAPI_VM_WASM_PACKAGE="$PWD/artifacts/performance/collection-index-wasm" \
  node --test tests/wasm/browser-build.test.mjs
```

This verifies the chosen target package. The public-API driver below measures
WASM execution in Node; it does not establish browser UI performance.


## Compare public native, WASM and Node APIs

The engine build also exports `workloads.json` from the same canonical Rust
fixtures, with source hashes and checked numeric outcomes. Use that export for
all three public adapters:

```sh
python3 bench/platforms.py build --engine-build artifacts/performance/engine-build \
  --output artifacts/performance/platform-build
python3 bench/platforms.py run --build artifacts/performance/platform-build \
  --output artifacts/performance/platforms.json --samples 7 --iterations 3 --processes 3
python3 bench/platform_ratios.py artifacts/performance/platforms.json
python3 bench/platforms.py compare BEFORE.json AFTER.json
```

`fresh_source` includes unique-comment source execution, context creation and
release. `warm_source` invokes the same source on reused owners. The native API
returns a string, WASM returns a result/log record, and Node's `vm` returns a
number. Their builtin sets, caching, result conversion and collection policies
differ. These are public API costs, not isolated interpreter instruction costs.
Native/WASM release their owners synchronously; Node drops context references
and relies on V8 collection. Host GC runs between batches, outside timers.
Five warmup calls do not establish an identical JIT tier across engines.

The report retains every batch in three fresh Node processes and summarizes
medians of process medians with MAD. It records separate first-use components:
native addon load, Node's vm builtin load, WASM glue import, WASM compilation,
and WASM instantiation/initialization. These exclude Node process startup and
are measured after artifact hashing has touched files, so they do not represent
cold disk loading. Construction/disposal has separate ns/instance rows; no
startup components or workloads are combined into a universal score.

Builds hash the complete WASM package, including linked JavaScript snippets,
and preserve compiler inputs and helper hashes. Runs verify those artifacts;
comparisons reject changed fixtures, settings, dependencies and harnesses.
`platforms.py stage --build OLD_BUILD --output NEW_BUILD` can reuse verified
compiled bytes while updating packaging/measurement metadata. It preserves the
original build record, requires an identical timed script and allows only the
package manifest whitelist to change. It does not claim the artifacts were
rebuilt from current runtime sources.

The `playground:build` command now includes wasm-bindgen's static helper snippets
in the generated npm package whitelist. The local pack/extract test verifies
that the resulting archive imports and passes the WASM tests. This addresses the
omission observed with the installed wasm-pack 0.15.0; its
[manifest generator](https://github.com/wasm-bindgen/wasm-pack/blob/v0.15.0/src/manifest/mod.rs#L603-L649)
lists generated top-level files without these helper directories.


## WASM result construction

`WasmVm.run` now constructs its result record in one static JavaScript helper;
nonempty log entries use one helper each. Object literals create own data
properties, preserving fresh result/log-array identities and avoiding inherited
setters on host `Object.prototype`. Error text, Unicode, property descriptors
and log order have explicit regression coverage.

A diagnostic derived copy of the generated glue counts imported calls while
leaving the compiled WASM bytes unchanged. For the retained tiny fixtures,
`bench()` drops from 22 to 11 imported calls, constant evaluation from 16 to 5,
and a one-line log from 53 to 37. Instrumented glue is not used for latency
measurements; fewer imported calls alone do not establish a timing improvement.
The paired public API reports measure that separately and retain unchanged
native/Node controls.


The [paired WASM review](../bench/baselines/platforms-2026-11-07/validation.json)
retains the fixed 49-workload before/after matrix, the original staged build
provenance, complete raw process samples, and instrumented import-count records.
Only `src/wasm.rs` differs in the compiler inputs; native addon bytes match.
The tiny warm-source fixture has WASM after/before ratios **0.739 and 0.928**
(50.10→37.04 µs and 45.12→41.86 µs). The second reduction is close to the
observed process scatter, and unchanged native/Node controls also move. No
stable percentage latency gain or neutrality claim is established. Heavy-case
movement is not attributed to result construction: host variation and changed
WASM code/layout are not isolated. Twenty WASM tests pass, including from the
extracted npm archive; two packaging tests, 21 benchmark-validator tests, WASM
Clippy and the Vite production build pass. At that capture, all 166 inputs of
the previously validated core worker matched its 102,992-variant gate; the
binding change was checked separately by WASM tests.

## Expanded synchronous comparison

The [five-group capture](../bench/baselines/full-suite-platforms-2026-11-07/suite.json)
uses the 65-workload matrix: 390 bare-engine timing rows, 398 public API rows,
81 host rows, eight memory rows and four HTTP rows. The guest sources and
expected outcomes match exactly across the bare and public layers. The paired
WASM review above deliberately retains the earlier fixed 49-fixture matrix;
those captures are not a before/after pair with this expanded suite.

Selected current bare-engine retained-call results are below. A ratio above one
means napi-vm took longer than pinned rquickjs for that fixture. These do not
measure Node/WASM bridges, context setup, asynchronous completion or I/O.

| Fixture | napi-vm / rquickjs time |
| --- | ---: |
| Arithmetic loop | 2.07× |
| Property reads | 4.59× |
| Array chain | 22.57× |
| Class methods | 22.99× |
| Generator iteration | 25.41× |
| Proxy / Reflect get | 14.47× |
| ISO Date parsing | 6.87× |
| Typed-array bulk set | 17.65× |
| String normalization | 1.69× |
| BigInt division/remainder | 3.90× |

These measurements identify substantial remaining guest-execution work. Profile
the relevant family before attributing the gap to parsing, dispatch, builtin
implementation or allocation. The complete per-fixture ratios and raw samples
are retained; collection-key collision controls and boundary limitations from
the preceding reviews still apply.


Selected public warm-source medians from the same expanded capture illustrate
why the API boundary also matters. Values are µs/call; they include each API's
own result conversion and differ from the bare retained-call table above.

| Fixture | Native API | WASM API in Node | Node vm context |
| --- | ---: | ---: | ---: |
| Tiny call | 20.64 | 35.86 | 38.89 |
| Arithmetic loop | 1,981.66 | 2,803.07 | 46.62 |
| Array chain | 7,233.07 | 8,503.70 | 94.18 |
| Generator iteration | 642.44 | 565.98 | 60.03 |
| Typed-array bulk set | 381.00 | 499.91 | 63.25 |
| String normalization | 567.09 | 762.63 | 151.96 |

Use the [per-fixture ratios](../bench/baselines/full-suite-platforms-2026-11-07/platform-ratios.json)
for the full matrix, MAD and independent startup components. This capture does
not establish that one API is fastest for every program or that Node's context
is an interchangeable embedding API for a Rust application.


## Promise jobs and completed async work

`bench/async_jobs.py` compares bare napi-vm-core and pinned rquickjs with 13
shared sloppy-script fixtures: resolved promises, chains, fan-out, all/plain
values, all/promises, allSettled, race, any with rejections, thenable assimilation,
catch/finally chains, await/plain values, await/promises and async generators.
Every invocation must complete exactly once with its checked numeric outcome
and no rejection. Checks are outside timers. Context creation and fixture setup
are excluded; cyclic GC runs outside each batch on both engines.

```sh
python3 bench/async_jobs.py build --output artifacts/performance/async-build
python3 bench/async_jobs.py run --build artifacts/performance/async-build \
  --output artifacts/performance/async.json --samples 15 --iterations 10 --processes 3
python3 bench/async_jobs.py ratios artifacts/performance/async.json
python3 bench/async_jobs.py compare BEFORE.json AFTER.json
```

| Mode | Timed boundary |
| --- | --- |
| `enqueue` | Source evaluation through registration; draining occurs afterward, outside the timer |
| `checkpoint` | Drain already prepared pending jobs to idle; invocation/setup occurs outside the timer |
| `source_completed` | Public source evaluation and all pending jobs |
| `call_completed` | Retained `start()` guest function call and a complete checkpoint on both engines |

The same fixtures are exported by the immutable worker. The driver hashes
sources, dependency/configuration inputs, compiler/profile/environment, itself,
worker bytes and fixture exports. Reports retain all independent process runs
and aggregate medians/MAD of process medians. Validation rejects missing phases,
changed source/expected outcomes, invalid statistics and missing job diagnostics.
Comparisons require matching harness/dependency/host/build settings; runtime
source changes are listed explicitly.

The invocation source is cached by napi-vm and reparsed by rquickjs; retained
calls avoid that difference. Per-call timers are not overhead-subtracted. The
job diagnostics count actual jobs during a separate untimed invocation. Counts
match between engines for the current fixtures but remain implementation
observations, not a general definition of equal work. Five warmups and these
finite synchronous checkpoint runs do not measure timer/I/O waiting, external
host events, Node/WASM wrapper APIs, or identical nested execution tiers.


The [async candidate review](../bench/baselines/async-jobs-2026-11-07/validation.json)
retains two before/candidate pairs, 104 rows each, with three processes, 15
samples and 10 iterations. The observed job counts match across engines for
all 13 fixtures. Only the promise builtin and registration implementation differ
in compiler inputs. Native species reactions now reuse their real result
promise instead of allocating an internal placeholder. A custom species may
return another object and retains the fallback path. GC regression tests cover
native subclass results and rejection propagation without direct guest roots,
alongside the existing custom-species closure test.

Fan-out checkpoint after/before ratios are 0.925/0.898; all/promises checkpoint
ratios are 0.928/0.924. The corresponding rquickjs controls are approximately
unchanged. Complete-call improvements are smaller and variable. Async-generator
enqueue is 2.1%/4.4% slower; complete-call ratios are 1.010/1.015. Latency
neutrality and a universal improvement are unproven. The review retains these
concerns. Its own complete reviewed conformance gate passed all 102,992 variants
with the same configuration, an exact variant set and zero outcome changes;
[worker/source checks](../bench/baselines/async-jobs-2026-11-07/source-checks.json)
verify the compiled inputs. Async-generator enqueue neutrality remains unproven.

The supplemental Linux owner-memory probe isolates pending fan-out from latency:

```sh
NAPI_BENCH_BINDING=/absolute/path/to/napi-vm.node \
  node --expose-gc bench/performance/promise-memory.cjs 512 native
NAPI_BENCH_BINDING=/absolute/path/to/napi-vm.node \
  node --expose-gc bench/performance/promise-memory.cjs 512 custom
```

Use matching before/candidate addons and repeated fresh processes. The probe
checks every resolved result, samples pending state after explicit guest GC,
and releases guest roots before a final collection. `native` exercises result
promise reuse; `custom` exercises the preserved fallback. Tracked registry
cells and whole-process RSS are separate measurements; neither is an exact
allocation-byte total. Sources, addon/helper digests and every phase are retained.


The pending-fan-out [memory comparison](../bench/baselines/async-jobs-2026-11-07/promise-memory.json)
retains three independent processes per build/case at 32, 512 and 4,096
registrations, with native and custom species controls. After explicit guest
GC, the native case has exactly one fewer tracked cell per registration:
359→327, 5,639→5,127 and 45,063→40,967 cells above ready. Custom-species
counts are unchanged. After root release, every case returns to a constant
one-cell increase over ready in both builds, independent of fan-out size.
This is evidence of reduced pending bookkeeping, not a byte allocation count.
At 4,096 native registrations the observed median RSS increase falls from
44.74 to 42.14 MiB, while the custom control also moves (26.71→26.04 MiB).
RSS is variable and includes allocator caches; no universal RAM reduction is
inferred from those process observations.

The [six-group capture](../bench/baselines/full-suite-async-2026-11-07/suite.json)
completes 390 embedded-engine rows, 398 public API rows, 81 host rows, eight
memory rows, four HTTP rows and 104 async rows. Native tests pass all 98 cases
and the explicit candidate WASM package passes all 20, with no skips. The
candidate now has its own complete reviewed Test262 result: 102,992 passing
variants, no skips or non-passing outcomes, with current worker/source hashes
verified. The preceding core gate was not reused for changed promise code.


## Intl and Temporal capability-aware comparisons

The shared [fixture registry](../bench/international-workloads.json) now contains
27 synchronous cases. The original 23 are joined by three cached bound-method
getter fixtures and a date-format reuse fixture. Number/date/collator/plural construction includes the
first operation on each new instance; separate reuse cases retain a formatter.
Other cases cover parts, segmentation, list/relative/duration formatting,
display names, locale maximization, and Temporal date/datetime/instant/zoned
arithmetic and parsing. Locale-sensitive fixtures specify `en-US`; dates use
UTC and fixed inputs. Guest assertions and numeric outcomes are identical.

```sh
python3 bench/international.py build --output artifacts/performance/international-build
python3 bench/international.py run --build artifacts/performance/international-build \
  --output artifacts/performance/international.json --samples 15 --iterations 10 --processes 3
python3 bench/international.py ratios artifacts/performance/international.json
python3 bench/international.py compare BEFORE.json AFTER.json
```

The Rust worker exports canonical setup/invocation sources and a member-presence
probe. All three backends preflight every available fixture against its expected
result before timing. The current complete matrix has three modes and three backends:
**243 rows**, including measured, unsupported and incompatible-result rows.
Unsupported or mismatching cases have no samples or ratios; they remain visible.
A later implementation fix can change a status to measured, but cannot invent a
before timing or claim a speedup from the earlier mismatch. Validators enforce
complete matrices, consistent capability/semantic outcomes, matching hashes and
settings, and raw/aggregate statistics. `--groups international` runs this layer
without the other suite groups.

`fresh_source` includes unique-comment source allocation, context construction,
builtins, evaluation and release; napi-vm includes explicit guest cycle GC.
Node releases context references and uses host GC outside batches. `warm_source`
uses each source API; napi-vm's source cache remains active. `warm_call` retains
the guest function. Checks of numeric batch totals occur outside timers, while
guest checks, result conversion and summation remain timed. Each backend gets
five warmups and collection outside warm batches. Rust and Node execute in
separate processes; reports retain all batches from independent processes and
summarize process medians/MAD. These are whole guest fixtures, not isolated ICU
instruction costs, and do not prove equal nested tiers or browser performance.

The configured rquickjs 0.14.0 `std`/full-context build reports none of the 15
required Intl/Temporal constructors. The captured Node build supplies all of
them. The original 23-fixture capture passes 23 Node fixtures and 22 napi-vm
fixtures; the later 27-fixture capture passes 27 and 26 respectively. napi-vm
supplies all 15 members. Its DisplayNames implementation returns `US` for the localized
`United States` fixture, so all three modes for that case are explicitly
non-equivalent. The implementation performs structural validation and code
fallback; constructor presence and conformance results do not establish that
it performs the same localized-name work as Node. That timing is excluded.

Current retained-call examples (napi-vm time / Node context time) are:

| Fixture | Ratio |
| --- | ---: |
| Number construction plus first format | 0.89× |
| Number formatter reuse | 2.94× |
| Number parts | 7.93× |
| Date parts | 11.84× |
| Collator reuse | 8.96× |
| Plural construction plus first selections | 0.46× |
| Duration parts | 0.81× |
| Temporal date addition | 11.26× |
| Temporal instant parsing | 3.87× |

Ratios below one mean napi-vm took less time in that fixture. The source review
found discarded deep copies in NumberFormat, DateTimeFormat and Collator brand
checks before returning a cached method. The getter optimization below removes
that copying; these original baseline ratios do not attribute the entire gap
to that operation.


The [retained Intl/Temporal baseline](../bench/baselines/international-2026-11-07/validation.json)
uses three processes, 15 samples and 10 iterations. Its 207 rows contain 135
measured rows, 69 unsupported rows for configured rquickjs, and three
DisplayNames mismatch rows. A supplemental [complete-output check](../bench/baselines/international-2026-11-07/output-check.json)
compares full NumberFormat/DateTimeFormat/DurationFormat parts, segmentation
details and representative Temporal strings/results through the default native
addon and Node. Nine checks match; DisplayNames alone differs. The addon digest
matches the seven-group capture and uses the same core inputs. This is fixture
validation, not all-locale or all-method parity.

The [seven-group capture](../bench/baselines/full-suite-international-2026-11-07/suite.json)
retains 390 embedded-engine, 398 public API, 81 host, eight memory, four HTTP,
104 async and 207 Intl/Temporal matrix rows. Intl non-measured rows are
capability/semantic observations, not timing samples. All group validators,
28 benchmark-validator tests and the new Rust harness Clippy check pass.
Selecting only the preceding six groups records an omitted international layer.

For ongoing comparisons, keep [rquickjs's Rust Runtime/Context interface](https://docs.rs/rquickjs/0.14.0/rquickjs/)
as the embedded baseline, and use [Node's V8 contexts](https://nodejs.org/api/vm.html)
for the public JS and Intl/Temporal layers. The current rquickjs configuration
cannot provide an Intl/Temporal timing reference. Select per-operation results
and needed capabilities rather than a universal engine score; broader I/O,
loaders, async host bridges and further Intl inputs remain in the audit inventory.


## Intl receiver checks and cached bound getters

NumberFormat and DateTimeFormat unwrapping now test internal-slot presence
without copying locale/options strings. Their bound `format` getters and the
Collator `compare` getter validate the receiver brand directly before checking
the cached method. Slot snapshots remain owned in operations that consume
formatting data. Metadata layout, cached function construction, legacy
fallback-symbol lookup and cycle-tracing fields are unchanged.

Four new behavioral tests cover wrong-brand receivers with populated caches
(including forged legacy wrappers that return another formatter type),
observable proxy/legacy unwrapping, descriptors/per-instance identity and
extracted-method liveness through GC. Both prepared and AST execution paths
are exercised where applicable. Node and WASM have matching binding checks.
Reference checks confirm that NumberFormat/DateTimeFormat legacy unwrapping
reads the fallback-symbol property,
while Collator rejects a proxy directly. Those observations are preserved.

The change passes 912 core tests, 45 targeted Intl tests and core Clippy.
Its worker passes the **complete Intl402 selection: 6,764 variants**, with an
exact variant-set/outcome match against the Intl subset of the preceding
reviewed full report. Only the three Intl implementation source files differ
in the worker input snapshot. This is a scoped Intl conformance gate; the older
102,992-variant report describes the preceding worker and is not relabeled as
a new whole-corpus result.

The paired experiment freezes matching 27-fixture exporters and uses three
processes, 15 samples and 10 iterations for each capture, alternating build
order in the second pair. The getter fixtures check repeated cached identity
at 1,000 accesses; the formatter fixtures include the complete checked guest
body. Unsupported rquickjs rows and the DisplayNames mismatch retain their
non-measured statuses. Raw Node controls and unrelated native workloads are
kept alongside every changed-path result.


The two retained-call pairs show the following **candidate/before time ratios**
(lower means less time). Node controls for the getter fixtures range from
0.986 to 1.020; raw process/batch medians and MAD are retained.

| Fixture | Pair 1 | Pair 2 |
| --- | ---: | ---: |
| NumberFormat cached getter, 1,000 accesses | 0.744 | 0.780 |
| DateTimeFormat cached getter, 1,000 accesses | 0.791 | 0.819 |
| Collator cached getter, 1,000 accesses | 0.905 | 0.938 |
| Complete number-format fixture, 100 iterations | 0.864 | 0.900 |
| Complete collation fixture, 1,000 iterations | 0.926 | 0.967 |
| Number parts | 0.934 | 1.022 |
| Date parts | 0.984 | 1.011 |
| Complete date formatting | 0.997 | 1.018 |

The getter and number-format gains repeat. Parts and complete date-format
changes are inconclusive; the collation fixture has smaller gains than the
isolated getter case. Other native/Node timings also vary, so these are
per-fixture observations, not global speedup or neutrality claims. The current
27-case matrix has 243 rows: 159 measured, 81 unsupported, and three
DisplayNames mismatches. The earlier 23-case captures retain their original
fixtures and are not treated as matching optimization pairs.


The [getter review](../bench/baselines/intl-getters-2026-11-07/validation.json)
retains all four raw captures, both comparisons, per-case process medians/MAD,
Node controls, exact before/candidate source snapshots and regression logs.
[Scoped source checks](../bench/baselines/intl-getters-2026-11-07/source-checks.json)
verify the worker digest, current compiler inputs and exact prior Intl variant
set. The strengthened wrong-brand/legacy tests pass against Node and an
immutable before addon as well as the optimized bindings. Final checks pass
99 Node tests and 21 explicit-package WASM tests, with no skips, plus 28
benchmark-validator tests.

The [updated seven-group capture](../bench/baselines/full-suite-intl-getters-2026-11-07/suite.json)
contains 390 embedded-engine, 398 public API, 81 host, eight memory, four HTTP,
104 async and 243 Intl matrix rows. Every selected group and report validator
completed; report digests, shared synchronous fixtures and stable host build
fingerprints are verified. The broader I/O/loader and host/API audit remains
open in the operation inventory.
