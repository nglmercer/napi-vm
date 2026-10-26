# Auto-accessor follow-up — 2026-10-26

Candidate v2 gains **eight passes with zero losses** against the exact
102,839 full-report projection. The targeted eight variants all pass. The
broader class selection passes **16,848 / 16,882**; its 34 nonpasses are the
existing decorator failures. It has zero timeouts, skips, crashes, or harness
errors. The standard six-variant timing slice also passes in full.

The implementation recognizes contextual unescaped `accessor` with its
no-line-terminator restriction while preserving members named accessor.
Both evaluator and compiler generate strict nonconstructible getters/setters
with separate private backing slots. Computed keys are evaluated/coerced once;
source-order initialization, redeclarations, inheritance, private/static
brands, receiver validation, descriptor flags, and anonymous initializer names
use the existing class/private operations. Internal initializer-name aliases
are read only when initializing private backing storage.

Candidate v1 passed the initial semantic tests but a public field named like
an internal slot exposed a naming collision. Its source, worker identity,
and failing probe are retained. Candidate v2 limits translation to private
storage initialization and passes the same probe; both instance and static
collision cases now have AST/bytecode regression coverage. The earlier public
object-method probe did not expose the issue and is retained as measured.

Validation: **397 core library tests**, five worker tests, all-target Clippy
with warnings denied, root all-target check, formatting, and diff whitespace.
The new differential tests verify top-level bytecode selection plus direct
AST execution, including early errors and initialization order. Exact source
and binary hashes were revalidated after all measurements.

Reports, baseline projections, comparisons, and the remaining class inventory
are retained here. The latest completed full-corpus score remains
**102,839 / 102,956**; these are focused gains, not an aggregate score claim.
Decorator grammar and application semantics remain the next implementation.
