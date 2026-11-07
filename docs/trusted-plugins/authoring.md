# Author a trusted plugin

Use the CLI to scaffold an independent project, keep application logic separate
from the process entry point, and test the contract before packaging.

## Create a project

From a checkout with workspace dependencies installed:

```sh
node packages/plugin-cli/src/index.mjs create artifacts/my-plugin --language ts
```

Choose `ts`, `js`, or `rust`. For an installed CLI the command is
`napi-vm-plugin create DIRECTORY --language LANGUAGE`.
The scaffold includes a manifest, a greeter contract and generated adapter,
business logic, a process entry point, and tests. JavaScript/TypeScript projects
need their package dependencies installed; Rust scaffolds include independent
licensed SDK source copies and use Cargo.

## Implement a contract handler

The generated TypeScript adapter exposes `register(handlers, hooks?)`. Its
handler signature uses generated input/output types and a `CallContext`:

```typescript
import { register } from './generated/greeter.js';

export const plugin = register({
  async greet({ name }, context) {
    context.throwIfCancelled();
    return { message: `Hola, ${name}` };
  }
});
```

The process entry calls `serve(plugin, { metadata })`. Keep that call out of the
business-logic module so unit tests can import the implementation without
connecting to a host. Rust adapters expose a `Handler` trait, a `register`
function, and contract helpers; the scaffold's main function runs the SDK's
process session. See the generated files and [Rust scaffold](../../packages/plugin-cli/templates/rust/).

## Test without a child process

After `npm run plugins:build:ts`, this checkout example executes a validated
handler using the SDK harness:

```javascript verify
const { definePlugin, createHarness } = await import('@napi-vm/plugin-sdk');
const { readFile } = require('node:fs/promises');
const contract = JSON.parse(await readFile(
  'contracts/trusted-plugins/generated/greeter.contract.json', 'utf8'));
const plugin = definePlugin(contract, {
  greet: async ({ name }) => ({ message: `Hola, ${name}` })
});
const harness = await createHarness(plugin);
try {
  const result = await harness.invoke(contract, 'greet', { name: 'Ana' });
  require('node:assert/strict').equal(result.message, 'Hola, Ana');
} finally {
  await harness.shutdown();
}
```

The harness checks contracts, initialization, calls, events, and lifecycle.
It does not measure or prove TCP transport, process startup, runtime discovery,
artifact relocation, or native ABI behavior. Add artifact and interoperability
checks for those boundaries.

## Use the call context

JavaScript contexts provide `signal`, `remainingMs`, `throwIfCancelled`,
`invoke`/`client` for host services, `emit`, `subscribe`, `log`, and `spawnTask`.
Use the context's deadline and tracked tasks rather than unbounded detached
work. Rust has analogous `CallContext`, cancellation, and tracked resources.

Throw declared domain errors through protocol/generated helpers. A method's
declared domain code and data schema are validated; arbitrary exception text is
not a substitute for a stable application error contract. Cancellation is
cooperative and does not undo an already completed external effect.

## Implement state and teardown

Lifecycle hooks include initialize, quiesce, snapshot, and shutdown. Snapshot
data uses `{ stateVersion, contract, data }` and must match the declared state
schema. Protocol v1 supports one state contract per plugin. Stateless plugins
return no snapshot. Track background resources so quiescing can finish before
state is captured.

Shut down listeners, tasks, connections, and other application resources.
Neither SDK shutdown nor a process kill can roll back external writes. See
[hosting lifecycle](hosting.md#reload-and-shutdown) for replacement behavior.
