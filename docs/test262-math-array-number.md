# Math, Array and Number quick wins

This batch starts from merged PR #27 (`ca6e7e0`). The source and fresh focused
baseline confirmed the missing functions before implementation. The rebuilt
baseline worker has SHA-256
`c40aac3ada611ce636becb5bf169ad10d58cc297ca5f01119d84feae5584035e`,
matching the worker used by the previously archived full baseline.

## Candidate selection and changes

| Candidate | Failing variants before | Approach |
| --- | ---: | --- |
| Twelve Math functions | 154 | Existing ToNumber and native-method metadata; standard binary64 operations |
| `copyWithin` | 66 | Generic ToLength, relative indexes, HasProperty/Get/Set/Delete, overlap direction |
| `toSpliced` | 58 | Generic reads, distinct omitted/undefined deleteCount, dense result, no species |
| `toReversed` | 32 | Descending generic reads into a dense fresh array |
| Number formatting / prototype metadata | 32 | Exact decimal rounding with existing BigInt; shared with toFixed |
| Array unscopables | 8 additional Array variants | Standard null-prototype exclusion object and descriptor |

Math additions are `acos`, `asin`, `atan`, `atan2`, `acosh`, `asinh`, `atanh`,
`cosh`, `sinh`, `tanh`, `expm1`, and `log1p`. Existing native-method installation
provides the function prototype, name, length, and property attributes.
`atan2` converts both arguments in specification order, even when the first is NaN.

Array operations retain the existing allocation and execution-budget limits.
`copyWithin` uses logical numeric indexes through MAX_SAFE_INTEGER, preserves
holes through deletion, and throws for rejected writes/deletions.
`toSpliced` subtracts the removal count before adding inserted elements to avoid
binary64 cancellation at MAX_SAFE_INTEGER. It never reads removed indexes.
The unscopables object includes the complete standard exclusion list;
its `toSorted` entry does not introduce a `toSorted` implementation.

`toExponential` and `toPrecision` use an exact binary rational and integer decimal
rounding, including ties and carry across powers of ten. `toFixed` reuses the
same bounded helper. Unspecified precision uses the existing shortest ECMAScript
formatter. Receiver validation, argument coercion, nonfinite values, signed zero,
and digit bounds follow the existing shared operations.

## Focused results

Pinned Test262 revision: `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.
Four workers, five-second timeout, no feature skips or modified corpus files.

| Group | Before | After | New passes | Regressions |
| --- | ---: | ---: | ---: | ---: |
| Math | 470 / 654 | 624 / 654 | 154 | 0 |
| Array | 5,241 / 6,117 | 5,405 / 6,117 | 164 | 0 |
| Number | 648 / 680 | 680 / 680 | 32 | 0 |
| Total | 6,359 / 7,451 | 6,709 / 7,451 | 350 | 0 |

All selected variants for each of the three added Array methods pass. All Number
variants pass. Remaining Math failures are `f16round` and `sumPrecise`.

## Full-corpus results and replay

The complete 102,956-variant run advances from 76,411 to 76,823 passes
(74.2171% to 74.6173%). There are 414 new passing variants and two raw
pass-to-fail transitions. All 414 gains reproduce in a separate exact selection:
0 / 414 pass on the baseline binary, 414 / 414 on the new binary.
The additional 64 gains outside the focused selection cover descriptor tests,
staging tests, and strict named-function bindings.

The two raw losses are the script variants of
`built-ins/Atomics/wait/waiterlist-order-of-operations-is-fifo.js` and its
`bigint/` counterpart. These scheduling-sensitive outcomes were already excluded
from credited gains in PR #27. In two fresh repeats per binary, both tests
(including their strict variants) exhaust instruction fuel on the unchanged
baseline as well as on the new engine. Raw reports retain these outcomes;
no results were substituted. There are zero unexplained regressions.
The same two deep-WeakMap staging tests time out. There are no skips, crashes,
or harness errors.

## Rust maintenance

A separate commit fixes strict assignment to a named-function self binding:
strictness belongs to the assigning execution context, not the outer environment
that owns the immutable binding. Module scopes are permanently strict before
function creation, so exported functions retain module strictness when called
later from a script. Existing failing core tests cover both cases; an additional
regression checks strict named-function assignment in AST and forced bytecode.

Three stale test fixtures now assert their intended contracts explicitly:

- The compact-cache test uses two actual GetProp sites and still requires exactly
  two caches and rejection of an invalid cache index.
- The shape-transition unit test warms the object explicitly before the low-level
  property addition, then requires the exact canonical shape ID.
- Destructuring assignment rest retains its intentional AST fallback. The test
  requires that tier and exact fallback reason, and checks unchanged results
  through both prepared execution and direct AST execution. Supported spread and
  rest declarations still require bytecode execution.

Six existing strict-Clippy findings are fixed without suppressions.
Core Rust tests: 642 passed, zero failed. Root tests: 335 passed; three existing
LSP runtime tests still fail because UnixListener::bind returns PermissionDenied
in this execution environment. Their assertions and transport remain intact.
N-API, host WASM-feature, and runtime feature checks pass. Actual wasm32 execution
and a full-corpus differential run across both execution tiers remain unvalidated.

The new built-in regressions exercise both direct AST and forced bytecode.
An independent Node v24.19.0 comparison checks 5,088 formatting expressions over
318 deterministic finite values, including subnormals, neighboring powers of
ten, random binary64 values, and precision up to 100: zero mismatches.

## Reproduction

Run from the repository root with the pinned corpus at `../test262`:

```sh
cargo fmt --check
CARGO_INCREMENTAL=0 cargo test --locked --no-default-features --no-fail-fast
CARGO_INCREMENTAL=0 cargo test -p napi-vm-core --locked --no-default-features --no-fail-fast
cargo clippy --locked --no-default-features --all-targets -- -D warnings
cargo clippy -p napi-vm-core --locked --no-default-features --all-targets -- -D warnings
cargo check --locked
cargo check --locked --no-default-features --features wasm
cargo check --locked --no-default-features --features runtime
cargo build --locked --release --no-default-features --bin napi-vm-test262
python3 tools/test262/check_number_formatting.py --engine target/release/napi-vm-test262
python3 tools/test262/run.py ../test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 \
  --path built-ins/Math --path built-ins/Array --path built-ins/Number \
  --output artifacts/test262/quick-wins.json
python3 tools/test262/triage.py artifacts/test262/quick-wins.json \
  --output artifacts/test262/quick-wins-triage.json
python3 tools/test262/run.py ../test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --output artifacts/test262/quick-wins-full.json
```

The Node comparison accepts `--node /path/to/node`. To rebuild the baseline,
create a detached worktree at `ca6e7e0`, build the same release worker there,
and run the same selections. `tools/test262/compare.py` compares compressed or
uncompressed reports without excluding variants. Evidence and test logs are in
`tools/test262/evidence/math-array-number/`; the implementation patch applies to
the baseline and does not include the Test262 corpus.

## Deferred failures

`f16round` requires correctly rounded binary16 conversion; `sumPrecise` requires
precise accumulation. They deserve separate confirmed fixes. Array's remaining
712 failures span sorting and generic iteration/mutation methods; changing their
shared receiver/coercion behavior requires another focused audit. Complete Proxy,
RegExp, realm, module-system, and GC work remains outside this quick-win batch.
No corpus files, expected outcomes, harness assertions, or skip lists were changed.
