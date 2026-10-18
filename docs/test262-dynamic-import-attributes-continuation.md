# Static and dynamic import attributes continuation

Static imports, re-exports, and dynamic `import(specifier, options)` now carry
their ordered, string-valued `with` attributes through the module linker to the
host `ModuleLoader`. The `load_with_attributes` hook defaults to `load`, so
existing embedders remain compatible. The module registry creates distinct
records for one canonical id requested with different attribute sets, while
resolving relative imports and `import.meta.url` against the canonical id.

The Test262 worker uses `type: "json"` to validate JSON source and produce a
default JSON export, and `type: "text"` to produce a default string export.
It applies these conversions to both filesystem and virtual fixtures. Script
tests execute the exact program parsed with the Script goal, preserving
contextual identifier behavior such as a non-module function named `await`.

On pinned revision `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, the current
focused worker passes **170 / 170** variants across:

- `language/import/import-attributes`: **17 / 17**
- `language/expressions/dynamic-import/import-attributes`: **44 / 44**
- `language/import/import-defer`: **109 / 109**

The neighboring `language/module-code` and
`language/expressions/dynamic-import` selection passes **2,454 / 2,502**. The
completed full-corpus run now includes this implementation at source `d8d1652`
and passes 101,806 / 102,956 (98.8830%). Its measured score and denominator are
recorded separately in [the full evidence](../tools/test262/evidence/full-tail-calls-2026-10-18/). Current focused reports and
the measured source patch are archived in
[`tools/test262/evidence/module-attributes-2026-10-18/`](../tools/test262/evidence/module-attributes-2026-10-18/).
The earlier 42 / 44 dynamic-import report remains in the
[`dynamic-import-attributes` evidence archive](../tools/test262/evidence/dynamic-import-attributes-2026-10-18/)
as the previous implementation checkpoint.

Validation passed for strict workspace Clippy, formatting, the worker build,
and the `engine_boundary` plus `test262_worker` integration targets (26 tests).

Reproduce the focused checks with the pinned corpus:

```sh
cargo build --locked --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 \
  --path language/import/import-attributes \
  --path language/expressions/dynamic-import/import-attributes \
  --path language/import/import-defer \
  --output artifacts/test262/module-attributes.json
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --path language/module-code \
  --path language/expressions/dynamic-import \
  --output artifacts/test262/module-async-regression.json
```
