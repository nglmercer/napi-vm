# Global-write, native-call and RegExp timeout continuation

This continues draft PR #40 from `d1bafe4`. It keeps the pinned Test262 corpus,
five-second deadline, two workers, feature selection and execution budgets.

## Changes

Repeated global counter updates previously constructed descriptors and switched
through generic global-property operations. Existing own writable data slots can
now update their actual global object record directly. Key coercion and RHS
execution precede the guard, so descriptor changes and deletion still matter.
Accessors, missing properties, protected data and different receivers retain the
generic operation. Global-property quota bookkeeping remains intact; recording
an already known name no longer allocates another duplicate string. A quota
regression checks that overwriting a builtin still consumes its property name.

Both owned-argument and borrowed-argument native calls have a same-realm fast
path. It preserves allocation realm, cancellation/argument guards and error
context. Bound, closure-backed, host and foreign-realm functions retain the
existing dispatch path. Ordinary property-cell data updates also preserve their
key layout rather than invalidating it, including hot RegExp lastIndex updates.

Repeated small classic RegExp patterns reuse a bounded compiled-pattern cache
owned by the interpreter. It holds at most eight successful patterns, each with
at most 256 ASCII source units and eight flag bytes. Unicode/property-set modes
use the uncached compiler; these features are not excluded from tests. Every
evaluation creates fresh RegExp identity, properties, lastIndex and legacy state.
Constructor coercion still runs before lookup; invalid patterns/flags are not
cached. The native-addon decoder retains its original uncached entry point.

## Validation

375 core tests and 59 focused integration tests pass. These cover global
descriptors, strict failures, RHS deletion/redefinition, with receivers, native
conversion errors, RegExp instance isolation, compile mutation, coercion and
bounded cache retention. Core Clippy across all targets with warnings denied,
formatting and all 12 Python runner tests pass.

Both measured workers use Rust 1.99.1 and the repository release profile: thin
LTO, one codegen unit. Builds and diagnostic probes do not overlap corpus runs.
The Date harness and its helpers already select bytecode (tier inspection is
archived); no AST-fallback claim is made for those workloads.

## Historical timeout selection

| Outcome | Frozen PR #40 baseline | Patched |
| --- | ---: | ---: |
| Pass | 3 | 26 |
| Timeout | 49 | 26 |
| Other outcomes | 0 | 0 |
| Total | 52 | 52 |

The exact comparison records **23 new passes and zero lost passes**. This is a
same-session comparison; older sessions' counts are not substituted for the
measured baseline. No overall Test262 conformance score is updated.

Remaining measured timeouts include all 16 exhaustive Date variants, four
large decodeURI/decodeURIComponent variants, two TypedArray sorting variants,
two giant parser/block variants, the script ArrayBuffer coercion variant, and
the script non-whitespace RegExp variant. Their strict counterparts pass for
the latter two files, so their measured timing sensitivity is retained.

**Passing all timeouts remains incomplete.** Focused gains do not establish a
full corpus pass. Further work needs to reduce call-frame/loop costs for the
exhaustive Date and URI workloads, and inspect sorting and huge eval parsing.

## Broader regression evidence

A matched constructor/protocol/string-method/global/cross-realm selection runs
4,078 variants. Both workers record 4,058 passes, 18 failures and two timeouts;
there are **zero lost passes or status changes**. The reports record every path
and no feature skips. This selection includes all RegExp prototype tests,
constructor/root tests, classic RegExp directories, non-generated property tests,
a generated White_Space failure case, string protocols, Function.call, globals,
with bindings, Reflect.set and the realms group.

The full patched RegExp/string/call/global sweep also completes: **4,775 passes,
18 failures and 22 timeouts out of 4,815**. The equivalent full baseline attempt
was interrupted before producing a report because the generated property/set
workloads made the comparison overly expensive. Its partial results are not
counted. All 18 semantic failure variants in the full patched report are present
in the matched baseline selection as failures. No full-sweep no-regression claim
is made for the unmatched generated workloads.

Two isolated diagnostics retain their exact sources and timings. Updating globals
100,000 times improves from 2.972 s to 0.303 s. The Date-call diagnostic is also
archived; microbenchmarks are not Test262 passes. Corpus measurements are separate
and never overlap these probes.

[Evidence](../tools/test262/evidence/loop-call-timeouts-2026-10-24/) includes raw
variant reports and binary hashes, both matched comparisons, complete selections,
the full patched sweep, the interrupted-baseline record, remaining inventory,
validation logs and diagnostics.

