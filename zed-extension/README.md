# Zed language-server extension

This extension locates or downloads the standalone Rust `napi-vm-lsp` executable
and connects Zed's JavaScript editor to it. It does not require a Node process
or resolve a language server through a consumer project's `node_modules`.

## Select the server

The launcher checks a configured binary path, then the worktree's PATH, then
release downloads. Configure an explicit local build in Zed settings:

```json
{
  "lsp": {
    "napi-vm": {
      "binary": { "path": "/absolute/path/to/napi-vm-lsp" }
    }
  }
}
```

Automatic downloads select the platform archive and cache its executable.
For a Linux host needing a musl release rather than the normal glibc archive,
install that archive separately and select its binary path. Build availability
and actual platform compatibility must be checked for the chosen release.

## Develop locally

From the repository root:

```sh
cargo build --release --locked --no-default-features --bin napi-vm-lsp
rustup target add wasm32-wasip2
npm run zed:build
```

Install `zed-extension/` using Zed's development-extension flow and configure the
native executable above, or add `target/release` to PATH. The extension's WASM
build and the native server are different artifacts. JavaScript keeps Zed's
existing language/highlighting; the extension contributes language-server
behavior.

## Add live application metadata

A running application's `VmSession` publishes `.napi-vm/runtime.json` plus a
local socket/named pipe. The language server discovers and authenticates that
session. Without one, static analysis continues. No default `.napi-vm.json`
configuration file is required.

See [editor setup](../docs/editor.md), [metadata protocol](../docs/runtime-protocol.md),
and [server source](../src/lsp/).
