# Abstract equality continuation

Guest `==` and `!=` now use the shared mutable runtime comparison from both
AST and bytecode execution. Object-to-primitive conversion uses the default
hint, invokes observable conversion methods, and propagates abrupt completion.
Object identity comparisons and comparisons against null/undefined do not
coerce objects. Boolean operands convert to numbers; Number/String comparisons
use the runtime StringNumericLiteral conversion.

BigInt/String equality parses StringIntegerLiteral without rounding through
Number. String-to-BigInt conversion also uses this grammar, rejecting numeric
separators, signed radix forms, malformed UTF-16, and non-ECMAScript whitespace.
Integer literal parsing remains separate. Resource failures propagate instead
of being replaced by a false comparison or a syntax error.

Mixed BigInt/Number comparison no longer rounds the BigInt to binary64. It
compares against the exact truncated finite Number integer and handles the
fractional remainder, NaN, and infinities separately. The shared helper also
serves existing relational operations.

Seven integration tests compare raw AST execution with forced verified
bytecode, covering conversion order, abrupt completion, skipped coercion,
primitive wrappers, Symbols, exact large-integer comparisons, fractional and
infinite Numbers, and shared BigInt string grammar. The existing thirteen
Object enumeration, eleven copy, and six array-elision fixtures also pass.
The minimal core suite passes 354 tests with the known sidecar native-addon
fixture excluded. Workspace Clippy with all targets/features and `-D warnings`,
formatting, and whitespace checks pass. Existing workspace native-addon
failures remain unresolved.

The source patch SHA-256 is
`3e7a452a34c916f317df7deed68b0fa601f7576056d21f44c87f8c240dcaac87`,
based on `9e4f017ee76ce538d9ac076fa180ac6d12ddd104` plus working-tree
changes frozen before the subsequent evidence-only documentation updates.
The frozen release worker SHA-256 is
`a58553c59b8e2784c62b087a93628ec6866b66ee6d79dc428fbdd897515e4ca0`.
The focused equality/relational/BigInt/Object selection passes 7,191 / 7,606
variants, gaining 120 passes with zero losses against the matching projection
of the preceding metadata full run. The projection is a comparison artifact,
not an independent baseline execution. Outcomes, comparisons, source patch,
core log, and archive hashes are retained in
[the evidence summary](../tools/test262/evidence/abstract-equality-continuation/summary.json).
The completed full execution uses the pinned revision, four workers,
five-second timeouts, no feature skips, and unchanged worker budgets. It passes
67,677 / 102,956 variants (65.7339%), gaining 187 passes with zero losses against
the preceding metadata full run. There are 35,277 failures, two existing
deep-WeakMap staging timeouts, and no skips, crashes, or harness errors.
Full outcomes, transitions, triage, and logs are retained in the evidence
archive. Subsequent relational operator changes are outside this snapshot.

This implements guest abstract equality; the legacy immutable Rust `bin_op`
and `leq` interfaces do not perform guest object coercion. The full ECMAScript
goal remains incomplete, including other numeric operators/coercions,
environments, descriptors, exotics, classes/private elements, agents/shared
memory, unsupported built-ins, and complete bytecode semantic parity.
