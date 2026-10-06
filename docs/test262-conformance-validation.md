# Test262 conformance validation: merged PR #22 baseline

This document preserves the PR #22 baseline. Current phase work is documented in [phases 1–3](test262-phases-1-3.md), and `tools/test262/latest.json` records the latest complete measurement.

This change improves conformance; it does not achieve a full Test262 pass or establish a stable ECMAScript compatibility claim.

## Implementation

- Collection constructors require `new`, capture overridden adders before iteration, honor iterator accessors, stream entries, and close iterators on insertion errors while preserving the original exception. Private collection brands prevent forged or cross-kind receivers. Map and WeakMap upsert methods handle existing `undefined` values and callback mutation.
- Native methods carry proper function identity, descriptors, names and lengths. Reflect.construct validates constructors and reads array-like arguments through getters, with alternate instance prototypes.
- Date operations use TimeClip, reject invalid ISO serialization, handle extended years and extreme numeric inputs without overflow, and preserve ECMAScript floating-point evaluation order.
- Atomic regular-expression repetition runs iteratively. Complex nested matching has a bounded depth and returns RangeError instead of overflowing the native stack.
- Constructor member parsing accepts keyword property names.
- The Test262 worker keeps asynchronous completion state globally, rejects caught `$DONE(error)`, exposes `$262.global`, and evaluates `$262.evalScript` globally. Explicit, canonicalized corpus access supports nested and dynamic module fixtures without granting filesystem access to embeddings.
- Scheduled CI runs every variant without feature skips and preserves failures, the dashboard, and triage artifacts. Failure grouping retains the complete denominator.

`FunctionData` now has a public `native` field. Embedders constructing this Rust struct directly must initialize it to `None` for JavaScript functions.

## Validation

The pinned corpus is `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.

Before this change, the complete development run recorded 33,547 passes / 102,956 variants (32.58%): 68,037 failures, 870 timeouts, 32 crashes, 470 harness errors and zero skips. That run used an unoptimized worker, 16 workers and a two-second timeout; its worker SHA-256 was `f35c670b84f76c1af8ae0ac06c7a8ea96347530b62f2f7d75e4207d997858d59`. Timing-dependent outcomes are not directly comparable with the optimized final run.

The final complete optimized run recorded **34,736 passes / 102,956 variants (33.74%)**, with 67,740 failures, two timeouts, eight crashes, 470 harness errors and zero skips. It used four workers and a five-second timeout, with worker SHA-256 `15db9b7181505007c634cdd45d2af36240e437f07a882b810835bf7e27a24a4b`. At merge time, the compact evidence was versioned in `tools/test262/latest.json`; full rows, the dashboard and triage are generated under `artifacts/test262/`.

The eight remaining crashes occur in three destructuring tests (both modes) and String subclassing (both modes). Four previously passing regex variants now fail with the explicit nesting-budget RangeError. The corrected async harness reports existing Promise/async failures previously masked by caught completion errors. These effects remain visible in the full score.

The focused final run passed 1,318 / 1,708 selected variants (77.17%), with 390 failures and no crashes, timeouts, skips or harness errors. The optimized confirmation used two workers, a five-second timeout and worker SHA-256 `15db9b7181505007c634cdd45d2af36240e437f07a882b810835bf7e27a24a4b`; the earlier unoptimized run produced the same outcomes. This subset percentage is not an overall compatibility metric.

| Selection | Passed / total |
| --- | --- |
| Map | 349 / 405 |
| Set | 438 / 764 |
| WeakMap | 277 / 281 |
| WeakSet | 166 / 170 |
| Reflect.construct | 20 / 20 |
| Date.UTC | 34 / 34 |
| Date.prototype.toISOString | 34 / 34 |

Additional verification: 652 Rust tests passed (four ignored), 73 native Node tests passed, 14 WebAssembly tests passed, four Python runner/triage tests passed, and formatting, Clippy with warnings denied, and workflow lint passed. Bun recorded 1,356 passes and the same 80 failing test names as the existing baseline.

## Remaining work

Temporal, strict/parser semantics, missing builtins, object descriptors, live collection iteration, modern Set operations, realms, and other language features still cause substantial failures. `$262.createRealm`, agents, GC and ArrayBuffer-detachment host hooks remain incomplete. Complex regex patterns can reach the depth limit, and Unicode property handling and execution/resource limits also affect conformance. No failures are hidden or counted as passes.
