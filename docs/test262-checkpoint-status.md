# Conformance development checkpoint

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

Implementation work is paused at the user's request. This commit is a source
and evidence checkpoint, not a completion claim.

Work later resumed with destructuring validation and then named evaluation
and exponentiation; see
[the named-evaluation continuation](test262-named-evaluation-continuation.md).
Declaration empty-completion and with-environment binding resolution
followed; see
[the declaration-completion continuation](test262-declaration-completion-continuation.md).
Date setters, receiver checks, `Symbol.toPrimitive`, and Annex B year
methods followed; see
[the date-setters continuation](test262-date-setters-continuation.md).
