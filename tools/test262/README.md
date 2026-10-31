# Test262 validation

Every Test262 result comes from the isolated worker `napi-vm-test262`, one fresh
process per variant with a hard deadline. The Rust tool `test262`
(`tools/test262-runner`) drives it. No Python or other interpreter is needed
for running or comparing tests.

Pinned corpus: `corpus.lock.json`. Test selections and named groups:
`groups.json`.

## Setup

```sh
cargo build --release --no-default-features --bin napi-vm-test262
cargo build --release -p napi-vm-test262-runner
target/release/test262 fetch              # clones the pinned revision into .napi-vm/test262
```

`fetch` is idempotent: it leaves a checkout at the locked revision untouched.
Runs refuse a corpus at another revision or with modified files, so every
report is reproducible.

## Validation loop

Use the cheapest step that answers the question.

| Step | Command | Time on the 2-core reference host |
| --- | --- | --- |
| Smoke: fixed cross-section, 1,014 variants | `target/release/test262 run --group smoke --output smoke.json` | about 16 s |
| Retest: only the non-passing tests of a report | `target/release/test262 retest smoke.json --output retest.json` | under 1 s for one test |
| Subset check: lost passes among retested variants | `target/release/test262 compare smoke.json retest.json --subset` | instant |
| Gate: the complete corpus | `target/release/test262 run --output full.json` | about 25 min |

`compare` exits with 1 when a variant that passed in the baseline no longer
passes. Before committing an engine change, run the smoke group and retest its
failures. The complete run remains the only gate for regressions in tests that
passed before: the smoke group and retest cannot show them.

## Commands

```text
test262 fetch [--corpus DIR]
test262 run [--path P]... [--group G]... [--skip-feature F]... [--jobs N]
            [--timeout S] [--output FILE] [--checkpoint FILE [--resume]]
test262 retest BASE [--output FILE] [--jobs N] [--timeout S]
test262 compare BASELINE CURRENT [--subset] [--output FILE] [--allow-config-change]
test262 summary REPORT [--limit N] [--show N] [--slowest N] [--json FILE]
test262 time PATH... [--timeout S] [--variant script|strict|module]
test262 request TEST [--variant script|strict|module] [--output FILE]
test262 bench FILE... [--harness] [--repeat N] [--timeout S]
```

- `run` defaults: `--jobs 2`, `--timeout 5` seconds per variant (the scheduled
  workflow uses 10), the whole corpus when no `--path`/`--group` is given. It
  prints progress with an estimate every 1,000 files.
- `--checkpoint FILE` journals each finished file in order. After an
  interruption, rerun the same command with `--resume`; a changed configuration
  or worker binary is rejected.
- `retest` reuses the base report's worker count, timeout and skip list, so its
  results are comparable with the base.
- `compare --subset` compares only the variants present in the second report.
  `retest` output is checked this way against the report it came from.
- `summary` prints counts, non-passing tests grouped by area (most first),
  and every non-passing variant with its engine message. `--json` writes the
  grouped counts for CI artifacts.
- `time` runs without a cap unless `--timeout` is given, and prints one line per
  variant.
- `request` writes the exact worker input for one variant; pipe it to the
  worker to profile a single test.
- `bench` times snippet files through the worker, optionally with the harness.

Report format: `domain`, `suite`, `revision`, `engine_sha256`, `worker_jobs`,
`timeout_seconds`, `counts`, `pass_percentage`, and one `results` row per
test variant with `test`, `variant`, `status`, `seconds`, and the engine
report. Statuses: `pass`, `fail`, `skip`, `timeout`, `crash`, `harness_error`.

## Reference numbers

- Full run `full-interpreter-overhead-2026-10-30`: 102,975 / 102,992 pass;
  1 failure; 16 timeouts; no crashes or harness errors. Evidence:
  [`evidence/full-interpreter-overhead-2026-10-30/`](evidence/full-interpreter-overhead-2026-10-30/).
- Previous full run `full-new-revision-2026-10-27`: 102,957 pass. Evidence:
  [`evidence/full-new-revision-2026-10-27/`](evidence/full-new-revision-2026-10-27/).
- Failure: `annexB/language/function-code/block-decl-func-skip-arguments.js`
  contradicts `staging/sm/lexical-environment/block-scoped-functions-annex-b-arguments.js`
  in the same revision ([upstream issue 5113](https://github.com/tc39/test262/issues/5113)).
  Both cannot pass.
- Timeouts: the 16 `staging/sm/Date/dst-offset-caching-*` variants.

Where the time goes in a full run: process startup is about 10 ms per variant
(about 1,100 seconds of worker time across 103,000 variants). The Unicode
property-escape tests, `built-ins/RegExp/property-escapes`, are 1,226 variants
taking about 1.1 seconds each: 1,309 of the 2,897 worker-seconds. The runner
itself is not the bottleneck; the engine's startup and that workload are.

## Files

- `corpus.lock.json`: repository and revision of the pinned corpus.
- `groups.json`: named selections (`smoke` plus focused groups such as
  `realms` and `shared-memory`). Add a group here rather than in scripts.
- `evidence/`: archived reports of the full runs referenced above.
- Table generators for the Unicode, CLDR and IANA data the engine commits (number
  patterns, time zones, emoji sets, locale data) are in
  [`tools/generators`](../generators/README.md). They are offline and not part of
  Test262 validation; `rgi-check` in that crate reads the corpus's emoji property
  lists, and its current mismatches are recorded there.

Experiment-by-experiment history of the earlier Python-based campaign is in
[`docs/test262-history.md`](../../docs/test262-history.md).
