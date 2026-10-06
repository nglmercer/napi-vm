# Test262 runner

Build the isolated worker without runtime or Node bindings:

```sh
cargo build --no-default-features --bin napi-vm-test262
python3 -m pip install -r tools/test262/requirements.txt
git clone https://github.com/tc39/test262.git .napi-vm/test262
git -C .napi-vm/test262 checkout 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81
mkdir -p artifacts/test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/debug/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 2 --output artifacts/test262/full-results.json
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

The initial full development baseline is in `baseline.json`: 32,359 passes out
of 102,956 variants, 31.43%. The initial run preceded binary snapshot support;
future reports include the worker digest. Full individual outcomes and the
HTML dashboard are generated artifacts rather than vendored corpus content.
The HTML shows independent ECMAScript, Web, Node and npm metrics, and up to 200
non-passing variants with a link to the full JSON file.

Module linking failures are reported in the resolution phase before evaluation.

Known runner limitations: module fixtures currently load sibling
`*_FIXTURE.js` files; `$262.createRealm`, agents, GC and detachArrayBuffer hooks are incomplete.
Those limitations contribute failures or harness errors; they are not silently
removed from the denominator. No stable compatibility claim is made.

Verification:

```sh
python3 -m unittest discover -s tools/test262 -p 'test_*.py'
cargo test --no-default-features --test test262_worker --test engine_boundary
```
