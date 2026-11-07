# CommonJS and native addons in Rust hosts

Native addons are trusted host code. Root restrictions and hash allowlists
control which binary may load; they do not restrict that binary's operating
system privileges. Use [safety](safety.md) and the backend-specific limits below
when choosing an embedding.

## Configure CommonJS

The snippets use application-owned `app` files and a native addon supplied by
the embedder. For repository fixtures, use the linked executable example below.

The filesystem CommonJS loader reads JavaScript and JSON as guest modules.
Configure explicit roots and an entry path before evaluating `require` calls:

```rust
use napi_vm_core::{FileCommonJsLoader, Interpreter};
use std::{path::PathBuf, rc::Rc};

let app_root = PathBuf::from("./app").canonicalize().unwrap();
let loader = FileCommonJsLoader::new([&app_root]).unwrap();
let mut engine = Interpreter::with_builtins();
engine.set_commonjs_loader(Rc::new(loader)).unwrap();
engine.set_commonjs_entry(app_root.join("main.cjs").to_string_lossy().into_owned());
let result = engine.eval_source("require('./config.json');").unwrap();
```

`require.resolve` uses the configured resolver without initializing the module.
File-backed ES modules can import `createRequire`, `isBuiltin` and
`builtinModules` from `node:module` when the loader is configured. Filesystem
and path capabilities still depend on the host configuration. A resolved
`.node` filename must pass the separate addon policy before loading.

## Select a backend

| Backend | Execution and dependencies | Compatibility boundary |
| --- | --- | --- |
| `NodeAddonOptions` / Node sidecar | A compatible Node executable runs the addon in a child process | Real Node-API environment; binary compatibility follows the selected Node runtime |
| `RustNodeApiOptions` / in-process host | Desktop `node-api-host` feature and a C compiler; no Node executable | Selected Node-API subset; no V8, NAN or libuv ABI |

The loader can use a JavaScript fallback when no addon provider is configured.
Native loading activates package `node-addons` conditions; selection still
passes root, extension, digest, binary format and architecture checks.
Preflight checks do not guarantee that dynamic libraries or every imported
API will be available at initialization time.

## Node sidecar

```rust
use napi_vm_core::{Interpreter, NodeAddonOptions};
use std::path::PathBuf;

let root = PathBuf::from("./app").canonicalize().unwrap();
let addon = root.join("native/example.node");
let mut engine = Interpreter::with_builtins();
let runtime = engine.enable_native_addons(
    NodeAddonOptions::new("node", [root.clone()])
        .allow_native_addon(addon.clone())
        .entry(root.join("main.cjs")),
).unwrap();
runtime.preflight_addon(&addon).unwrap();
let result = engine.eval_source("require('./native/example.node').run();");
runtime.shutdown().unwrap();
let result = result.unwrap();
```

`allow_native_addon` pins the installed file's digest during configuration.
Use `allow_native_addon_with_sha256` with a trusted manifest digest when that
identity must be established independently of the installed file. The backend
rechecks its policy when called directly as well as through CommonJS.
`runtime_info` reports the selected Node and Node-API versions;
`minimum_napi_version` can reject an inadequate Node executable during setup.

The bridge supports bounded values, guest callbacks, supported object graphs
and synchronous checkpoint writeback. It is not a complete identity-preserving
copy of the Node heap. Sparse arrays, some array descriptors/prototypes and
cross-boundary reflection have limits. Guest Date, RegExp and binary values
are copied; unsupported in-place mutation fails. Inspect the tests before
relying on a particular reflection or graph behavior.

## In-process Rust Node-API host

Enable the facade's `node-api-host` feature or the corresponding core feature.
This is a selected Node-API implementation, not a full Node replacement.
Linux has runtime validation; consult the current tests and historical platform
evidence before relying on macOS or Windows behavior.

```rust
use napi_vm_core::{Interpreter, RustNodeApiOptions};
use std::path::PathBuf;

let root = PathBuf::from("./app").canonicalize().unwrap();
let addon = root.join("native/example.node");
let mut engine = Interpreter::with_builtins();
let runtime = engine.enable_native_addons(
    RustNodeApiOptions::new([root.clone()])
        .allow_native_addon(addon)
        .max_napi_version(10)
        .entry(root.join("main.cjs")),
).unwrap();
let result = engine.eval_source("require('./native/example.node').run();");
runtime.shutdown().unwrap();
let result = result.unwrap();
```

The version ceiling controls registration admission and `napi_get_version`;
it does not imply that every function in those Node-API versions exists.
The supported surface includes values, object/array operations, callbacks,
selected property/descriptor APIs, buffers and views, promises, bounded async
work, thread-safe functions, references and cleanup hooks. Unsupported imports
fail at loading. Direct V8/NAN/libuv addons need a compatible Node sidecar.

Weak-reference and finalizer behavior is ownership-based at host checkpoints
and shutdown. It is not general tracing-GC finalization. Native async execute
callbacks must not call Node-API; completion and guest callbacks return to the
VM owner thread. Runtime shutdown rejects new work and coordinates cleanup;
retain and shut down the runtime on its owning thread.

## Package prebuilds

`allow_native_prebuild` selects a target prebuild and aliases a package whose
API is the addon itself. `allow_native_package_prebuild_with_sha256` supports
packages whose JavaScript wrapper calls `node-gyp-build` without replacing
that wrapper. Use the digest variants with trusted manifests.

Selection considers target platform/architecture, N-API tags and relevant
libc/ARM tags. Node-ABI-only and libuv-tagged builds are rejected by the Rust
backend. `PREBUILDS_ONLY`, package-specific prebuild overrides and the
executable-neighbor fallback remain inside configured CommonJS roots.

## Source and executable references

- [Embedding example](../examples/rust_native_addon.rs).
- [CommonJS loader](../crates/napi-vm-core/src/interpreter/commonjs.rs).
- [Sidecar and shared policy](../crates/napi-vm-core/src/interpreter/node_addon.rs).
- [Rust Node-API host](../crates/napi-vm-core/src/interpreter/rust_node_api.rs).
- [Historical loader plan](native-addon-loader-plan.md) and
  [runtime plan](node-addon-runtime-plan.md), with their original limitations.

Run the relevant native-addon tests for the backend and platform being changed;
a passing engine conformance suite does not establish addon compatibility.
