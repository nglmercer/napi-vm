# RegExp construction order and source escaping — 2026-10-26

2,124 / 2,136 variants pass: four gains and two losses against the matching
complete-report projection. Both variants of constructor-ordering-2.js and
escape.js now pass. The selection includes top-level RegExp tests, its whole
prototype suite, SpiderMonkey RegExp tests, and RegExp class subclassing.
The twelve nonpasses include two new regressions in constructor-ordering.js,
plus existing nested-group, large-string, and timeout cases. This candidate
fails the zero-loss gate; its report is retained while the ordering regression
is repaired.

RegExp allocation gets the newTarget prototype after pattern/flags extraction
and before string conversion. Source escaping handles escaped line terminators
and character-class slashes. Validation passed 388 core tests, five worker
tests, formatting, and all-target Clippy with warnings denied. The measured
source snapshot and all outcomes/comparison are retained. This focused score
does not replace the complete-corpus score of 102,783 / 102,956.
