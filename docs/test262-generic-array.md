# Generic Array quick wins

Historical implementation checkpoint. Its full-run results below describe an
earlier source snapshot; current overall results are in
[latest.json](../tools/test262/latest.json).

Base: merged PR #28, `1a8ab73`. Fresh focused baseline and source inspection
confirmed the behavior before implementation. The baseline worker digest is
`1b23d8da7be54d43b56446311251528129c06ea9bd647917412c2b5fe1017129`,
matching the archived full-corpus baseline in `evidence/math-array-number/`.

## Changes

- `every`, `some`, `forEach`, `reduce`, and `reduceRight` box receivers with the
  existing ToObject helper before reading length and validating the callback.
  They use the existing logical ToLength operation through MAX_SAFE_INTEGER,
  rather than the allocation-capped helper that treated Infinity as zero.
  Each iteration checks the existing loop/execution budget, including holes.
  Numeric index reads reuse HasProperty/Get and preserve inherited properties,
  length snapshots, callback receivers, and abrupt completion order.
- `fill` uses generic length/index coercion and throwing Set operations. Both
  bounds are converted before writes; empty ranges perform no writes. Sparse
  elements, inherited setters, immutable string properties, and rejected writes
  now follow ordinary property semantics.
- `reverse` uses ordered HasProperty/Get/Set/Delete operations for generic
  receivers, including holes and inherited elements. Failed writes/deletions
  retain prior observable mutations. It does not reject frozen objects unless
  an operation actually attempts a forbidden mutation.
- `toSorted` reuses the existing stable merge sort, reads every input index
  before sorting, densifies holes, and creates a fresh ordinary array without
  species access. Comparison handles undefined without invoking the comparator,
  uses UTF-16 string ordering by default, and applies shared ToNumber to custom
  comparison results. The latter correction also applies to existing `sort`.
  Existing native-method installation supplies the standard metadata/descriptors.
- The new reverse path and existing copyWithin/splice paths share the same
  throwing deletion helper. No AST, bytecode, loader, GC, or dependency changes.

## Focused Test262 results

Pinned revision: `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.
Four workers, five-second timeout, no feature skips.

| Method | Failing before | Failing after | New passes |
| --- | ---: | ---: | ---: |
| every | 30 | 20 | 10 |
| some | 30 | 20 | 10 |
| forEach | 28 | 22 | 6 |
| reduce | 32 | 26 | 6 |
| reduceRight | 34 | 26 | 8 |
| fill | 24 | 0 | 24 |
| reverse | 22 | 0 | 22 |
| toSorted | 38 | 0 | 38 |

All Array variants: **5,405 → 5,529 / 6,117**, 124 gains, zero losses,
zero skips, timeouts, crashes, or harness errors. All variants for fill,
reverse, and toSorted pass. The other Array methods retain their existing
outcomes in this selection.

## Full-corpus results and replay

The complete 102,956-variant run advances from **76,823 to 76,959 passes**
(74.6173% to 74.7494%): **136 new passes, zero regressions**. All 136 gains
reproduce in an independent exact selection: 0 / 136 pass on the baseline
binary and 136 / 136 on the new engine. Beyond the 124 Array gains, two
TypedArray sort variants benefit from shared comparator-result ToNumber,
and ten staging variants now pass.

The same two deep-WeakMap staging tests time out. There are no feature skips,
crashes, harness errors, or other status transitions. Previously documented
Atomics FIFO outcomes remain unchanged in this comparison. Raw outcomes were
retained without substitutions.

## Rust and execution-tier validation

Seven regression tests require forced bytecode execution and separately execute
the parsed AST. They cover boxed receivers, inherited elements, dynamic holes,
argument order, MAX_SAFE_INTEGER/Infinity lengths, loop limits, partial mutation
before rejection, Proxy trap order, stable sorting, dense copying, species
avoidance, UTF-16 ordering, and comparator-result coercion in sort/toSorted.

Root suite: 343 passed; the same three LSP tests fail at Unix socket binding
with PermissionDenied in this environment. Core suite: 642 passed, zero failed.
Formatting, strict root/core Clippy, N-API, host WASM-feature, and runtime feature
checks pass. No tests were skipped or assertions weakened. Actual wasm32
execution and full-corpus AST/bytecode differential execution remain unvalidated.

The local Rust 1.99 debug compiler produced a zero-byte codegen object with its
default codegen-unit split, preventing archive creation even after a clean build.
The final Rust test runs use a command-line codegen-units=1 override for the core
crate. This changes build partitioning only; no repository build configuration
or product behavior was changed. The release worker uses the normal release
configuration. The successful validation logs record the completed runs.

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
  --output artifacts/test262/generic-array.json
python3 tools/test262/triage.py artifacts/test262/generic-array.json \
  --output artifacts/test262/generic-array-triage.json
python3 tools/test262/run.py ../test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --output artifacts/test262/generic-array-full.json
```

Build the baseline worker in a detached worktree at `1a8ab73` and run the same
selections. `tools/test262/compare.py` compares the complete reports, accepting
compressed archives directly. All raw outcomes, comparisons, replay selections,
triage, implementation patch, and validation logs are archived under
`tools/test262/evidence/generic-array/`.

## Deferred failures

The 114 remaining variants in the five callback groups include Arguments-object
identity checks, constructor/prototype inheritance scenarios, and tests that
allocate arrays beyond the engine's existing allocation limit. Their underlying
runtime behavior needs separate diagnosis; method-level receiver/length fixes
cannot resolve every failure. No allocation limit was relaxed.

Array has 588 total remaining failures across these and other methods. Generic
map/filter, mutation methods, and existing sort semantics need separate focused
audits. Temporal, advanced RegExp, realm/module architecture, GC changes,
Math.f16round, and Math.sumPrecise remain outside this batch. No corpus file,
expected outcome, harness assertion, or skip list was changed.
