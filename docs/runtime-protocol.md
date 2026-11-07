# Live metadata protocol

This protocol connects the editor's language server to a running application.
It carries analysis metadata, not guest execution requests. It is independent
of the [trusted plugin RPC protocol](trusted-plugins/protocol.md).

## Discover a session

The application writes `.napi-vm/runtime.json` under the workspace. Version 1
locators contain:

| Field | Meaning |
| --- | --- |
| `protocolVersion` | `1` |
| `workspaceId` | First 20 hex characters of SHA-256 over the canonical workspace path |
| `sessionId` | Identity of this application session |
| `pid` | Publisher process ID |
| `authToken` | Per-session authentication capability |
| `transport.kind` | `unix` or `named-pipe` |
| `transport.address` | Unix socket path or Windows named pipe |

The publisher replaces the locator atomically. Consumers check the protocol
version/workspace identity and reconnect when the session changes. The Rust
client polls for locators at 250 ms intervals and backs off after failed
connections. Treat the token and locator as private application metadata.

## Authenticate and frame messages

Messages are UTF-8 JSON followed by a newline. The first client message is:

```json
{ "type": "auth", "token": "the-token-from-the-locator" }
```

The server closes unauthenticated/incorrect sessions. After authentication it
sends a snapshot. A client may request `{ "type": "snapshot" }` or send
`{ "type": "ping" }`, which receives a `pong` response. Metadata updates are
published as snapshots; clients should replace their previous snapshot rather
than merge arbitrary generations themselves.

Frames are bounded to 1 MiB by the current publisher/client implementations.
Socket buffering and snapshot publication are bounded/coalesced; a stalled
consumer must not retain an unlimited history. Authentication does not remove
the need for framing, shape, depth, and size validation.

## Read a snapshot

Snapshot messages have `type`, optional `reason`, and `payload`. The payload
includes `protocolVersion`, `sessionId`, `workspaceId`, `pid`, `generation`,
`startedAt`, and the `functions`, `globals`, `modules`, and `handlers` arrays.
Handler entries carry a shape and a bounded `lastValue` observation.

The editor validates metadata before installing it in its language service.
Session replacement/disconnection clears live metadata rather than retaining
symbols belonging to a previous runtime. An observed shape is evidence about
sample values; it is not a static proof of every value a handler may produce.

The wire owners are [protocol.rs](../src/lsp/protocol.rs),
[runtime_client.rs](../src/lsp/runtime_client.rs), and
[session.cjs](../runtime/session.cjs). The public publisher types are in
[session.d.cts](../runtime/session.d.cts).
