# Host trusted plugins

The JavaScript host launches JavaScript entries using a selected Node or Bun
runtime. The Rust host supports both JavaScript entries and target-compatible
native executables. Normal loading validates and launches built artifacts; it
does not install dependencies or compile source.

## Load a JavaScript plugin

Build and package the checkout fixtures with `npm run plugins:build` followed by
`npm run plugins:test:interop` (the full fixture workflow needs Node, Bun, and
Rust). The JavaScript greeter is then at `artifacts/trusted-plugins/greeter-ts`.
For your own artifact, set `PLUGIN_DIRECTORY` to its packaged directory instead.
The greeter requires a configuration service, registered before load:

```javascript verify
const { TrustedPluginHost, readManifest } = await import('@napi-vm/plugin-host');
const resolved = await readManifest(
  process.env.PLUGIN_DIRECTORY ?? 'artifacts/trusted-plugins/greeter-ts');
const greeter = resolved.contracts.find(c => c.descriptor.id === 'example.greeter');
const configuration = resolved.contracts.find(c => c.descriptor.id === 'app.configuration');
const assert = require('node:assert/strict');
assert(greeter && configuration);
const host = new TrustedPluginHost({ contracts: [greeter] });
host.registerService(configuration, { get: async () => ({ value: 'Hola' }) });
try {
  const handle = await host.load(resolved.path, { runtime: 'node' });
  const result = await handle.invoke(greeter, 'greet', { name: 'Ana' });
  assert.equal(result.message, 'Hola, Ana');
} finally {
  await host.shutdown();
}
```

Registered service versions/digests must match the plugin's requirements.
Load options can supply configuration/context data, runtime selection/paths,
environment additions, injected services, and an instance ID. Treat service
URLs/headers and child environments as sensitive application configuration.

## Work with handles

`invoke(contract, method, input, options?)` validates request and response data.
`client(contract)` creates a method-oriented client; generated clients add
static input/output types. `subscribe` returns an unsubscribe function.
`emit` publishes a host event to the instance. `get` and `list` inspect loaded
instances without conflating plugin package IDs with per-load instance IDs.

Request options include deadlines/cancellation and the causal call context.
Propagate an existing context when a handler invokes another interface so
deadline budgets and reentrancy checks remain meaningful. The host rejects
causal calls back into an already active endpoint; applications must not
depend on unbounded recursive host/plugin invocation.

## Runtime selection and native execution

JavaScript manifests declare supported runtimes, an optional preference, and
optional version constraints. Host overrides and explicit executable paths are
validated against that declaration. Development source execution is selected
explicitly; production launch uses the built entry.

The Rust `Host` provides async load/invoke/reload/unload/shutdown methods over
the same contracts. The checkout's `trusted-host-rust` executable can be built
with `cargo build -p trusted-host-rust`. CLI operations on native plugins require
`--host-executable` pointing to that Rust host; the JavaScript host does not
load native executable manifests.

## Reload and shutdown

The lifecycle moves through `DISCOVERED`, `STARTING`, `READY`, `DRAINING`,
`STOPPING`, and `STOPPED`, with `FAILED` representing errors. Successful startup
includes authentication, protocol/contract agreement, initialization, and ready
transition. Receiving a TCP connection alone does not make the instance ready.

Reload drains active calls/tracked work, quiesces the old plugin, validates its
snapshot, and initializes a replacement. Use the newly returned handle and
handle compatibility or initialization failures explicitly.
`forceWithoutState` is an explicit escape hatch for supported recovery paths;
do not silently discard incompatible state during ordinary reload.

Always call `host.shutdown()` in `finally`, including startup failure paths.
Timeouts, disconnects, or process exits reject pending calls. Bounded log
retention is diagnostic data, not a reliable audit log of every application
effect.

## Integrity and authority

Production host loading requires `plugin.lock.json`. It checks identity, contract
inventory, artifact metadata, and listed file digests. `readManifest` can inspect
a source tree without a lock, but `host.load` rejects that tree unless explicit
development mode is selected. Neither mode proves a signed release.
Package inventory hashes detect changes relative to that inventory, not who
authorized the package. See [packaging](packaging.md).

Choose trust and OS containment outside the RPC protocol. Managed external
services require explicit lifecycle configuration; an injected service is
application authority. For untrusted JavaScript governed by VM capabilities,
use [the VM host](../plugins.md).
