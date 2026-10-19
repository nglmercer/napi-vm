# Lexical scope continuation

## Implementation

Lexical `for` heads create every binding in the temporal dead zone before
initializers run, including destructuring and trailing declarators. Captured
`let` heads create a separate environment before the first test and before
each update. Captured `const` heads preserve their immutable binding without
per-iteration copying. Direct eval conservatively retains loop environments
because it can create closures absent from the parsed syntax.

Bare `var` heads preserve existing values. Destructuring assignments write
hoisted variable bindings in order, and named head initializers infer function
and class names. Object shorthand uses ordinary identifier resolution.
`typeof` tolerates absent identifiers but throws for uninitialized bindings,
including a shadowed `undefined`, in both execution tiers.

Catch parameter initialization and the lexical catch body use separate
environments, so closures created by parameter defaults cannot observe body
lexicals.

## Validation

Eleven focused integration tests cover both execution tiers, direct eval,
iterator closing on destructuring errors, immutable heads, named initializers,
and an explicit verified-bytecode check. Another 33 neighboring integration
tests and 79 selected bytecode tests pass. Strict workspace Clippy and
formatting pass. The existing unrelated super-destructuring parity failure
is excluded from the selected bytecode run; this is not a full Cargo-suite
claim.

The 5,908-variant release Test262 selection improves from 5,873 to 5,893 passes,
with 20 gains and zero losses. Release reports, comparisons, worker identity, and the implementation patch
are archived under [lexical scope evidence](../tools/test262/evidence/lexical-scopes-2026-10-18/). The selection
covers thirteen control-flow directories plus object expressions and typeof.

## Remaining work

The release selection retains fifteen failures: nested finally completion,
for-in deletion during traversal, mapped arguments aliasing, with/Proxy
binding resolution, and noncanonical numeric accessor names. These require
separate fixes. The latest completed full snapshot measures 101,889 / 102,956
on source 997b476; this feature is included in that measured full snapshot.

Latest full-run update (2026-10-19): the frozen a528b02 snapshot completed at
101,895 / 102,956 (98.9695%), with 951 failures and 110 timeouts. It gains
61 passes and loses 55 versus 997b476; all losses are measured timeouts.
Earlier full-run status statements above describe the historical stage.
See [archived full evidence](../tools/test262/evidence/full-empty-accessors-2026-10-19/).
