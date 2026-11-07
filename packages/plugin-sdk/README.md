# @napi-vm/plugin-sdk

Author trusted JavaScript/TypeScript plugins that run in Node or Bun and expose
validated asynchronous contracts. Importing the SDK does not launch a child
process. The process entry calls `serve`; business logic can be tested separately.

## Define handlers

Generate a contract adapter, then implement its typed handler interface:

```typescript
import { register } from './generated/greeter.js';
export const plugin = register({
  async greet({ name }, context) {
    context.throwIfCancelled();
    return { message: `Hola, ${name}` };
  }
});
```

In the entry module, call `serve(plugin, { metadata })` using metadata consistent
with `plugin.json`. `definePlugin` is the lower-level contract/handler API;
`PluginRegistry` groups multiple interfaces.

## Test and manage lifecycle

`createHarness(plugin, options)` runs validated calls and lifecycle in-process.
Use `harness.invoke`/`client`, `emitHostEvent`, `snapshot`, and `shutdown` to
exercise the application. Always shut down in `finally`. Harness success does
not prove child transport, artifact relocation, or runtime discovery.

Call contexts expose cancellation/deadline information, host service invocation,
events, subscriptions, logging, and tracked `spawnTask` work. Lifecycle hooks
include initialize, quiesce, snapshot, and shutdown. Track background work before
quiescing; deadline expiry cannot undo external effects already completed.

Subscriber queues retain their enqueue byte sizes. Listeners should still treat
payloads as application data and avoid relying on cross-listener mutation.
Queue count/byte limits and listener failures are handled independently.

## Develop

```sh
npm run plugins:build:ts
npm run plugins:test:node
npm run plugins:test:bun
```

Run these at the repository root. Plugins have the launching account's OS
privileges; RPC authentication is not a sandbox.

[Authoring guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/authoring.md) ·
[Testing guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/testing.md)
