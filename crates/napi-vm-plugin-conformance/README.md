# napi-vm-plugin-conformance

Repository-only cross-language trusted plugin conformance fixtures and lifecycle integration tests. This crate is intentionally `publish = false`: its tests and fixture executable depend on repository contracts and JavaScript examples and are not a public downstream API.

Run `cargo test -p napi-vm-plugin-conformance` from the repository for its regular tests. The cross-language interoperability test is ignored by default: build and stage the fixtures, then run `npm run plugins:test:rust-interop`. See the [verification guide](../../docs/trusted-plugins/testing.md) for prerequisites and the full matrix. Runtime tests require the runtimes declared by their fixtures; compiling a fixture alone is not runtime validation.
