# Cyclic and ambiguous export resolution

Export resolution now distinguishes a binding, a missing binding, and an
ambiguous binding. A circular indirect request returns missing to its caller,
allowing another star export to supply the binding. Nested star ambiguity
propagates instead of being ignored when another branch supplies one candidate.
All branches share the resolution set for the request.

Explicit indirect exports are validated after graph resolution, even without
an importing consumer. Missing exports and unresolvable cycles still produce
SyntaxError before module bodies execute. Named imports require an unambiguous
binding; ambiguous star names remain absent from namespace objects. Binding
identity continues to determine ambiguity rather than the current export value.

This follows [ResolveExport and InitializeEnvironment](https://tc39.es/ecma262/multipage/ecmascript-language-scripts-and-modules.html#sec-resolveexport).

Three regression tests cover single and multi-module cycles with live updates,
nested ambiguity in both star orders, and invalid re-exports without consumers
or side effects. Before the change, the cycle and nested-ambiguity tests fail;
after it, all 25 module integration tests pass. All 369 core-library tests pass without exclusions, as do strict workspace
Clippy and formatting. The final release passes **2,618 / 2,637** module/import variants, gaining
**two passes with zero losses**. This includes all 109 deferred-import cases.
See the [archived evidence](../tools/test262/evidence/export-resolution-2026-10-20/summary.json). The running full snapshot
ffc10c7 excludes this continuation and the preceding namespace TDZ fix.
