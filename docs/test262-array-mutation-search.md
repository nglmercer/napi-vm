# Array mutation, search and map/filter quick wins

Base: merged PR #29 (`1963230`). A fresh focused baseline and source inspection
confirmed the remaining implementation gaps. Baseline worker SHA-256:
`fdf183158d1d62f4b3de8e39472456d0fe4da5f6e81a290078ca41ab5c4c360c`.
This matches the previously archived full baseline under `evidence/generic-array/`.

## Changes and shared operations

- `push`, `pop`, `shift`, and `unshift` now use existing ToObject, logical
  ToLength, throwing Set, HasProperty/Get, and throwing Delete operations.
  Native arrays follow the same property semantics as generic receivers,
  including inherited accessors, holes, descriptors and Proxy traps. Zero-argument
  push/unshift and empty pop/shift still perform the required length write.
- Growth beyond MAX_SAFE_INTEGER throws TypeError before writes. The existing
  native array allocation cap is retained as a separate guard; generic object
  lengths are not limited by the capacity of allocated arrays.
- `indexOf` and `lastIndexOf` use logical numeric lengths and indexes through
  MAX_SAFE_INTEGER. They skip holes using HasProperty/Get, retain strict equality,
  normalize returned zero to +0, and preserve omitted versus explicit undefined
  fromIndex behavior. Empty receivers return before converting fromIndex.
- Bounded searches retain their existing per-iteration execution/cancellation
  checks. Logical lengths beyond the existing array bound additionally consume
  the loop budget. This preserves previously passing bounded sparse searches
  without allowing unbounded generic scans. The initial comparison caught four
  regressions from charging all bounded search iterations to the loop budget;
  these were fixed and covered by a regression before the final measurements.
  No global budget, runner timeout, or test assertion was relaxed.
- `map` and `filter` use logical lengths, numeric indexes and execution budgets.
  Existing ArraySpeciesCreate logic accepts the logical length, so custom
  constructors observe the full value. Default array creation checks the existing
  allocation cap before allocating. Numeric result indexes reuse the existing
  CreateDataProperty descriptor builder. Sparse map results, dense filter results,
  callback receivers, species order, and abrupt completions are retained.

Only the Array built-ins file and regression tests implement behavior changes.
There are no AST, bytecode, dependency, loader, or GC changes.

## Focused results

Pinned Test262 revision: `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.
Four workers, five-second timeout, no feature skips or corpus changes.

| Method | Failing before | Failing after | New passes |
| --- | ---: | ---: | ---: |
| push | 24 | 2 | 22 |
| pop | 20 | 0 | 20 |
| shift | 24 | 0 | 24 |
| unshift | 28 | 0 | 28 |
| indexOf | 12 | 6 | 6 |
| lastIndexOf | 10 | 6 | 4 |
| map | 18 | 6 | 12 |
| filter | 20 | 20 | 0 |

All Array variants advance from **5,529 to 5,645 / 6,117**, with **116 new
passes and zero regressions**. The final focused run has no skips, timeouts,
crashes, or harness errors. All pop, shift and unshift variants pass.
Filter's logical-length correction has regression coverage but is not credited
with any new Test262 passes in this selection.

## Full-corpus results and replay

The complete 102,956-variant run advances from **76,959 to 77,085 passes**
(74.7494% to 74.8718%): **126 new passes, zero regressions**. The additional
ten gains outside the focused Array selection are staging Array/strict tests.
All 126 gains reproduce on both binaries. The file-based replay selects 130
variants: the baseline passes four and fails 126; the final engine passes all
130. Its 126 newly passing variant keys exactly match the full comparison.

The same two deep-WeakMap staging tests time out. There are no feature skips,
crashes, harness errors, other status transitions, or result substitutions.
Previously documented Atomics FIFO outcomes remain unchanged in this comparison.
Only final corrected measurements contribute to the summary and latest report.

## Validation

Eight regression tests require forced bytecode and separately execute the parsed
AST. They cover logical lengths, negative indexes, signed zero, holes, inherited
elements, boxed receivers, getter/Proxy order, read-only descriptors, partial
mutations before exceptions, species observation, omitted/undefined arguments,
execution limits, and the previous bounded-search policy.

Root Rust suite: **351 passed**, with the same three LSP tests failing at Unix
socket binding with PermissionDenied in this execution environment. Core suite:
**642 passed, zero failed**. Formatting, strict root/core Clippy, N-API, host
WASM-feature, runtime feature checks, and all 12 Test262 tooling tests pass.
Actual wasm32 execution and full-corpus differential execution across both tiers
remain unvalidated. The root/core Rust test commands retain the command-line
codegen-units=1 override used in the previous batch to avoid a local Rust 1.99
zero-byte codegen object issue. No repository build configuration was changed;
release builds use the normal release configuration.

## Reproduction

Run from the repository root with the clean pinned corpus at `../test262`:

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
python3 tools/test262/run.py ../test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --path built-ins/Array \
  --output artifacts/test262/array-mutation.json
python3 tools/test262/triage.py artifacts/test262/array-mutation.json \
  --output artifacts/test262/array-mutation-triage.json
python3 tools/test262/run.py ../test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --output artifacts/test262/array-mutation-full.json
```

Build the baseline in a detached worktree at `1963230` and run the same selections.
`tools/test262/compare.py` accepts the compressed evidence archives directly and
compares all selected variants. Raw reports, comparisons, triage, implementation
patch, exact replay selections and validation logs are archived under
`tools/test262/evidence/array-mutation-search/`. The separately labelled diagnostic
search-budget comparison is superseded by the final zero-regression reports.

## Deferred failures

The two remaining push variants and the remaining search variants allocate arrays
beyond the existing engine allocation limit. Remaining map/filter cases include
Arguments-object identity, constructor/prototype scenarios, and allocation limits.
These need separate runtime diagnosis. No allocation limit was relaxed to make
tests pass.

Array has 472 remaining non-passing variants. Other Array methods, Set composition,
locale-sensitive strings, precise Math summation, Temporal, advanced RegExp, realm
architecture and GC changes remain outside this batch. No Test262 corpus file,
expected outcome, harness assertion, feature skip, or runner limit was changed.
