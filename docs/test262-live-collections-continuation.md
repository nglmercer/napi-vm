# Live collection iteration continuation

## Implementation

Map and Set iterators now retain their collection and traverse its stable data
list with a live cursor. Additions made before exhaustion are visited;
deletions are skipped; updated Map values are read when yielded. Deleting and
reinserting an entry appends a new visit. Clear preserves the data list and its
tombstones, so suspended iterators can observe later additions. Once an iterator
reports done, it releases the collection and remains done permanently.

Collection storage and iterator state now live in typed internal metadata.
Guest properties cannot replace the collection data or forge cursor state.
Map and Set iterator next methods validate distinct brands and reject proxies
and objects that merely inherit from an iterator. Frozen iterators can still
advance their internal slots.

Entry iterators create fresh `[key, value]` arrays instead of exposing the
internal entry record. Map keys canonicalize negative zero. Iterator objects,
step results, and entry arrays use the defining realms of the methods creating
them, including when methods are borrowed across realms.

The collector traces the retained collection and its backing data. Exhaustion
releases that retention; unreachable collection/iterator cycles are reclaimed.
Iterative teardown drains these metadata edges. Scanning data slots charges
instruction fuel without consuming the caller's guest-loop iteration budget.

## Measurement and validation

The corpus remains pinned to `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`,
with two workers, a five-second timeout, and no feature skips. The focused
selection covers Map, Set, MapIteratorPrototype, SetIteratorPrototype,
ArrayIteratorPrototype, WeakMap, WeakSet, and the Map/Set mutation for-of tests.
The release worker passes **1,726 / 1,726** variants, up from 1,708 / 1,726,
with **18 gains and zero losses**. The neighboring control-flow selection passes
**3,599 / 3,624**, up from 3,591 / 3,624 with eight overlapping gains and zero
losses. No selection has skips, timeouts, crashes, or harness errors.
Strict workspace Clippy, formatting, and **149 selected integration tests** pass.

Reports, comparisons, worker identity, and the source patch are archived under
[`live-collections-2026-10-18`](../tools/test262/evidence/live-collections-2026-10-18/).

Regression tests require bytecode preparation for the differential cases and
also execute the AST directly. They cover mutations, permanent exhaustion,
entry isolation, brands, guest slot names, negative zero, defining realms,
backing-data survival across collection, release on exhaustion, and cycle
reclamation. Collection, realm, weak-GC, lifecycle, engine-boundary, and iterator
protocol suites provide neighboring validation.

The latest completed full pinned Test262 run (2026-10-18, corpus
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, source `607d086`)
passes **101,728 / 102,956 variants (98.8073%)**, with 1,076 failures,
152 timeouts, and zero skips, crashes, or harness errors. It gains 18 passes
and loses 96 against the preceding full run, with two workers and a
five-second timeout. All lost passes are measured timeouts: 94 RegExp
property-escape variants and two staging variants. Outcomes are retained
without reclassification. The snapshot includes live collection iteration,
module attributes, control-flow completion, and strict tail calls.
See the [full live-collection evidence](../tools/test262/evidence/full-live-collections-2026-10-18/).
Later lexical scopes, nested finally, for-in enumeration, and mapped arguments
remain outside that full score. Full conformance remains incomplete.
