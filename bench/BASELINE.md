# Local performance baseline (2026-11-06)

The [machine-readable capture](baselines/2026-11-06/baseline.json) contains 41
representative workloads: 16 Criterion VM/frontend/plugin cases, 4 Node binding
cases, 2 language-service cases, 10 Rust scheduler cases, 4 Node async scheduler
cases, and 5 trusted-process Node plugin cases. It records the samples, medians,
median absolute deviations, environment, feature selections, binary hashes and
git state. See [performance methodology](../docs/performance.md) for commands,
interpretation, and coverage limits.

| Workload | Median | Sample meaning |
| --- | ---: | --- |
| Lexer, large source | 4.84 ms/iteration | Median of 10 Criterion batch averages |
| Fresh AST arithmetic | 9.25 ms/iteration | Median of 10 Criterion batch averages, including parse, VM setup and collection |
| Warm bytecode monomorphic property access | 0.551 ms/iteration | Median of 10 Criterion batch averages |
| Node binding arithmetic | 8.88 ms/call | Median of 20 calls, fresh VM and warmed source-template cache |
| Language-service completion | 0.534 ms/call | Median of 20 calls on a cached document |
| Rust scheduler, 1,000 VM timers | 6.18 ms/batch | Median of three independent run medians, each from 25 batches |
| Persistent Node async, trivial call | 0.0104 ms/call | Median of three independent run medians, each from 500 calls |
| Trusted Node plugin, 64-byte echo | 0.368 ms/call | Median of three independent run medians, each from 10 calls |

These numbers are measurements on an AMD BC-250 with background activity and
no CPU isolation; they are not a performance guarantee or a comparison against
an optimized implementation. Criterion measures batch averages; scheduler and
trusted-process samples above are run-level medians, not individual latencies.
The tracked JSON includes individual Node binding samples and the derived
Criterion samples; full Criterion source samples, run logs, and the explicitly
loaded addon are in the local ignored `artifacts/performance/baseline-2026-11-06-verified/raw/`
directory. The snapshot was taken from a dirty checkout at commit
`6866a18ba5b624e12a960556125b8f101db6b5ba`; the user's pre-existing
`packages/plugin-protocol/src/index.ts` edit was included in the emitted trusted
plugin code. The complete start/end status and input hashes are in the JSON.

The [previous baseline](history/BASELINE-legacy.md) is retained as historical
material and uses a different methodology.
