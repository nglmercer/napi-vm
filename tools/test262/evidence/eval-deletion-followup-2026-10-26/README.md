# Eval local binding deletion — 2026-10-26

986 / 996 variants pass: six gains and zero losses against the matching
complete-report projection. Both language/eval-code deletion residuals, the
exhaustive direct normal-code eval test, and both nested-delete staging files
and the Annex B eval case now pass. Ten existing runtime failures remain; no timeouts or worker errors.

New eval var/function bindings retain a deletable attribute. Existing local
bindings remain nondeletable, and bare var statements never recreate a deleted
binding after hoisting. Validation passed 391 core tests, five worker tests,
formatting, all-target Clippy, and the root all-target check. Source identity,
all outcomes, baseline projection, and comparison are retained.
This focused score does not replace the latest full score, 102,783 / 102,956.
