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

Pinned release comparisons for String, RegExp, and ordinary/Annex B regexp
literals are running. No release conformance gain is claimed until those reports
are complete and compared. Local reports use the `string-semantics` prefix under
artifacts/test262. The separate full daadb9b run excludes this implementation.
