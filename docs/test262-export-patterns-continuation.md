# Exported destructuring bindings

Export declarations now collect every name from a binding pattern using the
same pattern-name walker used by declaration hoisting. Previously only simple
identifier declarations appeared in the export table, so object and array
patterns initialized local variables without exposing their exports.

The change handles var, let, and const; nested object and array patterns;
renamed properties; defaults; rest bindings; and multiple declarators. The
existing module cells preserve live updates and lexical initialization checks.
Duplicate exports remain syntax errors.

Both new regressions fail before the change. All 27 module integration tests
pass after it. All 369 core-library tests pass without exclusions, as do strict workspace
Clippy and formatting. The final release passes **3,486 / 3,504** selected variants, gaining
**one pass with zero losses**. All **253 / 253** top-level-await variants
now pass: the remaining failure used a destructured export. See the
[archived evidence](../tools/test262/evidence/export-patterns-2026-10-20/summary.json). The
completed full snapshot ffc10c7 excludes this continuation.
