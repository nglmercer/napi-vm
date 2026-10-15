# Local implementation evidence

Read [results.md](results.md) for measured improvements, regressions, and the
remaining release blockers. The patch is not release approved.

Reviewed revision: `cf9d240a99747e29d94b970c30483d14ad831e9f`.
Machine details are in `baseline-environment.txt`; the activated Rust/Bun/Wasm
versions are recorded separately in `toolchain-versions.txt`. The initial shell
version probes predated toolchain activation. All Rust/Bun gate commands used
the activated toolchains.

- `baseline-results.json` and `baseline-00` through `baseline-11`: commands run
  against a detached checkout of the reviewed revision. Pre-existing failures
  include Bun worker-termination return semantics, unavailable native fixture
  dependencies in offline builds, the initially missing wasm-pack tool, and an
  unbounded fresh-AST Criterion workload killed by the OS.
- `baseline-compiler-memory.log`: the new isolated memory regression copied
  unchanged to that checkout; 20,000 synthesized compilations grow RSS from
  4,040 to 75,396 KiB after warming. The implementation passes its 4 MiB limit.
- `final-results.json`: all twelve full quality/release commands, with raw logs.
- `recheck-results.json` and `last-results.json`: checks repeated after the final
  lifecycle, panic-unwind and memory regression additions.
- `closeout-results.json`: the latest eleven passing local commands after all
  lifecycle and operand-borrowing changes. `closeout-release-all-features.log`
  contains the additional final full release Rust run.
- `pre-timer-reuse/` and `before-global-name-borrowing/`: earlier measurements
  retained for investigation; use root samples for the final comparison.
- `baseline/modified-*-N.jsonl`: seven interleaved process samples of allocation,
  queue and public API workloads. No competing builds run during these samples.
- `threshold-T-N.jsonl`: identical queue-only workloads compiled with thresholds
  16, 32, 64 and 128. The timer queue implementation differs only in `SMALL_LIMIT`; variants are
  all built before timing. Depths cover 1, 10, 32, 64, 100, 1,000 and 10,000.

The cold AST Criterion helper now drops the result and interpreter, then collects
through a separate live collector. The identical updated benchmark file is copied
to the baseline. This measures a viable fresh-AST embedding lifecycle; it should
not be compared to the earlier baseline's growing-memory timings. The AST mode
remains explicit rather than silently switching the benchmark to bytecode.

To reproduce the interleaved samples, copy `examples/call_metrics.rs`,
`examples/timer_queue_matrix.rs` and `examples/support/allocations.rs` into the
baseline checkout. Build native bindings and those examples in both checkouts
before running:

```bash
cargo build --release --no-default-features --example call_metrics --example timer_queue_matrix
python3 bench/compare-runtime.py --baseline /path/to/baseline --output /path/to/results
```

The public matrix covers fresh runCode, reused VM.run, compatible VM.runAsync,
AsyncSession.run/evaluate, callFunction, guest-to-host calls and module import.
Existing Criterion groups retain forced AST, prepared bytecode, plugin calls,
monomorphic/megamorphic properties and shape churn. Existing scheduler artifacts
cover virtual/real clocks, async host calls and idle CPU. Fields that cannot be
measured at the JS layer (Rust allocations) are explicitly null there; the Rust
allocation harness supplies their counts separately.

At the time of measurement, remote matrix results and published PRs were unavailable:
`gh auth status` rejected the configured CLI token. A later publication step
can use the authenticated GitHub connector; this does not supply remote results
for the recorded measurements. These logs establish local Linux
x64 validation, not remote platform/release readiness.

`recheck-03-before-final-library.log` preserves a failed check from overlapping
source edits and compilation; the completed panic-cleanup implementation passes
the subsequent library and release checks. Final comparisons run without builds.

## Test262 full pass (2026-10-15)

A complete pinned-corpus run at source `aec3df4458ae7866ecb1f0182b61d39dd95cd9ab`
(clean tree) reproduces the archived 95% snapshot exactly: **98,010 / 102,956
variants (95.1960%)**, 4,944 failures, 2 timeouts, zero skips, crashes, or
harness errors. Corpus `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`; worker
SHA-256 `dda45144ab2a946433f3819ad4041fd103451de1d17e051f910907761b29ec9d`
(freshly rebuilt, identical to the snapshot digest). Four workers, five-second
timeout, resumable journal with zero resumed files.

- Full rows: `artifacts/test262/full-results-2026-10-15.json`
- Journal: `artifacts/test262/full-2026-10-15.checkpoint.jsonl`
- Dashboard: `artifacts/test262/full-2026-10-15.html`
- Triage: `artifacts/test262/full-2026-10-15-triage.json` (top clusters:
  `built-ins/RegExp` 1,102, `intl402/NumberFormat` 500, `staging/sm` 271)
- Comparison vs `tools/test262/evidence/unicode-annex-builtins/full.json.gz`:
  `artifacts/test262/full-2026-10-15-comparison.json` (0 new passes, 0 lost)
- Timeout rerun: `artifacts/test262/timeout-rerun-2026-10-15.json` — both
  timeouts are `staging/sm/regress/regress-1507322-deep-weakmap.js`
  (script + strict) and time out again in isolation, so they are genuine
  over-budget cases, not flakes.

Reproduce with the command in [tools/test262/README.md](../../tools/test262/README.md),
adding `--checkpoint artifacts/test262/full-2026-10-15.checkpoint.jsonl`.
Runner unit tests pass (`python3 -m unittest discover -s tools/test262`,
12 tests OK). This establishes a local Linux x64 re-measurement, not a new
conformance claim: remaining failures are unchanged from the archived triage.
