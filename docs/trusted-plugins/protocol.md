# Trusted plugin protocol 1.0

Hosts and plugins exchange asynchronous JSON-RPC-style requests, responses,
and notifications over an authenticated loopback TCP session. stdout/stderr are
logs and CLI output channels, not the RPC transport.

## Frames and values

Each frame is a four-byte unsigned big-endian payload length followed by that
many UTF-8 JSON bytes. Zero-length, oversized, malformed UTF-8/JSON, excessive
depth, and invalid envelopes are rejected. Incremental decoders validate the
length before allocating the payload. Data obeys the
[contract wire-value rules](contracts.md#respect-the-data-boundary).

Requests have an ID, method, and params; responses carry a result or structured
error associated with the pending ID. Peers enforce unique/valid request and
response progression. Unknown/stale responses and invalid transitions do not
become successful application results.

## Start a session

The host launches the selected entry with session connection information and a
one-session authentication capability. The plugin sends `system.hello` with
token, instance/session/plugin identity, supported protocol range, provided
interfaces, required host interfaces, and extensions.

The host checks these declarations against its manifest/contracts and selects
protocol 1.0, matching interfaces, limits, and supported extensions. Initialization
then supplies configuration, context, and an optional validated snapshot.
Initialization-time calls/events follow the peer's startup ordering rules;
ordinary invocation requires the ready transition. Peers must reject duplicate
or out-of-order lifecycle operations.

Current extensions are empty. Interface identity uses exact version and digest;
the protocol range does not negotiate alternate method signatures.

## Calls, deadlines, and errors

Invocation identifies an interface, method, validated input, and causal request
context. Context carries the call chain and remaining budget. Descendant calls
must propagate that budget/context, and causal reentrancy is rejected rather
than deadlocking an active endpoint.

Cancellation is cooperative. Deadline expiry rejects local waiting and signals
cancellation; it is not a guarantee that remote side effects stopped or were
reversed. No automatic retry follows from `idempotent` metadata.

Protocol errors use stable symbolic codes with numeric wire codes. Examples
include `INVALID_ARGUMENT`, `INVALID_RESULT`, `NOT_READY`, `CONTRACT_MISMATCH`,
`DEADLINE_EXCEEDED`, `CANCELLED`, `OVERLOADED`, `PLUGIN_EXITED`,
`CONNECTION_CLOSED`, `REENTRANT_CALL`, and `STATE_INCOMPATIBLE`. Application
errors use a declared domain code and schema-validated data. Message text is
bounded diagnostic text; use codes/data for application decisions.

## Queues, events, and lifecycle

Peers bound frames, pending calls, queued bytes, call-chain length, logs, and
subscriber queues. The JavaScript defaults include an 8-MiB frame limit,
16-MiB queued-byte limit, depth 64, 256 pending calls/events, 30-second calls,
10-second startup, and 5-second shutdown. Negotiation tightens the two peers'
limits rather than increasing the smaller one.

Events are contract-validated, session-scoped, and sequence-checked. Each
subscriber has bounded delivery; overload or listener failure is isolated by
the implemented subscription policy. Enqueue byte accounting uses the original
size even if another listener mutates a shared in-process payload.

Quiesce stops admission and waits for tracked work. Snapshot validates the
declared state contract. Shutdown drains/cleans up within a deadline and closes
the peer. Disconnect and child exit reject pending work and trigger cleanup.

## Source of truth

The [protocol schema](../../contracts/trusted-plugins/protocol.schema.json),
[JavaScript protocol implementation](../../packages/plugin-protocol/src/index.ts),
and [Rust protocol crate](../../crates/napi-vm-plugin-protocol/src/) define wire
details. Run the cross-language tests before changing any representation. This
protocol is independent of the editor's newline-delimited
[live metadata channel](../runtime-protocol.md).
