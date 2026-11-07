# Trusted process plugins

This additive subsystem runs trusted plugins as supervised OS processes over private loopback TCP. It does not use the VM, remove legacy permission checks, or sandbox code. Choose this API explicitly; legacy manifests belong to the legacy host.

## Compatibility boundary

| Host | JavaScript/TypeScript artifact | Rust native executable |
|---|---|---|
| Node or Bun TS host | Yes, under its declared Node/Bun runtime | Explicitly rejected |
| Rust host | Yes, under its declared Node/Bun runtime | Yes, matching native target |

A single compiled portable TS package is the interoperability target. A Rust host does not reinterpret TS or translate it into Rust. External-runtime profiles can depend on specific runtime/native dependencies; portable-js profiles reject native dependencies. See [compatibility](compatibility.md) for the execution boundary and evidence.

## Repository commands

- `npm ci --ignore-scripts`
- `npm run plugins:generate` / `npm run plugins:check-generated`
- `npm run plugins:build:ts` / `npm run plugins:typecheck`
- `npm run plugins:build:rust`
- `npm run plugins:test:node` / `npm run plugins:test:bun`
- `npm run plugins:test:rust`
- `npm run plugins:test:interop`
- `npm run plugins:test:packaged`
- `npm run plugins:test:consumers` / `npm run plugins:test:rust-consumer`
- `npm run plugins:test:cargo-package` / `npm run plugins:check-assets`
- `npm run plugins:bench`

Node-compatible build uses TypeScript directly. Bun is required only for its declared runtime matrix, and Cargo only for Rust builds. Existing VM scripts remain separate. New package names are local workspace names; no packages have been published.

## Guides

[Architecture](architecture.md) · [Protocol](protocol.md) · [Manifest](manifest.md) · [Contracts](contracts.md) · [Development](development.md) · [Packaging](packaging.md) · [Testing](testing.md) · [Performance](performance.md) · [State migration](migration-ts-to-rust.md) · [Implementation status](implementation-status.md)
