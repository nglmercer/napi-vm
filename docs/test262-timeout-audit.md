# Fresh timeout audit and recommendations

Cloned with `gh repo clone nglmercer/napi-vm`. The initial checkout was clean
on `main` at `39f6a97b77e7de1001581cc242b8c0cafb5bd4b6`.
The investigation is on local branch `investigate/test262-timeouts`.

## Fresh measurement

Both workers use Rust 1.99.1 on aarch64 Termux and the repository's
`ci-optimized` profile: optimization level 3, LTO disabled, 16 codegen units.
The corpus is pinned to `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.
Runs use two workers and the unchanged five-second deadline, without skips.
No compilation overlaps conformance runs or timing samples.

The selection contains all variants of the 23 files holding the last archived
46 remaining timeouts. A separate selection covers RegExp exec/test/flags,
Annex B compile, and TypedArray constructor internal operations.

| Selection | Clean main | Experimental worker |
| --- | ---: | ---: |
| Timeout selection | 0 passes, 46 timeouts | 0 passes, 46 timeouts |
| Neighboring regression | 780 / 780 passes | 780 / 780 passes |

There are zero new passes and zero lost passes in either comparison. There are
no failures, crashes, skips, or harness errors in these runs. **Fixing all
timeouts remains incomplete.** This is not a new overall conformance score.
The older full-suite result remains separate: 101,978 / 102,956 variants,
753 failures, and 225 timeouts on an earlier source snapshot.

## Rejected experiment

The experimental worker caches immutable compiled RegExp patterns while
creating fresh guest objects, and bypasses numeric-key string conversion in
two TypedArray access paths. All 11 RegExp integration tests and two numeric
access tests pass, including identity, compile mutations, UTF-16, negative
zero, detachment, and BigInt coercion. All 12 Python runner tests also pass.

Three isolated samples per worker include process startup:

| Microbenchmark | Baseline median | Candidate median |
| --- | ---: | ---: |
| 20,000 simple RegExp literal matches | 1.667 s | 1.627 s |
| 100,000 TypedArray writes and reads | 10.947 s | 11.036 s |

These small differences do not establish a useful speedup. Neither experiment
fixes a recorded timeout. The candidate engine and test changes were reverted;
`candidate.patch` preserves the experiment for inspection. The compiled
candidate worker remains at `artifacts/test262/candidate-worker`, not the current
source. The main build output was restored from the saved baseline worker.

## Recommended order

1. Profile complete fixtures and record their execution tier and fallback
   reason. The APIs already expose `PreparedProgram::tier`, `fallback_reason`,
   and bytecode statistics. Measure guest-loop dispatch, scope access, calls,
   allocation, and built-in work separately before choosing another patch.
2. Address ordinary-object insertion scaling in the TypedArray fixtures.
   `makeArrayLike` creates 10,000 indexed properties; `set_cell_property`
   linearly scans the slot vector for each write. Investigate a dictionary
   index with reliable invalidation for deletion, descriptor changes, and host
   mutations through the exposed slot storage. Preserve insertion order and
   prototype/setter behavior. Merely caching successful lookups does not remove
   the scan for each new key.
3. Optimize the measured loop/call costs in exhaustive Date and URI workloads.
   The Date harness nests four timestamp-selection loops. Native built-in
   tweaks alone may leave most of that guest execution cost intact. Verify
   bytecode coverage and semantic parity before expanding fast paths.
4. Profile the remaining RegExp eval/literal and large TypedArray sorting and
   shared-buffer workloads. Dynamic regexp tests create many distinct sources;
   a repeated-pattern cache alone cannot eliminate that work.
5. After focused gains and neighboring regressions pass, run the entire pinned
   corpus on a frozen worker with a checkpoint. Then triage the remaining
   semantic failures. Only that completed report can establish a full pass.

The 46 timeout variants comprise 16 exhaustive Date, eight URI, eight RegExp
literal/eval, six TypedArray construction/copy, two RegExp replacement, and
six other workload variants. The exact cases, worker hashes, candidate patch,
raw outcomes, comparisons, and reproducible timing sources are in
[the audit evidence](../tools/test262/evidence/timeout-audit-2026-10-24/).
