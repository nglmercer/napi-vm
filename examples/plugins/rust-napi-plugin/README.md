# Rust N-API plugin example

This example keeps the plugin host in Rust and implements one native module
with Rust and napi-rs. It does not start or embed Node.js.

From the repository root, build the native crate and copy its cdylib to the
`.node` path used by the guest:

```bash
cargo build --release --manifest-path examples/plugins/rust-napi-plugin/native/Cargo.toml
cp examples/plugins/rust-napi-plugin/native/target/release/librust_napi_plugin_native.so examples/plugins/rust-napi-plugin/native/addon.node
```

The copy source varies by target OS and architecture. The Rust host receives a
trusted SHA-256 digest and checks it before loading the addon:

```bash
cargo run --no-default-features --features node-api-host --example rust-plugin-napi -- \
  examples/plugins/rust-napi-plugin \
  "$(sha256sum examples/plugins/rust-napi-plugin/native/addon.node | cut -d ' ' -f1)"
```

The plugin imports `node:fs` and `node:path` from Rust host facades. Its
manifest requests read access to `config.json`, write access to `cache/**`,
and path helpers; host policy grants those same operations. It loads the
allowlisted addon with `require("./native/addon.node")` and awaits an
`AsyncTask` from napi-rs. The Rust host runs load, reload, and unload hooks and
prints their JSON results.

Native addons execute with the desktop process's OS privileges. Keep the
trusted digest in signed or otherwise trusted application metadata. This
example uses Node-API calls supported by napi-vm; it does not establish
compatibility with every Node-API function or with V8, NAN, Node C++, or
libuv-based addons.
