# CI tooling

The workflow definitions in [`.github/workflows`](../../.github/workflows/) are
the source of truth for current matrices and commands. The shared
[`setup-build`](../../.github/actions/setup-build/action.yml) action configures
native linkers and Cargo caches after Rust is installed. It uses mold for Linux
GNU targets, Rust's bundled `lld-link` for Windows MSVC, and the platform linker
on macOS; Miri opts out of native linker configuration. Cache keys include the
compiler, target and linker configuration, workload, and manifests.

The [runtime matrix](../../.github/workflows/runtime-matrix.yml) builds and
reuses native addons across its Node versions. The main
[CI workflow](../../.github/workflows/ci.yml) has separate Rust, wasm, language
server and Node-API checks; the [trusted plugin workflow](../../.github/workflows/trusted-plugins.yml)
contains the process-plugin matrix. The [release workflow](../../.github/workflows/releases.yaml)
checks and publishes platform assets. A workflow definition alone is not proof
that its jobs ran successfully.

Useful local entry points from the repository root:

```sh
npm run test:rust
npm run test:wasm
npm run lsp:test
npm run plugins:check
RUNNER_TEMP="${TMPDIR:-/tmp}" bash tools/ci/lint-workflows.sh
```

The workflow linter downloads a pinned, checksum-verified Linux x64 actionlint
binary. Other commands have their own Rust, Node, Bun, wasm-pack, target, and
fixture requirements; consult their scripts and workflow jobs before running
them. See [trusted plugin testing](../../docs/trusted-plugins/testing.md) for
plugin prerequisites. A local timing result does not establish CI performance
on other runners.
