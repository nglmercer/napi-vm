# Math.sumPrecise

Math.sumPrecise consumes an iterable through its ordinary iterator protocol.
Elements must be Numbers: objects, BigInts, and other values throw TypeError
without coercion, and the iterator is closed on that rejection. Iteration
continues after NaN or infinity so later non-number elements still reject.

Finite values are decomposed into their exact binary significand and exponent
and accumulated as signed integers in units of `2^-1074`. This preserves
cancellation through intermediate overflow and needs only about 2,100 bits for
ordinary finite inputs. The result is rounded once to nearest, ties to even,
with explicit subnormal and overflow handling. NaN, opposite infinities, and
negative zero have separate state. Empty or all-negative-zero input returns -0;
other exact-zero results return +0.

Four native tests cover cancellation, subnormals, rounding ties, overflow,
special values, iterator closing, abrupt completion, and native metadata.
The frozen baseline fails all 20 pinned Math.sumPrecise variants. The debug worker passes all **20 / 20** variants. All 369 core tests, the four
regressions, strict workspace Clippy, and formatting pass. See the
[native and debug evidence](../tools/test262/evidence/math-sum-precise-2026-10-20/).
Optimized release verification is pending; this debug result is not added to
the published full release score.

Optimized release verification on `d18faf7` passes **654 / 654** Math variants,
gaining **20 passes with zero losses** against the frozen pre-change worker.
No failures, timeouts, skips, crashes, or harness errors remain in this selection.
See the [release evidence](../tools/test262/evidence/math-sum-precise-2026-10-20/release-summary.json).
This remains separate from the completed full snapshot.
