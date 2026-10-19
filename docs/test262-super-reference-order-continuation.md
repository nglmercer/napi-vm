# Super reference receiver order continuation

A super property reference must resolve the owning activation's this binding
before evaluating its property-key expression. Previously the key expression
ran first, permitting side effects (including super()) before the required
ReferenceError in an uninitialized derived constructor.

AST evaluation now checks the receiver before evaluating super keys in reads,
calls, assignments, compound/logical assignments, updates, and destructuring
targets. Bytecode emits the equivalent receiver check before the key expression.
The existing reference construction and key conversion paths remain in place,
including deferred conversion after the RHS of a simple assignment.

Four regressions compare prepared bytecode and raw AST execution. They cover
nine uninitialized reference forms, conversion timing after initialization,
lexical this in arrows, and ordinary property writes before super(). All four
pass, along with eight optional-chain regressions, three public-hash regressions,
and all 368 core-library tests. Strict workspace Clippy passes.

The fresh release passes **19,080 / 19,125** variants, gaining **ten passes
with zero losses** against the frozen public-hash release. There are zero skips,
timeouts, crashes, or harness errors. The remaining 45 failures include 38
parse failures and seven runtime failures. Reports, comparison, worker identity,
and the source patch are archived in
[super receiver-order evidence](../tools/test262/evidence/super-reference-order-2026-10-19/).
The latest completed full snapshot separately measures a528b02 at
101,895 / 102,956; this continuation is outside that score.
