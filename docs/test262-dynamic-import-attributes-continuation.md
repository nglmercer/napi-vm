# Dynamic import attributes continuation

Dynamic `import(specifier, options)` now carries its enumerable string-valued
`with` attributes through the import job to the host `ModuleLoader`. The new
`load_with_attributes` hook defaults to the existing `load` method, so existing
loaders keep their behavior. The Test262 worker uses `type: "json"` to expose
parsed JSON and `type: "text"` to expose a module's source as a string.
The module graph is still keyed by canonical module id, so loading one id
under multiple different attribute sets does not create separate module
records. Static import attributes are also not yet routed through this hook.

On the pinned Test262 revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, the dynamic-import
`import-attributes` directory passes **42 / 44** variants. The two failures are
the script and strict variants of `2nd-param-await-ident.js`; the parser
rejects a function named `await`. Both variants failed the same way in the
previous archived module/dynamic-import selection, so this work adds no new
failure there. The four JSON/text attribute variants that previously failed
now pass.

The combined `language/module-code` and `language/expressions/dynamic-import`
selection passes **2,448 / 2,502** variants, gaining four passes and losing
zero compared with the previous committed release evidence. This adds four
passes over the deferred-import checkpoint's 2,444 / 2,502 focused result.
The full pinned corpus was not rerun, so its recorded 101,488 / 102,956 score
remains unchanged. Focused reports and the measured source patch are archived
in [`tools/test262/evidence/dynamic-import-attributes-2026-10-18/`](../tools/test262/evidence/dynamic-import-attributes-2026-10-18/).

Validation passed for workspace Clippy with warnings denied, formatting,
`cargo check --no-default-features --bin napi-vm-test262`, and the
`engine_boundary` plus `test262_worker` integration targets (26 tests).

Reproduce the focused checks with the pinned corpus:

```sh
cargo build --locked --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 \
  --path language/expressions/dynamic-import/import-attributes \
  --output artifacts/test262/dynamic-import-attributes.json
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 6 --timeout 5 --path language/module-code \
  --path language/expressions/dynamic-import \
  --output artifacts/test262/module-async-regression.json
```
