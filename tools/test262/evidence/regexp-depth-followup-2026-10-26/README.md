# RegExp order, escaping, and bounded nesting — 2026-10-26

2,130 / 2,136 variants pass: eight gains and zero losses against the matching
complete-report projection. All four previously failing 200-group variants
and both variants of constructor-ordering-2.js and escape.js pass. The two
constructor-ordering.js regressions from the earlier candidate are restored.
Four existing large-string failures and two existing timeouts remain.

RegExp.prototype.compile uses intrinsic allocation rather than inheriting an
outer construction newTarget. Grammar validation and parsing grow the native
stack, accepting up to 256 nested groups/sets; matching retains a 512-frame
budget with existing stack growth. Both 200-group variants and rejection past
the grammar limit are covered on 512 KiB embedding stacks.

Validation passed 389 core tests, five worker tests, formatting, and all-target
Clippy with warnings denied. The measured source and full outcomes/comparison
are retained. The rejected predecessor remains in
[earlier evidence](../regexp-order-source-followup-2026-10-26/).
The latest complete-corpus score remains 102,783 / 102,956.
