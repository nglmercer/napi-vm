# Field decorator follow-up — 2026-10-26

Candidate v3 clears all eight previously remaining pinned decorator variants.
The broad selection passes **32,596 / 32,596**, with **42 gains and zero losses**
against the exact latest full-report projection. The matching 26,479-variant
method-stage projection gains **eight passes with zero losses**. The targeted
eight variants and the standard six-variant timing slice pass. No exclusions,
skips, crashes, or harness errors occur. Source and worker hashes were
revalidated after measurement; exact reports/projections/comparisons remain.

Regular field decorators now evaluate before computed names, apply in reverse
order (static fields before instance fields), and accept initializer functions
or undefined. Initializer transformations run in source order after raw value
evaluation and named evaluation, with the instance/class receiver. Storage
precedes per-field extra initializers, which precede the following field.
Public/private access.has/get/set retain canonical keys, private identities,
strict writes, and private brands. Duplicate names retain separate positional
states. Internal instance state uses the nearest class environment and existing
tracing. Shared method/field context construction verifies visible property
creation order. Getter/setter and auto-accessor decorators remain unsupported;
this stage does not claim complete decorator semantics.

Candidate v1 failed the Array-subclass getter-named field probe; source and
failure log are retained. Candidate v2 extends explicit data-property tags to
array descriptors, reads, writes, conversion, deletion, and truncation, passing
all 405 core tests and other checks. Its single-worker 30,000-element tagged
array pop probe timed out at five seconds because shrinking by one element
scanned every marker. Candidate v3 removes individual markers for short
truncations and scans the smaller marker set for larger removals. The same
probe passes in approximately 1.62 seconds. This is a diagnostic, not a new
corpus pass; both outcomes and source identities are preserved.

Validation: **405 core tests**, five worker tests, all-target Clippy with
warnings denied, root all-target checking, formatting, and diff whitespace.
The raw v2/v3 gate logs are retained. Differential cases explicitly verify
bytecode selection and direct AST execution for transforms, extras, exceptions,
field presence, inheritance, replacement, symbols, duplicate keys, inferred
names, context ordering, array descriptors, pop, deletion, and truncation.

Latest completed full score remains **102,839 / 102,956**. The 42 syntax gains
need confirmation in another complete run. Next: getter/setter and auto-accessor
decoration, including arbitrary callable accessor identities, followed by the
remaining runtime and timeout causes and the clean committed full-pass gate.
