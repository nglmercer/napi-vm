# Embed the engine in Rust

Choose a dependency for the surface you need:

| Crate | Role |
| --- | --- |
| `napi-vm-core` | Parser, interpreter, bytecode, builtins, loaders, guest values, and language service; no default features |
| `napi-vm-runtime` | Explicit runtime capabilities, permissions, I/O, and scheduling; defaults to `runtime` |
| `napi-vm` | Facade, VM plugin host, Node/WASM bindings, and executable entry points; defaults to `napi` |

For development in this checkout, a separate consumer can use a path dependency:

```toml
[dependencies]
napi-vm-core = { path = "../node-vm/crates/napi-vm-core" }
```

Adjust the path to your checkout. For published versions, use the version and
crate availability appropriate to your release. Do not infer release status
from a workspace package's presence.

## Create an interpreter

```rust verify
use napi_vm_core::{Value, runtime::EngineBuilder};

fn main() -> Result<(), napi_vm_core::VmErr> {
    let mut vm = EngineBuilder::new()
        .loop_budget(10_000)
        .load_spec("math", "export const double = x => x * 2;")
        .build()?;
    let result = vm.eval_source("import { double } from 'math'; double(21);")?;
    assert!(matches!(result, Value::Number(n) if n == 42.0));
    Ok(())
}
```

`EngineBuilder::build` returns an `Interpreter` with ECMAScript builtins.
It does not install host I/O. The simpler `Interpreter::with_builtins()` is
useful when builder configuration is unnecessary.

Builder options include `loop_budget`, `fuel_budget`, `host_bridge`,
`commonjs_loader`, `load_spec`, and `load_spec_file`. Reading a spec file is an
immediate trusted host filesystem operation; the preloaded module body stays
lazy until import.

## Prepare a repeated program

```rust verify
use napi_vm_core::{Interpreter, Value};

fn main() -> Result<(), napi_vm_core::VmErr> {
    let program = Interpreter::compile("6 * 7")?;
    let mut vm = Interpreter::with_builtins();
    let result = vm.execute(&program)?;
    assert!(matches!(result, Value::Number(n) if n == 42.0));
    Ok(())
}
```

Preparation chooses a verified bytecode path where supported and retains an AST
fallback. Inspect `PreparedProgram::tier()` and `fallback_reason()` instead of
assuming every function uses the same tier. Nested callees can fall back
independently. Registering a JIT backend is an embedding seam; it does not mean
the default build ships a native optimizing compiler.

## Convert application data

Use `value_from_json` and `value_to_json` for a JSON-shaped message boundary.
JavaScript strings are `JsString` values containing UTF-16 code units. Rust
UTF-8 strings cannot represent an isolated surrogate; use the code-unit APIs
when losslessness matters. Formatted display output is not a serialization
format.

Core `Value`, `Interpreter`, and their `Rc`-based graphs belong to one owner.
Do not move guest objects into background workers or retain opaque callback
captures without rooting them. See [ownership and collection](architecture.md#ownership-and-collection).

## Supply host capabilities

Implement `HostBridge` for callbacks and external events, and `ModuleLoader`
or `CommonJsModuleLoader` for source resolution. Supplying a loader is itself a
trusted capability: the host must enforce its read/network/package policy.
See [modules](modules.md) and the implementations under
[module_loader.rs](../crates/napi-vm-core/src/module_loader.rs).

For OS scheduling, use `RuntimeBuilder` from `napi-vm-runtime`. It returns a
`Runtime`, rather than an interpreter. [Runtime setup](runtime.md) explains
feature selection, grants, and event-loop driving.

## Inspect the implementation API

```sh
cargo doc --no-deps -p napi-vm-core -p napi-vm-runtime
cargo test --profile ci --locked -p napi-vm-core
```

The [Rust plugin example](../examples/rust_plugin_host.rs) uses the facade's
`RustPluginHost`. The [native addon example](../examples/rust_native_addon.rs)
adds an explicit native-code allowlist. These are different trust boundaries;
review [safety](safety.md) before choosing one.
