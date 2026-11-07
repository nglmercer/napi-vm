# Host plugins inside VMs

The VM plugin host gives each plugin its own guest VM, installs selected
capability modules, and calls a small lifecycle API. This is separate from
[trusted process plugins](trusted-plugins/README.md), which execute as normal
Node, Bun, or native applications.

## Plugin layout

A plugin directory contains `plugin.json` and an entry module:

```json
{
  "name": "example-plugin",
  "version": "1.0.0",
  "apiVersion": 1,
  "entry": "./plugin.js",
  "permissions": { "path": true }
}
```

The entry exports an object, or a class the host can instantiate:

```javascript
export default {
  onLoad(context) {
    return { name: context.name, count: 0 };
  },
  onUnload(context) {
    return { count: 1 };
  },
  onReload(context, previousState) {
    return previousState ?? { count: 0 };
  }
};
```

`onLoad` receives the plugin name/version. `onUnload` also receives a reason
(`unload` or `reload`) and can return transferable state. `onReload` receives
that previous state; absent a reload hook, the host uses the load hook.
Guest lifecycle wrappers use `Vm.callFunction`, so results cross the structured
conversion boundary rather than a generated source-evaluation envelope.

## TypeScript host

The `plugins/index.ts` entry exports portable platform and capability
interfaces. Node/Bun-specific filesystem and native adapters live in
`plugins/node.ts`. Run this checkout example with Bun after building the addon:

```typescript
import { PluginHost } from './plugins/index';
import { nodePlatform } from './plugins/node';

const host = new PluginHost({
  platform: nodePlatform(),
  policy: { capabilities: {} }
});
const loaded = host.load('./examples/plugins/example-plugin');
host.reload(loaded.manifest.name);
host.unload(loaded.manifest.name);
```

An empty dynamic policy still permits the TypeScript host's manifest-scoped
filesystem substrate and requested path helpers. This fixture therefore works
with its declared file/path permissions. A portable host supplies its own
`HostPlatform`; the default missing-filesystem backend does not read disk.

## Requests, grants, and runtime controls

Dynamic capability requests are checked against host grants, their definition's
`allows` rule, and capability kill switches. Unknown permission names fail
validation unless registered. Definitions own request validation, resolution,
installation, and teardown.

The TypeScript host treats `fs` as an always-installed substrate: manifest
read/write patterns and unconditional plugin-root confinement control calls.
It does not intersect those patterns with a separate filesystem policy grant.
Its `path: true` rule is also manifest-only because path helpers do not perform
I/O. Review/approve those manifest requests or inject a more restrictive
filesystem backend before loading an untrusted package. Crypto, timers, fetch,
and custom dynamic capabilities follow their registered grant rules.

The Rust host has separate host `fs_read`, `fs_write`, and `path` grants, which
intersect with manifest requests. Do not assume the two host policies are
identical. Native host modules execute with host privilege in either integration.

For trusted operator extensions, use `defineCapability`; use
`setCapabilityEnabled` to disable/re-enable supported capability installations.
Review [capability-registry.ts](../plugins/capabilities/capability-registry.ts)
and [plugin-host.ts](../plugins/core/plugin-host.ts) when adding a definition.

## Rust host

`napi_vm::RustPluginHost` provides analogous manifest, policy, VM, and lifecycle
ownership without a Node host. Try the complete fixture:

```sh
cargo run --no-default-features --example rust-plugin-host -- examples/plugins/example-plugin
```

`RustPluginPolicy` includes `grant_fs_read`, `grant_fs_write`, `grant_path`,
and named capability `grant`/`deny` methods. `RustPluginCapability::export`
registers trusted Rust callbacks. Pin guest values captured by those callbacks
with `heap::RootPin`; closure captures are opaque to the collector.

Use `call_json` for direct JSON requests where appropriate. Native addon support
requires additional backend selection and digest policy; see
[the Rust napi-rs fixture](../examples/plugins/rust-napi-plugin/README.md).

## Loading packages and handling reloads

The TypeScript guest-package layer resolves package metadata and source through
injected filesystem/compiler interfaces. It evaluates guest source in the VM;
it does not authorize arbitrary host `require`. Unsupported syntax and package
assumptions need explicit handling. See [module loading](modules.md).

Treat reload as lifecycle replacement with explicit state transfer. Hold onto
the new loaded handle returned by reload, inspect failures, and release plugin
resources on unload. A successful export-table update does not undo filesystem,
network, or other host effects performed by an earlier lifecycle hook.
