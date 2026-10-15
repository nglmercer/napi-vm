# Raw JSON continuation

Adds JSON.rawJSON and JSON.isRawJSON with a private brand, explicit null
prototype, frozen rawJSON property, primitive JSON grammar validation, and
observable string coercion. Stringification checks the brand after toJSON and
the replacer. Proxies do not inherit the target brand. The output buffer now
preserves UTF-16 code units, including literal unpaired surrogates in raw text.
Existing string escaping and allocation limits remain in place.

The pinned JSON selection passes **330 / 330 variants (100%)**, with 32 gained
passes and zero lost passes against the preceding complete Temporal report's
JSON projection. No skips, timeouts, crashes, or harness errors occurred.
Evidence and the implementation snapshot are in
`tools/test262/evidence/json-raw/`.

The debug worker SHA-256 is
`57636268b89912d28d1b70b89f76c506c35dd6419892fe9666c9a118d4b2453f`.
The pinned corpus is `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.
The run used four jobs and a five-second timeout without feature skips.

Validation passes: six JSON regressions, eight Temporal regressions, 22 builtin
regressions, 17 UTF-16 regressions, and 17 worker regressions (70 Rust tests).
JSON and Temporal regression helpers independently run AST and verified bytecode.
Strict Clippy for the library, worker, and JSON/Temporal tests, all 12 Python
Test262 tooling tests, formatting, and whitespace checks pass.

The latest measured full-suite result is **79,243 / 102,956 (76.9678%)**, before
this raw JSON extension. Its archived comparison records 1,954 gained passes and
zero lost passes versus the preceding JSON reviver full run. A complete run
including raw JSON has not been measured. The overall 90% goal remains open;
focused gains are not presented as a measured full-suite result.

Reproduce:

```sh
cargo build --no-default-features --bin napi-vm-test262 \
  --config profile.dev.package.napi-vm-core.codegen-units=1
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/debug/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --path built-ins/JSON \
  --output artifacts/test262/json-raw-focused.json
cargo test --no-default-features --test json_reviver --test temporal \
  --test builtin_quick_wins --test utf16 --test test262_worker \
  --config profile.dev.package.napi-vm-core.codegen-units=1
```
