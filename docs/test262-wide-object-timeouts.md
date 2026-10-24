# Wide-object timeout continuation

This continues draft PR #39 from `87ad3d9`. The pinned corpus, two-worker
configuration, five-second variant deadline, feature selection and sandbox
budgets remain unchanged. Diagnostic probes are separate from corpus results.

## Root cause and changes

The copyWithin detachment fixtures construct every TypedArray with all supported
argument factories. Isolating each factory showed that `makeArrayLike` alone
exceeded a 12-second diagnostic deadline; the other factories completed in about
0.1–1.1 seconds. A first property-write optimization reduced that factory to about
2.3 seconds, but full fixtures still exceeded five seconds.

Ordinary same-receiver data writes now preserve layouts instead of allocating
and applying descriptors on every assignment. Protected data, accessors, mapped
arguments, exotic receivers/prototypes and Proxy receiver rules retain their
shared operations. Own reads use the existing property index.

Wide-object growth also needs reusable negative lookups. ObjectCell's ordinary
mutable slot APIs now invalidate layouts before exposing storage. Writes that
maintain keys explicitly use private storage and preserve/update the layout.
If a host exposes the underlying RefCell through Deref, cached absence is disabled
permanently: a retained raw reference could mutate after a later cache rebuild.
Hits still verify their actual slot. Tests cover same-length host renames,
retained raw references, failed lookups followed by insertion, and wide objects
that only receive writes.

Number-keyed Array/TypedArray element operations avoid key string round trips
when receiver and index semantics permit it. Numeric negative zero still becomes
"0"; the literal string "-0" remains distinct. Invalid TypedArray writes still
coerce values. Global own-data binding reads avoid redundant descriptors and
realm storage switching; accessor and inherited lookups retain the generic path.
The fill prototype guard ignores unrelated named accessors while still rejecting
indexed prototype properties.

The bytecode cache test now checks the intended behavior: a stable slot cache
survives ordinary value writes, while cached reads observe updated values. It no
longer requires a cache miss after every otherwise layout-preserving assignment.

## Validation

- 373 core unit tests and 58 focused integration tests pass.
- 12 Python runner tests pass; formatting and core Clippy with warnings denied pass.
- The six complete copyWithin detachment variants pass at the unchanged deadline.
- Final measured workers use Rust 1.99.1, repository release optimization,
  thin LTO and one codegen unit; no builds or probes overlap measured corpus runs.

The initial same-session PR #39 baseline recorded 52 timeouts out of the 52
variants in the historical selection, rather than the earlier session's 46.
This difference is preserved. Intermediate candidates also timed out on all 52;
no unstable isolated pass is counted as a fixed timeout. Final counts and raw variant comparisons are preserved in the evidence.

## Measured results

| Selection | Baseline pass / timeout | Patched pass / timeout |
| --- | ---: | ---: |
| Historical timeout files (52 variants) | 0 / 52 | 5 / 47 |
| Property/global regressions (547 variants) | 547 / 0 | 547 / 0 |
| Array/TypedArray neighbors (240 variants) | Not rerun | 240 / 0 |
| Detachment fixtures only (6 variants) | Timed out in the historical selection | 6 / 0 |

The historical comparison has **five new passes and zero lost passes**. Of the
original inventory's 49 variants, five pass and 44 still time out. The other three
selected neighboring variants also time out. The script variant of
`coerced-values-end-detached-prototype.js` timed out in the historical run despite
passing both focused detachment and neighboring runs; that outcome is retained.
Do not describe all six variants as unconditionally fixed across selections.

The final isolated `makeArrayLike` diagnostic changes from a timeout beyond
12 seconds to completion in 0.583 seconds. It uses a deliberately restricted
factory list and is **not** a Test262 conformance result. The full fixtures used
for corpus results retain every factory and the original source.

**The all-timeouts goal remains incomplete.** Remaining families include
exhaustive Date combinations, URI/RegExp guest loops, large TypedArray workloads,
and the timing-sensitive detachment variant. No overall Test262 score is updated.
Further work should profile these full fixtures and reduce interpreter loop/call
costs; focused passes are not a substitute for a completed full corpus run.

[Evidence](../tools/test262/evidence/wide-object-timeouts-2026-10-24/) contains
raw per-variant reports and worker hashes, both unfiltered comparisons, the exact
remaining inventory, diagnostic inputs/results, and Rust validation logs.

