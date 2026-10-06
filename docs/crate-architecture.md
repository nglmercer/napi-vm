# Crate ownership and embedding

`napi-vm-core` owns the lexer, parser/AST, bytecode compiler and VM, ECMAScript
builtins, guest heap, Promise jobs, module-loader contracts and engine limits.
It has no dependency on the runtime crate, Tokio, HTTP/WebSocket transports,
npm extraction or TypeScript tooling. A new interpreter exposes no filesystem,
network, environment, console, timers or Buffer globals. Native-addon backend
implementations remain optional trusted host integrations; core initialization
never enables them or loads an addon. Explicit loader contracts remain available
to embedders.

`napi-vm-runtime` depends on core and owns capability installation, event-loop
adapters, external events, permission checks, OS transports, npm/Node loaders,
TypeScript transformation, console, timers, Buffer, Web globals and CLI
implementation (including the process-isolation supervisor). Its Cargo
features select implementations; `RuntimeBuilder` selects installed capabilities
and permissions. Compiling a feature grants no guest access by itself.

`napi-vm` reexports the engine for existing Rust consumers and owns N-API, wasm,
LSP, trusted plugin host and a thin CLI entry adapter. Its default feature remains `napi`.
Runtime features forward to the optional runtime dependency.

```rust
use napi_vm_core::{Interpreter, JsString, Value};
let mut engine = Interpreter::with_builtins();
let result = engine.eval_source("'😀'.length").unwrap();
assert_eq!(result, Value::Number(2.0));
let lone = JsString::from_units(vec![0xd800]);
assert!(lone.to_utf8().is_err());
assert_eq!(lone.units(), &[0xd800]);
```

Engine values now use `Value::String(JsString)`; migrate Rust string constructors
with `.into()`. Use `units()` for lossless host transport, `to_utf8()` for strict
UTF-8 and `as_str()`/`Display` only for intentional replacement at text boundaries.
`len()` counts UTF-16 units. Serde represents valid scalar strings as text and
other strings as a unit array. The guest JSON builtin independently implements
well-formed ECMAScript JSON stringification.

The former `Interpreter::with_runtime_builtins()` convenience constructor is now
`napi_vm_runtime::runtime::with_runtime_builtins()`. Prefer `RuntimeBuilder` for
capability and permission configuration. The facade also exposes the helper as
`napi_vm::runtime::with_runtime_builtins()` when runtime is enabled.

The core's test-only host fixtures exercise engine scheduling and native interop;
they are excluded from production builds. Runtime integration tests live in
`crates/napi-vm-runtime/tests`. Workspace CI tests both extracted packages and
standalone minimal configurations.

The existing module-loader identifier and source-file interfaces accept Rust
UTF-8 text. Module specifiers with unpaired surrogates are rejected explicitly
at that host contract rather than aliasing another module through replacement.
Guest-created `eval` and `Function` source is lexed from code units directly.
`Interpreter::compile_utf16` and `eval_utf16` also accept raw code-unit source;
N-API and wasm script entry points use these APIs. Binding module registration
rejects non-Unicode identifiers/source rather than replacing their units; UTF-8
module files can express surrogate literals through JavaScript Unicode escapes.

Validation evidence and remaining compatibility failures are recorded in
[utf16-extraction-validation.md](utf16-extraction-validation.md).
