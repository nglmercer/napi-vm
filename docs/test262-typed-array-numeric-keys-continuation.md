# TypedArray ordinary numeric-looking keys

TypedArray property definition now uses the shared CanonicalNumericIndexString
helper, which compares ECMAScript number formatting with the original key.
The previous duplicate check compared Rust's decimal rendering and incorrectly
classified `1000000000000000000000` and `0.0000001` as integer-indexed keys.
Their canonical forms are `1e+21` and `1e-7`, respectively.

Noncanonical names follow ordinary property semantics, including data/accessor
conversion, reflection, deletion, and inherited properties. Canonical exponent
names remain integer-indexed and out-of-range definitions are rejected.
The shared helper preserves negative-zero, NaN, and infinity handling.

One new native regression fails before the change; the canonical-name control
passes before and after. Both tests now pass across numeric and BigInt typed
arrays. All 369 core tests and strict workspace Clippy pass. Formatting passes.
Evidence is in the [archive](../tools/test262/evidence/typed-array-numeric-keys-2026-10-20/).

The frozen release passes **4,290 / 4,336** TypedArray and constructor variants,
gaining **24 passes with zero losses**. There are 40 failures and six timeouts,
with no skips, crashes, or harness errors. All six timeout outcomes are unchanged.
This worker excludes the subsequent require-new and buffer-coercion fixes.
See the [release evidence](../tools/test262/evidence/typed-array-numeric-keys-2026-10-20/summary.json).
