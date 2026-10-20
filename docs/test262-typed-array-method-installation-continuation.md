# TypedArray method installation

The shared TypedArray constructor's from method now has length 1. Its optional
mapping callback and this argument remain supported. The shared TypedArray
prototype's toString property now references the same intrinsic function object
as Array.prototype.toString, retaining writable, non-enumerable, configurable
attributes and generic receiver behavior.

Both new regression tests fail before the change and pass afterward. The realm
suite's prior length-3 expectation is corrected to 1 while retaining cross-realm
identity, inheritance, and ownership checks. All 83 realm tests, five Array
string tests, two new tests, and 369 core tests pass. Strict workspace Clippy
and formatting pass. See the [native evidence](../tools/test262/evidence/typed-array-method-installation-2026-10-20/).

Release verification is pending. Earlier frozen TypedArray measurements exclude
these subsequent installation changes.
