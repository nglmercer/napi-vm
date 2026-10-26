# Auto-accessor decorator follow-up — 2026-10-26

The pinned broad selection passes **32,596 / 32,596** variants, with zero
gains or losses against the preceding getter/setter stage and **42 gains,
zero losses** against the latest full-report projection. There are no skips,
timeouts, crashes, or harness errors. The standard six-variant timing slice
also passes. This is a focused result; the latest complete score remains
**102,839 / 102,956** until a full run confirms it.

Auto-accessor decorators now receive the current getter/setter pair and an
accessor context; they may return callable replacements and initialization
transforms. Decorators apply in reverse order, transformations in source order,
and per-accessor extra initializers after storage. Static/instance groups,
private brands, observable return-property access order, errors, computed keys,
and duplicate names are covered. Bytecode verification checks the backing
field and generated accessor pairing. AST and bytecode tests cover these
behaviors. Additional worker diagnostics cover static private elements,
composition, duplicate names, and initializer lifetime after a throw.

Validation passed **409 core tests**, five worker tests, Clippy on all core
targets with warnings denied, root all-target checking, formatting, whitespace,
and a locked optimized build. Exact source diff, identity, worker probes,
reports, comparisons, timing, and check logs are retained; source and worker
hashes were checked after the broad run. The broad test ran without concurrent
builds or core tests. More specification and neighboring coverage may reveal
additional decorator edge cases; the focused result does not establish a full
corpus pass.
