# napi-vm Runtime Protocol v1

The Runtime Protocol allows a running application to stream live metadata
(host functions, registered modules, observed event shapes) to the
`napi-vm-lsp` language server so the editor can provide rich completions
and hover information without executing guest scripts.

---

## Overview

```
application (Node, Electron, Tauri, …)
    │
    ├─ writes  <workspace>/.napi-vm/runtime.json   (locator)
    │
    └─ listens on:
        • Unix domain socket  (Linux / macOS)
        • Windows named pipe  (Windows)
```

```
napi-vm-lsp
    │
    ├─ polls   <workspace>/.napi-vm/runtime.json
    │
    └─ connects to the socket/pipe and reads newline-delimited JSON
```

The LSP is always the **client**. The application is the **server**.

---

## Discovery

### Locator file

Path: `<workspace>/.napi-vm/runtime.json`

The application writes this file when its session starts and removes it (or
lets it become stale) when the session ends. The native LSP currently polls this file every
250 ms.

### Locator schema

```json
{
  "protocolVersion": 1,
  "workspaceId": "a3f1b9c4e82d67f0ab12",
  "sessionId": "550e8400-e29b-41d4-a716-446655440000",
  "pid": 12345,
  "authToken": "a-256-bit-random-capability-token",
  "transport": {
    "kind": "unix",
    "address": "/tmp/napi-vm-a3f1b9c4.sock"
  }
}
```

| Field             | Type     | Description                                             |
|-------------------|----------|---------------------------------------------------------|
| `protocolVersion` | `number` | Must be `1`. Mismatches are rejected.                   |
| `workspaceId`     | `string` | SHA-256 of the realpath of the workspace root, first 20 hex chars. |
| `sessionId`       | `string` | Unique opaque ID regenerated for every process start.   |
| `pid`             | `number` | OS process ID of the application. Used to detect stale sessions. |
| `authToken`       | `string` | Random capability required in the first client frame. Treat as secret. |
| `transport.kind`  | `string` | `"unix"` or `"named-pipe"`.                             |
| `transport.address` | `string` | Socket path or named-pipe path.                       |

### Workspace ID computation

The application and LSP compute the same workspace ID to identify the workspace.
The ID is not secret; transport access also requires the capability token:

```
sha256(realpath(workspaceRoot)).hex().slice(0, 20)
```

JavaScript (Node / Electron):

```js
const crypto = require("node:crypto");
const fs = require("node:fs");

function workspaceId(root) {
  return crypto
    .createHash("sha256")
    .update(fs.realpathSync(root))
    .digest("hex")
    .slice(0, 20);
}
```

Rust (napi-vm):

```rust
use sha2::{Digest, Sha256};

fn workspace_id(root: &Path) -> std::io::Result<String> {
    let resolved = std::fs::canonicalize(root)?;
    let digest = Sha256::digest(resolved.to_string_lossy().as_bytes());
    Ok(hex_prefix(&digest, 20))
}
```

---

## Transport

### Linux / macOS — Unix domain socket

```json
{ "kind": "unix", "address": "/tmp/napi-vm-<workspaceId>.sock" }
```

The application creates a Unix domain socket with mode `0600`. The LSP
connects, authenticates, and messages flow server→client as newline-delimited
JSON until the socket is closed. The server accepts at most 16 simultaneous
clients and ignores unauthenticated clients when publishing snapshots.

### Windows — Named pipe

```json
{ "kind": "named-pipe", "address": "\\\\.\\pipe\\napi-vm-<workspaceId>" }
```

Same semantics, different transport. The application creates the named pipe;
the LSP opens it for reading. The capability token remains required because
named-pipe ACL behavior varies by host and Node version.

---

## Encoding

All messages are **UTF-8 newline-delimited JSON** (`\n` terminated).
Each line is one complete JSON object. Empty lines are ignored. Frames are
limited to 1 MiB and clients that exceed the limit are disconnected.

The first frame from every client must be an authentication message:

```json
{"type":"auth","token":"<authToken>"}
```

The server sends the first snapshot only after the token matches. Clients
that do not authenticate within five seconds are disconnected. Implementations
should compare capability tokens in constant time and must never log them.

---

## Messages (application → LSP)

### `snapshot`

Delivers a complete view of the current runtime state. The LSP replaces its
entire analysis context with the payload; it does not merge with previous
snapshots.

```json
{
  "type": "snapshot",
  "payload": {
    "functions": [ ... ],
    "modules":   [ ... ],
    "handlers":  [ ... ]
  }
}
```

The application should send a `snapshot` immediately after the LSP connects
and whenever the state changes significantly (e.g., after `exposeFunction`,
`registerModule`, or `observeHandler`).

#### `payload.functions` — host-exposed functions

Array of host functions callable from guest scripts.

```json
[
  {
    "name": "alert",
    "params": [
      { "name": "message", "typeName": "string" }
    ],
    "returns": "void",
    "documentation": "Displays a message in the host UI.",
    "async": false
  }
]
```

| Field           | Type      | Description                                             |
|-----------------|-----------|---------------------------------------------------------|
| `name`          | `string`  | Function name as it appears in the guest script.        |
| `params`        | `array`   | Positional parameter list (may be empty).               |
| `params[].name` | `string`  | Parameter name.                                         |
| `params[].typeName` | `string` | TypeScript-style type annotation (see below).       |
| `returns`       | `string`  | Return type annotation (default `"unknown"`).           |
| `documentation` | `string?` | Optional Markdown documentation string.                 |
| `async`         | `boolean` | Whether the function returns a `Promise`.               |

`typeName` and `returns` are free-form display text, not a closed vocabulary.
Names the server recognises (`string`, `number[]`, `Promise<string>`, …) become
structured types for member completion; anything else (`User`, `Result<User>`,
`string | null`) is rendered verbatim in the hover signature and treated as
`unknown` for inference. A server must never reject a function for naming a type
it does not know.

They are still bounded. A name longer than 256 bytes is replaced with
`"unknown"`, and array/`Promise` wrapper nesting is interpreted only to a depth
of 8 — beyond that the remainder collapses to `unknown`. An entry whose `name`
exceeds 128 bytes, or which declares more than 64 parameters, is skipped.

#### `payload.modules` — registered modules

Array of modules importable from guest scripts.

```json
[
  {
    "name": "utils",
    "source": "export function format(v) { return String(v); }"
  }
]
```

| Field    | Type     | Description                                                  |
|----------|----------|--------------------------------------------------------------|
| `name`   | `string` | Module specifier used in `import … from "name"`.             |
| `source` | `string` | Full source text of the module.                              |

#### `payload.handlers` — runtime event shapes

Array of observed shapes for VM event handler functions. The LSP uses these
to type the first parameter of matching handler functions in guest scripts.

```json
[
  {
    "name": "handleChat",
    "shape": {
      "kind": "object",
      "properties": {
        "platform": { "kind": "string" },
        "data": {
          "kind": "object",
          "properties": {
            "nickname": { "kind": "string" },
            "comment":  { "kind": "string" }
          }
        }
      }
    },
    "lastValue": {
      "platform": "tiktok",
      "data": { "nickname": "Ada", "comment": "hello" }
    }
  }
]
```

| Field       | Type      | Description                                                  |
|-------------|-----------|--------------------------------------------------------------|
| `name`      | `string`  | Handler function name in the guest script.                   |
| `shape`     | `object`  | JSON type schema (see [Shape schema](#shape-schema)).        |
| `lastValue` | `any?`    | Last observed value. Shown in hover to aid debugging.        |

---

## Shape schema

Shapes are recursive JSON objects used to describe the structure of runtime
values.

### Primitive kinds

| `kind`    | Description         |
|-----------|---------------------|
| `string`  | String value        |
| `number`  | Number value        |
| `boolean` | Boolean value       |
| `null`    | Null value          |
| `unknown` | Unknown/untyped     |

### Object kind

```json
{
  "kind": "object",
  "properties": {
    "<key>": { /* nested shape */ }
  }
}
```

### Array kind

```json
{
  "kind": "array",
  "items": { /* element shape */ }
}
```

---

### Resolution order

A name may be declared by more than one layer. The server resolves the most
specific declaration first:

```text
local declaration in the edited file
> runtime generic global      (payload.globals)
> runtime host function       (payload.functions)
> manifest generic global     (--config globals)
> manifest host function      (--config hostFunctions)
> built-in global
```

The live runtime always outranks the static manifest, so a manifest that has
drifted from the running program never masks what the program actually exposes.
Within one source, an explicit declarative shape outranks the legacy host
function form, which lets a server publish a richer shape for a name it also
exposes as a plain function.

---

## Session lifecycle

```
application starts
    │
    ├─ bind socket / pipe
    ├─ write .napi-vm/runtime.json
    │
    │   (LSP polls, detects new session, connects)
    │
    ├─ accept connection
    ├─ receive `{ "type": "auth", "token": "…" }`
    ├─ verify capability
    ├─ send snapshot
    │
    │   (functions / modules / handlers change)
    ├─ send updated snapshot
    │
application stops (or crashes)
    │
    ├─ close socket / pipe
    └─ delete / leave stale .napi-vm/runtime.json
        (LSP detects stale PID and clears its context)
```

---

## Security considerations

- The `workspaceId` field prevents one project's locator from being used by
  another project's LSP instance.
- The `authToken` is a per-session capability. It prevents another local
  process that discovers the socket path from reading runtime metadata.
- Socket and pipe paths are scoped to the session and regenerated on restart.
- The protocol carries module source as well as metadata, so the locator and
  token files must be protected (`.napi-vm` mode `0700`, locator mode `0600`).
- The protocol is local-only; no network listeners are opened. Authentication
  is an additional capability check, not an OS security boundary.

---

## Implementing a server

Any language that can write the locator file and listen on a Unix socket or
Windows named pipe can implement this protocol.

### Node / Electron (`VmSession`)

The included `VmSession` class in `index.js` implements the full server side
for Node.js and Electron applications:

```js
import { VM, VmSession } from "napi-vm";

const vm = new VM();
const session = new VmSession(vm, process.cwd());
session.start();

vm.exposeFunction("alert", (msg) => alert(msg));
session.flush(); // send updated snapshot immediately
```

### Rust / Tauri (future)

A pure-Rust `VmSession` implementation for Tauri and native desktop
applications can implement the same protocol without any Node dependency.
The socket and locator formats are identical.

---

## Version history

| Version | Notes                     |
|---------|---------------------------|
| `1`     | Initial protocol version. |
