# @napi-vm/plugin-sdk

Define and run trusted JavaScript process plugins under Node or Bun.

Requires Node.js 22 or newer. Package version: `0.1.0`; wire protocol: `1.0`.

Trusted process plugins execute with the launching user’s OS privileges. They are **not sandboxed**. The protocol token authenticates the session; it does not restrict filesystem, network or process access.

```js
import { definePlugin, createHarness } from "@napi-vm/plugin-sdk";
```

Define handlers against a validated contract. `createHarness` runs an in-process test harness; `serve` starts the process endpoint for a host. Lifecycle hooks support initialization, quiescence, state snapshots and shutdown.

See the [trusted plugin documentation](https://github.com/nglmercer/napi-vm/tree/main/docs/trusted-plugins) for contracts, lifecycle and platform limits. Licensed under MIT; see LICENSE.
