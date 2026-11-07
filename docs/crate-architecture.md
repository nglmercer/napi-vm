# Crate architecture and Rust embedding

## Ownership

| Crate or facade | Owns |
| --- | --- |
| `napi-vm-core` | Lexer, parser/AST, bytecode compiler and VM, ECMAScript builtins, values and heap, Promise jobs, module-loader contracts, engine limits and language analysis |
| `napi-vm-runtime` | Explicit capability installation, permissions, event-loop adapters, external events, filesystem/network/npm/Node integrations, TypeScript transformation and CLI implementation |
| `napi-vm` | Engine reexports, Node and WASM bindings, LSP, legacy Rust guest-plugin host and thin CLI entry point |
| `napi-vm-plugin-*` | Independent trusted process protocol, SDK, host and conformance tooling |

Core has no dependency on the runtime crate, Tokio, HTTP transports, npm
extraction or TypeScript tooling. Native-addon contracts and optional trusted
backends are available to embedders, but core initialization does not install
them or grant guest privileges.

The root facade defaults to the `napi` feature. Its runtime feature flags
forward to the optional runtime dependency. Runtime features make code
available; `RuntimeBuilder` and explicit host configuration choose installed
capabilities and permissions. See [optional runtime](optional-runtime.md).

## Minimal engine

```rust
use napi_vm_core::{Interpreter, JsString, Value};

let mut engine = Interpreter::with_builtins();
let result = engine.eval_source("'😀'.length").unwrap();
assert_eq!(result, Value::Number(2.0));

let lone = JsString::from_units(vec![0xd800]);
assert!(lone.to_utf8().is_err());
assert_eq!(lone.units(), &[0xd800]);
```

A bare engine provides JavaScript builtins, not Node's process, filesystem,
network, timers or console. CommonJS and native addons require explicit host
configuration; [native addon embedding](native-addons.md) explains that boundary.

## UTF-16 and host text

`Value::String(JsString)` retains JavaScript UTF-16 code units, including lone
surrogates. In Rust, convert ordinary string constructors with `.into()`.
Use `units()` for lossless transport, `to_utf8()` for strict UTF-8 validation,
and `as_str()` or `Display` only when replacement at a host text boundary is
intentional. `len()` counts code units.

Serde uses text for valid scalar strings and unit arrays for other strings.
Guest `JSON.stringify` implements its own well-formed ECMAScript behavior.
`compile_utf16` and `eval_utf16` accept raw source code units; Node and WASM
script entry points use those paths.

The module loader's identifiers and file-source contracts use UTF-8. A
specifier with unpaired surrogates is rejected rather than aliased through
replacement. Binding module registration likewise rejects non-Unicode host
identifiers/source. UTF-8 module files can express surrogate literals with
JavaScript Unicode escapes.

## Migration and validation

The runtime convenience constructor lives at
`napi_vm_runtime::runtime::with_runtime_builtins()`; the root facade reexports
it through `napi_vm::runtime` when runtime is enabled. Prefer `RuntimeBuilder`
for explicit capability and permission setup.

Core test-only fixtures are excluded from production builds. Runtime tests
live in `crates/napi-vm-runtime/tests`; CI checks extracted packages and minimal
feature configurations. See [development](development.md) for commands,
[UTF-16 extraction history](utf16-extraction-validation.md) for the original
migration record, and [performance](performance.md) for current measurements.
