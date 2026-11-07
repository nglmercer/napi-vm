# Historical implementation evidence

This directory preserves raw build/test logs, machine contexts, comparison
summaries, and scheduler measurements from earlier implementation work. The
old narrative checkpoints and plans have been replaced by current guides;
their original prose remains available in Git history.

| Record | How to use it |
| --- | --- |
| [baseline-results.json](baseline-results.json) | Identify baseline commands and outcomes |
| [comparison-summary.json](comparison-summary.json) | Read the matching historical before/after analysis |
| [scheduler-final-results.json](scheduler-final-results.json) | Inspect the recorded scheduler validation |
| [pr8-final-validation](pr8-final-validation/README.md) | Native/ownership validation artifacts and context |
| [pr8-final-performance](pr8-final-performance/README.md) | Paired lifecycle/runtime measurements |
| [release-status.json](release-status.json) | Recorded release-gate state at that time |

Keep the raw records immutable when updating user-facing docs. A command in an
old log can reference an older API or file layout. Reproduce present behavior
through [development](../development.md), assess compatibility through
[validation](../validation.md), and use the current
[performance workflow](../performance.md) for a new measurement.
