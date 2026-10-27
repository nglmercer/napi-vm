# Global increment performance follow-up — 2026-10-26

The affected selection passes **12,397 / 12,399** variants, gaining **four
passes and zero losses** against the latest complete report: two overlapping
TypedArray buffer variants and the two earlier TypedArray assignment repairs.
Both large-sort variants still time out. The two newly passing variants, two
Proxy ownKeys variants, six TypedArray copyWithin variants, and the standard
six-case timing slice pass together in a repeat run using two workers and
five-second deadlines. The latest complete score remains **102,906 / 102,956**
until another full run confirms the focused gains.

Plain writable numeric own properties on the global object use a guarded
increment/decrement path when no intervening lexical or with environment can
intercept the binding. Accessors, non-writable properties, object coercion,
and other observable cases use the existing reference algorithm. Differential
tests cover those boundaries and prefix/postfix values in both execution paths.

Validation passed **424 core tests**, five worker tests, all-target core Clippy
with warnings denied, root all-target checking, formatting, whitespace, and a
locked release build. Source and worker hashes were checked after the broad
run. Exact source diff, checks, reports, projection, comparison, and timing
are retained.
