# Select runtime capabilities

Runtime setup has three independent steps: compile the implementation, select
the capability, and grant the authority needed by its operations. A Cargo
feature alone grants no filesystem, network, environment, process, or native
access.

## Choose features

These facade features forward to the runtime crate or core as appropriate:

| Feature | Makes available |
| --- | --- |
| `runtime` | Runtime builder, permission policy, console/timer installation, OS event adapter |
| `runtime-cli` | The `napi-vm` executable |
| `runtime-fs` | Permission-checked filesystem operations and file loading |
| `runtime-net` | Network operations and socket transports on native hosts |
| `runtime-web` | Implemented web wrappers and crypto helpers |
| `runtime-npm` | Explicit npm package resolution/install support |
| `runtime-node` | Implemented Node compatibility modules and npm support |
| `runtime-typescript` | Optional TypeScript source transformation |
| `runtime-native-addons` | Native-addon integration, including Node-API hosting |
| `node-api-host` | In-process Node-API backend and runtime-node wiring in the facade |

`napi-vm-core` has no default features. The `napi-vm` facade defaults to `napi`;
`napi-vm-runtime` defaults to `runtime`. Inspect the respective manifests when
selecting a minimal build. Enabling `node-api-host` directly on the core only
selects that core backend; facade feature implications differ.

## Build a Rust runtime

Use `napi-vm-runtime` with its `runtime` feature for this example:

```rust verify
use napi_vm_core::Value;
use napi_vm_runtime::runtime::RuntimeBuilder;

fn main() -> Result<(), napi_vm_core::VmErr> {
    let mut runtime = RuntimeBuilder::new()
        .console()
        .timers()
        .loop_budget(10_000)
        .build()?;
    let result = runtime.eval("6 * 7")?;
    assert!(matches!(result, Value::Number(n) if n == 42.0));
    runtime.run_until_idle()?;
    Ok(())
}
```

`EngineBuilder::build` returns an interpreter; `RuntimeBuilder::build` returns
an OS runtime. Installing timers provides guest functions, while driving the
runtime makes their callbacks progress. See [async execution](async.md).

## Grant only the required authority

`Permissions::new()` denies access by default. Builder methods include
`allow_read(directory)`, `allow_write(directory)`, `allow_net(host, port)`,
`allow_env(name)`, `allow_process()`, and `allow_ffi()`. Filesystem grants are
fallible because paths must be resolved. Network grants specify an exact host
and optionally a port; they are not a general wildcard URL policy.

To enable file operations, compile `runtime-fs` and pass the policy to
`RuntimeBuilder::filesystem(permissions)`. For network operations, compile
`runtime-net` and use `network(permissions)`. `environment()` selects the
environment API; actual variable access still requires `allow_env`.
Capability builder methods merge the supplied policies where implemented.

Filesystem guest functions include `napiVm.readTextFile` and
`napiVm.writeTextFile`; environment access uses `napiVm.env`. Custom loaders
and bridges are trusted host implementations and must enforce their own
policy. See [permissions.rs](../crates/napi-vm-runtime/src/runtime/permissions.rs).

## Configure limits

`RuntimeLimits` includes instruction fuel, call-stack depth, job count, timer
count, external-event capacity, concurrent I/O resources, file bytes, and an
optional elapsed timeout. Pass it through `.limits(limits)`.
The default file limit is 16 MiB; timer, external-event, and I/O capacities
default to 1,024 each. Core execution defaults come from `ExecutionBudget`.
An elapsed deadline is different from OS CPU accounting or a hard process
memory limit.

`external_promise()` returns an ID and guest promise. An external sender can
submit JSON completion data for that ID with backpressure. Keep guest `Value`s
on the owner; send host data across threads.

## Enable globals in a Node host

Build an addon with the required features, for example:

```sh
npx napi build --platform --release -- --features runtime,runtime-web
```

Then call `vm.enableRuntime({ console: true, timers: true, webApis: true })`.
This Node API selects compiled globals; it is not a general permission-grant
interface. Node compatibility covers implemented modules rather than the
complete Node.js platform. Use [the compatibility guide](validation.md) and
the runtime source/tests to decide whether a particular dependency is supported.

For command-line applications and OS isolation flags, continue with [CLI](cli.md).
