# Conformance development checkpoint

## Current measured state

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
[full empty-accessor evidence](../tools/test262/evidence/full-empty-accessors-2026-10-19/).

A fresh full pinned run is in progress on clean source `ffc10c7` (worker
`2c59c922…`). It includes default-export names and snapshot graph fixes;
the later namespace descriptor TDZ work is outside that worker. The completed
score above remains authoritative until the new report finishes.

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
initialization fix are included in the latest a528b02 full snapshot.

Lexical loop heads, TDZ reads, and catch-body environments subsequently pass **5,893 / 5,908** selected variants, gaining **20 passes with zero losses**. These changes are included in the completed full snapshot. See the [lexical scope continuation](test262-lexical-scopes-continuation.md).

Nested finally completions subsequently pass **5,897 / 5,908** selected variants, gaining **4 passes with zero losses**. These changes are included in the completed full snapshot. See the [finally completion continuation](test262-finally-completions-continuation.md).

Descriptor-aware for-in enumeration subsequently passes **6,498 / 6,515** selected variants, gaining **2 passes with zero losses**. These changes are included in the completed full snapshot. See the [for-in enumeration continuation](test262-for-in-enumeration-continuation.md).

Mapped arguments subsequently pass **13,621 / 13,661** selected variants, gaining **31 passes with zero losses**. All 43 mapped-arguments variants and all 1,442 for-of variants pass. These changes are included in the completed full snapshot. See the [mapped arguments continuation](test262-mapped-arguments-continuation.md).

With and Proxy binding resolution subsequently passes **14,466 / 14,502** selected variants, gaining **6 passes with zero losses**. See the [with-binding continuation](test262-with-bindings-continuation.md).

Empty accessor metadata and duplicate-parameter initialization subsequently pass **14,615 / 14,639** selected variants, gaining **21 passes with zero losses** against the completed full-run projection. Six gains overlap the prior with-binding work. These changes remain outside the full score. See the [descriptor continuation](test262-empty-accessors-continuation.md).

Async generator delegation subsequently passes **2,858 / 2,858** selected variants, gaining **4 passes with zero losses**. These changes remain outside the completed full score. See the [async delegation continuation](test262-async-delegation-continuation.md).

Canonical numeric property keys subsequently pass **2,702 / 2,729** selected variants, gaining **10 passes with zero losses**. All 2,252 object-expression variants pass. These changes remain outside the completed full score. See the [numeric key continuation](test262-numeric-property-keys-continuation.md).

Class property layout subsequently passes **18,986 / 19,037** selected variants, gaining **38 passes with zero losses**. All computed-name and object-expression variants pass. This remains outside the completed full score. See the [class layout continuation](test262-class-property-layout-continuation.md).

Var destructuring reference order subsequently passes **528 / 528** selected variants, gaining **one pass with zero losses**. All 368 core-library tests pass without exclusions. This remains outside the completed full score. See the [var destructuring continuation](test262-var-destructuring-continuation.md).

Optional-chain references subsequently pass **16,722 / 16,765** selected variants, gaining **12 passes with zero losses**. All 76 optional-chaining expression variants pass. This remains outside the completed full score. See the [optional-chain continuation](test262-optional-chain-references-continuation.md).

Public hash-named keys subsequently pass **31,739 / 31,860** selected variants, gaining **two passes with zero losses**. This remains outside the completed full score. See the [public hash-key continuation](test262-public-hash-keys-continuation.md).

Super reference receiver order subsequently passes **19,080 / 19,125** selected variants, gaining **ten passes with zero losses**. This remains outside the completed full score. See the [super receiver-order continuation](test262-super-reference-order-continuation.md).

Default export names subsequently pass **18,120 / 18,184** selected variants, gaining **seven passes with zero losses**. This remains outside the completed full score. See the [default export-name continuation](test262-default-export-names-continuation.md).

Namespace descriptor TDZ fixes subsequently pass **7,887 / 7,916** selected variants, gaining **six passes with zero losses**; deferred imports remain **109 / 109**. This continuation is excluded from the running full snapshot. See the [namespace continuation](test262-namespace-tdz-continuation.md).

Cyclic and ambiguous export resolution subsequently passes **2,618 / 2,637** module/import variants, gaining **two passes with zero losses**. This remains outside the running full snapshot. See the [export resolution continuation](test262-export-resolution-continuation.md).

Exported destructuring bindings subsequently pass **3,486 / 3,504** selected variants, gaining **one pass with zero losses**. All **253 / 253** top-level-await variants pass. This remains outside the running full snapshot. See the [export patterns continuation](test262-export-patterns-continuation.md).

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
