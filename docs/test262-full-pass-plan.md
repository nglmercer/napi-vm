# Test262 status and validation policy

## Selected target

The separately labelled **`reviewed-5112` profile passed 102,992/102,992
variants** on 2026-11-06, using two workers and the existing CI timeout of
ten seconds. There were no failures, skips, timeouts, crashes or harness errors.
[Complete report, provenance and audit](../tools/test262/evidence/reviewed-5112-full-2026-11-06/README.md).

This profile was selected explicitly by the user. It overlays one reviewed,
unmerged [upstream correction](https://github.com/tc39/test262/pull/5112)
on the original pin; it is not the unmodified upstream corpus.

| Identity | Value |
| --- | --- |
| Original pin | `0b4e0812022b18236b5276d2804948fdd9f71603` |
| Reviewed PR head | `1f3d994e2658c2934a64781ffbb7a9b3782d3310` |
| Derived profile revision | `cff513bc172b79b97963046e83078e114b2a20ff` |
| Changed test | `annexB/language/function-code/block-decl-func-skip-arguments.js` |

The original profile remains the default and its checkout remains unchanged.
The runner records profile provenance, validates the clean derived revision,
and rejects incompatible comparisons or resumes unless the relevant explicit
configuration-change option is supplied for comparison.

## Original-corpus boundary

The original Annex B test expects the Arguments object after a block-level
`function arguments(){}`; a staging test expects the function for the same
sloppy-mode construct. An engine cannot satisfy both unchanged expectations.
[Issue #5113](https://github.com/tc39/test262/issues/5113) and the reviewed PR
explain the obsolete algorithm quoted by the old test.

Five-second runs also encounter Date DST-caching timeouts. In the completed
reviewed-profile run, all 16 Date variants passed at ten seconds, with a
maximum of 6.046 seconds. Keep deadline and host conditions with each result;
do not turn a timeout into a pass when summarizing an older report.

[The progress log](test262-full-pass-progress.md) retains original-corpus
counts, short-deadline losses, timings and rejected optimization experiments.
Older revision evaluations remain available in Git history, including
`git show 6866a18:docs/test262-full-pass-plan.md`.

## Validate a change

Use [the runner guide](../tools/test262/README.md) for the exact commands.
Build the worker from current source, select a corpus profile explicitly,
and begin with smoke or focused tests. A retest checks only its selected
variants; a complete run is needed to establish a new full-corpus result.

Compare against an appropriate complete baseline. Investigate lost passes,
added/removed variants and configuration differences. Profile and timeout
changes are material; `--allow-config-change` acknowledges them rather than
making two configurations equivalent.

Archive the report, engine digest, corpus provenance, counts and comparison.
Full-pass claims require every selected variant to pass with zero skips and
errors. Performance work additionally needs a reproducible
[measurement baseline](performance.md); conformance pass counts alone do not
show a speed improvement.
