# Development

## Prerequisites and ownership

Use Node.js 22+, npm, Rust, and Bun for the JavaScript regression suite.
CI pins Rust in [ci.yml](../.github/workflows/ci.yml); check that workflow
when reproducing CI. Browser builds also require `wasm-pack`, and Zed
builds require the `wasm32-wasip2` target. Build native code before testing
or benchmarking through Node so an old `.node` artifact cannot mask changes.

```sh
npm ci --ignore-scripts
npm run build
```

The [architecture guide](crate-architecture.md) explains engine/runtime
boundaries. [Generated declarations](../index.d.ts) are build outputs;
edit their Rust or wrapper source and regenerate them through the build scripts.

## Choose the smallest relevant check

| Change | Commands |
| --- | --- |
| Core engine | `cargo test -p napi-vm-core --lib`; relevant Rust integration tests |
| Root Rust facade | `npm run test:rust` |
| Node bridge | `npm run build`, `npm run test:node`, targeted Bun tests |
| Full JavaScript suite | `npm test` (includes Node and WASM test invocations) |
| Rust style and lints | `npm run lint:rust` |
| Main TypeScript sources | `npm run lint:ts` |
| Playground | `npm run test:wasm` |
| Generated root bindings | `npm run check:generated` |
| Test262 runner/profile | `cargo test -p napi-vm-test262-runner`; [profile smoke](../tools/test262/README.md) |
| Trusted plugin SDKs | [Plugin development](trusted-plugins/development.md) |
| Documentation | `node tools/docs/check.mjs` |

`lint:ts` uses the root TypeScript configuration. The playground has its own
configuration and generated WASM imports; `test:wasm` builds those imports,
type-checks the playground and runs the WASM regression suite. A successful
Node build does not validate WASM-specific code.

The full local gate is:

```sh
npm run lint
npm run check:generated
npm run build
npm test
npm run test:rust
npm run test:wasm
```

CI also tests the supported Node matrix, extracted crate configurations,
and optional runtime/platform configurations. The workflows are the
executable source for matrix values and toolchain versions.

## Test262

Build the isolated worker and runner, then iterate with a focused group or
`retest`. Complete-corpus reports remain necessary for broad engine changes.
Always identify the corpus profile, engine digest, worker count, timeout,
and skipped features. The reviewed one-file correction profile and original
upstream corpus are distinct validation targets.

[Commands and evidence](../tools/test262/README.md) ·
[completed reviewed-profile gate](../tools/test262/evidence/reviewed-5112-full-2026-11-06/README.md)

## Measure before optimizing

Use [the performance workflow](performance.md) to build first, capture a
baseline with raw samples and provenance, then compare a changed binary on
the same host. Avoid competing builds or benchmarks while measuring. Keep
cold startup, parsing, warm execution, host bridges, scheduling and plugin
transport results separate. Correctness gates accompany a measured improvement.

Blaze may be used for iteration when it supports a configuration. It failed
on ICU provider types in the recorded Test262 build; the verified worker
was built with Cargo. The project's pinned toolchain and CI remain the gate.

## Source map

| Path | Responsibility |
| --- | --- |
| `crates/napi-vm-core/src/` | Lexer, parser, bytecode VM, builtins, values, heap, language service |
| `crates/napi-vm-runtime/src/` | Optional capabilities, permissions, runtime adapters and CLI |
| `src/bindings/`, `src/wasm.rs` | Node and browser boundaries |
| `src/lsp/`, `src/bin/napi-vm-lsp.rs` | Native language server |
| `src/plugin_host.rs`, `plugins/` | Legacy guest-plugin hosting |
| `crates/napi-vm-plugin-*`, `packages/plugin-*` | Trusted process protocols, hosts, SDKs, tools |
| `runtime/`, `lsp/` | Opt-in live metadata sessions and compatibility tooling |
| `playground/`, `zed-extension/` | Browser/editor integration |
| `tests/`, `examples/` | Executable behavior and embedding examples |
| `tools/test262-runner/`, `tools/generators/` | Validation runner and committed data generation |
| `bench/`, `benches/` | Performance workloads and baseline tooling |

## Stackful coroutine changes

Generators run on the caller thread with a separate stack. For changes to
coroutine lifecycle or ownership, run the targeted generator stress tests.
A Linux ThreadSanitizer configuration is:

```sh
rustup component add rust-src --toolchain nightly
RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test -Zbuild-std \
  --release --no-default-features --test generator_stress \
  --target x86_64-unknown-linux-gnu -- --test-threads=1
```

A historical sanitizer result is not a result for a new patch. Record the
actual toolchain, platform and command with new evidence.

## Documentation maintenance

Use [the documentation index](README.md) for canonical pages. Keep current
instructions separate from dated checkpoints and immutable reports. Update
links and relevant examples when behavior changes; check Markdown links with
`node tools/docs/check.mjs`. Generated contracts and scaffold copies have
separate generator checks and should not be manually rewritten.
