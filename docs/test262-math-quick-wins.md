# Test262 Math quick wins

Base: `17098f0` (2026-10-14 checkout). Corpus: `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.
Rust: 1.99.0, Linux x86_64. No dependencies, corpus edits, skips, or assertion changes.

## Confirmed candidates and ranking

The recorded `latest.json` describes an older snapshot (67,677/102,956), so it is
not used as the before measurement. Archived outcomes were triaged first, then
Math and Number were rerun against an unmodified release worker at the base SHA.

| Candidate | Newly passing variants | Cost | Decision |
| --- | ---: | --- | --- |
| `Math.clz32` | 20 | Small native method | Implemented using shared ToNumber and ToInt32 bits |
| `Math.fround` | 18 | Small native method | Implemented with binary32 round-to-nearest, ties-to-even |
| `Math.imul` | 10 | Small native method | Implemented using shared conversions and wrapping product |
| `Math.min` / `Math.max` conversion after NaN | 4 | Two small loop changes | Implemented; later conversion exceptions still propagate |
| `Math[Symbol.toStringTag]` descriptor | 2 | Standard symbol property installation | Implemented with existing symbol metadata mechanism |

Native methods reuse `native_method`, including function metadata,
nonconstructibility, and Function.prototype inheritance. Number coercion is shared
by AST and bytecode calls. ToInt32 followed by reinterpretation as u32 provides
the ToUint32 bits needed by clz32; modulo-2^32 signed multiplication gives imul.

## Focused measurement

| Selection | Variants | Before passes | After passes | New passes | Lost passes |
| --- | ---: | ---: | ---: | ---: | ---: |
| built-ins/Math | 654 | 416 | 470 | 54 | 0 |
| built-ins/Number | 680 | 522 | 522 | 0 | 0 |
| Combined | 1,334 | 938 | 992 | 54 | 0 |

There are no skips, timeouts, crashes, or harness errors in either focused run.
These percentages (70.3148% before, 74.3628% after) describe this selection only.
Worker hashes and every outcome are in `tools/test262/evidence/math-quick-wins/`.

## Full-corpus measurement

Fresh base: **75,563 / 102,956** passes (73.3935%). Changed engine:
**75,637 / 102,956** passes (73.4654%). **74 newly passing variants, zero lost
passes**, and exactly 74 status transitions. Both runs have two identical
pre-existing timeouts, no skips, no crashes, and no harness errors.

The 20 gains beyond the focused selection are 14 Array variants, two String
variants, and four staging variants using the corrected Math built-ins. These
are improvements against the fresh base, not against the older 67,677 snapshot.
Complete before/after outcomes, triage, transitions, logs, hashes, and the exact
implementation patch are archived beside the focused evidence. Full-corpus runs
use the existing worker's normal evaluation path; AST/bytecode parity is asserted
by the seven Rust regressions, not claimed for the entire corpus.

## Validation

Seven new Rust regression tests assert verified bytecode selection and separately
execute parsed ASTs. They cover missing/undefined arguments, modulo wrapping,
nonfinite numbers, signed zero, binary32 ties/subnormals/overflow, string coercion,
Symbol and BigInt rejection, coercion order, abrupt completion, constructor
rejection, function metadata, and property descriptors. All seven pass. The first
six also ran before the implementation and all failed for the expected bugs.

- `cargo fmt --check`: passes.
- Targeted regression tests: 7 pass, 0 fail.
- `cargo test --locked --no-default-features --no-fail-fast`: 307 pass, 5 fail.
- `cargo test --locked -p napi-vm-core --no-default-features`: 354 pass, 1 fail.
- Standard targeted Clippy: passes with six existing warnings.
- Strict Clippy (`-D warnings`): blocked by six warnings in untouched code;
  unchanged base reproduces all six. No warning suppressions were added.
- N-API plus WASM feature compilation (`cargo check --locked --features wasm`): passes on the host target. This is not a wasm32 execution test.
- Test262 Python tooling tests: 12 pass (see archived log).

The six broader Rust failures were individually reproduced on the unchanged base:

- `evaluation_modes::compact_property_cache_verification`: expected two caches,
  got one, unrelated to Math.
- Three `lsp_runtime` tests: this environment rejects Unix socket binds.
- `object_spread::rest_enumerates_after_named_bindings_and_excludes_symbol_keys`:
  existing AST fallback where the test expects bytecode.
- `shape::tests::add_transition_matches_literal`: existing shape lookup returns
  None rather than the expected shape.

These failures remain visible and are not skipped, suppressed, or repaired as
unrelated work. This patch cannot claim a completely green repository baseline.

## Reproduce

Build and record the base worker first; build the changed checkout separately.
Use the same pinned, clean corpus and runner settings for both workers:

```sh
cargo build --locked --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 \
  --path built-ins/Math --path built-ins/Number \
  --output artifacts/test262/math-number-after.json
python3 tools/test262/triage.py artifacts/test262/math-number-after.json \
  --output artifacts/test262/math-number-after-triage.json
python3 tools/test262/compare.py artifacts/test262/math-number-before.json \
  artifacts/test262/math-number-after.json \
  --output artifacts/test262/math-number-transitions.json
cargo test --locked --no-default-features --test math_quick_wins
cargo fmt --check
cargo clippy --locked --no-default-features \
  --lib --bin napi-vm-test262 --test math_quick_wins
```

For the full corpus, omit both `--path` arguments and use the full-before/full-after
output filenames. The complete comparison uses the same `compare.py` command.

## Deferred failures

The focused post-change run still has 184 Math and 158 Number failing variants.
Math failures include absent inverse/hyperbolic functions, expm1/log1p, f16round,
and sumPrecise. Their precision, exceptional inputs, and platform libm behavior
need independent audits and are deferred to keep this patch independently reviewable.
Number failures include constructor/prototype behavior, conversion semantics, and
metadata. They require separate focused reproduction rather than speculative edits.
Object, Reflect, Array, String, Date, JSON, and collections were not changed in this
batch. Proxy, RegExp, realms, modules, and GC remain outside this quick-fix scope.
