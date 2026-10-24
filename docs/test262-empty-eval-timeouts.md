# Eval allocation, binding reads and numeric sorting

This continues draft PR #41 from `d571b5b`. All conformance measurements retain
Test262 revision `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, two workers, the
five-second deadline, the selected features and engine execution caps. No
variant is skipped to improve the result.

## Changes

Eval recognizes a restricted grammar containing only empty blocks, empty
statements, whitespace and comments directly from UTF-16 units. These scripts
have no declarations or observable operations and return undefined. Recognition
avoids allocating millions of tokens, AST nodes and lexical-scope index entries
for large no-op scripts. It counts blocks and semicolons and charges the same
statement fuel as full AST evaluation, checking cancellation approximately every
4,096 code units. Other syntax, malformed input and nesting deeper than 32
blocks use the normal parser. There are no test-path checks.

Binding reads traverse plain lexical environments once and directly read an
existing own global data slot. With environments, accessors, missing names and
inherited globals retain observable reference resolution. Lexical shadowing,
live bindings, TDZ and the constructor this special case remain intact.

Date reads can directly resolve a data slot on the current immediate ordinary
prototype when no own property or accessor interferes. No prototype or method
value is retained across reads. Shadowing, replacement, accessors, Proxy
prototypes and module namespaces retain their ordinary operations. The generic
Date prototype delegation now correctly calls Proxy get traps, retaining the
original receiver. Exotic instance property allocation also avoids resolving
Object.prototype only to immediately discard it, keeping realm and prototype
flags unchanged.

Large numeric TypedArrays with the default comparator use stable radix sorting.
The float key orders negative zero before positive zero and all NaNs last,
retaining their input order and payloads. Ordered input returns after a linear
scan. Custom comparators, BigInts and smaller inputs retain merge sorting.
Sorting passes retain loop-budget and cancellation checks; errors do not write
partially sorted values back to the TypedArray.

## Validation

380 core tests and 65 focused integration tests pass, including both execution
tiers, binding shadowing, with/unscopables, TDZ, global deletion/accessors,
Date getter receivers and Proxy prototypes. Numeric tests compare random float
bit patterns and special values against the old comparator, check custom
comparators, BigInts, toSorted source isolation and budget exhaustion. Empty-eval
tests compare completion and fuel with full parsing and check malformed syntax,
directive boundaries and cancellation. All 12 Python runner tests, formatting
and core Clippy across all targets with warnings denied pass.

Frame pooling was investigated and removed after its timing results did not
establish a clear benefit. It is not part of this implementation.

## Measurements

| Selection | Parent | Patched |
| --- | ---: | ---: |
| Historical passes | 32 | 32 |
| Historical timeouts | 20 | 20 |
| Historical other outcomes | 0 | 0 |
| Regression passes | 2,737 | 2,737 |
| Regression failures | 2 | 2 |
| Regression timeouts | 0 | 0 |

There are **zero newly passing variants, zero lost passes and zero status
changes** in these fresh matched selections. The historical set runs 52
variants; the regression set runs 2,739. The two existing regression failures
are the script variants of eval-code/direct/var-env-func-init-local-new-delete.js
and var-env-var-init-local-new-delete.js. No overall Test262 score is updated.

**All remaining timeouts are not fixed.** All 16 Date DST-cache variants and
four exhaustive decodeURI/decodeURIComponent variants still exceed five
seconds. The full exact inventory is archived with the reports. These workloads
need substantially more throughput than the current changes provide.

Separate diagnostic requests use a 30-second observation limit and are not
counted as conformance passes. The initial eval/sort candidate reduces the giant
eval script diagnostic from 4.366 seconds to 0.076 seconds. Both variants already
pass in the fresh five-second parent selection, so no newly passing variant is
credited. Initial diagnostics also record Date at 13.424 seconds for the parent
and 12.131 seconds for that candidate; URI is 15.824 and 16.076 seconds. The final
binding/Date-read candidate records representative Date and URI diagnostics of
10.456 and 12.795 seconds respectively. These are separate probes with their
raw outcomes retained, not a replacement for the five-second comparison.

The reports before the environment restart did not survive; none of their
partial outcomes is substituted for these fresh reports. The implementation
source is checkpointed in draft PR #42. Initial-candidate and final reports
identify their exact binary hashes; source commits are recorded in summary.json.

[Evidence](../tools/test262/evidence/empty-eval-timeouts-2026-10-24/) contains raw
reports, comparisons, selections, diagnostics, validation logs and the exact
remaining inventory.
