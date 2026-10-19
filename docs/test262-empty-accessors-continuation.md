# Empty accessor descriptors and duplicate parameters

Empty accessor properties now retain explicit private metadata when both getter
and setter are undefined. Ordinary objects and arrays preserve their accessor
descriptor kind across identical definitions and attribute changes. Converting
to data or deleting the property clears the marker. Paired callable getters and
setters keep their own identities; a companion property alone does not determine
the primary slot's kind. This replaces the incomplete inference in 5846880.

Six regressions require verified bytecode and also execute AST. They cover
objects, arrays, nonconfigurable definitions, accessor/data conversion, deletion
and recreation, inherited writes, strict assignment, mapped arguments, and paired
accessor identities.

The full run on 997b476 exposed one semantic regression: small AST function
frames retained the first duplicate parameter value while large frames retained
the last. The small binding constructor now deduplicates consistently. A mapped
arguments regression covers initial parameter values before any write, missing
arguments, and concatenation. This fixes the semantic loss without changing the
archived full-run outcome.

Strict workspace Clippy, formatting, 79 selected bytecode tests, and 72 selected
integration tests pass. The known unrelated super-destructuring parity failure
is excluded from the bytecode selection; this is not a full Cargo-suite claim.
The release descriptor, concat, binding, eval, generator, and Proxy selection
passes **14,615 / 14,639** variants, with **21 gains and zero losses** against
a projection of the completed 997b476 full report (14,594 passes). Six gains
overlap the prior with-binding work; fourteen cover empty descriptors and one
fixes the duplicate-parameter concat regression. All measured outcomes remain
in the denominator, including 24 failures and zero skips, timeouts, crashes,
or harness errors. See the [archived evidence](../tools/test262/evidence/empty-accessors-2026-10-18/).

The latest completed full measurement is 101,889 / 102,956 (98.9636%), with
982 failures and 85 timeouts. It gains 191 and loses 30 against the previous full
run: one duplicate-parameter regression and 29 timeouts. Outcomes are retained.
With bindings, empty accessors, and the duplicate-parameter initialization fix
are outside that full snapshot.

Latest full-run update (2026-10-19): the frozen a528b02 snapshot completed at
101,895 / 102,956 (98.9695%), with 951 failures and 110 timeouts. It gains
61 passes and loses 55 versus 997b476; all losses are measured timeouts.
Earlier full-run status statements above describe the historical stage.
See [archived full evidence](../tools/test262/evidence/full-empty-accessors-2026-10-19/).
