# Async cycle outcomes and imported namespaces

Async module traversal now keeps an ordered ancestor path. A back edge assigns
the cycle root to every member on that path. Overlapping cycles merge their
previously discovered members as well, including members whose recursive
traversal has already returned. Acyclic dependencies retain separate outcomes.

Importing a member of a cycle waits for the cycle root's evaluation outcome,
including an already-recorded rejection. On success the import resolves to the
requested member's namespace, rather than returning the root module's namespace.
The existing internal namespace/dependency aggregator is shared with deferred
imports, preserving dependency waiting without public promise hooks.

This follows the cycle-root behavior of
[Module Evaluate](https://tc39.es/ecma262/multipage/ecmascript-language-scripts-and-modules.html#sec-moduleevaluation).

Two regressions fail before the change. Three tests cover shared failure
identity across all members, completed members waiting for a pending root,
correct namespace identity on success, overlapping cycles, and independent
acyclic dependencies. All 33 module integration tests pass. All 369 core-library tests without exclusions, 19 worker tests, strict workspace
Clippy, and formatting pass. The release comparison passes **2,627 / 2,637** selected variants, gaining
**two passes with zero losses**. All **1,900 / 1,900** dynamic-import variants
now pass. See the [archived evidence](../tools/test262/evidence/async-cycle-2026-10-20/summary.json). The completed
full snapshot ffc10c7 excludes this continuation.
