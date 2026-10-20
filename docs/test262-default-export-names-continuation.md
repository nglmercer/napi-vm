# Default export declaration names continuation

Anonymous default function and class declarations use the parser's inaccessible
*default* binding, but that synthetic binding name must not leak through the
public name property. A shared declaration-display helper now supplies default
to both execution tiers while retaining the hidden binding for module linking.
Class names are available before static initializers execute.

Export-default expressions also use the existing named-evaluation path for
anonymous functions, arrows, and classes. Explicit declaration/expression names,
existing value names, and static name fields retain their own values.

Four regressions require prepared bytecode and independently execute the AST,
with an additional linked-module import check. They cover regular/async/generator
functions, classes, default expressions, static initializer timing, and explicit
names. All four regressions, 20 module integration tests, strict workspace
Clippy, and formatting pass.

The core-library run passed 367 tests and failed the native-addon fixture with
an invalid guest object reference. The bridge synchronization issue remains
intermittent; this is retained as an unresolved validation failure rather than
reported as a full-core pass.

The fresh release passes **18,120 / 18,184** variants, gaining **seven passes
with zero losses** against the frozen predecessor. All class runtime cases in
this selection pass; the remaining class failures concern parsing. There are
zero skips, timeouts, crashes, or harness errors. Reports, comparison, worker
identity, and the source patch are archived in
[default export-name evidence](../tools/test262/evidence/default-export-names-2026-10-20/).
The latest full Test262 score separately measures a528b02 at 101,895 / 102,956;
this change and the following host snapshot-order fix are outside that snapshot.
