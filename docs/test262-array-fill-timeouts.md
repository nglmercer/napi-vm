# Array fill timeout continuation

The timeout investigation continues from main `635da74`. No deadline, corpus,
feature exclusion, or sandbox budget is changed. This is a focused performance
change, not a claim of full Test262 conformance.

## Implementation

- Indexed writes retain an existing all-true presence bitmap rather than
  scanning the entire bitmap after each write. Filling a holey array no longer
  performs quadratic presence checks. Bulk operations can still normalize it.
- `Array.prototype.fill` checks receiver descriptors and the prototype chain
  after argument coercion. Plain extensible arrays without indexed descriptors
  can write directly to indexed storage. Every element still consumes the
  ordinary loop budget.
- Accessors, indexed prototype properties, exotic prototypes (including
  proxies), non-extensible receivers, and protected indexed properties use the
  existing throwing property-write path. Coercion that shrinks the receiver
  beyond the requested end also retains that path.

Dense native Array iterator draining also avoids temporary iterator-result
objects and realm entry for each element. It requires the intrinsic `next`,
a dense Array with no indexed accessors, and a values iterator with ordinary
internal-slot storage. It preserves partial cursor updates, the final done step,
and per-step loop-budget checks. Customized `next`, holes, and source accessors
retain the generic iterator protocol.

## Validation

The 371 core library tests, 42 focused integration tests, 12 Python runner
tests, formatting, and core-library Clippy with warnings denied pass.
The integration tests cover hole filling followed by deletion, appending,
truncation and growth, coercion-time prototype mutation, inherited setters,
read-only elements, non-extensible receivers, and partial writes before errors. Iterator regressions cover a partially consumed
iterator, overridden next, getters that mutate later elements, budget exhaustion,
partial cursors, spreading, iterator helpers, and closing behavior.

Both measured workers use Rust 1.99.1 with the repository release profile
(thin LTO and one codegen unit). Measurements run after builds and checks,
with two workers, five-second deadlines, no exclusions, and pinned corpus
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`. The selection contains the 26
files holding the previous report's 49 remaining timeout variants and runs all
52 variants of those files.

## Results and remaining work

| Selection | Pass | Timeout | Other outcomes |
| --- | ---: | ---: | ---: |
| Historical files, clean baseline | 7 | 45 | 0 |
| Historical files, patched | 6 | 46 | 0 |
| Array fill/values, TypedArray copyWithin/from, patched | 234 | 6 | 0 |

The comparison records **zero new passes and one lost pass**, the strict
`encodeURIComponent/S15.1.3.4_A2.3_T1.js` variant. An earlier baseline rerun also
had six passes and 46 timeouts; the extra clean-baseline URI pass was not stable.
The loss is retained as measured, not reclassified. None of the 49 previously
recorded timeout variants is claimed fixed by this patch. **Passing all timeouts
remains incomplete.** No overall conformance score is updated.

Three isolated end-to-end samples (fresh worker per sample, including startup)
show median holey-array fill falling from 1.558 s to 0.060 s (about 26x), and
10,000-element TypedArray construction from 0.057 s to 0.015 s (about 3.7x).
These microbenchmarks do not establish Test262 passes. The complete sources and
sample times are archived alongside the raw reports.

The six neighboring timeouts are the same three copyWithin detachment fixtures,
with both script and strict variants. They exercise all constructor factories,
including large ordinary array-like objects and guest byte-copy loops. The
remaining historical cases also include URI/RegExp loops, exhaustive Date
combinations, large TypedArray sorting and shared-buffer workloads. Further work
needs profiles of these full fixtures, especially wide-object property writes,
guest loops and calls, rather than more deadline changes.

[Evidence](../tools/test262/evidence/array-fill-timeouts-2026-10-24/) includes
worker digests, every selected variant, the unfiltered comparison, microbenchmark
sources/times, and the exact remaining-case inventory. A full corpus rerun is
still required before updating the project-wide Test262 score.

