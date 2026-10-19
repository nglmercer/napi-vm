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
  --jobs 4 --timeout 5 --output artifacts/test262/full-results.json
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

The latest completed full pinned Test262 run (archived 2026-10-19, corpus
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, source `a528b02`)
passes **101,895 / 102,956 variants (98.9695%)**, with 951 failures,
110 timeouts, and zero skips, crashes, or harness errors. It gains 61 passes
and loses 55 against the previous full run on `997b476`; all 55 losses are
measured timeouts. Outcomes are retained without reclassification. This snapshot
includes with bindings, empty accessor descriptors, and the duplicate-parameter
initialization fix. Later async delegation, numeric keys, class layout, host
bridge/JSON work, var destructuring, and optional chains are outside this score.
Reports, comparison, source identity, and triage are archived in
[full empty-accessor evidence](evidence/full-empty-accessors-2026-10-19/).

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

Empty accessor metadata and duplicate-parameter initialization subsequently pass **14,615 / 14,639** selected variants, gaining **21 passes with zero losses** against the completed full-run projection. Six gains overlap the prior with-binding work. These changes remain outside the full score. See the [descriptor continuation](../../docs/test262-empty-accessors-continuation.md).

Async generator delegation subsequently passes **2,858 / 2,858** selected variants, gaining **4 passes with zero losses**. These changes remain outside the completed full score. See the [async delegation continuation](../../docs/test262-async-delegation-continuation.md).

Canonical numeric property keys subsequently pass **2,702 / 2,729** selected variants, gaining **10 passes with zero losses**. All 2,252 object-expression variants pass. These changes remain outside the completed full score. See the [numeric key continuation](../../docs/test262-numeric-property-keys-continuation.md).

Class property layout subsequently passes **18,986 / 19,037** selected variants, gaining **38 passes with zero losses**. All computed-name and object-expression variants pass. This remains outside the completed full score. See the [class layout continuation](../../docs/test262-class-property-layout-continuation.md).

Var destructuring reference order subsequently passes **528 / 528** selected variants, gaining **one pass with zero losses**. All 368 core-library tests pass without exclusions. This remains outside the completed full score. See the [var destructuring continuation](../../docs/test262-var-destructuring-continuation.md).

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
