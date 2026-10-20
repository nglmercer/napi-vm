# Dynamic deferred imports and asynchronous dependencies

Dynamic deferred imports now gather and start asynchronous transitive module
dependencies, then resolve their namespace promise only after every dependency
succeeds. Synchronous ancestors remain linked until namespace access requires
them. A dependency rejection rejects the import without executing those
synchronous bodies. Pending external work is propagated to the result promise.

Internal dependency aggregation and the final dynamic-import handoff use
promise reactions directly, without consulting Promise.prototype.then. The
namespace resolution still follows the existing promise resolution path.

This implements the deferred branch of
[ContinueDynamicImport](https://tc39.es/proposal-defer-import-eval/#sec-ContinueDynamicImport),
including its asynchronous dependency aggregation.

Two regressions fail before the change. Three tests now cover direct and
transitive await, manually released pending promises, rejection, patched then,
and multiple asynchronous branches that start together and finish separately.
All 30 module integration tests, 369 core-library tests without exclusions,
19 worker tests, strict workspace Clippy, and formatting pass. The release comparison passes **2,625 / 2,637** selected variants,
gaining **four passes with zero losses**. All four previously failing dynamic
deferred-import cases now pass. See the [archived evidence](../tools/test262/evidence/dynamic-defer-2026-10-20/summary.json). The completed full snapshot ffc10c7 excludes
this continuation.
