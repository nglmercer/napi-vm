# Conformance development checkpoint

## Current measured state

The latest completed full pinned Test262 run (2026-10-18, corpus
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, source `997b476`)
passes **101,889 / 102,956 variants (98.9636%)**, with 982 failures,
85 timeouts, and zero skips, crashes, or harness errors. It gains 191 passes
and loses 30 against the preceding full run. One loss is duplicate-parameter
initialization in `Array.prototype.concat_sloppy-arguments-with-dupes.js`;
the other 29 are measured timeouts. Outcomes are retained. The snapshot
includes mapped arguments, lexical scopes, nested finally, descriptor-aware
for-in, live collections, module attributes, completion values, and tail calls.
See [`latest.json`](../tools/test262/latest.json) and the
[full mapped-arguments evidence](../tools/test262/evidence/full-mapped-arguments-2026-10-18/).

Later static import attributes pass 17 / 17, dynamic attributes pass 44 / 44,
and deferred imports pass 109 / 109 in the combined 170-variant selection.
The neighboring module-code and dynamic-import selection passes 2,454 / 2,502.
See the [module attributes continuation](test262-dynamic-import-attributes-continuation.md).

Control-flow completion passes all **150 / 150** statement completion
variants. Switch passes **213 / 216**, with three proper-tail-call failures
at that checkpoint. The neighboring control-flow selection passes
**3,574 / 3,624**, gaining **71 passes with zero losses**. AST and prepared
bytecode completion checks, 93 integration tests, and 79 selected bytecode
tests pass; the known unrelated super-destructuring parity failure is excluded
from that bytecode selection. See the
[control-flow continuation](test262-control-flow-completion-continuation.md).
Tail-call execution subsequently passes **35 / 35** variants, with **34 gains
and zero losses**. Switch now passes **216 / 216** and the neighboring
control-flow selection passes **3,591 / 3,624** (17 further gains, zero losses).
Native call/apply/bind/proxy regression remains 420 / 424 and module regression
remains 170 / 170. Strict Clippy, 104 integration tests, 83 realm tests, and the
79 selected bytecode tests pass. See the
[tail-call continuation](test262-tail-calls-continuation.md).

Live collection iteration subsequently passes **1,726 / 1,726** selected
variants, gaining **18 passes with zero losses**. Its neighboring control-flow
selection passes **3,599 / 3,624**, gaining eight overlapping passes with zero
losses. Strict Clippy and 149 selected integration tests pass.

Static module attributes, control-flow completion, and tail-call execution are
included in the completed full-run snapshot, along with the changes tracked in
the [live collection continuation](test262-live-collections-continuation.md).
Lexical scopes, nested finally, for-in, and mapped arguments are included in
that full score. With bindings, empty accessors, and the duplicate-parameter
initialization fix remain outside it.

Lexical loop heads, TDZ reads, and catch-body environments subsequently pass **5,893 / 5,908** selected variants, gaining **20 passes with zero losses**. These changes are included in the completed full snapshot. See the [lexical scope continuation](test262-lexical-scopes-continuation.md).

Nested finally completions subsequently pass **5,897 / 5,908** selected variants, gaining **4 passes with zero losses**. These changes are included in the completed full snapshot. See the [finally completion continuation](test262-finally-completions-continuation.md).

Descriptor-aware for-in enumeration subsequently passes **6,498 / 6,515** selected variants, gaining **2 passes with zero losses**. These changes are included in the completed full snapshot. See the [for-in enumeration continuation](test262-for-in-enumeration-continuation.md).

Mapped arguments subsequently pass **13,621 / 13,661** selected variants, gaining **31 passes with zero losses**. All 43 mapped-arguments variants and all 1,442 for-of variants pass. These changes are included in the completed full snapshot. See the [mapped arguments continuation](test262-mapped-arguments-continuation.md).

With and Proxy binding resolution subsequently passes **14,466 / 14,502** selected variants, gaining **6 passes with zero losses**. See the [with-binding continuation](test262-with-bindings-continuation.md).

Empty accessor metadata and duplicate-parameter initialization subsequently pass **14,615 / 14,639** selected variants, gaining **21 passes with zero losses** against the completed full-run projection. Six gains overlap the prior with-binding work. These changes remain outside the full score. See the [descriptor continuation](test262-empty-accessors-continuation.md).

Async generator delegation subsequently passes **2,858 / 2,858** selected variants, gaining **4 passes with zero losses**. These changes remain outside the completed full score. See the [async delegation continuation](test262-async-delegation-continuation.md).

Canonical numeric property keys subsequently pass **2,702 / 2,729** selected variants, gaining **10 passes with zero losses**. All 2,252 object-expression variants pass. These changes remain outside the completed full score. See the [numeric key continuation](test262-numeric-property-keys-continuation.md).

## Historical checkpoint

The following records an earlier paused checkpoint; its counts and pause state
are historical.

This checkpoint includes array elisions, observable spread/rest/assign copying,
Object enumeration and descriptor maps, Object/Math metadata, abstract equality,
exact mixed BigInt/Number comparisons, relational conversions, and the latest
numeric binary-operator conversions. Associated integration tests compare raw
AST execution with forced verified bytecode. Deprecation fixes and native-addon
fixture isolation changes are also included.

The last completed full pinned Test262 measurement in latest.json covers the
equality snapshot: 67,677 / 102,956 passes (65.7339%), 35,277 failures, two
existing timeouts, and no skips, crashes, or harness errors. It gains 187 passes
with zero losses against the preceding metadata full run. Subsequent relational
and numeric binary changes are outside that measured source snapshot.

The relational focused selection passes 10,599 / 12,396 variants, gaining 60
passes with zero losses against the equality full-run projection. All 650
selected language comparison variants pass. Its full run was still active
when checkpoint preparation started; it uses a frozen worker and a resumable
local journal under artifacts/test262/relational-operators-full.checkpoint.jsonl.

The latest numeric binary changes add ordered ToNumeric conversions to guest
arithmetic and bitwise binary expressions and reject BigInt unsigned right
shift. Six new AST/bytecode tests pass. The combined integration run passes
67 tests across seven suites. The minimal core suite passes 354 tests with the
known sidecar native-addon test excluded. Numeric-binary focused/full Test262
measurements have not been performed. Clippy was last conclusively verified
for the preceding relational snapshot; completion of the latest Clippy job
was not captured before the user paused implementation work.

Full conformance is not achieved. Known workspace native-addon failures,
compound-assignment evaluation/coercion order, other numeric edge cases,
unsupported built-ins, environment/descriptor/exotic semantics, classes/private
elements, agents/shared memory, realm coverage, foundation phases 1–3, and
phase 4 bytecode coverage remain open. Full-corpus AST/bytecode differential
validation has not been measured.

Historical full and focused measurements, source patches, logs, identities,
comparisons, and hashes remain in tools/test262/evidence. The continuation
documents describe the measured source snapshot for each result. Local build
binaries and active-run artifacts remain outside Git.

Implementation was paused at the time of this historical checkpoint and later
resumed. Current implementation and conformance state are tracked in
[the ShadowRealm/full-suite continuation](test262-shadow-realm-continuation.md).

Work later resumed with destructuring validation and then named evaluation
and exponentiation; see
[the named-evaluation continuation](test262-named-evaluation-continuation.md).
Declaration empty-completion and with-environment binding resolution
followed; see
[the declaration-completion continuation](test262-declaration-completion-continuation.md).
Date setters, receiver checks, `Symbol.toPrimitive`, and Annex B year
methods followed; see
[the date-setters continuation](test262-date-setters-continuation.md).
