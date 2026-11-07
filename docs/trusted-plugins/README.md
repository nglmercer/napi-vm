# Trusted process plugins

Run a trusted plugin as a separate Node, Bun, or native executable, and communicate
with it through validated asynchronous contracts. These packages are independent
of the JavaScript VM and do not require embedding the engine.

The plugin is an ordinary process with the launching account's OS authority.
The protocol supplies identity checks, bounded messages, deadlines, and lifecycle
coordination; it does not sandbox plugin implementation code.

## Choose your path

| Task | Guide |
| --- | --- |
| Create and test a JavaScript, TypeScript, or Rust plugin | [Authoring](authoring.md) |
| Launch plugins and provide host services | [Hosting](hosting.md) |
| Define interfaces and shared data | [Contracts](contracts.md) |
| Build and distribute relocatable artifacts | [Packaging](packaging.md) |
| Implement or inspect the wire protocol | [Protocol](protocol.md) |
| Run interoperability and package checks | [Testing](testing.md) |

## Components

| JavaScript package | Rust crate | Role |
| --- | --- | --- |
| `@napi-vm/plugin-protocol` | `napi-vm-plugin-protocol` | Wire values, contract validation, errors, framing, RPC |
| `@napi-vm/plugin-sdk` | `napi-vm-plugin-sdk` | Handlers, call contexts, lifecycle, event subscriptions, test harness |
| `@napi-vm/plugin-host` | `napi-vm-plugin-host` | Discovery, launch, authentication, calls, reload, shutdown |
| `@napi-vm/plugin-codegen` | Generated Rust adapters | Deterministic contract JSON, types, clients, and references |
| `@napi-vm/plugin-cli` | Native host executable integration | Scaffolding, generation, explicit build/pack, development tools |

The current protocol is 1.0; manifests use version 2 and
`execution: "trusted-process"`. This format is different from a VM plugin's
`apiVersion: 1` manifest. Interface versions and digests must match exactly;
there is no automatic method-version negotiation or retry.

## Run the checkout examples

```sh
npm ci --ignore-scripts
npm run plugins:generate
npm run plugins:build
npm run plugins:test:interop
```

Install Node, Bun, and the Rust toolchain for the full example matrix. The
greeter examples call an `app.configuration` host service; the counter examples
exercise state transfer. Generated bindings and contract artifacts are committed
so a plugin author does not need to invent wire objects manually.

For untrusted guest JavaScript evaluated under VM capabilities, use
[VM plugins](../plugins.md) instead.
