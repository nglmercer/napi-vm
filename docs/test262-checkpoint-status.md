# Conformance development checkpoint

## Current measured state

The latest completed full measurement passes **101,633 / 102,956 (98.7150%)**,
with 1,246 failures, 77 timeouts, and zero skips, crashes, or harness errors.
It measures source `8333a38`, including Object integrity, `Object.fromEntries`,
deferred imports, and the earlier dynamic-import attributes implementation.
It gains 169 passes and loses 24 against the previous full run: 23 former
passes time out and one JSON-module test fails. Concurrency changed from four
to two workers. No timeout outcomes are reclassified. The JSON-module failure
is fixed by the later static module-attributes commit `5b119d4`.
See [`latest.json`](../tools/test262/latest.json) and the
[full refresh evidence](../tools/test262/evidence/full-refresh-2026-10-18/).

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

Static module attributes, control-flow completion, and tail-call execution are
outside the completed full-run snapshot. Newer focused gains are not added to
its score.

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
