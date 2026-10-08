# Test262 runner

Build the isolated worker without runtime or Node bindings:

```sh
cargo build --release --no-default-features --bin napi-vm-test262
python3 -m pip install -r tools/test262/requirements.txt
git clone https://github.com/tc39/test262.git .napi-vm/test262
git -C .napi-vm/test262 checkout 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81
mkdir -p artifacts/test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --output artifacts/test262/full-results.json
python3 tools/test262/dashboard.py artifacts/test262/full-results.json \
  --output artifacts/test262/compatibility.html
```

The corpus commit is pinned in `corpus.lock.json`; the runner checks the exact
revision and rejects modified/untracked corpus files. It snapshots the worker
binary before starting subprocesses and records its SHA-256 so concurrent
builds cannot change the implementation midway through a future run.

Metadata uses YAML. Default tests run both strict and non-strict variants;
`onlyStrict`, `noStrict`, `raw`, module and async flags are honored. Harness
includes are loaded before the test. Parse-negative source is parsed separately
from harness execution, and negatives require both the expected phase and error
type. Async tests require exactly one successful `$DONE`; missing or duplicate
completion fails. Every variant gets a fresh process and a hard timeout.

`--path language/expressions/addition` selects a subset; repeat `--path` for
multiple selections. `--skip-feature FEATURE` records explicit skips. The
percentage denominator includes **all selected variants**, including skips,
failures, timeouts, crashes and harness errors. A selected subset's percentage
is never advertised as overall ECMAScript compatibility. Empty/non-passing
runs return exit status 1 while still producing evidence.

For long runs, add `--checkpoint artifacts/test262/full.checkpoint.jsonl`.
After an interrupted process, repeat the same command with `--resume`. The
journal retains every completed variant outcome, including errors and timeouts.
Resume rejects changes to the corpus revision, worker digest, selection, skips,
worker count, or timeout. An interrupted final journal write is rerun. The final
report still contains all selected outcomes and records the restored file count.

The merged PR #23 development measurement is in `latest.json`: 41,407 passes
out of 102,956 variants (40.2182%), with zero feature skips. This does not establish
full conformance.

The initial full development baseline is in `baseline.json`: 32,359 passes out
of 102,956 variants, 31.43%. The initial run preceded binary snapshot support;
future reports include the worker digest. Full individual outcomes and the
HTML dashboard are generated artifacts rather than vendored corpus content.
The HTML shows independent ECMAScript, Web, Node and npm metrics, and up to 200
non-passing variants with a link to the full JSON file.

Module linking failures are reported in the resolution phase before evaluation.
The isolated worker receives an explicit corpus directory capability. It loads
nested and dynamically imported fixture files lazily, checks canonical paths
remain inside that directory, and also accepts virtual modules for worker tests.
The embeddable engine does not gain ambient filesystem access.

Async completion lives in the persistent global environment, so callbacks and
module scopes cannot lose `$DONE` state. A caught `$DONE(error)` still fails the
variant. `$262.evalScript` evaluates in the global environment and `$262.global`
is exposed; cross-realm construction is still incomplete.

Group all outcomes into actionable failure clusters without dropping skips,
crashes, timeouts, or harness failures:

```sh
python3 tools/test262/triage.py artifacts/test262/full-results.json \
  --output artifacts/test262/triage.json
```

See `docs/test262-conformance-validation.md` for the current validation evidence.


The host implements `$262.createRealm` and real worker agents with shared-memory
coordination. Full realm ownership and GC/finalization semantics remain incomplete.
ArrayBuffer detachment is implemented; GC requests run at quiescent host boundaries
and do not guarantee synchronous collection or finalization.
See [phases 1–3 implementation notes](../../docs/test262-phases-1-3.md).
See [foundation continuation status](../../docs/test262-foundations-completion.md)
for the new agent implementation and remaining completion gates.
Those limitations contribute failures or harness errors; they are not silently
removed from the denominator. No stable compatibility claim is made.

Verification:

```sh
python3 -m unittest discover -s tools/test262 -p 'test_*.py'
cargo test --no-default-features --test test262_worker --test engine_boundary
```

## Compile-only source audit

`audit_syntax.py` checks source acceptance with the public compile-with-goal API.
It uses the runner's pinned-corpus selection and strict/Script/Module variants,
without evaluating a harness or guest code. These results are not execution
passes. Invalid sources accepted and valid sources rejected remain explicit
outcomes, including unsupported proposals and execution-limit errors.

```sh
python3 tools/test262/audit_syntax.py /workspace/test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --output artifacts/test262/syntax-audit.json
```

The command builds a compile-only Rust companion and records source/engine
identities. An existing immutable companion can be supplied with `--engine`.
Full execution and AST/bytecode differential validation remain separate gates.
