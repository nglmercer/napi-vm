# Binary-data continuation

DataView methods now perform observable ToIndex and ToNumber/ToBigInt
conversions in order, before buffer-state/bounds validation. Accessors use
branded prototype descriptors; constructor metadata, new.target prototype
lookup, and post-lookup resize validation are implemented. Reads no longer
reverse the backing storage temporarily; writes encode locally before updating
backing bytes. Float16 get/set use direct binary64-to-binary16 ties-to-even
rounding, preserving signed zero, subnormals and infinities.

TypedArray callbacks use live indexed reads, initial length, and the original
receiver instead of an Array snapshot. Includes reduceRight, findLast,
findLastIndex, original-view copyWithin/reverse with overlapping byte copies,
correct method lengths, branded accessors and Symbol.toStringTag. CopyWithin
rechecks resized backing storage after ordered argument conversion. Existing
allocation and execution limits remain in force.

The combined pinned DataView/TypedArray selection passes **3,584 / 4,012**:
DataView **1,100 / 1,122**, TypedArray **2,484 / 2,890**. Comparison against the
complete ZonedDateTime report records **940 gains, zero losses**, retaining
2,644 passes. No skips, timeouts, crashes, or harness errors occurred. Remaining
DataView failures all require the absent immutable ArrayBuffer feature. Other
TypedArray methods and coercion/resize corner cases remain incomplete.

Measured debug worker SHA-256:
d9d1c80c76a78d4f62efdf4e2d407bc689816ea80deb8694b84a4479fe2f6e9b.
Evidence: tools/test262/evidence/binary-data. The patch includes the preceding
Temporal work and new regression file on source-base.txt. Eight JSON unit-test
arguments are also adapted to JsString after the earlier UTF-16 API change;
assertions and expected outputs are unchanged. This test-only adjustment does
not affect worker behavior. Corpus revision remains
5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81, four jobs and five-second timeout,
without skips or changes to runner assertions/denominator/limits.

All 88 selected Rust regressions pass across data_view_semantics, temporal,
json_reviver, builtin_quick_wins, utf16, and test262_worker. Six new tests run
independently as AST and verified bytecode, checking coercion order, resize,
BigInt, Float16 ties/subnormals/signed zero, callback mutation and initial
length, original receivers, overlap copies and reversal. The core library
passes 354 tests with the historically failing native-addon fixture
loads_and_invokes_a_real_node_api_addon excluded. Both focused Atomics fixture
tests pass. Strict Clippy for library/worker/new tests, formatting and whitespace
checks pass.

The optimized full measurement is **91,224 / 102,956 (88.6048%)**, with
972 gains and zero losses against the preceding Temporal full run. It has two
timeouts and zero skips, crashes or harness errors. The report and transitions
are archived in this batch's evidence directory; latest.json reflects it.
The original 90% goal requires 92,661 passing variants and remains active.
Worker SHA-256: 77cb984540cafa5b2dc44035fe988187a16660867f89116d58aaa0b714f62b2a.
This measured source snapshot predates the subsequent conformance follow-up.
