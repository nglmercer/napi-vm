# Detached-checkout built-in verification

Release worker d7bd1b71… was built from clean detached commit a5a2362. Its checkout
HEAD and status were validated before and after building, and the executable was
copied to an immutable worker path. This prevents later main-checkout edits from
changing source attribution while a queued build is running.

The selection passes **13,800 / 13,808**, with two failures and six timeouts.
There are no skips, crashes, or harness errors. It gains 76 passes with zero
losses against the earlier e484dcea… worker. That baseline's executable hash is
verified, but its exact clean-source commit attribution remains unverified.

All 6,117 Array, 2,443 String, 654 Math, 208 SharedArrayBuffer, and 1,446
TypedArrayConstructors variants pass, as do the two previously regressed capture
name cases. TypedArray has no failures and six timeouts. Tagged templates pass
46/48; the two constructor-precedence failures are fixed by subsequent 9d58dce
and verified separately in debug execution.

Earlier original String/regexp reports remain intact, including their two
regressions and timeout transitions. They are not replaced by this narrower
selection. The completed full release score also remains separate.

See the [archive](../tools/test262/evidence/verified-builtins-2026-10-20/summary.json).
