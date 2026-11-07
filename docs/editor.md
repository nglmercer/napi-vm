# Editor integration

## Shared language service

`LanguageService` is the frontend-independent Rust implementation for
completion, hover, diagnostics, import/export analysis, and host-function
metadata:

```javascript
const { LanguageService } = require("./index.js");

const service = new LanguageService();
service.registerHostFunction(
  "alert",
  [{ name: "message", typeName: "string" }],
  "void",
  "Displays a message.",
);
service.open("file:///main.js", 'alert("hello");');
console.log(service.hover("file:///main.js", 3));
```

The browser playground, native LSP server, and future native desktop editors
all call this same service instead of implementing separate inference engines.

## Native LSP server

`napi-vm-lsp` is a standalone native Rust binary that implements the
[Language Server Protocol](https://microsoft.github.io/language-server-protocol/)
over stdio. It does **not** depend on Node.js, `node_modules`, or any
application-level package layout.

### Build

```bash
cargo build --release --no-default-features --bin napi-vm-lsp
```

The binary is at `target/release/napi-vm-lsp`; its platform runtime and shared
library requirements depend on how it was built.

### Capabilities

| LSP method                        | Behaviour                                      |
|-----------------------------------|------------------------------------------------|
| `initialize`                      | Discovers workspace root and connects runtime. |
| `textDocument/didOpen`            | Parses and indexes the document.               |
| `textDocument/didChange`          | Re-parses and publishes fresh diagnostics.     |
| `textDocument/didClose`           | Releases the document and clears diagnostics.  |
| `textDocument/completion`         | Returns context-aware completions.             |
| `textDocument/hover`              | Returns type and documentation on hover.       |
| `textDocument/publishDiagnostics` | Pushed automatically on every change.          |

The server also implements document symbols, definition, references, document
highlight, rename, signature help, inlay hints, semantic tokens, formatting and
code actions. See [`src/lsp/server.rs`](../src/lsp/server.rs) and the
[`lsp_protocol`](../tests/lsp_protocol.rs) and [`lsp_runtime`](../tests/lsp_runtime.rs)
tests for the supported requests and behavior.

### Optional static manifest

Pass `--config` to pre-register host functions and modules without a running
application:

```bash
napi-vm-lsp --config ./napi-vm-manifest.json
```

Manifest format:

```json
{
  "hostFunctions": [
    {
      "name": "alert",
      "params": [{ "name": "message", "typeName": "string" }],
      "returns": "void",
      "documentation": "Displays a message."
    }
  ],
  "modules": [
    { "name": "utils", "path": "./src/utils.js" }
  ]
}
```

## Live VM metadata via Runtime Protocol

`VmSession` is an optional local runtime bridge. When active, the LSP
receives live host-function registrations, module sources, and observed event
shapes through the [Runtime Protocol v1](./runtime-protocol.md).

The application writes a temporary `.napi-vm/runtime.json` locator and streams
the actual metadata through a local Unix socket (Linux/macOS) or Windows named
pipe. The locator is ignored by Git, regenerated for every process restart,
and removed when the session stops.

The hot-reload example enables this bridge explicitly:

```bash
NAPI_VM_SESSION=1 bun examples/hotreload.ts
```

Without `NAPI_VM_SESSION=1`, the example runs entirely in-process and creates
no runtime locator. The LSP reconnects automatically when the session restarts
and rebuilds its analysis context when functions or modules change.

## Zed

The `zed-extension/` directory contains a Zed extension that downloads
`napi-vm-lsp` from GitHub Releases and launches it as a native process:

```
Zed → napi-vm-lsp (native binary) → LanguageService (Rust core)
```

There is **no dependency on Node.js** or `node_modules` in the editor path.
The binary works regardless of how the consumer application is packaged
(Electron, Tauri, ASAR, Bun compiled, etc.).

The extension resolves the binary in this order: an explicitly configured path
(`lsp.napi-vm.binary.path`), then `$PATH`, then the platform archive from the
latest GitHub release. For local development, build the binary and put it on
`$PATH`:

```bash
cargo build --release --no-default-features --bin napi-vm-lsp
export PATH="$PWD/target/release:$PATH"
```

Build the extension WASM:

```bash
npm run zed:build
```

### Linux binary compatibility

The release workflow builds two Linux variants for x64 and arm64: a glibc build
(`napi-vm-lsp-linux-<arch>.tar.gz`) and a statically linked musl build
(`napi-vm-lsp-linux-<arch>-musl.tar.gz`). The extension downloads the glibc
build; on a distribution older than the CI image, that can fail with
`GLIBC_x.xx not found`. Download the musl archive instead and point
`lsp.napi-vm.binary.path` at it — it is built with static musl linking.

Install the `zed-extension/` directory using Zed's local development-extension
flow. If a `VmSession` is running in the workspace, the editor automatically
receives live host functions, module metadata, observed JSON event shapes, and
the latest bounded event value. When a host calls
`session.observeHandler("handleChat", event)`, the editor can complete nested
properties such as `event.data.nickname` and show the latest payload when
hovering the handler parameter, without executing the guest script.

## IPC-style VM commands

`examples/lib/vm-ipc.ts` provides a small in-process command/event API:

```typescript
const ipc = new VmIpc();

ipc.handle("system.ping", (payload) => ({ ok: true, payload }), {
  params: [{ name: "payload", typeName: "unknown" }],
  returns: "object",
  documentation: "Round-trip test command.",
});

ipc.on("test:response", (payload) => console.log(payload));
ipc.attach(vm, session);

vm.run('ipc.send("test:response", ipc.invoke("system.ping", { ok: true }));');
```

Use `handleAsync` with `ipc.invokeAsync` and `vm.runAsync` for asynchronous
commands. Commands and host listeners survive VM replacement; the VM-side
wrapper is recreated on every reload.

Run the focused test with:

```bash
npm run ipc:smoke
```

MCP is not required for this local editor connection. An MCP adapter can be
built later on top of the same runtime snapshot if AI tooling needs access.
