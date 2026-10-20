# Sparse array indices continuation

Array indices above the dense-storage bound now use the array's existing traced
property storage. Writing index `4294967294` updates logical length to
`4294967295` without allocating intervening elements. `4294967295` itself remains
an ordinary named property.

Shared indexed reads and writes serve ordinary assignment, property definitions,
reflection, host `Value::set_prop`, and direct Node-API array property writes.
Deletion preserves logical length. Shrinking length deletes sparse entries in descending semantic order:
the highest non-configurable entry stops truncation, entries above it disappear,
and a requested non-writable length still takes effect after partial failure.
Length truncation removes sparse accessor companions with their indexed properties.

Default ArraySpeciesCreate accepts valid sparse lengths through `2^32 - 1`.
This lets `map` preserve holes and create high-index results. Length descriptors
report logical length rather than dense allocation size. Sparse entries share
the existing named-property GC tracing and integrity-level handling.

Seven integration tests cover reads, reflection, deletion, accessors, integrity,
partial truncation, non-writable length, callbacks, species results, and physical
allocation size. All seven guest-language assertion scenarios fail on the frozen
pre-change worker and pass in the native regression suite. The related realm and
array-string suites also pass: 95 integration tests in total. All 369 core tests,
strict workspace Clippy, and formatting checks pass.

The focused pinned selection covers Array, Object.defineProperty,
Object.getOwnPropertyDescriptor, Reflect.defineProperty, and Reflect.ownKeys.
The baseline is 8,975 / 9,037 with 62 failures and no other non-pass outcomes.
The final release passes **9,005 / 9,037**, gaining **30 passes with zero losses**.
There are 32 failures and 0 timeouts; no skips, crashes, or harness errors.
See the [archived evidence](../tools/test262/evidence/sparse-array-indices-2026-10-20/summary.json).

This work does not finish Array conformance. At that checkpoint, generic methods still contained
length clamps and dense result limits. The subsequent [logical length
continuation](test262-array-logical-lengths-continuation.md) addresses concat,
splice, slice, and push. The completed full run on `daadb9b` excludes this
continuation; its outcomes must remain separate from the focused measurement.
