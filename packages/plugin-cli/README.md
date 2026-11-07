# @napi-vm/plugin-cli

Create, generate, build, inspect, test, and package trusted process plugins.
This CLI is a development tool; ordinary host loading never compiles or installs.

## Create a project

```sh
napi-vm-plugin create my-plugin --language ts
```

Languages are `ts`, `js`, and `rust`. Install the created project's dependencies,
then use its README for language-specific commands. The Rust scaffold includes
licensed SDK/protocol source copies and builds independently with Cargo.

## Commands

| Command | Purpose |
| --- | --- |
| `codegen FILE --out DIR [--check]` | Generate/verify contract adapters and references |
| `inspect DIR` | Print manifest and contracts |
| `validate DIR` | Validate manifest, artifacts, and any package lock |
| `doctor DIR` | Diagnose tool/runtime availability |
| `build DIR --format js\|executable` | Compile explicitly and update launch metadata |
| `pack DIR --out NEW_DIR` | Package built files into a relocatable artifact and inventory |
| `invoke DIR --interface ID --method NAME --input JSON` | Invoke through an actual host |
| `test DIR --artifact` | Run declared artifact scenarios |
| `dev DIR` | Build/watch with lifecycle-aware reload |

Invocation/test/development can select `--runtime node|bun` and an application
`--setup` module. Native executable plugins require `--host-executable` pointing
to a Rust host. Full accepted arguments are printed by `napi-vm-plugin help`.
Use shell quoting appropriate to your platform for JSON input.

Results are JSON on stdout; diagnostics/compiler output go to stderr. Build
requires the appropriate TypeScript/Cargo/Bun toolchain. Declaring an artifact
profile does not prove it was built, distributed, or tested on another platform.

## Maintain scaffold assets

From the repository root:

```sh
node packages/plugin-cli/scripts/sync-assets.mjs
npm run plugins:check-assets
node --test packages/plugin-cli/test/*.test.mjs
```

Synchronization owns copied contracts, generated adapters, and Rust SDK sources.
Change the original crate/generator before refreshing copies.

[Authoring](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/authoring.md) ·
[Packaging](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/packaging.md)
