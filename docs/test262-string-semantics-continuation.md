# String coercion, metadata, and legacy escapes

String.raw boxes both template and raw before property reads, converts raw length
through LengthOfArrayLike with observable numeric coercion, and preserves logical
length under execution-budget checks. Symbol and BigInt lengths throw TypeError;
large lengths reach indexed getters before any output allocation limit.

String.prototype.startsWith now receives standard native method metadata,
including a configurable, non-writable, non-enumerable length of 1.

The regexp matcher accepts incomplete hexadecimal and Unicode escapes as identity
escapes in non-Unicode mode, restoring partially consumed digits. Braced Unicode
escapes are decoded only in Unicode mode. Unicode mode remains strict and valid
surrogate-pair decoding is preserved.

Five new native regressions fail before the implementation; the Unicode strictness
case passes before and after. All six now pass, alongside 83 realm tests, six
legacy regexp tests, and all 369 core tests. Strict workspace Clippy passes.
Native evidence and the implementation patch are in
[the evidence directory](../tools/test262/evidence/string-semantics-2026-10-20/).

The frozen release passes **6,541 / 6,819** selected String, RegExp, and
ordinary/Annex B regexp literal variants: 38 failures and 240 timeouts, with no
other non-pass outcomes. There are **172 gained passes and 142 lost passes**.
The gains include eight previous failures and 164 previous timeouts. Losses
include 140 timeouts and two actual regressions in non-Unicode capture names.
All outcomes are retained without reclassification.

All **2,443 String variants pass** on this worker. The capture-name regressions
are addressed by a subsequent correction; this original report is retained as
regression evidence rather than replaced. See the
[release evidence](../tools/test262/evidence/string-semantics-2026-10-20/summary.json).
The completed full `daadb9b` run excludes these changes.

Provenance limitation: the queued build shared a mutable checkout, so the worker
hash identifies measured behavior but exact full-source commit attribution is
unverified. A detached-checkout rerun is required for that stronger claim.
