# Relational operators continuation

Guest `<`, `>`, `<=`, and `>=` use a shared mutable runtime comparison in
AST and bytecode execution. Both operands undergo ToPrimitive with the number
hint in original left-to-right order. Abrupt completion of the first operand
prevents conversion of the second. Numeric conversion follows primitive
conversion; Symbol operands throw instead of silently producing false.

String/String comparison preserves UTF-16 code-unit order. BigInt/String
comparison uses StringIntegerLiteral and returns an unordered result for
invalid strings. Mixed BigInt/Number comparison uses the exact shared helper
introduced in the equality continuation. NaN/unordered results produce false
for all four operators, including the inclusive comparisons.

Three additional tests compare forced verified bytecode with raw AST execution
for coercion order/hints, thrown conversions, Symbol errors, BigInt string
grammar, UTF-16 ordering, and nullish operands. All ten comparison tests pass.
The minimal core suite passes 354 tests with the known sidecar native-addon
fixture excluded. Workspace Clippy with all targets/features and `-D warnings`,
formatting, and whitespace checks pass. Known workspace native-addon failures
remain unresolved.

The source patch SHA-256 is
`544b07396c386bcc811dc85ee4769a27518ac8bdfd957b620aeffa8a2d93a87c`,
based on commit `9e4f017ee76ce538d9ac076fa180ac6d12ddd104` plus working-tree
changes frozen before this continuation's evidence-only documentation.
The frozen release worker SHA-256 is
`0a70367cd1dac7529e613ed318792f59a2a6c0f4b7f3b2b16f56754bbe1d1611`.
The expanded comparison/Object/BigInt/TypedArray/DataView/Atomics selection
passes 10,599 / 12,396 variants, gaining 60 passes with zero losses against
the matching projection of the preceding equality full run. The cumulative
comparison against the metadata full run gains 180 with zero losses. Baseline
projections are comparison artifacts rather than independently executed runs.
All selected language equality and relational comparison variants pass.
Outcomes, comparisons, logs, source patch, and archive hashes are retained in
[the evidence summary](../tools/test262/evidence/relational-operators-continuation/summary.json).
The full corpus run is in progress with the pinned revision, four workers,
five-second timeouts, no feature skips, and unchanged worker budgets.
Until completion, latest.json retains the equality full-run result of
67,677 / 102,956 passes.

Full conformance remains incomplete. The legacy immutable Rust `bin_op`
interface does not perform guest object coercion; guest binary expressions
route through the shared mutable runtime entry point. Other arithmetic and
assignment operators still require broader observable-coercion coverage, along
with the remaining environment, exotic, class/private, agent/shared-memory,
built-in, and bytecode/foundation requirements.
