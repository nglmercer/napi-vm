# Test262 progress log

Current profile status and validation policy: [`test262-full-pass-plan.md`](test262-full-pass-plan.md).
Commands and reports: [`tools/test262/README.md`](../tools/test262/README.md).
Earlier experiment-by-experiment notes are listed in [`test262-history.md`](test262-history.md).

## Complete runs

The selected `reviewed-5112` profile passed **102,992/102,992 variants** at
the existing CI settings (two workers, ten seconds), with no failures,
skips, timeouts, crashes or harness errors. All 16 Date variants passed.
The audit verified that the variant keys exactly match the original
corpus and that only the reviewed test file differs. Evidence:
[`reviewed-5112-full-2026-11-06`](../tools/test262/evidence/reviewed-5112-full-2026-11-06/).

The 2026-11-06 native Date call snapshot completed all 102,992 variants with
102,976 passes, one failure and 15 timeouts. Comparison with the archived
2026-10-30 baseline found one gain and no losses, additions or removals.
Evidence: [`date-native-fast-path-2026-11-06`](../tools/test262/evidence/date-native-fast-path-2026-11-06/).
The worker digest is
`c5d969756d70eb207cc94241cfd1aba96d1a4e7e2a251f0c8105a57ad7615313`.
This snapshot predates the subsequent borrowed constructor arguments,
explicit Date allocation realm, and consuming cached-property dereference
changes. Those later changes are covered by the subsequent complete runs
recorded below and the reviewed-profile gate above. Both engine snapshots
passed 1,013/1,014 original-corpus smoke variants, with only
the conflicting Annex B test failing.

The subsequent worker (SHA-256
`cedda1803516ed6d0b3f9e46b624e36d7dc05aec459ff4561d63dc4b7a8dcad9`)
passed all 16 historical Date variants when retested with the existing CI
timeout of ten seconds and two workers. The contradictory Annex B test was
the only failure among the 17 retested variants; this is a subset result,
not a complete-corpus pass. Its report is archived as
[`latest-ci-timeout-retest.json`](../tools/test262/evidence/date-native-fast-path-2026-11-06/latest-ci-timeout-retest.json).

Two later experiments (in-place pooled environment reset and cached
property-key escape classification) passed their regression tests but were
removed after combined DST timings regressed from 4.97/5.03 seconds to
5.09/5.17 seconds under the same background workload. The restored release
binary matches the above digest.

The latest worker's original-corpus five-second run finished at
102,974/102,992: one failure and 17 timeouts, with no crashes, harness
errors, skips, additions, or removals. Comparison against the archived
baseline found one Date gain and two URI-decoding timeout losses. Both URI
variants passed subsequent five-second probes (4.22 and 4.26 seconds), so
the short-deadline full run must retain its measured timeout losses rather
than be presented as regression-free. Evidence:
[`latest-original-5.json.gz`](../tools/test262/evidence/date-native-fast-path-2026-11-06/latest-original-5.json.gz)
and [`comparison`](../tools/test262/evidence/date-native-fast-path-2026-11-06/latest-original-5-comparison.json).

The user selected the separate `reviewed-5112` profile to resolve the
contradictory corpus expectations explicitly. Its deterministic revision is
`cff513bc172b79b97963046e83078e114b2a20ff`, a one-file overlay of PR head
`1f3d994e2658c2934a64781ffbb7a9b3782d3310` onto the original pin. Its smoke
gate passes all 1,014 variants without skips or errors
([report](../tools/test262/evidence/date-native-fast-path-2026-11-06/reviewed-5112-smoke.json)).
The full corrected-corpus gate subsequently passed, as recorded above.

| Date | Source | Revision | Pass / total | Fail | Timeout | Evidence |
| --- | --- | --- | ---: | ---: | ---: | --- |
| 2026-10-25 | `259402f` | `5992dc3` | 102,760 / 102,956 | 133 | 63 | removed; see history |
| 2026-10-27 | `d1d403d` | `0b4e081` | 102,957 / 102,992 | 1 | 34 | [`full-new-revision-2026-10-27`](../tools/test262/evidence/full-new-revision-2026-10-27/) |
| 2026-10-30 | `85ba746` | `0b4e081` | 102,975 / 102,992 | 1 | 16 | [`full-interpreter-overhead-2026-10-30`](../tools/test262/evidence/full-interpreter-overhead-2026-10-30/) |
| 2026-10-30 | `9f5c446` | `0b4e081` | 102,975 / 102,992 | 1 | 16 | not archived; zero gains and losses against `85ba746` |
| 2026-10-30 | `9bc2103`, Rust runner | `0b4e081` | 102,973 / 102,992 before the CR fix | 3 | 16 | not archived; the two extra failures were fixtures with bare-CR line endings in the runner's metadata parser, fixed in the same commit |

The `85ba746` run gained 18 variants over `d1d403d` and lost none. The
`9bc2103` tree matches `85ba746` variant for variant once the metadata parser
handles bare carriage returns; that was checked with the Rust runner's
`compare`.

## Changes measured by these runs

- `85ba746`: boxed rarely used object and frame payloads, lazily allocated
  hash collections, cached `Object`/`Array`/`Date` allocation prototypes, a
  direct `new Date` path for primitive arguments, in-place writes to existing
  global data properties and numeric lexical bindings, cached closure chain
  context, and inlined instruction dispatch. RegExp literal tests dropped from
  about 5.2 to 1.1 seconds alone; URI decoding from 5.5 to about 4.2; the
  TypedArray counting sort from 5.6 to about 4.8.
- `9f5c446`: single-load call arguments written directly into argument
  registers; statement completion tracking skipped in function bodies; scalar
  overwrites without drop glue; captured bindings returned without a second
  chain walk.
- `9bc2103`: Date property objects created on first use (`new Date` about
  twice as fast; the DST tests moved from about 6.5 to about 6.1 seconds alone).

## Original-profile five-second limitations

- `annexB/language/function-code/block-decl-func-skip-arguments.js`:
  contradicted by a staging test in the same revision (see the plan).
- `staging/sm/Date/dst-offset-caching-1..8-of-8` (script and strict): timeouts,
  historical measurements around 6.1 seconds alone against a five-second limit.
  The selected reviewed-profile full run passes all 16 at ten seconds.
