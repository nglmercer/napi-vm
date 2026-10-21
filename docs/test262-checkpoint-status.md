# Conformance development checkpoint

## Current measured state

The latest completed full pinned Test262 run (archived 2026-10-20, corpus
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, source `daadb9b`)
passes **101,978 / 102,956 variants (99.0501%)**, with 753 failures,
225 timeouts, and zero skips, crashes, or harness errors. It gains 54 passes
and loses 155 against the previous full run on `1bffdb8`; all 155 losses are
measured timeouts, retained without reclassification. Of the gains, 30 were
previous failures and 24 were previous timeouts. The frozen worker is
`852c7c9c…`; this run did not resume a checkpoint.
It includes Object constructor metadata, Object locale-string invocation,
and Array string invocation, along with earlier module and disposal changes.
Reports and comparison are archived in [full Array-string evidence](../tools/test262/evidence/full-array-string-2026-10-20/).

This snapshot excludes subsequent sparse indices, logical Array lengths,
core constructor metadata/unscopables, and String semantic fixes. Focused
results remain separate from this full score. A new full run is active on detached clean source `f708e0c` (worker
`8dab6498…`), including the verified built-in and tagged-constructor fixes.
The completed full score remains authoritative until that report finishes.

Function/Proxy metadata and inheritance now pass **1,624 / 1,628** focused
variants in an isolated optimized build: **12 gains, zero lost passes**.
This later change is excluded from the active full run. See the
[Function/Proxy continuation](test262-function-proxy-metadata-continuation.md).

Bound functions now retain intermediate target identities and resolve deep
call, construction, and instanceof chains iteratively. Isolated optimized
verification passes **1,831 / 1,837**, with **two gains and zero lost passes**.
The subsequent primitive-operand ordering fix passes **107 / 107**
focused debug instanceof variants, gaining **four passes with zero losses**.
Both changes are excluded from the active full run. See the
[bound-chain continuation](test262-bound-function-chains-continuation.md) and
[instanceof ordering continuation](test262-instanceof-primitive-order-continuation.md).

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

Empty accessor metadata and duplicate-parameter initialization subsequently pass **14,615 / 14,639** selected variants, gaining **21 passes with zero losses** against the completed full-run projection. Six gains overlap the prior with-binding work. These changes are included in the completed full score. See the [descriptor continuation](test262-empty-accessors-continuation.md).

Async generator delegation subsequently passes **2,858 / 2,858** selected variants, gaining **4 passes with zero losses**. These changes are included in the completed full score. See the [async delegation continuation](test262-async-delegation-continuation.md).

Canonical numeric property keys subsequently pass **2,702 / 2,729** selected variants, gaining **10 passes with zero losses**. All 2,252 object-expression variants pass. These changes are included in the completed full score. See the [numeric key continuation](test262-numeric-property-keys-continuation.md).

Class property layout subsequently passes **18,986 / 19,037** selected variants, gaining **38 passes with zero losses**. All computed-name and object-expression variants pass. This is included in the completed full score. See the [class layout continuation](test262-class-property-layout-continuation.md).

Var destructuring reference order subsequently passes **528 / 528** selected variants, gaining **one pass with zero losses**. All 368 core-library tests pass without exclusions. This is included in the completed full score. See the [var destructuring continuation](test262-var-destructuring-continuation.md).

Optional-chain references subsequently pass **16,722 / 16,765** selected variants, gaining **12 passes with zero losses**. All 76 optional-chaining expression variants pass. This is included in the completed full score. See the [optional-chain continuation](test262-optional-chain-references-continuation.md).

Public hash-named keys subsequently pass **31,739 / 31,860** selected variants, gaining **two passes with zero losses**. This is included in the completed full score. See the [public hash-key continuation](test262-public-hash-keys-continuation.md).

Super reference receiver order subsequently passes **19,080 / 19,125** selected variants, gaining **ten passes with zero losses**. This is included in the completed full score. See the [super receiver-order continuation](test262-super-reference-order-continuation.md).

Default export names subsequently pass **18,120 / 18,184** selected variants, gaining **seven passes with zero losses**. This is included in the completed full score. See the [default export-name continuation](test262-default-export-names-continuation.md).

Namespace descriptor TDZ fixes subsequently pass **7,887 / 7,916** selected variants, gaining **six passes with zero losses**; deferred imports remain **109 / 109**. This change is included in the completed full snapshot. See the [namespace continuation](test262-namespace-tdz-continuation.md).

Cyclic and ambiguous export resolution subsequently passes **2,618 / 2,637** module/import variants, gaining **two passes with zero losses**. This change is included in the completed full snapshot. See the [export resolution continuation](test262-export-resolution-continuation.md).

Exported destructuring bindings subsequently pass **3,486 / 3,504** selected variants, gaining **one pass with zero losses**. All **253 / 253** top-level-await variants pass. This change is included in the completed full snapshot. See the [export patterns continuation](test262-export-patterns-continuation.md).

Harness-error classification subsequently passes **2,621 / 2,637** selected module/import variants, correcting **two passes with zero losses**. This is a worker measurement correction, included in the completed full snapshot. See the [harness error continuation](test262-harness-error-continuation.md).

Dynamic deferred imports subsequently pass **2,625 / 2,637** selected module/import variants, gaining **four passes with zero losses** by awaiting asynchronous dependencies through internal reactions. This change is included in the completed full snapshot. See the [dynamic defer continuation](test262-dynamic-defer-continuation.md).

Async cycle outcomes subsequently pass **2,627 / 2,637** selected module/import variants, gaining **two passes with zero losses**. All **1,900 / 1,900** dynamic-import variants pass. This change is included in the completed full snapshot. See the [async cycle continuation](test262-async-cycle-continuation.md).

Immutable byte modules subsequently pass **4,334 / 4,341** selected variants, gaining **five passes with zero losses**; all **5 / 5** import-bytes variants pass. Two pre-existing ArrayBuffer timeouts remain measured. The engine and binary loader API support file, HTTP, package, and TypeScript adapters with permission and size checks. This change is included in the completed full snapshot. See the [byte module continuation](test262-byte-modules-continuation.md).

Source-phase imports subsequently pass **2,645 / 2,645** selected module/import/intrinsic variants, gaining **13 combined passes with zero losses** versus `75c6857`. Static and dynamic source bindings and re-exports share cached host-branded objects without executing target modules. This is included in the completed full score. See the [source-phase continuation](test262-static-source-imports-continuation.md).

Async iterator disposal subsequently passes **1,788 / 1,788** selected resource-management and async generator variants, gaining **18 passes with zero losses**. AsyncIteratorPrototype now supplies `Symbol.asyncDispose`, awaiting return and forwarding rejection through intrinsic promise operations. This is included in the completed full score. See the [async disposal continuation](test262-async-iterator-disposal-continuation.md).

Object constructor metadata subsequently passes **7,984 / 8,001** selected Object, Function, and Reflect variants, gaining **12 passes with zero losses**. Object now owns ordered length/name properties and inherits Function.prototype. This is included in the completed full score. See the [Object metadata continuation](test262-object-constructor-metadata-continuation.md).

Object locale-string invocation subsequently passes **122 / 122** selected Object, Array, and TypedArray locale-string variants, gaining **two passes with zero losses**. Getter reads retain the original receiver and propagate accessor/proxy errors. This is included in the completed full score. See the [locale-string continuation](test262-object-locale-string-continuation.md).

Array string invocation subsequently passes **755 / 763** selected string and Proxy variants, gaining **two passes with zero losses**. All selected Array, Object, join, and typed-array string cases pass; eight Proxy failures are unchanged. Join accessors, intrinsic fallback, and proxy brand handling are corrected. This is included in the completed full score. See the [array string continuation](test262-array-string-invocation-continuation.md).

Sparse array indices subsequently pass **9,005 / 9,037** selected Array and property-reflection variants, gaining **30 passes with zero losses**. Large indices use traced sparse storage; logical length, descriptors, deletion, truncation, host writes, and map species results share the updated paths. All 369 core tests, 95 integration tests, strict workspace Clippy, and formatting pass. This remains outside the completed full snapshot. See the [sparse array continuation](test262-sparse-array-indices-continuation.md).

Logical Array lengths subsequently pass **9,027 / 9,037** selected Array and property-reflection variants, gaining **22 passes with zero losses**. Slice, splice, and concat retain safe-integer indices and species ordering; push and unshift preserve writes before array-length errors. All 369 core tests, 101 integration tests, strict workspace Clippy, and formatting pass. This remains outside the completed full snapshot. See the [logical length continuation](test262-array-logical-lengths-continuation.md).

Core constructor metadata subsequently passes **12,255 / 12,261** selected Array, String, Number, Boolean, and property-reflection variants, gaining **14 passes with zero losses**. Object, Array, String, Number, and Boolean share ordered own length/name metadata; Array unscopables excludes with. All **6,117 / 6,117** Array variants pass. All 369 core tests, 107 integration tests, strict workspace Clippy, and formatting pass. This remains outside the completed full snapshot. See the [core constructor continuation](test262-core-constructor-metadata-continuation.md).

TypedArray numeric keys subsequently pass **4,290 / 4,336** selected TypedArray and constructor variants, gaining **24 passes with zero losses**. Forty failures and six unchanged timeouts remain. Noncanonical numeric-looking keys use ordinary property semantics. This remains outside the completed full snapshot. See the [numeric-key continuation](test262-typed-array-numeric-keys-continuation.md).

Exact Math.sumPrecise subsequently brings the full Math selection to **654 / 654**, gaining **20 passes with zero losses**. Finite numbers accumulate exactly before one ties-to-even rounding; iterator closing, signed zero, and non-number rejection are verified. All 369 core tests, four native regressions, strict workspace Clippy, and formatting pass. This remains outside the completed full snapshot. See the [Math continuation](test262-math-sum-precise-continuation.md).

A detached clean checkout of `a5a2362` verifies **13,800 / 13,808** selected built-in and tagged-template variants, gaining **76 passes with zero losses** against an earlier measured worker. Array, String, Math, SharedArrayBuffer, TypedArrayConstructors, and the targeted capture-name regression all pass. Two tagged-constructor failures are fixed by subsequent `9d58dce`; six TypedArray timeouts remain. This source-verified focused result stays separate from the full score. See the [verified built-ins continuation](test262-verified-builtins-continuation.md).


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
