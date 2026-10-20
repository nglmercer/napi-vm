# TypedArray shared buffer coercion

SharedArrayBuffer-backed TypedArray construction now checks offset alignment,
converts explicit length, then reads current shared buffer size and validates
bounds. Length coercion can grow a growable shared buffer; a window that was
previously out of bounds may therefore become valid. Abrupt length coercion
propagates before offset bounds errors.

Two new regressions fail before the change and pass afterward across numeric
and BigInt constructors. The alignment/length-tracking control passes before
and after. All three tests, three ArrayBuffer coercion tests, and seven shared
growth tests pass. All 369 core tests, strict workspace Clippy, and formatting
pass. See the [native evidence](../tools/test262/evidence/typed-array-shared-coercion-2026-10-20/).

Release measurement is pending. The active calls/buffer frozen release run
excludes this subsequent SharedArrayBuffer change.
