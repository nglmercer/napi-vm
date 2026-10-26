# Class decorator follow-up — 2026-10-26

The 16,882-variant class selection now passes **16,874**, leaving eight
existing element-decorator failures. It gains **26 passes with zero losses**
against the auto-accessor worker, and **34 with zero losses** against the
exact latest full-report projection. There are no timeouts, skips, crashes,
or harness errors in this selection. The standard six-variant timing slice
also passes in full. All reports, comparisons, the projected full baseline,
and remaining class outcomes are retained here.

This stage implements class decorator grammar and application in both tiers.
Expressions evaluate before heritage and computed class keys; property/private
references retain their receivers. Applications run in reverse order, accept
callable replacements or undefined, and reject other results. Each context
has an independent addInitializer lifetime backed by internal traced bound
state. Replacement precedes static initialization and inner-name binding;
extra initializers run afterward against the final value. Static super keeps
the original home object. Before/after-export syntax is accepted separately;
decorators on both sides are rejected. Element decorators remain pending.

The new differential tests explicitly verify bytecode selection and direct
AST results, covering order, context shape, replacement, TDZ, private/super
references, inferred names, initializer receiver/callability, late calls,
abrupt completion, and negative syntax/export forms. All **399 core tests**,
five worker tests, all-target Clippy with warnings denied, root all-target
checking, formatting, and whitespace checks pass. Source and binary hashes
were revalidated after conformance measurement.

Design reference: [TC39 decorators PR 2417](https://github.com/tc39/ecma262/pull/2417),
head 59003ed603d09485b957a0f579b46697f49a96a6, plus the proposal README.
The source snapshot and exact identity are in source-v1.diff/source-v1.json.

These are focused measurements. The latest complete score remains
**102,839 / 102,956**; the full-pass goal is incomplete. The eight remaining
class variants require element decorator application, replacement validation,
context/access operations, and correctly ordered per-element initializers.
