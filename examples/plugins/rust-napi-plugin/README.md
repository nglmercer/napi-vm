# Rust VM plugin with a napi-rs addon

This fixture hosts guest JavaScript in Rust and exposes a native Rust addon
through the in-process Node-API implementation. It does not start or embed
Node.js. The addon remains trusted native code with the host's OS authority.

## Run the full workflow

From the repository root on Linux/macOS or a supported Bash environment:

```sh
examples/plugins/rust-napi-plugin/build-and-run.sh
```

On Windows PowerShell:

```powershell
examples/plugins/rust-napi-plugin/build-and-run.ps1
```

The scripts build the native crate, copy its platform library to the package's
`.node` entry, calculate its digest for the local fixture, and run the Rust
plugin host. The script is a demonstration build workflow; production digests
should come from trusted application metadata.

## Understand the pieces

| Path | Role |
| --- | --- |
| `native/` | Independent napi-rs crate producing a cdylib |
| `plugin.json` | VM plugin manifest and requested permissions |
| `node_modules/rust-napi-plugin.node/` | Fixture package metadata and copied addon entry |
| `config.json` | Permission-checked plugin input |
| `build-and-run.sh` / `.ps1` | Platform-specific build/copy/host commands |

The package's `node-addons` export resolves to `build/Release/addon.node`.
The Rust host grants the fixture's file/path operations and allowlists the
selected addon/digest. The guest uses Rust `node:fs`/`node:path` facades and
awaits a napi-rs async task. Load, reload, and unload results demonstrate lifecycle.

To build only the addon:

```sh
cargo build --release --manifest-path examples/plugins/rust-napi-plugin/native/Cargo.toml
```

For manual copying, use the generated `.so` on Linux, `.dylib` on macOS, or
`.dll` on Windows as the `.node` file. The complete scripts show the exact
platform path and host arguments; do not load a binary for another target.

This exercises supported Node-API behavior. It does not establish every
Node-API function, V8/NAN/Node C++/libuv compatibility, or an unprivileged native
sandbox. See [native integration](../../../docs/modules.md#native-addons) and
[VM plugin policies](../../../docs/plugins.md).
