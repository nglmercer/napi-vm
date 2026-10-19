# Numeric property key conversion continuation

The parser now uses the shared ECMAScript number-to-string formatter for numeric
object keys, class member keys, destructuring keys, and literal computed method
and accessor keys. Literal-key optimization therefore agrees with runtime
ToPropertyKey, including the small/large exponent boundaries and subnormal
numbers. Inferred method/getter/setter names use the same canonical key.

Five regressions compare prepared execution with raw AST execution, cover
1e-7, 1e-6, 1e20, 1e21, and the minimum positive subnormal, and verify that
literal and runtime-computed keys overwrite the same property. A separate tier
assertion requires verified bytecode for literal computed accessors. Dynamic
computed methods retain their existing prepared AST fallback.

All five integration tests, 37 parser tests, and 79 selected bytecode tests
pass. The known unrelated super-destructuring parity failure is excluded from
the bytecode selection. Strict workspace Clippy and formatting pass.

The release selection passes **2,702 / 2,729** variants, with **10 gains and
zero losses** against 2,692 baseline passes. All **2,252 object-expression**
variants and selected class name cases pass. The remaining 27 failures concern
computed-property evaluation and destructuring; there are zero skips, timeouts,
crashes, or harness errors. The selection covers object expressions, computed
property names, destructuring, and class accessor/method/numeric names. Reports,
comparisons, worker identity, and the source patch are archived under
[numeric property key evidence](../tools/test262/evidence/numeric-property-keys-2026-10-18/).
The latest completed full report remains 101,889 / 102,956 on 997b476; this
property-key change is outside that measured snapshot. The separate full run
on a528b02 remains active.
