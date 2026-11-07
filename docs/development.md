# Develop and maintain the project

Run commands from the repository root. Install Rust, a native compiler/linker,
Node.js 22+, npm, and Bun at the version in `.bun-version`. Browser builds also
need wasm-pack and `wasm32-unknown-unknown`; Zed extension builds need
`wasm32-wasip2`. The workflows pin CI toolchains and platform dependencies.

## Install and build

```sh
npm ci --ignore-scripts
npm run build:all
```

`build:all` builds the addon once and emits CommonJS, ESM, and declaration files
from the same export set. `build:debug` is useful for local native debugging.
Cargo defaults to the facade workspace member; use `--workspace` or a package
selection when changing other members. Core/runtime path dependencies also
have their own tests.

## Select verification for the change

| Changed surface | Commands |
| --- | --- |
| Core engine / facade Rust | `npm run test:rust`; `cargo test --profile ci --locked --no-default-features` |
| Native Node boundary | Rebuild addon; `npm run test:node` |
| Guest language behavior | `bun test ./tests --path-ignore-patterns '**/{node,wasm}/**'`; appropriate [Test262 gates](../tools/test262/README.md) |
| WASM / shared engine behavior | `npm run test:wasm` |
| LSP and live metadata | `npm run lsp:test`; `npm run runtime:smoke` |
| Trusted plugin stack | `npm run plugins:check` |
| Types and lint | `npm run lint`; `npm run lint:playground` after generating WASM |
| Documentation | `node tools/docs/check.mjs`; `node tools/docs/verify-examples.mjs` |

`npm test` runs the main Bun suite, Node boundary tests, and existing WASM
regressions. `test:wasm` builds the browser package and typechecks it before
running its tests; merely invoking the last suite can otherwise use stale or
missing generated artifacts. A green narrow suite does not prove all language
or host boundaries passed.

## Regenerate owned outputs

```sh
npm run build:all
npm run plugins:generate
node packages/plugin-cli/scripts/sync-assets.mjs
npm run plugins:check-generated
npm run plugins:check-assets
```

The first command owns `index.js`, `index.mjs`, and `index.d.ts`. Plugin
generation owns contract JSON, TypeScript/Rust bindings, Markdown references,
and example metadata. Asset synchronization owns copied scaffold contracts and
independent Rust SDK sources. Commit changed generated files with their input
or generator change. Do not edit scaffold vendor copies by hand.

Unicode/CLDR/timezone generation is an explicit maintenance operation with
pinned data provenance; use [the generator guide](../tools/generators/README.md).

## Use the build profiles intentionally

`release` uses thin LTO and one codegen unit for shipped artifacts. `ci` inherits
debug behavior while disabling debug data/incremental compilation. `ci-optimized`
keeps release optimization without whole-program LTO and emits line tables.
Tests of coroutine stack switching need the intended optimized target/profile;
an unrelated debug result does not establish platform release behavior.

The CI setup action selects supported fast linkers and Cargo caches. See
[build tooling](../tools/ci/README.md) before reproducing a target-specific
failure. Native loading and teardown require platform runs; cross-compiling an
artifact proves only that it built.

## Maintain the docs

Each topic has one owning guide linked from [the documentation index](README.md).
Package READMEs should stand alone for package consumers. Mark self-contained
checkout snippets with `javascript verify` or `rust verify`; the example runner
executes those exact blocks. Generated contract prose belongs to the generator.
Keep plans in issues or Git history instead of adding another current-status
page that will drift from code.

`npm run docs:check`, `npm run docs:test`, and `npm run docs:test:rust` are the
package-script entry points. Build the addon and `plugins:build:ts` before
executing examples. The Rust option also compiles/runs the marked Rust blocks;
the default example check runs the marked Node blocks. See
[documentation tooling](../tools/docs/README.md) for prerequisites and scope.

Before submitting, check generated outputs, relevant tests, formatting, and
`git diff --check`. Preserve raw validation/benchmark evidence when documenting
results. A source revision, platform matrix definition, and successful execution
report are different pieces of evidence.
