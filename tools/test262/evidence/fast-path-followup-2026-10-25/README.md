# Test262 working-tree follow-up — 2026-10-25

The focused runs use corpus revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, a five-second timeout, and the
two timing-sensitive files recorded in each report. The subsequent complete run passes 102,783 / 102,956 with 23 gains and zero
losses against clean source `259402f`; see
[the complete evidence](../full-worktree-2026-10-26/).

The retained source snapshots are based on `42ae715`. `source-worker-a0eef.diff`
records the source for worker SHA-256 `a0eef7b3…`; `source.diff` records the
latest worker, SHA-256 `0714d545067fe2c3a4520d76016941ceceee154506882654ac12ef0148c12e0d`.
The earlier timing runs use worker SHA-256 `07493799…`. Each report records its
exact worker hash and selection.

| Report | Workers | Pass | Timeout | Notes |
| --- | ---: | ---: | ---: | --- |
| `test262-fast-path-timing.json` | 1 | 2/4 | 2 | Before the ICU casing-property guard |
| `test262-fast-path-timing-v2.json` | 1 | 3/4 | 1 | Strict casing variant recovered |
| `test262-fast-path-timing-v3.json` | 1 | 4/4 | 0 | Current retained source; direct UTF-16 scalar decode |
| `test262-fast-path-timing-stability.json` | 2 | 3/4 | 1 | Script casing variant timed out |
| `test262-fast-path-timing-v4.json` | 2 | 2/4 | 2 | Rejected root simple-case shortcut |
| `test262-fast-path-timing-v4-jobs1.json` | 1 | 3/4 | 1 | Same rejected shortcut; change reverted |
| `test262-fast-path-retained-jobs2.json` | 2 | 4/4 | 0 | Current retained worker, SHA-256 `07493799…` |
| `test262-fast-path-retained-jobs2-repeat.json` | 2 | 3/4 | 1 | Same worker; script casing variant timed out |
| `test262-primitive-string-delete.json` | 1 | 4/4 | 0 | String `length` and indexed property deletion |
| `test262-primitive-string-delete-jobs2.json` | 2 | 4/4 | 0 | Same four variants under two workers |
| `test262-fast-path-final-jobs1.json` | 1 | 2/4 | 2 | Worker `a0eef7b3…`; both casing variants timed out |
| `test262-fast-path-final-jobs2.json` | 2 | 2/4 | 2 | Same worker; both casing variants timed out |
| `test262-fast-path-final-diagnostic15.json` | 1 | 4/4 | 0 | Same worker, 15-second diagnostic timeout |
| `test262-fast-path-v5-jobs1.json` | 1 | 3/4 | 1 | Worker `ecafa3f7…`; script casing variant timed out |
| `test262-fast-path-v5-jobs2.json` | 2 | 2/4 | 2 | Same worker; both casing variants timed out |
| `test262-primitive-string-delete-v5.json` | 2 | 4/4 | 0 | Current worker; same four delete variants |
| `direct-eval-capture-v1.json` | 2 | 8/10 | 2 | Intermediate closure fix; class-field arrow cases still failed |
| `direct-eval-capture-v2.json` | 2 | 10/10 | 0 | Five direct-eval closure tests, all script/strict variants |
| `direct-eval-suites-v1.json` | 2 | 739/741 | 2 | Eval-code and block-scope suites; both failures are in the full-run residual list |
| `direct-eval-capture-v3.json` | 2 | 12/12 | 0 | All six residual SpiderMonkey class/destructuring tests |
| `current-worker-timing-delete.json` | 2 | 7/8 | 1 | Current hash `0329d9d9…`; script casing variant timed out, other seven pass |
| `set-composition-order-v1.json` | 2 | 4/6 | 2 | Iterator setup reordered; difference snapshot behavior still failed |
| `set-composition-order-v2.json` | 2 | 6/6 | 0 | Intermediate source snapshot fix; broader run exposed predicate regressions |
| `set-composition-suites-v1.json` | 2 | 764/768 | 4 | Exposed live-iteration regressions in `isSubsetOf` and `isDisjointFrom` |
| `set-composition-suites-v2.json` | 2 | 768/768 | 0 | Final source; full built-ins/Set and Set subclass selection |
| `set-composition-order-v3.json` | 2 | 6/6 | 0 | Final source; all three SpiderMonkey composition files |

The direct-eval capture change keeps lexical bindings in bytecode frame/block
environments when dynamic lookup can reach names absent from parsed closures.
It clears the 12 variants in six `staging/sm` tests. The wider 741-variant
eval-code and block-scope selection has 739 passes; its only two failures,
`var-env-func-init-local-new-delete.js` and
`var-env-var-init-local-new-delete.js`, are already listed in the pinned full
run's residual inventory. The strict casing variant passes its full BMP
mapping under five seconds; the 15-second run confirms the script casing
semantics but does not count as a pass under the required timeout.

Worker `0329d9d9…` passes the strict casing variant, both `regress-610026.js`
variants, and all four primitive string deletion variants at five seconds with
two workers. The script casing variant timed out; earlier runs on other workers
also timed out the strict variant. Require repeated passes of both casing
variants under the full-run configuration before closing the timing task.

The Set composition change gets the other set-like object's iterator and its
`next` method before copying the receiver. `difference` iterates the copied
receiver entries when its `has` callback mutates the source; `isSubsetOf` and
`isDisjointFrom` retain live iteration. The final worker passes all 6/6 variants
in the three affected SpiderMonkey tests and all 768/768 selected built-in Set
and subclass variants. The first broad rerun caught four predicate regressions;
the final version removed them. The full-corpus run on this source snapshot is
separate and recorded outside this focused report.
