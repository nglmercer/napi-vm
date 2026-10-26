# Annex B labeled declarations and arguments — 2026-10-26

The pinned declaration and control-flow selection passes **2,947 / 2,949**
variants, gaining **two passes and zero losses** against the latest full-report
projection. Labeled block function declarations now hoist with their enclosing
block, including nested labels and duplicate declaration ordering. The standard
six-variant timing slice passes. The latest complete score remains
**102,839 / 102,956** pending another complete run.

Two staging variants still fail: `block-scoped-functions-annex-b-arguments.js`
and `regress-602621.js`. They expect a block function named `arguments` to
replace a function's implicit arguments object. The pinned normative
`annexB/language/function-code/block-decl-func-skip-arguments.js` instead
requires that object to remain and cites the FunctionDeclarationInstantiation
rule which adds `arguments` to the parameter names when an arguments object is
needed. The staging test and normative test therefore demand opposing results
for the same declaration behavior. Candidate v1 implemented the staging
expectation and gained four staging variants but lost the normative variant;
its source, full comparison, and check log are retained. Candidate v2 retains
the normative rule and does not disguise the two staging failures.

Validation passed **418 core tests**, five worker tests, all-target core Clippy
with warnings denied, root all-target checking, formatting, whitespace, and a
locked release build. Differential tests cover label hoisting, duplicate names,
implicit arguments, explicit parameters, and strict syntax. Source and worker
hashes were checked after the run; reports, projection, comparisons, timing,
source diffs, and check logs are archived.
