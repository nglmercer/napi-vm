# Developing trusted process plugins

These plugins execute with the launching account's operating-system privileges. They are separate from the legacy VM/sandbox plugins. Importing a plugin definition does not start it; `serve` explicitly starts the managed protocol endpoint. Running a plugin's `main` file without a host produces a bootstrap error.

The commands below use a POSIX shell. On Windows, use equivalent PowerShell paths and the `.exe` suffix for the Rust host. Native execution evidence is listed separately in [compatibility.md](compatibility.md).

## Prepare the local development tools

From this checkout:

```sh
npm ci --ignore-scripts
npm run plugins:build:ts
REPO="$PWD"
CLI="$REPO/packages/plugin-cli/src/index.mjs"
node "$CLI" --help
```

`plugins:build:ts` builds the independent JS packages and examples. It does not build the legacy N-API VM or require Cargo. Node-compatible templates use TypeScript 6.0.3 for strict checking and emission, including `checkJs` for plain-JS source. Loading their built ESM requires only the declared Node/Bun runtime.

The `@napi-vm/*` packages in this checkout have not been published. `create` therefore copies the installed payloads of the protocol, SDK, host, code generator and CLI into the new project's `vendor/` directory. Only each package's declared file list and license are copied; `node_modules`, package caches and the new destination are excluded. Internal package dependencies become sibling `file:` references, and the project's dependencies point to `file:vendor/...`.

This means ordinary `npm install` or `bun install` never needs unpublished `@napi-vm` registry packages. Installation is still an explicit developer step for the public TypeScript/type dependencies. With those packages already in npm's cache, the verified offline form is:

```sh
npm install --offline --ignore-scripts --no-audit --no-fund
```

Creating into an existing directory is refused. Missing built local package payloads are reported rather than creating an unusable scaffold.

## TypeScript: Bun-first workflow

```sh
node "$CLI" create ../my-ts-plugin --language ts
cd ../my-ts-plugin
bun install
bun run typecheck
bun test
bun run build
bun run test:artifact
bun run dev --runtime bun
```

The development host waits for readiness, watches relevant source/contracts/manifests/assets, builds into a private staging directory, and replaces the running instance after a successful build. Edit `src/plugin.ts` while it runs. Ctrl-C stops the host and its owned child processes. Output directories are not watched; rebuilds/reloads are serialized and newer edits are coalesced. A compiler failure preserves the existing running instance.

Contract generation is explicit. After changing an interface, regenerate its bindings/metadata before rebuilding; watching a source schema does not silently install tools or invent a new business contract. See [contracts.md](contracts.md) and the schema profile for generator configuration.

## TypeScript or JavaScript: Node-only workflow

Choose `ts` or `js` when creating the project; all subsequent commands are identical:

```sh
node "$CLI" create ../my-js-plugin --language js
cd ../my-js-plugin
npm install
npm run typecheck
npm run build
npm run test:node
npm run test:artifact -- --runtime node
npm run dev -- --runtime node
```

No Bun or Rust compiler is required for these Node commands. The Node tests execute emitted JS. Bun tests can exercise source definitions through the same in-process SDK harness. Both templates keep business code in `src/plugin.ts` or `src/plugin.js`; only `src/main.*` calls `serve`.

The ordinary shell does not automatically include a project's `node_modules/.bin`. Package scripts above resolve the installed CLI correctly. For direct commands, the vendored CLI provides an explicit, cross-platform entry path:

```sh
node ./vendor/plugin-cli/src/index.mjs doctor .
node ./vendor/plugin-cli/src/index.mjs inspect .
node ./vendor/plugin-cli/src/index.mjs validate .
node ./vendor/plugin-cli/src/index.mjs pack . --out ../my-plugin-package
node ./vendor/plugin-cli/src/index.mjs invoke ../my-plugin-package \
  --interface example.greeter --method greet --input '{"name":"Ana"}' --runtime node
```

`test --artifact` reads the project's declared `plugin.test.json` scenarios. If the build directory has no package lock yet, this explicit test command creates and tests a temporary checksummed package. It does not build or install dependencies. `pack` materializes in-project dependency links, includes only declared files, and refuses overwriting, escaping paths, external symlinks, secret files and invalid inventories.

Logs and compiler diagnostics go to stderr; command results are JSON on stdout. `doctor` validates runtime names before probing their versions and never executes the plugin.

## Rust: independent Cargo workflow

Create a standalone project using the optional JS CLI once:

```sh
# Run from the checkout, with REPO and CLI set as above.
node "$CLI" create ../my-rust-plugin --language rust
cd ../my-rust-plugin
cargo check
cargo test
cargo build --release
```

The project contains checked-in generated Rust bindings and vendored protocol/SDK crate sources with a lockfile. These Cargo commands do not invoke Node, Bun, npm or a JS generator. Business code lives in `src/lib.rs`; `src/main.rs` supplies metadata and enters the Tokio runtime. The tests include a generated-contract SDK harness invocation.

### Optional managed Rust development and artifact testing

Native executable plugins use an explicitly selected Rust host. The TypeScript host never launches them, and no hidden Rust sidecar is installed by the JS SDK.

Build the example adapter explicitly from the checkout, or provide your application's equivalent prebuilt Rust host:

```sh
cd "$REPO"
cargo build -p trusted-host-rust
HOST="$REPO/target/debug/trusted-host-rust"
cd ../my-rust-plugin
node "$CLI" build . --format executable
node "$CLI" test . --artifact --host-executable "$HOST"
node "$CLI" dev . --host-executable "$HOST"
```

The explicit executable build copies the Cargo binary into `bin/` and records the actual OS, architecture and Linux libc. Rust development stages a separate executable for each reload, avoiding replacement of a running file. `--host-executable` speaks the Rust adapter's managed JSON-lines protocol; it is optional authoring tooling, not a dependency of a Rust library consumer.

For a packaged one-shot invocation:

```sh
node "$CLI" pack . --out ../my-rust-package
node "$CLI" invoke ../my-rust-package --host-executable "$HOST" \
  --interface example.greeter --method greet --input '{"name":"Ana"}'
```

The Node CLI is optional for these management commands. Rust applications can use `napi-vm-plugin-host` directly. Ordinary production loading only starts an existing, verified artifact; it never runs Cargo, npm, Bun installation, a compiler or a downloader.

## Custom development services

For JavaScript-hosted plugins, pass `--setup /absolute/path/to/dev-host.mjs`. Its default export receives the host and can register application-specific services before loading. Without a setup module, the CLI supplies the generated `app.configuration` demonstration service when required. JavaScript setup modules are not loaded into the native Rust adapter; configure those services in the selected Rust host executable.

## Reproduce the template workflow checks

After building the JS packages, with Node, npm and Bun on `PATH` and the public build dependencies in npm's cache:

```sh
node packages/plugin-cli/scripts/test-templates.mjs
```

This creates fresh TS and JS projects and executes their actual install/package scripts. It checks cached offline npm installation; Node typecheck/build/logic/artifact tests and watched reload; then Bun's offline migration of that npm lock, typecheck/source tests/build/artifact tests and watched reload. Temporary projects are removed in `finally`; add `--keep` to retain them for inspection. This is not a claim that Bun's separate cache was populated by a fresh Bun-only installation.

See [testing](testing.md) for broader reproduction and [evidence provenance](evidence/README.md) for historical observations.
