# Final PR #7 scheduler benchmarks

> **Historical record.** These are historical PR #7 measurements on the stated environment; they are not the current performance baseline. See the [implementation map](roadmap.md) and [development guide](development.md) for current entry points.

Measured 2026-09-30 on Linux x86_64, 12 available CPUs, Rust 1.97.1 and Node 26.10.0, release builds. Original baseline: `0fa987d8860d620cd1008a84f2a17c9b67c495cd`. Final implementation: `b918b389f7f2c12cdca0844bbd45c0c86ca48d46`. Subsequent commits do not change the benchmarked runtime implementation.

## Reproduce

```sh
bench/run-scheduler.sh
```

The harness archives the original baseline into a separate source/build directory, copies identical common workloads to both builds, and builds before timing. Baseline and implementation timing runs are sequential. It records the implementation revision in `machine.txt`; use an unchanged source checkout throughout. Linux x64 addon filenames are currently hard-coded. Node admission retries accommodate the original baseline’s occasional post-completion busy race; the final measured run required zero retries.

[Raw JSONL and machine metadata](benchmarks/pr7-final/machine.txt) are stored in `docs/benchmarks/pr7-final/`. Queue workloads use one warmup and 25 batches. Node workloads use 20 warmups and 500 sequential calls per workload. This is one host run, not a confidence interval. Batch latency includes setup, execution, drain and teardown. Rust allocations count allocation/reallocation calls across the entire batch, not bytes or retained memory. Unmeasured Node allocations remain `null`.

## Timer queues, cancellation, promises and mixed events

| Workload | Baseline ops/s | Final ops/s | Final / baseline | Allocations/batch baseline → final |
| --- | ---: | ---: | ---: | ---: |
| timers/100 | 5,612,164 | 2,563,769 | 0.46× | 6.00 → 18.00 |
| cancel/100 | 3,235,207 | 3,609,832 | 1.12× | 13.00 → 24.00 |
| timers/1000 | 656,883 | 1,472,665 | 2.24× | 9.00 → 167.00 |
| cancel/1000 | 392,978 | 1,688,362 | 4.30× | 19.00 → 178.00 |
| timers/10000 | 56,244 | 1,117,977 | 19.88× | 13.00 → 1667.00 |
| cancel/10000 | 36,724 | 1,553,129 | 42.29× | 27.00 → 1680.00 |
| vm/timers | 247,011 | 320,680 | 1.30× | 19515.28 → 17670.12 |
| vm/cancel | 501,323 | 450,196 | 0.90× | 19514.80 → 18513.92 |
| vm/promises | 319,233 | 271,422 | 0.85× | 24520.20 → 23521.12 |
| vm/mixed | 674,344 | 893,666 | 1.33× | 34520.96 → 32685.84 |

| Workload | Baseline batch p50 / p95 / p99 (µs) | Final batch p50 / p95 / p99 (µs) |
| --- | ---: | ---: |
| timers/100 | 17.5 / 19.5 / 19.5 | 29.0 / 102.9 / 181.7 |
| cancel/100 | 30.4 / 33.6 / 43.0 | 27.1 / 31.7 / 40.1 |
| timers/1000 | 1,517.4 / 1,713.4 / 1,786.9 | 609.1 / 1,144.6 / 1,386.3 |
| cancel/1000 | 2,523.8 / 2,926.6 / 2,927.3 | 571.7 / 671.8 / 983.4 |
| timers/10000 | 173,220.6 / 205,031.4 / 214,928.8 | 9,132.0 / 9,624.9 / 9,967.3 |
| cancel/10000 | 271,415.5 / 284,427.5 / 314,478.6 | 5,996.2 / 7,799.1 / 8,420.6 |
| vm/timers | 3,986.0 / 4,594.4 / 5,038.7 | 3,062.2 / 3,789.1 / 3,979.6 |
| vm/cancel | 1,980.6 / 2,276.7 / 2,482.2 | 2,151.0 / 2,619.3 / 2,648.4 |
| vm/promises | 3,183.4 / 3,674.6 / 4,002.8 | 3,760.7 / 4,343.1 / 4,530.7 |
| vm/mixed | 5,404.8 / 7,167.8 / 7,488.4 | 4,247.8 / 5,396.4 / 5,499.7 |

Large queues benefit from ordered removal compared with the original vector scan. The small-queue and interpreter results above include regressions; they must not be summarized as a universal speedup. The tree still allocates more than a vector. Cancellation churn in `vm/cancel` repeatedly schedules and cancels one timer, unlike the isolated batch cancellation workload. Promise and mixed-event figures also include checkpoint, wake, root and teardown accounting.

Removing the reverse timer index has direct allocation evidence: the exploratory queue trial changed timer registration/drain allocations for 100 / 1,000 / 10,000 timers from 24 / 177 / 1,680 to 18 / 167 / 1,667. Cancellation batches changed 30 / 188 / 1,693 to 24 / 178 / 1,680. See [trial caveats](benchmarks/pr7-final/trial-notes.txt). Trial wall times were not commit-isolated and are not used as a controlled speedup claim. A small-vector/tree hybrid was not introduced without additional threshold and churn measurements.

## Repeated async execution and awaited host calls

| Workload / owner | Calls/s | p50 / p95 / p99 (ms) | CPU for 500 calls (ms) | Idle CPU / 250 ms (ms) |
| --- | ---: | ---: | ---: | ---: |
| trivial-async / Original legacy | 21,364 | 0.0383 / 0.0877 / 0.1531 | 27.139 | 1.630 |
| awaited-host-async / Original legacy | 11,633 | 0.0714 / 0.1848 / 0.3598 | 47.880 | unmeasured |
| trivial-async / Final legacy | 18,821 | 0.0460 / 0.0923 / 0.1584 | 31.176 | 1.627 |
| awaited-host-async / Final legacy | 11,455 | 0.0724 / 0.1776 / 0.3412 | 49.017 | unmeasured |
| trivial-async / Final persistent | 57,170 | 0.0118 / 0.0319 / 0.0612 | 9.585 | 1.148 |
| awaited-host-async / Final persistent | 25,710 | 0.0289 / 0.0614 / 0.1463 | 20.176 | unmeasured |

The original baseline has no `AsyncSession`; its legacy worker is the comparison for persistent ownership. The host-call workload explicitly exposes an async Node callback and awaits it on every execution. Idle CPU is process-wide and includes Node background activity. These figures do not establish Node allocation counts or retained-memory use.

## Real-time timers, wakes and bounds

- 2,500 timer samples: 17,828 jobs/s; lateness p50 / p95 / p99: 41.9 / 84.3 / 292.8 µs; peak queue 100.
- 10,000 ingress samples: 3,011,400 events/s; wake latency p50 / p95 / p99: 1.06 / 5.91 / 12.19 µs; 1713 coalesced wakes; capacity 32; idle thread CPU 3.0 µs / 250 ms.
- vm/timers measured peak queue: 1000.
- vm/cancel measured peak queue: 1.
- vm/promises measured peak queue: 1.

These new-clock/wake metrics have no equivalent original-baseline API, so no baseline timer-lateness or wake-latency ratio is claimed. Browser idle CPU and other operating systems were not measured.

## Verification

See [PR #7 verification](pr7-verification.md) for exact commands, reproductions, test counts, available native-addon/WASM checks, CI status and unresolved limitations.
