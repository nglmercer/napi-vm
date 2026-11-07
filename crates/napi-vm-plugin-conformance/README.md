# Trusted-plugin conformance fixtures

Repository-only Rust fixtures and integration tests for the process-plugin
stack. This package is not published as a downstream authoring library.
Applications should depend on the protocol, SDK, or host crate instead.

The fixtures exercise real peer behavior and lifecycle/error boundaries using
the same generated contracts as the JavaScript stack. Unit tests are only one
part of interoperability evidence; actual Node/Bun/native process combinations,
package relocation, and target ABI checks require their own runs.

From the repository root:

```sh
cargo test -p napi-vm-plugin-conformance
npm run plugins:test:interop
npm run plugins:test:rust-interop
```

Build the fixtures and install the selected runtimes first. Consult the
[testing guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/testing.md)
for the full gate and runtime prerequisites.
