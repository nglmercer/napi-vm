# Async generator delegation continuation

Async `yield*` now preserves the value field supplied by a real async iterator
without awaiting that field again. The iterator result is still awaited. The
async-from-sync adapter continues to await its own values before constructing
its iterator result, so sync iterable delegation keeps its existing behavior.

A return completion received at the delegation suspension point now awaits its
value before looking up the delegate's return method. A rejected value therefore
throws at the yield-star expression before any return-method access. Missing
return methods and final return completions keep their required subsequent
return-value processing.

Three new regressions cover promise identity, return-method lookup order, and
rejection before lookup. All 23 async-generator foundation tests, 36 generator
stress tests, and 11 iterator-protocol tests pass. Strict workspace Clippy and
formatting pass. The native stackful worker is the measured target.

The release Test262 selection passes **2,858 / 2,858** generator and
async-generator statement/expression variants, with **four gains and zero
losses**. Both baseline and final reports have no skips, timeouts, crashes, or
harness errors. The baseline (2,854 passes) is projected from the empty-accessor
release report on a528b02. Reports, comparisons, worker identity, and the
source patch are archived under [async delegation evidence](../tools/test262/evidence/async-delegation-2026-10-18/).

The completed full measurement remains 101,889 / 102,956 on source 997b476.
This async delegation change is outside that full snapshot. A separate full
run on a528b02 is still active; its outcomes will be archived when terminal.
