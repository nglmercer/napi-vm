# Test262 runner

Build the isolated worker without runtime or Node bindings:

```sh
cargo build --release --no-default-features --bin napi-vm-test262
python3 -m pip install -r tools/test262/requirements.txt
git clone https://github.com/tc39/test262.git .napi-vm/test262
git -C .napi-vm/test262 checkout 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81
mkdir -p artifacts/test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 2 --timeout 5 --output artifacts/test262/full-results.json
python3 tools/test262/dashboard.py artifacts/test262/full-results.json \
  --output artifacts/test262/compatibility.html
```

The corpus commit is pinned in `corpus.lock.json`; the runner checks the exact
revision and rejects modified/untracked corpus files. It snapshots the worker
binary before starting subprocesses and records its SHA-256 so concurrent
builds cannot change the implementation midway through a future run.

Metadata uses YAML. Default tests run both strict and non-strict variants;
`onlyStrict`, `noStrict`, `raw`, module and async flags are honored. Harness
includes are loaded before the test. Parse-negative source is parsed separately
from harness execution, and negatives require both the expected phase and error
type. Async tests require exactly one successful `$DONE`; missing or duplicate
completion fails. Every variant gets a fresh process and a hard timeout.

`--path language/expressions/addition` selects a subset; repeat `--path` for
multiple selections. `--skip-feature FEATURE` records explicit skips. The
percentage denominator includes **all selected variants**, including skips,
failures, timeouts, crashes and harness errors. A selected subset's percentage
is never advertised as overall ECMAScript compatibility. Empty/non-passing
runs return exit status 1 while still producing evidence.

For long runs, add `--checkpoint artifacts/test262/full.checkpoint.jsonl`.
After an interrupted process, repeat the same command with `--resume`. The
journal retains every completed variant outcome, including errors and timeouts.
Resume rejects changes to the corpus revision, worker digest, selection, skips,
worker count, or timeout. An interrupted final journal write is rerun. The final
report still contains all selected outcomes and records the restored file count.

## Current full result

The latest complete pinned run finished **2026-10-26** on an uncommitted
source snapshot based on `42ae715`. It passes **102,839 / 102,956 (99.8864%)**,
with 65 failures, 52 timeouts, and zero skips, crashes, or harness errors.
It gains **20 passes with zero losses** against preceding full worker
`ef2a45bd…` (102,819 passes). The **117 remaining variants** comprise 42 parse
failures, 23 runtime failures, and 52 timeouts.

The run uses corpus `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, two workers,
five-second deadlines, no exclusions, and no resumed outcomes. Worker SHA-256:
`aacb94ea7e8fccf9e3ddd92adf9af09a8932e0b0ec6a678fffe2fb1daf8c39b5`.
[Full evidence](evidence/full-activation-followup-2026-10-26/) contains every
outcome, comparison, triage, residual inventory, source identity, and exact
source diff. Source and binary hashes were revalidated after completion.
The final gate also requires clean committed source.

The gains cover thirteen legacy caller/arguments variants, two ArrayBuffer
coercion variants, four URI-decode variants, and strict toSpliced-dense.
Every previous pass is retained. The six-variant timing slice passed twice,
and 16,279 broader focused variants passed without losses. Rejected candidate
regressions remain in the [activation evidence](evidence/function-activation-followup-2026-10-26/).
All 395 core tests, five worker tests, Clippy, root all-target checking,
formatting, and whitespace checks pass.

The [preceding full run](evidence/full-runtime-followup-2026-10-26/) is
preserved. See the [full-pass plan](../../docs/test262-full-pass-plan.md)
for the remaining causes and unchanged acceptance criteria.

The [field-decorator follow-up](evidence/field-decorator-followup-2026-10-26/)
passes **32,596 / 32,596** focused variants: eight gains since the method stage,
or 42 gains with zero losses against the latest complete-report projection.
All pinned parse residuals are clear in that selection. All 405 core tests,
five worker tests, static checks, and the standard timing slice pass. Rejected
array-data and pop-performance candidates remain archived. The subsequent
[getter/setter follow-up](evidence/accessor-decorator-followup-2026-10-26/)
retains **32,596 / 32,596** passes with zero losses, all **408 core tests**,
five worker tests, and static checks. Accessor identities, cache/Node bridge
reads, and inherited exotic reads are covered; rejected candidates are retained.
The [auto-accessor decorator follow-up](evidence/auto-accessor-decorator-2026-10-26/)
also retains **32,596 / 32,596** focused passes and passes **409 core tests**. The latest full score is
unchanged until another complete run confirms these gains.

The [conversion and length follow-up](evidence/conversion-length-2026-10-26/)
passes **18,247 / 18,247** focused variants, gaining **three passes with zero
losses** against the latest full projection. All 412 core tests, five worker
tests, static checks, and the standard timing slice pass. The complete score
above remains unchanged until another full run.

The [buffer follow-up](evidence/buffer-cap-2026-10-26/) gains both DataView
variants with zero losses in a 4,664-variant selection; six prior TypedArray
timeouts remain. All 414 core tests and the standard timing slice pass.

The [hour-cycle follow-up](evidence/intl-hour-cycle-2026-10-26/) passes
**838 / 838** focused variants, gaining both NUL-option variants with zero
losses. All 415 core tests and the standard timing slice pass.

The [large-string follow-up](evidence/string-cap-2026-10-26/) passes
**3,663 / 3,663** focused variants, gaining six Unicode escape variants with
zero losses. All 415 core tests and the standard timing slice pass.

## Historical measurements

The preceding completed full pinned Test262 run (archived 2026-10-20, corpus
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, source `daadb9b`)
passes **101,978 / 102,956 variants (99.0501%)**, with 753 failures,
225 timeouts, and zero skips, crashes, or harness errors. It gains 54 passes
and loses 155 against the previous full run on `1bffdb8`; all 155 losses are
measured timeouts, retained without reclassification. Of the gains, 30 were
previous failures and 24 were previous timeouts. The frozen worker is
`852c7c9c…`; this run did not resume a checkpoint.
It includes Object constructor metadata, Object locale-string invocation,
and Array string invocation, along with earlier module and disposal changes.
Reports and comparison are archived in [full Array-string evidence](evidence/full-array-string-2026-10-20/).

This snapshot excludes subsequent sparse indices, logical Array lengths,
core constructor metadata/unscopables, and String semantic fixes. Focused
results remained separate from that full score. The run started on detached
source `f708e0c` (worker `8dab6498…`) never completed; its partial journal is
superseded by the completed 2026-10-25 baseline.

Function/Proxy metadata and inheritance now pass **1,624 / 1,628** focused
variants in an isolated optimized build: **12 gains, zero lost passes**.
This change was excluded from the unfinished `f708e0c` run and is included in
the completed 2026-10-25 baseline. See the
[Function/Proxy continuation](../../docs/test262-function-proxy-metadata-continuation.md).

Bound functions now retain intermediate target identities and resolve deep
call, construction, and instanceof chains iteratively. Isolated optimized
verification passes **1,831 / 1,837**, with **two gains and zero lost passes**.
The subsequent primitive-operand ordering fix passes **107 / 107**
focused debug instanceof variants, gaining **four passes with zero losses**.
Both changes are included in the completed 2026-10-25 baseline. See the
[bound-chain continuation](../../docs/test262-bound-function-chains-continuation.md) and
[instanceof ordering continuation](../../docs/test262-instanceof-primitive-order-continuation.md).

That full snapshot includes Object integrity and `Object.fromEntries`, deferred
imports, and the earlier dynamic-import attributes implementation. Their
historical focused reports remain available under
[`object-integrity`](evidence/object-integrity-2026-10-18/),
[`deferred-import`](evidence/deferred-import-2026-10-18/), and
[`dynamic-import-attributes`](evidence/dynamic-import-attributes-2026-10-18/).

Later static and dynamic import attributes plus deferred imports pass a combined
**170 / 170** variants. The neighboring module-code/dynamic-import selection
passes **2,454 / 2,502**. See the
[module attributes continuation](../../docs/test262-dynamic-import-attributes-continuation.md)
and [`module-attributes` evidence](evidence/module-attributes-2026-10-18/).

The control-flow completion release selection passes all **150 / 150**
completion-only variants and **213 / 216** switch variants. Its neighboring
control-flow selection passes **3,574 / 3,624**, gaining **71 passes with zero
losses**. Reports and comparisons are under
[`evidence/control-flow-completion-2026-10-18/`](evidence/control-flow-completion-2026-10-18/).
See the [control-flow continuation](../../docs/test262-control-flow-completion-continuation.md).
The subsequent strict-tail-call selection passes **35 / 35**, up from 1 / 35,
with 34 gains and zero losses. The neighboring control-flow selection now
passes **3,591 / 3,624**, with 17 further gains and zero losses. Switch is fully
green at **216 / 216**. Native call/apply/bind/proxy regression retains 420 / 424
passes, and the combined module regression remains 170 / 170. Reports and
comparisons are under [`evidence/tail-calls-2026-10-18/`](evidence/tail-calls-2026-10-18/);
see the [tail-call continuation](../../docs/test262-tail-calls-continuation.md).
The live collection release selection passes **1,726 / 1,726** variants,
gaining 18 passes with zero losses. It covers Map, Set, their iterator
prototypes, ArrayIteratorPrototype, WeakMap, WeakSet, and selected mutation
for-of tests. The neighboring control-flow selection passes **3,599 / 3,624**,
with eight overlapping gains and zero losses. Evidence is archived under
[`evidence/live-collections-2026-10-18/`](evidence/live-collections-2026-10-18/).

The static module-attributes, control-flow, and tail-call changes are included
in the completed full snapshot, along with live collection iteration; see the
[live collection continuation](../../docs/test262-live-collections-continuation.md).

Lexical loop heads, TDZ reads, and catch-body environments subsequently pass **5,893 / 5,908** selected variants, gaining **20 passes with zero losses**. These changes are included in the completed full snapshot. See the [lexical scope continuation](../../docs/test262-lexical-scopes-continuation.md).

Nested finally completions subsequently pass **5,897 / 5,908** selected variants, gaining **4 passes with zero losses**. These changes are included in the completed full snapshot. See the [finally completion continuation](../../docs/test262-finally-completions-continuation.md).

Descriptor-aware for-in enumeration subsequently passes **6,498 / 6,515** selected variants, gaining **2 passes with zero losses**. These changes are included in the completed full snapshot. See the [for-in enumeration continuation](../../docs/test262-for-in-enumeration-continuation.md).

Mapped arguments subsequently pass **13,621 / 13,661** selected variants, gaining **31 passes with zero losses**. All 43 mapped-arguments variants and all 1,442 for-of variants pass. These changes are included in the completed full snapshot. See the [mapped arguments continuation](../../docs/test262-mapped-arguments-continuation.md).

With and Proxy binding resolution subsequently passes **14,466 / 14,502** selected variants, gaining **6 passes with zero losses**. See the [with-binding continuation](../../docs/test262-with-bindings-continuation.md).

Empty accessor metadata and duplicate-parameter initialization subsequently pass **14,615 / 14,639** selected variants, gaining **21 passes with zero losses** against the completed full-run projection. Six gains overlap the prior with-binding work. These changes are included in the completed full score. See the [descriptor continuation](../../docs/test262-empty-accessors-continuation.md).

Async generator delegation subsequently passes **2,858 / 2,858** selected variants, gaining **4 passes with zero losses**. These changes are included in the completed full score. See the [async delegation continuation](../../docs/test262-async-delegation-continuation.md).

Canonical numeric property keys subsequently pass **2,702 / 2,729** selected variants, gaining **10 passes with zero losses**. All 2,252 object-expression variants pass. These changes are included in the completed full score. See the [numeric key continuation](../../docs/test262-numeric-property-keys-continuation.md).

Class property layout subsequently passes **18,986 / 19,037** selected variants, gaining **38 passes with zero losses**. All computed-name and object-expression variants pass. This is included in the completed full score. See the [class layout continuation](../../docs/test262-class-property-layout-continuation.md).

Var destructuring reference order subsequently passes **528 / 528** selected variants, gaining **one pass with zero losses**. All 368 core-library tests pass without exclusions. This is included in the completed full score. See the [var destructuring continuation](../../docs/test262-var-destructuring-continuation.md).

Optional-chain references subsequently pass **16,722 / 16,765** selected variants, gaining **12 passes with zero losses**. All 76 optional-chaining expression variants pass. This is included in the completed full score. See the [optional-chain continuation](../../docs/test262-optional-chain-references-continuation.md).

Public hash-named keys subsequently pass **31,739 / 31,860** selected variants, gaining **two passes with zero losses**. This is included in the completed full score. See the [public hash-key continuation](../../docs/test262-public-hash-keys-continuation.md).

Super reference receiver order subsequently passes **19,080 / 19,125** selected variants, gaining **ten passes with zero losses**. This is included in the completed full score. See the [super receiver-order continuation](../../docs/test262-super-reference-order-continuation.md).

Default export names subsequently pass **18,120 / 18,184** selected variants, gaining **seven passes with zero losses**. This is included in the completed full score. See the [default export-name continuation](../../docs/test262-default-export-names-continuation.md).

Namespace descriptor TDZ fixes subsequently pass **7,887 / 7,916** selected variants, gaining **six passes with zero losses**; deferred imports remain **109 / 109**. This change is included in the completed full snapshot. See the [namespace continuation](../../docs/test262-namespace-tdz-continuation.md).

Cyclic and ambiguous export resolution subsequently passes **2,618 / 2,637** module/import variants, gaining **two passes with zero losses**. This change is included in the completed full snapshot. See the [export resolution continuation](../../docs/test262-export-resolution-continuation.md).

Exported destructuring bindings subsequently pass **3,486 / 3,504** selected variants, gaining **one pass with zero losses**. All **253 / 253** top-level-await variants pass. This change is included in the completed full snapshot. See the [export patterns continuation](../../docs/test262-export-patterns-continuation.md).

Harness-error classification subsequently passes **2,621 / 2,637** selected module/import variants, correcting **two passes with zero losses**. This is a worker measurement correction, included in the completed full snapshot. See the [harness error continuation](../../docs/test262-harness-error-continuation.md).

Dynamic deferred imports subsequently pass **2,625 / 2,637** selected module/import variants, gaining **four passes with zero losses** by awaiting asynchronous dependencies through internal reactions. This change is included in the completed full snapshot. See the [dynamic defer continuation](../../docs/test262-dynamic-defer-continuation.md).

Async cycle outcomes subsequently pass **2,627 / 2,637** selected module/import variants, gaining **two passes with zero losses**. All **1,900 / 1,900** dynamic-import variants pass. This change is included in the completed full snapshot. See the [async cycle continuation](../../docs/test262-async-cycle-continuation.md).

Immutable byte modules subsequently pass **4,334 / 4,341** selected variants, gaining **five passes with zero losses**; all **5 / 5** import-bytes variants pass. Two pre-existing ArrayBuffer timeouts remain measured. The engine and binary loader API support file, HTTP, package, and TypeScript adapters with permission and size checks. This change is included in the completed full snapshot. See the [byte module continuation](../../docs/test262-byte-modules-continuation.md).

Source-phase imports subsequently pass **2,645 / 2,645** selected module/import/intrinsic variants, gaining **13 combined passes with zero losses** versus `75c6857`. Static and dynamic source bindings and re-exports share cached host-branded objects without executing target modules. This is included in the completed full score. See the [source-phase continuation](../../docs/test262-static-source-imports-continuation.md).

Async iterator disposal subsequently passes **1,788 / 1,788** selected resource-management and async generator variants, gaining **18 passes with zero losses**. AsyncIteratorPrototype now supplies `Symbol.asyncDispose`, awaiting return and forwarding rejection through intrinsic promise operations. This is included in the completed full score. See the [async disposal continuation](../../docs/test262-async-iterator-disposal-continuation.md).

Object constructor metadata subsequently passes **7,984 / 8,001** selected Object, Function, and Reflect variants, gaining **12 passes with zero losses**. Object now owns ordered length/name properties and inherits Function.prototype. This is included in the completed full score. See the [Object metadata continuation](../../docs/test262-object-constructor-metadata-continuation.md).

Object locale-string invocation subsequently passes **122 / 122** selected Object, Array, and TypedArray locale-string variants, gaining **two passes with zero losses**. Getter reads retain the original receiver and propagate accessor/proxy errors. This is included in the completed full score. See the [locale-string continuation](../../docs/test262-object-locale-string-continuation.md).

Array string invocation subsequently passes **755 / 763** selected string and Proxy variants, gaining **two passes with zero losses**. All selected Array, Object, join, and typed-array string cases pass; eight Proxy failures are unchanged. Join accessors, intrinsic fallback, and proxy brand handling are corrected. This is included in the completed full score. See the [array string continuation](../../docs/test262-array-string-invocation-continuation.md).

Sparse array indices subsequently pass **9,005 / 9,037** selected Array and property-reflection variants, gaining **30 passes with zero losses**. Large indices use traced sparse storage; logical length, descriptors, deletion, truncation, host writes, and map species results share the updated paths. All 369 core tests, 95 integration tests, strict workspace Clippy, and formatting pass. This remains outside the completed full snapshot. See the [sparse array continuation](../../docs/test262-sparse-array-indices-continuation.md).

Logical Array lengths subsequently pass **9,027 / 9,037** selected Array and property-reflection variants, gaining **22 passes with zero losses**. Slice, splice, and concat retain safe-integer indices and species ordering; push and unshift preserve writes before array-length errors. All 369 core tests, 101 integration tests, strict workspace Clippy, and formatting pass. This remains outside the completed full snapshot. See the [logical length continuation](../../docs/test262-array-logical-lengths-continuation.md).

Core constructor metadata subsequently passes **12,255 / 12,261** selected Array, String, Number, Boolean, and property-reflection variants, gaining **14 passes with zero losses**. Object, Array, String, Number, and Boolean share ordered own length/name metadata; Array unscopables excludes with. All **6,117 / 6,117** Array variants pass. All 369 core tests, 107 integration tests, strict workspace Clippy, and formatting pass. This remains outside the completed full snapshot. See the [core constructor continuation](../../docs/test262-core-constructor-metadata-continuation.md).

TypedArray numeric keys subsequently pass **4,290 / 4,336** selected TypedArray and constructor variants, gaining **24 passes with zero losses**. Forty failures and six unchanged timeouts remain. Noncanonical numeric-looking keys use ordinary property semantics. This remains outside the completed full snapshot. See the [numeric-key continuation](../../docs/test262-typed-array-numeric-keys-continuation.md).

Exact Math.sumPrecise subsequently brings the full Math selection to **654 / 654**, gaining **20 passes with zero losses**. Finite numbers accumulate exactly before one ties-to-even rounding; iterator closing, signed zero, and non-number rejection are verified. All 369 core tests, four native regressions, strict workspace Clippy, and formatting pass. This remains outside the completed full snapshot. See the [Math continuation](../../docs/test262-math-sum-precise-continuation.md).

A detached clean checkout of `a5a2362` verifies **13,800 / 13,808** selected built-in and tagged-template variants, gaining **76 passes with zero losses** against an earlier measured worker. Array, String, Math, SharedArrayBuffer, TypedArrayConstructors, and the targeted capture-name regression all pass. Two tagged-constructor failures are fixed by subsequent `9d58dce`; six TypedArray timeouts remain. This source-verified focused result stays separate from the full score. See the [verified built-ins continuation](../../docs/test262-verified-builtins-continuation.md).


Full reports, status transitions, measured source patches, source hashes,
build environment, and validation logs are archived in `evidence/`, including
the [2026-10-18 final run](evidence/shadow-realm-2026-10-18/). See the
[ShadowRealm continuation](../../docs/test262-shadow-realm-continuation.md)
for the implementation and remaining work.
Validation counts in individual continuation documents describe their own
source snapshots.
Full-corpus tier differential and actual wasm32 execution remain unmeasured.

The initial full development baseline is in `baseline.json`: 32,359 passes out
of 102,956 variants, 31.43%. The initial run preceded binary snapshot support;
future reports include the worker digest. Full individual outcomes and the
HTML dashboard are generated artifacts rather than vendored corpus content.
The HTML shows independent ECMAScript, Web, Node and npm metrics, and up to 200
non-passing variants with a link to the full JSON file.

Module linking failures are reported in the resolution phase before evaluation.
The isolated worker receives an explicit corpus directory capability. It loads
nested and dynamically imported fixture files lazily, checks canonical paths
remain inside that directory, and also accepts virtual modules for worker tests.
The embeddable engine does not gain ambient filesystem access.

Async completion lives in the persistent global environment, so callbacks and
module scopes cannot lose `$DONE` state. A caught `$DONE(error)` still fails the
variant. `$262.evalScript` evaluates in the global environment and `$262.global`
is exposed. ShadowRealm-focused conformance is tracked separately from the host
`$262.createRealm` and finalization semantics described above.

Group all outcomes into actionable failure clusters without dropping skips,
crashes, timeouts, or harness failures:

```sh
python3 tools/test262/triage.py artifacts/test262/full-results.json \
  --output artifacts/test262/triage.json
```

See `docs/test262-conformance-validation.md` for the current validation evidence.


The host implements `$262.createRealm` and real worker agents with shared-memory
coordination. ShadowRealm construction, evaluation, callable wrapping, and
`importValue` now pass their pinned 124-variant Test262 selection. Broader realm
ownership and GC/finalization semantics remain incomplete.
ArrayBuffer detachment is implemented; GC requests run at quiescent host boundaries
and do not guarantee synchronous collection or finalization.
See [phases 1–3 implementation notes](../../docs/test262-phases-1-3.md).
See [foundation continuation status](../../docs/test262-foundations-completion.md)
for the new agent implementation and remaining completion gates.
Those limitations contribute failures or harness errors; they are not silently
removed from the denominator. No stable compatibility claim is made.

Verification:

```sh
python3 -m unittest discover -s tools/test262 -p 'test_*.py'
cargo test --no-default-features --test test262_worker --test engine_boundary
```

## Compile-only source audit

`audit_syntax.py` checks source acceptance with the public compile-with-goal API.
It uses the runner's pinned-corpus selection and strict/Script/Module variants,
without evaluating a harness or guest code. These results are not execution
passes. Invalid sources accepted and valid sources rejected remain explicit
outcomes, including unsupported proposals and execution-limit errors.

```sh
python3 tools/test262/audit_syntax.py /workspace/test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --output artifacts/test262/syntax-audit.json
```

The command builds a compile-only Rust companion and records source/engine
identities. An existing immutable companion can be supplied with `--engine`.
Full execution and AST/bytecode differential validation remain separate gates.
