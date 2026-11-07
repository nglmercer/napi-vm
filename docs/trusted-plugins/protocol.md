# Protocol v1

Transport: private `127.0.0.1` TCP with four-byte unsigned big-endian byte length followed by UTF8 JSON. Zero, oversized, invalid UTF8/JSON and truncated frames fail the connection. stdout/stderr are logs only. Duplicate JSON keys use last-key-wins parsing before business validation.

Bootstrap environment variables: `NAPI_VM_PLUGIN_ENDPOINT`, `NAPI_VM_PLUGIN_TOKEN`, `NAPI_VM_PLUGIN_INSTANCE_ID`, `NAPI_VM_PLUGIN_SESSION_ID`. Tokens are random per session and never CLI arguments. First request `system.hello` carries token, instanceId, sessionId, pluginId/pluginVersion, protocol major/minMinor/maxMinor, provided and required version/digest identities. Successful negotiation precedes `system.initialize` with configuration/context/snapshot. READY follows successful initialization, not PID creation.

The host validates the successful initialize result and admits readiness before
processing the next frame, including when a reply and event arrive together.
The SDK holds startup events until its initialize reply is queued; required host
service calls remain available during initialization. Held events share the
transport byte budget and are released or discarded on success or disconnect.
An invalid result, retired request or already-closed connection cannot produce a
successful READY load.

Control methods: hello, initialize, invoke, event, cancel, quiesce, snapshot, shutdown and ping under `system.`. Business calls use invoke with interface/version/method/input and context `{timeoutMs,callChain}`. String request IDs are `h:<session>:<counter>` and `p:<session>:<counter>`. Exactly one result/error; null is a real result. No batches or automatic retry. Replies match ID independently of arrival order.

Inputs, results, domain errors and events are validated on both sides. Cancellation is cooperative. A local deadline settles once and sends cancellation; a handler that ignores it retains the endpoint slot. An expired caller is not evidence its action did not occur. Event callbacks are separated from readers and bounded; stale-session delivery is rejected.

Default engineering limits: 8 MiB frame, 64 KiB pre-auth frame, depth 64, 256 pending calls, 16 MiB queued bytes, call chain 16, startup 10 s, calls 30 s, shutdown 5 s, logs 1 MiB. Limits are explicit and negotiated downward. These are reliability bounds, not benchmark-derived support promises.

Errors have stable numeric and string identities defined in both protocol implementations. Domain errors use APPLICATION_ERROR plus domainCode and validated data. No stack trace or bootstrap secret is part of the business error contract.
