# Conformance follow-up

Historical implementation checkpoint. Its full-suite measurement below is
superseded by the current result in
[latest.json](../tools/test262/latest.json).

The complete optimized run measures **92,800 / 102,956 (90.1356%)** on pinned
Test262 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81. This exceeds the 90% target
of 92,661 passes by 139 variants. Against the preceding binary-data full run,
there are **1,580 gains and four losses (net +1,576)**. The remaining outcomes are 10,154
failures and two timeouts, with zero skips, crashes or harness errors.
All 102,956 variant identities match the baseline. Four jobs, the original
five-second timeout, corpus assertions, denominator and execution/allocation
caps are unchanged.

Worker SHA-256: f13bdc1fd0724206aeffd1d6dc97950be605e3f36290edd222be1430ce58b40e.
The exact implementation patch and its source base are in the evidence folder;
latest.json points to the full archived report and records the patch hash.

Implemented behavior:

- TypedArray array-like `set` with observable offset/length/index conversion,
  overlapping source snapshots, content-type validation, and raw matching-kind
  copies; observable index/fill/search conversion; stable numeric sort with
  guest comparator support, BigInt ordering, NaN and signed zero; copying
  `with`, `toReversed`, and `toSorted`; locale conversion with internal length.
- Annex B HTML string wrappers, quote escaping and lossless UTF-16; trim aliases
  with shared identity; `substr`; legacy `escape` and `unescape` code-unit rules.
- Temporal.Now uses the host clock and branded Temporal values, including
  explicit time-zone conversion. Its default time zone matches the VM's UTC
  Date default. Clock resolution is milliseconds.
- All seven Set composition/predicate methods, ordered set-like record
  acquisition, size-based branches, live source traversal, and early iterator
  closing. Map/Set deletion and clear retain tombstones; forEach visits live
  entries and additions in insertion order.
- Map.groupBy and Object.groupBy preserve key identity/property keys, callback
  indices and abrupt iterator closing. Object groups have a null prototype.
- Function.prototype.toString checks callability and emits standard native
  function syntax because original source text is not retained by the VM.
  Function call/apply/bind and Symbol.hasInstance metadata are corrected.
- DisposableStack uses private branded state, traced resource records, reverse
  disposal, move, pending-state checks, and nested SuppressedError values.
  SuppressedError has error branding, standard metadata and property order.

Known remaining gaps include AsyncDisposableStack and using declarations,
immutable ArrayBuffers, live Map/Set iterators, and other pre-existing engine
limitations. Their tests remain in the denominator and continue to fail.

## Validation

Eight new tests execute independently as AST and verified bytecode. They check
coercion order, stable copying sorts, UTF-16 HTML, the actual clock/time zone,
Set record acquisition and iterator closing, collection mutation, disposal
move/error ordering, and grouping/legacy escaping. All 96 selected root
regressions pass. All 354 core library tests pass with the historically failing
native-addon fixture excluded; all 13 collection integration tests pass.
Strict Clippy, formatting/whitespace checks and all 12 runner tests pass.

Focused evidence is archived in tools/test262/evidence/conformance-followup.
An optimized focused run resolved all six debug timeouts. It exposed eight
TypedArray search regressions involving detachment and undefined; the corrected
searches pass their full focused directories. Overlaying the latest focused
reports on the binary-data full report projects 92,714 passes, with 1,490 gains
and no losses. This projection spans different workers and is only a planning
estimate; the complete full run is authoritative.

## Full evidence

`tools/test262/evidence/conformance-followup/full.json.gz` preserves all results;
`full-transitions.json.gz` preserves every status change;
`full-comparison.json` records the denominator and regression audit. The net 1,576
full-run gain exceeds the 1,490 focused projection because shared changes also
improve tests outside the focused selections. Both full-run timeouts remain
recorded as timeouts; neither is excluded from the pass percentage.

The four regressions are both script/strict variants of
`staging/sm/TypedArray/sort_byteoffset.js` and
`staging/sm/TypedArray/sorting_buffer_access.js`. They now exhaust the existing
loop budget during stable sorting. The limits are unchanged, and these failures
are retained in the report and denominator. Focused selections did not cover
these staging tests; their earlier zero-loss comparisons remain selection-bound.
