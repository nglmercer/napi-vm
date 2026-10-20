# Logical lengths in Array methods

`slice`, `splice`, and `concat` use LengthOfArrayLike and numeric property keys
through `2^53 - 1`, instead of clamping logical indices to the dense allocation
bound. Their loops retain execution-budget checks. `splice` subtracts its removal
count before checking growth, preserving exact arithmetic near the safe-integer
boundary. Its tail deletion loop also consumes execution budget.

`slice` and `splice` delegate result-length validation to ArraySpeciesCreate.
The default Array constructor still rejects lengths above `2^32 - 1`; a custom
species can observe a larger count and return another object. Indexed reads,
writes, deletes, and abrupt completions follow that constructor call.

`push` and `unshift` validate growth against the safe-integer limit before indexed
writes. Actual arrays rely on their ordinary length setter to enforce the Uint32
length limit afterward. Thus pushing to an array of length `2^32 - 1` writes the
ordinary property `4294967295` before the length assignment throws RangeError.
Zero-argument calls preserve a valid huge array length. `concat` checks logical
overflow before reading the spread object's indexed properties.

Six integration tests cover nine guest scenarios. Every scenario fails on the
frozen pre-change worker. All six tests, the seven sparse-array tests, five
array-string tests, and 83 realm tests pass: 101 integration tests in total.
All 369 core-library tests, strict workspace Clippy, and formatting pass.

The pinned focused selection covers Array, Object.defineProperty,
Object.getOwnPropertyDescriptor, Reflect.defineProperty, and Reflect.ownKeys.
Its baseline passes 9,005 / 9,037 variants, with 32 failures and no other
non-pass outcomes. The final release passes **9,027 / 9,037**, gaining **22 passes with zero losses**.
There are 10 failures and 0 timeouts, with no skips, crashes, or harness errors.
The remaining cases at that checkpoint concerned Array length/name metadata,
Array unscopables, and String/Boolean constructor length metadata. The subsequent
[core constructor continuation](test262-core-constructor-metadata-continuation.md)
addresses those failures.
See the [archived evidence](../tools/test262/evidence/array-logical-lengths-2026-10-20/summary.json).

The completed full run on `daadb9b` excludes this change and the preceding sparse
index implementation. Neither focused result should be added to the full score.
Other array methods still have dense materialization limits and some shared
length clamps; this continuation does not finish all Array semantics.
