# Run Test262 against the engine

The Rust runner executes the pinned ECMAScript suite through the isolated
`napi-vm-test262` worker. Each variant runs in a fresh process with a deadline,
so a worker crash or hang becomes a reported outcome rather than stopping the
entire campaign.

## Build and fetch

From the repository root:

```sh
cargo build --release --locked --no-default-features --bin napi-vm-test262
cargo build --release --locked -p napi-vm-test262-runner
target/release/test262 fetch --profile reviewed-5112
```

The runner defaults to `target/release/napi-vm-test262`; select another artifact
with `--engine FILE`. If you build into a separate target directory, pass that
worker explicitly rather than accidentally running an older default binary.

`corpus.lock.json` pins the upstream revision. Fetch is idempotent for a matching
checkout. Runs validate corpus revision and modifications. Named selections
are in `groups.json` rather than duplicated shell scripts.

## Choose a corpus profile

`original` is the default pinned upstream corpus, left unchanged.
`reviewed-5112` uses a separate checkout with one explicitly identified correction
from upstream PR #5112. Profile metadata records the base revision, overlay blob,
patch hash, and provenance. A reviewed-profile result is not a claim that the
unmodified original corpus passed.

The [retained reviewed run](evidence/reviewed-5112-full-2026-11-06/README.md)
contains 102,992 passing variants with zero skips/non-passing outcomes.
[Project validation](../../docs/validation.md) explains its scope.

## Run a validation loop

```sh
target/release/test262 run --profile reviewed-5112 --group smoke --jobs 2 \
  --timeout 10 --output artifacts/test262-smoke.json
target/release/test262 retest artifacts/test262-smoke.json \
  --output artifacts/test262-retest.json
target/release/test262 compare artifacts/test262-smoke.json \
  artifacts/test262-retest.json --subset
```

The smoke group is a fixed cross-section. Retest runs only the base report's
non-passing variants and reuses its worker count, timeout, profile, and skip
list unless explicitly overridden. A subset comparison checks only the
retested selection; it cannot prove no other variant regressed.

For the complete selected-corpus gate:

```sh
target/release/test262 run --profile reviewed-5112 --jobs 2 --timeout 10 \
  --output artifacts/test262-full.json \
  --checkpoint artifacts/test262-full.checkpoint
```

To resume, use the same configuration/checkpoint with `--resume`. A changed
worker or run configuration is rejected. Whole runs can take tens of minutes;
the CLI reports progress. The runner defaults to two workers and five seconds
per variant; specify the ten-second CI deadline explicitly when reproducing
the retained reviewed run.

## Inspect and compare

`summary REPORT` prints grouped counts and non-passing outcomes; `--json FILE`
writes its summary. `compare BASELINE CURRENT` reports gains, losses, changes,
additions, and removals, exiting with 1 when a previously passing variant loses
its pass. Use `--allow-config-change` only after inspecting why configuration
differs; it does not make mismatched results equivalent.

Reports identify revision/corpus provenance, engine SHA-256, jobs, timeout,
counts, and per-variant results. Outcome categories distinguish failure,
timeout, crash, skip, and harness error. Check exact variant-set equality when
claiming a complete no-regression result.

## Focus or profile a workload

| Command | Use |
| --- | --- |
| `run --path P --group G` | Select paths/groups; options can be repeated where supported |
| `time PATH --variant script\|strict\|module` | Time individual variants; uncapped unless a timeout is supplied |
| `request TEST --variant ... --output FILE` | Produce the exact worker input for a variant |
| `bench FILE --harness --repeat N` | Time snippet files through the worker |

Run `target/release/test262 help` for the complete argument list. Data-table
generation is separate from conformance execution; see
[the generators](../generators/README.md).
