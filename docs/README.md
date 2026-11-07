# Documentation

Start with [getting started](getting-started.md) for a local build and working
examples. Use [the API reference](api.md) for the Node boundary and
[crate architecture](crate-architecture.md) for Rust ownership and features.

## Current guides

| Topic | Guide |
| --- | --- |
| Setup, callbacks, modules, async use | [Getting started](getting-started.md) |
| `Vm`, `AsyncSession`, language service, sessions | [API](api.md) |
| Contributor commands and source layout | [Development](development.md) |
| Measured baseline and optimization workflow | [Performance](performance.md) |
| Engine/runtime/binding boundaries and UTF-16 | [Crate architecture](crate-architecture.md) |
| Resource limits and host privileges | [Safety](safety.md) |
| Explicit runtime capabilities and permissions | [Optional runtime](optional-runtime.md) |
| Clocks, event turns, async ownership | [Event loop](event-loop.md) |
| CommonJS and native addon backends | [Native addons](native-addons.md) |
| Sandboxed VM plugins | [Plugins](plugins.md) |
| Independent trusted process SDKs | [Trusted plugins](trusted-plugins/README.md) |
| LSP, Zed, playground and live metadata | [Editor integration](editor.md) |
| Locator, transport and runtime messages | [Runtime protocol](runtime-protocol.md) |
| Runtime ownership, caches and collection | [Runtime improvements](runtime-improvements.md) |
| Implemented surfaces and boundaries | [Implementation map](roadmap.md) |
| Test262 commands and corpus profiles | [Test262](../tools/test262/README.md) |

Package READMEs cover their own builds and consumers. Generated TypeScript
signatures are in [index.d.ts](../index.d.ts); implementation and executable
tests take precedence over prose when diagnosing drift.

## Evidence and history

[The completed reviewed-profile Test262 report](../tools/test262/evidence/reviewed-5112-full-2026-11-06/README.md)
records the selected full pass. [Test262 progress](test262-full-pass-progress.md)
keeps the original-corpus results and measured experiments separate.
[Historical Test262 campaigns](test262-history.md) explain older measurements.

Dated audits, implementation checkpoints, old plans, benchmark snapshots,
and evidence directories are historical records. Their status notices link
to current guides. Retain their original revision, platform, limitations,
and measured results when citing them; an old successful run is not proof
that a new binary or platform passed.

Generated contracts, copied scaffolds, vendored documentation, and immutable
validation reports are maintained by their owning generators or projects.
Documentation cleanup does not rewrite those records.

## Maintain these docs

Update the owning guide with behavior changes and link it from this index.
Keep API signatures in generated bindings, build commands in package scripts
or workflows, and raw measurement data in the relevant evidence directory.
Avoid copying long API or compatibility inventories into multiple guides.
Run the documentation checker described in [development](development.md).
