# Nested finally completion continuation

## Implementation

Bytecode frames retain a stack of suspended abrupt completions. Exception
handlers record its depth on entry. Landing a nested catch consumes that catch's
exception without replacing an enclosing cleanup's completion; landing a finally
preserves its own completion until cleanup finishes. A return or exception that
escapes cleanup discards the suspended completions it crosses.

Break and continue exits from exceptional cleanup pads explicitly discard the
completion they replace. Local loops inside cleanup leave the enclosing pending
completion intact. Inline finally copies and for-of break cleanup pop their
handlers before executing cleanup, so errors from cleanup cannot enter the same
handler and execute it twice.

## Validation

Eleven integration regressions require verified bytecode and also execute AST.
They cover nested catches and finally pads, original throws and returns,
replacement returns and throws, local and escaping break/continue, labeled
outer-loop exits, nested iterator cleanup, and iterator return methods that
throw. Five of the initial six regressions failed before the implementation.
The neighboring declaration-completion, iterator-protocol, and tail-call suites
pass thirty tests. All 79 selected bytecode tests, strict workspace Clippy, and formatting pass.
The existing unrelated super-destructuring parity failure is excluded from
the bytecode selection; this is not a full Cargo-suite claim.

The release Test262 selection passes **5,897 / 5,908**, with
**4 gains and zero losses** against the lexical-scope checkpoint.
All 388 selected try-statement variants pass. The selection has eleven
remaining failures and zero skips, timeouts, crashes, or harness errors.
It uses the same selection, pinned corpus, two workers, five-second timeout,
and no feature skips. Reports, comparisons, worker identity, and the source
patch are archived under [finally completion evidence](../tools/test262/evidence/finally-completions-2026-10-18/). The last completed
full score measures 101,728 / 102,956 on 607d086; focused gains are not added to
that score.
