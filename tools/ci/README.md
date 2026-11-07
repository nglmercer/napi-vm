# Native build and CI tools

The [workflow files](../../.github/workflows/) define current jobs, targets,
toolchains, prerequisites, and release artifacts. Shared setup scripts configure
compilers/linkers and cache fingerprints; they do not replace the actual test
or compatibility reports from a job.

## Build helpers

| Tool | Responsibility |
| --- | --- |
| `build-bindings.mjs` | Build the addon once and emit matching CommonJS, ESM, and declaration exports |
| `node-headers.cjs` | Locate/download headers for native fixture builds |
| `setup-mold.sh` | Configure the pinned fast linker for supported Linux GNU builds |
| `setup-msvc.ps1`, `setup-lld.ps1` | Configure Windows compiler/SDK and linker |
| `cache-key.cjs` | Fingerprint compiler, target, linker, and build configuration |
| `lint-workflows.sh` | Run a pinned, checksum-verified actionlint binary |

The [setup-build action](../../.github/actions/setup-build/action.yml) selects
platform setup and caches. macOS uses the platform linker. Miri opts out of
native-linker configuration. Workload-specific cache groups avoid conflating
incompatible artifact configurations; Cargo still validates fingerprints.

## Reproduce a job

Install the workflow's selected toolchain/target and dependencies, then run its
exact command and feature selection. Useful root-level entry points include:

```sh
npm run test:rust
npm run test:wasm
npm run lsp:test
npm run plugins:check
```

Each command has prerequisites. WASM needs wasm-pack and its target; plugin
interop needs selected runtimes/build fixtures; native addon tests need platform
compilers. Do not infer a runtime pass from successful cross-compilation.

The workflow linter expects a supported Linux environment and downloads its
pinned executable. Run `bash tools/ci/lint-workflows.sh` with the temporary
directory environment required by that script when outside CI.

For routine contributor commands and profiles, use
[development](../../docs/development.md). For interpretation of platform and
conformance evidence, use [validation](../../docs/validation.md).
