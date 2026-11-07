# Runtime ownership, evaluation, and collection

Node.js 22 or newer is required, matching `package.json`. The runtime remains
single threaded: guest `Rc`/`RefCell` values are never available to two execution
contexts at once.

## Synchronous and asynchronous ownership

`VM.run` and `VM.runAsync` continue to share globals, module live bindings,
closures, exposed host functions, and symbol identities. A VM starts one worker
lazily on its first async call. That worker waits on a channel between calls;
there is no per-call thread creation or idle polling. Overlapping operations and
reentrant host-to-guest calls still fail with the busy error.

Each VM owns a detached arena containing its heap registry and collector roots,
shape root and counter, symbol registry and counter, and collection prototype
cache. The runtime gate leases and installs the arena for an operation and
restores the previous thread context on every exit, including unwinding. Native
threads retain no reference to that arena in TLS between leases. Input
marshalling that creates guest objects also runs under the arena lease. The
in-process native-addon backend retains its original thread affinity in Rust
owners; the N-API `VM` surface does not install that backend.

`AsyncSession` remains the canonical asynchronous owner when no synchronous VM
access is needed. It creates and destroys its interpreter on its persistent
owner and transfers only wire values to Node. Prefer it for high frequency
asynchronous handlers. No migration or versioned removal of `runAsync` occurs.

Exported-function finalizers enqueue only a generational export ID. They never
acquire the runtime gate or wait for a worker. The owner applies releases at the
next operation. Slots reuse a free list; stale and repeated releases are harmless.
Each live slot pins its guest function until release. Dropping the runtime removes
all remaining pins in the runtime's arena.

## Evaluation and caches

Preparation parses source, selects bytecode when supported, and verifies it.
Unsupported units explicitly retain the AST evaluator. A verifier failure is an
internal compiler error and never falls back. `runCode`, synchronous VM runs,
legacy async runs, async session runs/evaluations, modules, CommonJS wrappers,
Node-API nested scripts, and browser execution use this pipeline.

`DrainPolicy::UntilIdle` is the default for `runCode`, `VM.run`, `runAsync`, and
`AsyncSession.run`. `AsyncSession.evaluate` uses `None`; `Microtasks` is available
to Rust embeddings. Pending checkpoints retain the previous hard budget and must
complete before another script is admitted. Nested wrappers execute the prepared
body without starting a new execution or draining their caller's jobs.

Prepared programs are cached per interpreter/owner by exact source and source
kind, with LRU bounds of 64 entries and 2 MiB of source. The shared parse cache
compares exact source contents and retains at most 1024 entries and 8 MiB of
source, evicting one least-recently-used entry at a time. Parse failures are not
cached. Bytecode's mutable property-cache and tier state is owner local.

`vm.evaluationStats()` and `await session.evaluationStats()` return JSON diagnostic
strings containing the last tier and prepared-cache counters. They are intended
for testing and profiling, not execution decisions.

## Collector lifecycle and host roots

Fresh `runCode` converts its result to the final string, drops the interpreter and
result, then collects cycles in the same isolated arena. Persistent Node owners
check allocation debt after converting execution results. The default threshold
is 4096 tracked containers. Rust embeddings can use `set_collection_threshold`
and `maybe_collect_cycles` at a quiescent boundary; zero disables automatic
collection. Do not call the latter while holding unpinned results.

`vm.collectCycles()` returns the reclaimed-container count and `vm.heapStats()`
returns a JSON string with tracked and cumulative reclaimed counts.
`await session.collectCycles()` provides explicit collection on its owner.
Collection refuses to run during execution or while opaque generator/async stacks
are suspended. Releasing or completing the suspension allows the next safe pass.
Multiple interpreters on a thread publish separate root sets. The final owner
teardown releases cached intrinsic pins before collection, including an
AsyncSession worker exit.

Externally retained values must use `heap::RootPin`, or be reported by
`HostBridge::trace_roots`. Audited roots include exports, plugin instances, module
scopes and caches, queued jobs and settlements, native callback records, N-API
handle scopes and strong references, deferred promises, TSFN callbacks, async
resources, native wraps/finalizers/type tags, external buffers, and pending/fatal
exceptions. Wire-only Node bridges and browser bridges retain no guest values.
Custom Rust closures that capture opaque guest values must pin them explicitly.

## Bytecode and scheduling

Ordinary bytecode calls borrow verified caller register slices. Bound, spread,
host, native, generator, and async paths materialize ownership when required.
Capture-free slot-only functions reuse the parent environment. Captured slots,
arguments and nested functions retain distinct environments. Class constructors
use the same capture analysis.
`this` stays in the bytecode frame, including `super()` calls. Register and slot
buffers are cleared before returning to a pool bounded to 16 buffers, each with
at most 4096 retained entries.

Instructions are copied by value. Fuel decrements remain exact on every
instruction. Cancellation/deadline polling occurs at most every 64 dispatches
(weighted costs can trigger an earlier check), and zero-cost instructions count
against that bound. Function entries, calls/constructors, loop iterations, jobs,
and host transitions also check controls. A native operation without a polling
hook still determines its own interruption latency; the bound describes bytecode
dispatch rather than arbitrary blocking host code. Sleeping owners use wake signals.
Tier counters default to `TierTracking::Disabled`; tests/profilers can select
`CountersOnly`, and installing a backend enables its required feedback.

Property caches are indexed only by property sites. Verification checks both the
compact table size and every index. Monomorphic guards, key verification,
accessors, readonly paths, and invalidation retain their existing behavior.
Polymorphic caches and a borrowed native ABI are deferred unless measurement
justifies their additional complexity.

Timers use a sorted vector up to 128 entries and an ordered tree with an ID index
above that threshold. Promotion persists until the queue is empty. Empty vector storage is reused;
a queue that has promoted also reuses empty tree/index storage, retaining at
most 16,384 ID slots. Small-only queues allocate no index. Deadline order,
FIFO ties, nonaliasing IDs, cancellation, root tracing, clocks, fairness, and
checkpoint/backpressure behavior remain independent of representation.

Diagnostics retain one shared source string. Line offsets are computed lazily;
CRLF, trailing newlines, and missing final newlines match `str::lines`. CommonJS
and nested script evaluation restore the previous context on success and error.
Module sources share `Arc<str>` between bookkeeping and the interpreter.

## Evidence and release gates

The [runtime evidence](implementation-evidence/results.md) records a historical
set of measurements and acceptance decisions for the revision it tested. It
is not a current release verdict. For the current benchmark procedure and
baseline, see [performance](performance.md).

See `implementation-evidence` for raw commands, results, memory regressions and
benchmark runs. Build all variants before timing, run identical source workloads,
and interleave baseline and modified runs. Allocation metrics are from the
serialized `call_metrics` example; queue metrics are from `timer_queue_matrix`.
Criterion provides confidence intervals for existing Rust canaries. RSS thresholds
are tested in dedicated processes after warming allocator caches. Do not infer a
speedup from one run or include competing builds in timing samples.

Local checks alone cannot establish cross-platform release readiness. Review
the current CI workflows and remote results for the platforms being released,
as well as generated bindings and retained-memory results.

## Review guide

The historical workstreams below map to principal review surfaces; shared
interpreter and ownership changes cross more than one workstream. Paths are
relative to the relevant crate or test directory.

| Workstream | Principal implementation | Regression/evidence |
| --- | --- | --- |
| 1. Compiler lifetimes | `bytecode/compiler.rs`: borrowed source views and owned synthesized slices | `compiler_memory.rs`, old/new RSS |
| 2. Async ownership | `runtime.rs`, `bindings/vm.rs`, owner-local TLS contexts | alternating sync/async, finalizers, worker teardown |
| 3. Collection lifecycle | `heap.rs`, host root tracing, export slab, plugin JSON boundary | pins, multiple interpreters, suspension, 20,000 persistent cycles |
| 4. Unified evaluation | interpreter preparation/options, Node/Wasm/CommonJS routes | tier/cache/drain and AST-bytecode parity |
| 5. Small calls | borrowed args, frame metadata, bounded activation pool | allocation matrix, host-panic scope restoration |
| 6. Dispatch controls | Copy instructions, exact fuel, bounded polling, tier policy | cancellation/deadline bounds, scheduler budgets |
| 7. Timer queue | small sorted vector and indexed tree | hybrid ordering/cancel/root tests, threshold matrix |
| 8. Diagnostics | shared source and lazy offsets | CRLF/line-boundary tests |
| 9. Property caches | compact property-site indices and shape-key reuse | verifier rejection, mono/mega/shape canaries |
| 10. Source caches | exact-key bounded parser LRU and per-owner programs | forced hash collision and LRU tests |
| 11. Growth/duplication | generational exports and shared module sources | stale releases and export churn |
| 12. Release quality | Node requirements, docs, expanded native CI and focused Miri | historical local gate logs; check current remote results |

Bytecode guest errors and Rust host panics clear/recycle activation storage and
restore bytecode function scopes. A Rust panic still propagates to its caller; it is not
converted into an AST fallback or successful execution. The legacy N-API async
boundary reports its existing execution-panic error after unwinding cleanup.
