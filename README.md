# napi-vm

A sandboxed JavaScript virtual machine written in Rust with Node.js NAPI and
WebAssembly integrations. It includes a shared language service, browser
playground, local LSP, optional Zed extension, live VM metadata, and an
IPC-style command/event bridge for deterministic tests.

## Quick start

Prerequisites: **Rust** (1.96+), **Node.js** (18+) and **[Bun](https://bun.sh)**.
Bun runs the main test suite; the library itself has no Bun dependency at
runtime.

```bash
npm install
npm run build
npm test          # main suite (requires Bun)
npm run test:node # Node compatibility suite
npm run lint      # cargo fmt + clippy, then tsc --noEmit
```

```javascript
const { Vm } = require("./index.js");

const vm = new Vm();
vm.run("let answer = 40 + 2;");
console.log(vm.run("answer;")); // 42
```

## Documentation

- [Getting started](docs/getting-started.md) — installation, VM usage, host bridge, modules, and playground
- [API reference](docs/api.md) — `Vm`, `LanguageService`, and `VmSession`
- [Editor integration](docs/editor.md) — playground, LSP, Zed, live metadata, and IPC commands
- [Plugins](docs/plugins.md) — manifests, filesystem permissions, `napi:fs` / `napi:path`, and the plugin lifecycle
- [Sandbox safety](docs/safety.md) — containment guards and operational limits
- [Development](docs/development.md) — quality gate, scripts, benchmarks, and project structure
- [Roadmap](docs/roadmap.md) — implemented features and known boundaries

## Rust embedding and CommonJS

Rust applications can opt into guest `require()` with a filesystem resolver
restricted to application roots. The resolver reads JavaScript and JSON as
guest source; it never executes them through the host's `require()`.

```rust
use napi_vm::{FileCommonJsLoader, Interpreter};
use std::{path::PathBuf, rc::Rc};

fn main() {
    let app_root = PathBuf::from("./app").canonicalize().unwrap();
    let loader = FileCommonJsLoader::new([&app_root]).unwrap();
    let mut runtime = Interpreter::with_builtins();
    runtime.set_commonjs_loader(Rc::new(loader)).unwrap();
    runtime.set_commonjs_entry(app_root.join("main.cjs").to_string_lossy().into_owned());

    let result = runtime
        .eval_source("const config = require('./config.json'); config;")
        .unwrap();
    println!("{result:?}");
}
```

The optional `NodeAddonSidecar` provider runs a Node.js child process for
allowlisted `.node` addons. Node initializes each addon with a real Node-API
environment; Rust and the VM exchange bounded values and synchronous calls
over a private loopback connection.

```rust
use napi_vm::{Interpreter, NodeAddonOptions};
use std::path::PathBuf;

fn main() {
    let app_root = PathBuf::from("./app").canonicalize().unwrap();
    let addon = app_root.join("node_modules/example/build/Release/example.node");
    let mut runtime = Interpreter::with_builtins();
    let addon_bridge = runtime
        .enable_node_addons(
            NodeAddonOptions::new("node", [app_root.clone()])
                .allow_native_addon(addon)
                .entry(app_root.join("main.cjs")),
        )
        .unwrap();
    println!("Node-API v{}", addon_bridge.runtime_info().napi_version);
    let result = runtime.eval_source("require('example').run();").unwrap();
    println!("{result:?}");
}
```

Native addons execute as trusted host code in the Node child process, outside
the VM sandbox. The root restriction and per-file allowlist decide which addon
may load; they do not constrain what that trusted addon can do on the host.
`allow_native_addon()` pins the binary's SHA-256 digest when configured and
checks it again before each load. For build-time integrity, use
`allow_native_addon_with_sha256(path, expected_digest)` with the digest from a
trusted host manifest; setup fails if the installed binary differs, and the
digest is rechecked before loading. `NodeAddonSidecar::runtime_info()` reports
the Node and Node-API versions that were actually started. Hosts can set
`NodeAddonOptions::minimum_napi_version(version)` to reject a Node executable
that does not provide the required Node-API version during setup.
The bridge supports synchronous function calls and constructors, Promise
settlement, primitive values, arrays, byte buffers, BigInts, Dates, regular
expressions, symbol identity, and identity-preserving native object proxies.
Proxy property reads and writes, `in` checks, deletion, enumeration, method
calls, object spread, and `Object.assign` reach the original Node object. Node
`Buffer` results stay live native objects;
`ArrayBuffer`, typed array, and `DataView` values cross as copied bytes while
preserving their view type. Asynchronous addon callbacks are queued and run at
VM event-loop checkpoints; use `run_event_loop_once` from the desktop runtime
to pump external events. Synchronous callbacks into guest JavaScript run on
the interpreter thread and return values and thrown errors to the addon. Native
addon calls made inside a synchronous guest callback are serviced while the
Node worker waits for that callback. Nested addon calls and nested synchronous
guest callbacks therefore preserve their nested call order without entering
the interpreter from a native thread. Shared and cyclic plain object/array
graphs preserve identity within each native call, including Node-created
return graphs.
Guest classes cross into addons as constructable functions; native addons can
construct instances, call inherited guest methods, and read or update static
data. Guest-created proxies with object, array, function, or class targets
also cross into addons. Their supported `get`, `set`, `has`, `deleteProperty`,
`ownKeys`, `apply`, and `construct` traps run back on the interpreter thread.
Guest-side and native-side object changes are synchronized at synchronous
callback checkpoints, so a native call can observe a target change made by a
guest trap. Unsupported proxy traps are ignored to match napi-vm's proxy
model, and cyclic graphs that pass through a guest proxy fail clearly.
Mutations to plain guest objects and arrays are written back to the original
VM values after native calls, including calls that throw. Writeback includes
nested objects, named array properties, and property attributes on objects.
Guest `Date`, `RegExp`, and binary values are copied; in-place changes to
those values fail clearly.
Accessor and symbol-keyed properties on plain objects cross the native addon
bridge. Accessors invoke guest getter and setter callbacks, and symbol keys
retain identity in both directions. Explicit guest object prototypes preserve
inherited property reads and method calls through the bridge, and prototype
changes on plain guest objects roundtrip through native calls, including null
and the default prototype. Array prototype mutation and built-in prototype
fidelity remain incomplete. Accessors or symbol keys on arrays, sparse arrays,
and array descriptor changes are not fully compatible yet. Some reflection
behavior still differs across the VM/Node boundary.
A compatible Node executable must be installed or bundled with the desktop
application.

### Experimental Rust Node-API host

Linux desktop builds can enable the `node-api-host` Cargo feature to load a
small Node-API v1 addon directly into the Rust process, without a Node
executable. It uses the same guest `require()` resolver, root restrictions,
and hash allowlist:

```rust
use napi_vm::{Interpreter, RustNodeApiOptions};
use std::path::PathBuf;

let app_root = PathBuf::from("./app").canonicalize().unwrap();
let addon = app_root.join("native/example.node");
let digest: [u8; 32] = trusted_manifest_digest();
let mut runtime = Interpreter::with_builtins();
runtime
    .enable_rust_node_api_addons(
        RustNodeApiOptions::new([app_root.clone()])
            .allow_native_addon_with_sha256(addon, digest)
            .entry(app_root.join("main.cjs")),
    )
    .unwrap();
let result = runtime.eval_source("require('./native/example.node').run();").unwrap();
```

This is an early compatibility slice, not a general Node replacement. It
currently supports a synchronous Node-API version 1 subset: C callbacks and
callback info, controlled synchronous guest callback entry through
`napi_call_function` and `napi_new_instance`, local handle scopes, object
creation and named properties, `napi_get_global`, general property reads and
writes, membership, deletion and own-property checks, and
`napi_get_property_names` (including inherited enumerable names). It supports
undefined/null/boolean values, double/int32/uint32/int64 numbers, UTF-8 strings,
`napi_create_symbol`, `napi_define_properties` on ordinary object targets
(data values, symbol keys, native methods, and accessors), `napi_typeof`, array
creation/index/length operations, and `napi_define_class` with native
constructors plus static and instance data, methods, and accessors. Core error
creation and pending-exception propagation are also supported. Returning a null
callback value without a pending exception produces guest `undefined`. Imports
outside that subset fail when the library is loaded. Strong `napi_ref` creation,
lookup, count changes, and deletion are supported; because the VM has no tracing
GC, zero-count references stay live until explicitly deleted. `napi_wrap`,
`napi_unwrap`, and `napi_remove_wrap` work for VM values with stable object
identity. Wrap finalizers run once on the runtime's owning thread when the
Rust Node-API host shuts down; removing a wrap does not call its finalizer.
There is no guest-object garbage collector, so wrap finalizers do not run at
ordinary object collection time. `napi_create_buffer`,
`napi_create_buffer_copy`, `napi_get_buffer_info`, and `napi_is_buffer` are
supported; these bytes appear to guest code as `Uint8Array` views. External
buffers are not implemented. ArrayBuffer, typed-array, and DataView creation,
type checks, and info APIs share backing storage with guest views and preserve
byte offsets. `napi_create_promise`, deferred resolution/rejection, and
`napi_is_promise` use the VM's Promise and microtask implementation. During
module initialization, deferreds can be settled directly with primitive
resolutions or any rejection. Object and promise resolutions require an active
interpreter callback dispatcher because checking thenability can execute guest
code. `napi_create_async_work`, `napi_queue_async_work`,
`napi_cancel_async_work`, and `napi_delete_async_work` use a bounded pool of
four worker threads with a 128-item queue. Execute callbacks run off-thread;
completion callbacks enter the VM through its event queue. Execute callbacks
must not call Node-API. Thread-safe functions remain unavailable.
Direct V8/NAN/Node C++/libuv addons must use the Node sidecar. The feature
requires a C compiler at build time, and native addons have the desktop
process's full privileges in either backend.

## Useful examples

```bash
bun examples/plugins.ts
bun examples/hotreload.ts
NAPI_VM_SESSION=1 bun examples/hotreload.ts
npm run ipc:smoke
```

The first command runs the VM entirely in-process. The second opt-in command
publishes live metadata for the LSP through `.napi-vm/runtime.json`; the
temporary locator is ignored and removed when the session stops.

## Disposing a VM

A VM that has run `runAsync` holds a native handle for dispatching host calls,
and that handle keeps the Node process alive. Call `dispose()` when you are
finished with a VM that used `runAsync`, or the script will not exit:

```javascript
const vm = new Vm();
vm.exposeAsyncFunction("fetchRow", async (id) => db.get(id));
await vm.runAsync(`await fetchRow(1);`);
vm.dispose();
```

`dispose()` is idempotent, and safe to call while an async worker is still in
flight. VMs that only use the synchronous `run()` do not need it.

## Status

The core language and bridge are actively developed and covered by the native
regression suite. Run `npm test` to see the current verified count.

## License

MIT — see [LICENSE](LICENSE).
