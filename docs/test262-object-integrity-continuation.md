# Object integrity and iterator continuation

The Object built-ins now apply `SetIntegrityLevel` and
`TestIntegrityLevel` through the interpreter's internal methods. Proxy traps
run in specification order, abrupt completions propagate, and variable-length
typed arrays reject `[[PreventExtensions]]`: all resizable-ArrayBuffer views
and length-tracking views over growable SharedArrayBuffers are covered. Fixed-
length views over growable SharedArrayBuffers remain preventable.

`Object.fromEntries` now processes each iterator entry before requesting the
next one, reads both entry fields before converting the key, and closes the
iterator while preserving the original error when `next`, iterator-result
access, or entry processing fails.

The focused AST/verified-bytecode regression suite passes **32 / 32** tests.
The pinned Test262 selection for `Object.fromEntries`, `freeze`, `seal`,
`isFrozen`, `isSealed`, `preventExtensions`, and `Reflect.preventExtensions`
passes **622 / 622** variants, with no skips, timeouts, crashes, or harness
errors. Focused outcomes and the exact implementation patch are archived in
[`tools/test262/evidence/object-integrity-2026-10-18/`](../tools/test262/evidence/object-integrity-2026-10-18/).
Strict workspace Clippy, formatting, whitespace, and all 12 Test262 runner
unit tests pass.

The last complete pinned full-corpus measurement remains **101,488 / 102,956
(98.5741%)**. It predates these changes and the follow-up `Object.fromEntries`
fixes, so the focused gains are not added to the overall score. The compact
status records the full-run worker separately from this focused worker in
[`latest.json`](../tools/test262/latest.json).

The next focused implementation completed deferred module evaluation: its
pinned 109-variant `language/import/import-defer` selection now passes 109 / 109.
The adjacent `language/module-code` and `language/expressions/dynamic-import`
selection passes 2,444 / 2,502, gaining four passes with zero losses against
the previous full-run outcomes for those exact variants. See the
[deferred-import continuation](test262-deferred-import-continuation.md) and
its archived evidence. The full-corpus score remains the older completed
measurement until the corpus is rerun.
