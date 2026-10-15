# Scoped regular-expression modifiers continuation

Implementation resumed toward the user's minimum 95% full pinned Test262 goal.
The latest completed full baseline before this continuation is
`artifacts/test262/conformance-followup-release-full.json`: 92,800 of 102,956
variants pass (90.1356%), with 10,154 failures and two timeouts. Subsequent
Promise, Array.fromAsync, async disposal, and resource statement changes are
present in the working tree and need a new full measurement.

This continuation implements scoped `i`, `m`, and `s` modifiers in the regexp
parser and matcher. Matcher state is restored before invoking the enclosing
continuation and reinstated when backtracking into the modified group. Nested
groups and backreferences therefore see their current lexical modifier flags.
Unicode word classes and boundaries recognize long s and Kelvin sign when
Unicode case folding is active. Unicode property escapes precompute folded
positive and complemented sets; `u` and `v` apply complement/folding in their
respective orders.

The pinned modifier selection passes all 140 variants with both debug and release workers,
without skips, timeouts, crashes, or harness errors. Evidence is in
`artifacts/test262/regexp-modifiers-debug-final.json`, worker SHA-256
`f2b0510d2448a42fbdc9cab6f3a5a3c8bab7715e1649cc4e1ac0bc8e7e1858b5`.
All 140 variants failed in the completed full baseline. This focused result is
not an overall compatibility measurement.

Verification completed:

- Scoped modifier unit test: nested addition/removal, backtracking, flag
  restoration, Unicode word boundaries, and property complement order pass.
- All six `regexp_legacy` integration tests pass.
- All nine `promise_semantics` and three `resource_semantics` tests pass.

No corpus, harness, denominator, runner limits, or feature skips were changed.
The 95% full-suite goal remains open. Full release measurement and comparison
must precede any new overall pass-rate claim.

Release verification uses `artifacts/test262/regexp-modifiers-release-final.json`,
SHA-256 `c987b1a2f786addf784f377862bb7f92f8599e5c8e813d521e422e4bdcd275b8`.
The fresh full release run writes `artifacts/test262/async-resource-regexp-full.json`
and its resumable `.checkpoint.jsonl` journal, with four workers and a five-second
per-variant timeout. It was started after successful release verification.
