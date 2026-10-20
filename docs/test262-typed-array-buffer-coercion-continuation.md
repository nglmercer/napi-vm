# TypedArray buffer argument coercion

ArrayBuffer-backed TypedArray construction now converts byte offset, checks
alignment, converts an explicit length, then checks detachment and reads current
buffer size. Observable coercion can detach, grow, or shrink a buffer, so the
constructor must validate the resulting state rather than an earlier snapshot.

Offset coercion exceptions and alignment errors precede detachment checks.
Explicit length coercion precedes bounds validation. A buffer detached during
either conversion throws TypeError. Growth during length conversion can make a
previously out-of-bounds window valid; shrinkage can invalidate an existing one.

All three new native tests fail before the change. They cover numeric and BigInt
typed arrays, abrupt conversion, detached buffers, alignment, and resize effects.
All 369 core tests and 14 related integration tests pass, as do strict workspace
Clippy and formatting. Native evidence is in the
[archive](../tools/test262/evidence/typed-array-buffer-coercion-2026-10-20/).
Release conformance measurements are recorded separately;
no release gain is claimed before a matching report is complete.
