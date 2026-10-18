# Deferred module evaluation continuation

Static `import defer * as ns` now preserves its import phase through parsing,
linking, and both AST and bytecode execution. The linker instantiates deferred
modules without evaluating synchronous bodies. Their cached namespace objects
evaluate on export-facing property operations, while `then` and Symbol-keyed
operations remain non-triggering. Private element addition rejects
non-extensible receivers. Dynamic `import.defer()` links the module and resolves
to its lazy namespace.

Async module evaluation also gathers top-level-await dependencies reachable
through deferred imports. It runs synchronous dependency prefixes in source
order, retains promise suspension at `await`, and tracks async cycle roots so
outside dependents wait for the full strongly connected component. The
Test262 worker uses its async module entry point for async module tests and
loads JSON fixtures as JSON modules.

On the pinned Test262 revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, the complete
`language/import/import-defer` selection passes **109 / 109** variants, with no
skips, timeouts, crashes, or harness errors. The adjacent
`language/module-code` and `language/expressions/dynamic-import` selection
passes **2,444 / 2,502** variants. Compared with those exact variants in the
previous full-run evidence, this is four additional passes and zero losses.
Reports are archived in
[`tools/test262/evidence/deferred-import-2026-10-18/`](../tools/test262/evidence/deferred-import-2026-10-18/).
The implementation patch is included in that archive with its SHA-256 in the
compact result. Three focused class/private-element Test262 variants also pass.

The full pinned corpus has not been rerun after these changes. Its recorded
score remains **101,488 / 102,956 (98.5741%)** and must not be adjusted by
focused results. Strict workspace Clippy, formatting, the workspace check, and
the engine-boundary suite pass.

The implementation follows the current
[TC39 deferred-import evaluation proposal](https://tc39.es/proposal-defer-import-eval/).

Reproduce the focused results from the repository root with the pinned corpus:

```sh
cargo build --locked --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --path language/import/import-defer \
  --output artifacts/test262/import-defer.json
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --path language/module-code \
  --path language/expressions/dynamic-import \
  --output artifacts/test262/module-async-regression.json
```
