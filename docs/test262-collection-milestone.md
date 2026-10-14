# TypedArray, Set composition, and observable Array methods

Base: merged PR #30 (`dc1511fa534a469ad7b9dffcba193627f5b95318`).
Corpus: `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.

## Changes

1. Observable Array operations: generic `sort`, corrected `join`, and new
   `toLocaleString`. Sort reuses ToObject/ToLength, HasProperty/Get, existing
   stable merge sort and comparison, throwing Set and Delete operations. Holes,
   inherited elements, getters, comparator mutations, and partially completed
   writes use the ordinary shared operations. Stringification reads properties
   and invokes conversions in order, forwards locales/options, preserves the
   existing string limit, and uses a scoped array recursion guard. TypedArray
   join's separator conversion/live reads are corrected in this same commit:
   Test262 compares Array and TypedArray joins, so fixing only one exposes an
   existing bug in the other and causes two apparent regressions.
2. TypedArray adds `copyWithin`, `reduceRight`, `findLast`, `findLastIndex`, and
   `with`. Existing view validation, callback validation, abstract conversions,
   allocation, and element helpers are reused. Callback methods capture length
   and read the live view, passing the original receiver. CopyWithin copies raw
   bytes for overlap/NaN payload preservation. With converts index then value,
   validates against the current view, allocates the same type without species,
   and copies the captured initial length. Set now distinguishes typed and
   array-like sources, preserves raw same-kind encodings, handles overlap,
   rejects mixed Number/BigInt types even for empty sources, and observes
   length/indexed reads. Fill converts once before start/end and revalidates.
   Native methods use existing name/length/descriptor support.
3. Set adds union, intersection, difference, symmetricDifference, isSubsetOf,
   isSupersetOf, and isDisjointFrom. One GetSetRecord helper validates/captures
   size/has/keys in order. Algorithms select the specified size branch, use
   SameValueZero, dedupe and normalize new zero entries, preserve insertion
   order, close iterators on early predicate returns, and allocate intrinsic
   Sets without guest add/species hooks. Live receiver scans track existing
   entry-pair identities across compaction/reinsertion; storage is unchanged.
   Shared guest iterator operations handle next/done/value access and closing.

The work is three stacked commits/PRs in the order above. No Test262 corpus,
runner, assertion, dependency, global resource limit, host API, or interpreter
architecture changes are included. Bounded Array scans retain execution checks;
logical lengths beyond the allocation bound additionally consume the existing
loop budget. Obsolete private snapshot-rendering helpers are removed.

## Validation

Final results, worker digests, and before/after/replay details are recorded in
`tools/test262/evidence/collection-milestone/summary.json`. Compressed raw reports,
comparisons, triage, check logs, implementation patch, and reproduction paths are
archived alongside it. Replay results do not replace full-corpus outcomes. Timing-sensitive Atomics
FIFO/notify gains are retained in raw full results but excluded from credited
code improvements, because no Atomics implementation is changed. An initial first-PR run
under concurrent Rust compilation recorded a timeout in unchanged
`Array.prototype.indexOf/15.4.4.14-10-1.js`; both variants pass on direct replay
with that same worker. The initial raw diagnostic is retained, and focused
selections affected by compilation-time contention are rerun after builds and
the complete corpus finish.

| Measurement | Before passes | After passes | New passes | Lost passes |
| --- | ---: | ---: | ---: | ---: |
| Focused Array / TypedArray / Set (9,771 variants) | 8,003 | 8,958 | 955 | 0 |
| PR 1: Array and coupled TypedArray join | 8,003 | 8,136 | 133 | 0 |
| PR 2: remaining TypedArray changes | 8,136 | 8,656 | 520 | 0 |
| PR 3: Set composition | 8,656 | 8,958 | 302 | 0 |
| Complete corpus (102,956 variants), raw | 77,085 | 78,127 | 1,045 | 3 |

Raw full pass rate rises from **74.8718% to 75.8839%**. All **1,042 credited
code improvements** reproduce on both workers. Three timing-sensitive Atomics
FIFO/notify gains remain in the raw report and are excluded from credited gains,
including one that happened to reproduce in the replay. No Atomics fix is claimed.

The three raw lost passes are timeouts: `staging/sm/Array/sort_small.js` (script)
and `staging/sm/Proxy/ownkeys-linear.js` (both modes). These raw outcomes are
retained. Isolated runs at the same five-second limit show the final sort worker
passing both modes, while the baseline itself times out in strict mode. Both
workers time out in both modes of the unchanged Proxy stress test. This identifies
baseline timing sensitivity on this host rather than a new functional failure;
there are **zero unexplained regressions**. The isolated results, elapsed times,
full results, and replay results are archived separately. No timeout is raised,
assertion weakened, test skipped, or full-run outcome replaced.

Full counts include nine timeouts versus two in the archived baseline: the three
lost passes above, four previously failing variants that time out (Unicode RegExp,
a comment stress test, and large TypedArray sorting), and the same two deep WeakMap
timeouts. These nonpassing transitions are retained and replayed as well.
Root tests: **362 passed, three existing LSP Unix socket PermissionDenied failures**.
Core: **642 passed, zero failed**. Formatting, strict root/core Clippy, N-API,
host WASM-feature and runtime checks pass, as do all twelve Test262 tooling tests.

Eleven regression tests require forced bytecode and separately execute parsed
AST. They cover metadata/descriptors, callback receiver identity and live reads,
resizable views, raw encodings, coercion order, Number/BigInt arguments, Set
results/predicates, mutation during has, iterator close, species/add avoidance,
duplicate keys/signed zero, generic sorting, partial writes, holes, getters,
observable join/cycles, and locale argument forwarding.

The test build retains the command-line codegen-units=1 override used previously
for local Rust 1.99 zero-byte object archives. Repository build configuration is
unchanged. Actual wasm32 execution and full-corpus differential execution in both
tiers are not claimed. Workspace maintenance removed an earlier checkout and
interrupted full run; source was recovered exactly from uploaded GitHub blobs,
and final evidence was rerun rather than inferring the interrupted outcome.

## Deferred failures

Existing Map/Set forEach and collection iterators snapshot entries; their
mutation-sensitive tests need dedicated iterator lifecycle and GC validation.
Older TypedArray callback delegates also materialize arrays. TypedArray locale,
toSorted/toReversed, immutable buffers, constructor/tag issues, and remaining
large harness-loop failures are separate follow-ups. Existing BigInt increment
produces a Number in one fill coercion-count test; numeric update expressions
are outside this built-in patch. Existing resource limits were retained.

The Arguments audit found shared creation already installs callee, iterator
metadata, and attributes, while indexed arguments are explicitly detached
copies. Mapped arguments require parameter/environment aliasing across execution
tiers. Remaining Array prototype failures also involve existing function
hoisting/prototype semantics; neither warrants an interpreter rewrite here.

## Reproduction

From the repository root with the clean pinned corpus at `../test262`:

```sh
cargo fmt --check
CARGO_INCREMENTAL=0 cargo test --locked --no-default-features --no-fail-fast \
  --config 'profile.dev.package.napi-vm-core.codegen-units=1'
CARGO_INCREMENTAL=0 cargo test -p napi-vm-core --locked --no-default-features \
  --no-fail-fast --config 'profile.dev.package.napi-vm-core.codegen-units=1'
cargo clippy --locked --no-default-features --all-targets -- -D warnings
cargo clippy -p napi-vm-core --locked --no-default-features --all-targets -- -D warnings
cargo check --locked
cargo check --locked --no-default-features --features wasm
cargo check --locked --no-default-features --features runtime
cargo build --locked --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py ../test262 --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 --jobs 4 --timeout 5 \
  --path built-ins/Array --path built-ins/TypedArray --path built-ins/Set \
  --output artifacts/test262/milestone-after.json
python3 tools/test262/triage.py artifacts/test262/milestone-after.json \
  --output artifacts/test262/milestone-after-triage.json
python3 tools/test262/run.py ../test262 --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 --jobs 4 --timeout 5 \
  --output artifacts/test262/milestone-full.json
python3 tools/test262/compare.py \
  tools/test262/evidence/array-mutation-search/full-after.json.gz \
  artifacts/test262/milestone-full.json --output artifacts/test262/milestone-full-transitions.json
```
