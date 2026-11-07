# @napi-vm/plugin-host

Launch built trusted JavaScript plugins under a declared Node or Bun runtime,
provide contract-validated services, and own calls, events, reload, and teardown.
For native executable plugins, use the Rust host instead.

## Load an artifact

```javascript
import { TrustedPluginHost, readManifest } from '@napi-vm/plugin-host';
const resolved = await readManifest('/path/to/plugin');
const contract = resolved.contracts.find(c => c.descriptor.id === 'your.interface');
if (!contract) throw new Error('Required contract artifact missing');
const host = new TrustedPluginHost({ contracts: [contract] });
try {
  // Register any services named in requiresHost before load().
  const plugin = await host.load(resolved.path, { runtime: 'node' });
  const client = plugin.client(contract);
  // Invoke a method declared by your contract through client or plugin.invoke().
} finally {
  await host.shutdown();
}
```

`registerService` installs host handlers. `load` options can supply configuration,
context, runtime paths, environment additions, services, and an instance ID.
Handles provide `invoke`, `client`, `subscribe`, `emit`, `reload`, and `unload`.
Use the new handle returned by reload.

Production loading requires an inventory lock and validates the
manifest/contract agreement and recorded bytes. It does not compile or install
dependencies. `readManifest` can inspect a source tree without a lock;
`host.load` requires explicit development mode for that tree. Inventory hashes
do not establish publisher authenticity.

Each child is an ordinary trusted OS process. Bound RPC work and select OS
containment separately. Cancellation is cooperative; neither a rejected call
nor a killed child rolls back completed side effects.

## Develop

From the repository root, run `npm run plugins:build:ts`, then
`npm run plugins:test:node`. Use `npm run plugins:check` for the complete
cross-language/package gate with its Node/Bun/Rust prerequisites.

[Hosting guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/hosting.md) ·
[Packaging guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/packaging.md)
