# Generated empty-block eval timeout

Evaluating large generated programs consisting only of flat empty blocks used
to allocate millions of tokens, AST nodes and lexical environments. The eval
runtime now validates that complete source shape with a linear UTF-16 scan and
returns its undefined completion without those allocations. This is independent
of the TypedArray experiment.

The accepted source contains only flat `{}` blocks and ASCII whitespace. Nested
blocks, comments, expressions, declarations, malformed suffixes and other text
use the original parser and evaluator. Compilation/parser APIs are unchanged.
Cancellation is checked during scanning; execution still charges exactly one
fuel unit per empty block, as the AST evaluator does. The complete source is
validated before charging fuel, preserving parse-before-execution behavior.

## Measured results

The pinned `staging/sm/regress/regress-610026.js` fixture changes from **two
timeouts to two passes** with two workers and the unchanged five-second deadline.
Three further runs pass both variants (6/6). Only the script variant belonged to
the previously recorded 49-timeout inventory; this does not establish a new
full-suite score or imply that inventory fell by two.

The neighboring `built-ins/eval`, `language/eval-code` and
`language/statements/block` selection remains **512/514** with zero status
changes. Its two existing failures concern deletion of eval-created bindings:
`var-env-func-init-local-new-delete.js` and `var-env-var-init-local-new-delete.js`.
They are retained without exclusions.

All **374 core tests**, **five integration regressions**, **12 runner tests**,
formatting and core Clippy pass. Regressions cover completion values, malformed
and meaningful source, strict/indirect eval, fuel exhaustion and cancellation.

Both immutable workers use release optimization with LTO disabled and 16 codegen
units. Their hashes are distinct; the candidate was built in a fresh, separate
target directory. The earlier identical-worker comparison problem in the
TypedArray investigation is not used as evidence here.

[Full outcomes, source identities and logs](../tools/test262/evidence/empty-eval-2026-10-24/)
are archived alongside the repeat runs. No test deadlines or skip lists changed.

```sh
CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
  CARGO_TARGET_DIR=target/empty-eval-frozen \
  cargo build --locked --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py /path/to/test262 \
  --engine target/empty-eval-frozen/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 2 --timeout 5 --path staging/sm/regress/regress-610026.js \
  --output artifacts/test262/empty-eval.json
```
