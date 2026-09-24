# Rust N-API plugin example

This example keeps the plugin host in Rust and implements one native module
with Rust and napi-rs. It does not start or embed Node.js.

From the repository root, build the native crate:

```bash
cargo build --release --manifest-path examples/plugins/rust-napi-plugin/native/Cargo.toml
```

Copy the platform cdylib to the `.node` path used by the guest and calculate
its SHA-256 digest:

| Build host | cdylib | Copy and digest |
| --- | --- | --- |
| Linux | `native/target/release/librust_napi_plugin_native.so` | `cp examples/plugins/rust-napi-plugin/native/target/release/librust_napi_plugin_native.so examples/plugins/rust-napi-plugin/native/addon.node` then `sha256sum examples/plugins/rust-napi-plugin/native/addon.node` |
| macOS | `native/target/release/librust_napi_plugin_native.dylib` | `cp examples/plugins/rust-napi-plugin/native/target/release/librust_napi_plugin_native.dylib examples/plugins/rust-napi-plugin/native/addon.node` then `shasum -a 256 examples/plugins/rust-napi-plugin/native/addon.node` |
| Windows PowerShell | `native/target/release/rust_napi_plugin_native.dll` | `Copy-Item examples/plugins/rust-napi-plugin/native/target/release/rust_napi_plugin_native.dll examples/plugins/rust-napi-plugin/native/addon.node` then `(Get-FileHash examples/plugins/rust-napi-plugin/native/addon.node -Algorithm SHA256).Hash.ToLowerInvariant()` |

Pass the digest as the final argument to the Rust host:

```bash
cargo run --no-default-features --features node-api-host --example rust-plugin-napi -- \
  examples/plugins/rust-napi-plugin \
  "$(sha256sum examples/plugins/rust-napi-plugin/native/addon.node | cut -d ' ' -f1)"
```

The host example reads the plugin name from `plugin.json`. An optional second
path argument selects the addon; a relative addon path is resolved from the
plugin directory, while an absolute path is accepted for build layouts that
keep artifacts elsewhere. The supplied digest must match that selected file.

On Windows PowerShell, store the digest and pass it as the last argument:

```powershell
$digest = (Get-FileHash examples/plugins/rust-napi-plugin/native/addon.node -Algorithm SHA256).Hash.ToLowerInvariant()
cargo run --no-default-features --features node-api-host --example rust-plugin-napi -- examples/plugins/rust-napi-plugin $digest
```

The plugin imports `node:fs` and `node:path` from Rust host facades. Its
manifest requests read access to `config.json`, write access to `cache/**`,
and path helpers; host policy grants those same operations. It loads the
allowlisted addon with `require("./native/addon.node")` and awaits an
`AsyncTask` from napi-rs. The Rust host runs load, reload, and unload hooks and
prints their JSON results.

Use the Rust facades for ordinary file and path operations. Add a napi-rs
module when a plugin needs native code or an existing package already ships a
Node-API addon; those native calls still require the host's digest allowlist.

Native addons execute with the desktop process's OS privileges. Keep the
trusted digest in signed or otherwise trusted application metadata. This
example uses Node-API calls supported by napi-vm; it does not establish
compatibility with every Node-API function or with V8, NAN, Node C++, or
libuv-based addons. The example workflow has been run on Linux; native macOS,
Windows GNU, and Windows MSVC execution still require platform CI verification.
