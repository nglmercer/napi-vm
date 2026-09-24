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
- [Plugins](docs/plugins.md) — manifests, filesystem permissions, `node:fs` / `node:path`, and the plugin lifecycle
- [Sandbox safety](docs/safety.md) — containment guards and operational limits
- [Development](docs/development.md) — quality gate, scripts, benchmarks, and project structure
- [Roadmap](docs/roadmap.md) — implemented features and known boundaries
- [Native addon loader plan](docs/native-addon-loader-plan.md) — roadmap for Node-API and `.node` compatibility in Rust desktop hosts

## Rust embedding and CommonJS

Rust applications can opt into guest `require()` with a filesystem resolver
restricted to application roots. The resolver reads JavaScript and JSON as
guest source; it never executes them through the host's `require()`.
Guest CommonJS functions also expose `require.resolve(specifier)`, which uses
the same configured resolver to return a module filename without loading or
initializing it. This works for `.node` paths too, so a package wrapper can
select a native binary before the host's addon allowlist and integrity checks
run at the eventual `require()` call.

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
    let addon_runtime = runtime
        .enable_native_addons(
            NodeAddonOptions::new("node", [app_root.clone()])
                .allow_native_addon(addon.clone())
                .entry(app_root.join("main.cjs")),
        )
        .unwrap();
    addon_runtime.preflight_addon(&addon).unwrap();
    let sidecar = addon_runtime.node_sidecar().expect("Node sidecar backend");
    println!("Node-API v{}", sidecar.runtime_info().napi_version);
    let result = runtime.eval_source("require('example').run();").unwrap();
    println!("{result:?}");
    addon_runtime.shutdown().unwrap();
}
```

Native addons execute as trusted host code in the Node child process, outside
the VM sandbox. The root restriction and per-file allowlist decide which addon
may load; they do not constrain what that trusted addon can do on the host.
Package `node-addons` export and import conditions are enabled when an addon
provider is configured. Without one, the resolver skips that condition and can
use a package's JavaScript fallback, matching Node's `--no-addons` mode. A
selected `.node` file still has to pass the root and per-file integrity policy.
`allow_native_addon()` pins the binary's SHA-256 digest when configured and
checks it again before each load. For build-time integrity, use
`allow_native_addon_with_sha256(path, expected_digest)` with the digest from a
trusted host manifest; setup fails if the installed binary differs, and the
digest is rechecked before loading. `NodeAddonSidecar::runtime_info()` reports
the Node and Node-API versions that were actually started. Hosts can set
`NodeAddonOptions::minimum_napi_version(version)` to reject a Node executable
that does not provide the required Node-API version during setup. The selected
backend repeats the root, extension, and digest checks when called directly,
so bypassing CommonJS resolution does not bypass the native addon policy. Both
backends also reject malformed or wrong-architecture ELF, Mach-O, and PE files
before loading them. Hosts can call `NativeAddonRuntime::preflight_addon(path)`
to run these checks without invoking the addon initializer; missing dynamic
dependencies can still fail when the addon is loaded.
`NativeAddonPolicy` lets a desktop host share the same filesystem roots, addon
integrity allowlist, and application entry between backends while keeping
backend-specific settings separate:

```rust
use napi_vm::{NativeAddonPolicy, NodeAddonOptions};

let policy = NativeAddonPolicy::new([app_root.clone()])
    .allow_native_addon_with_sha256(addon.clone(), trusted_manifest_digest())
    .entry(app_root.join("main.cjs"));
let sidecar_options = NodeAddonOptions::with_policy("node", policy.clone());
// With `node-api-host`, use `RustNodeApiOptions::with_policy(policy)` instead.
```

Native module exports enter the CommonJS cache before the Rust Node-API
initializer runs. If initialization fails, the cache entry is removed so a
later `require()` can retry it. During initialization, synchronous guest entry
uses the interpreter's paused-call checkpoint; `napi_run_script` can re-require
the same addon and observe its provisional exports object.

For a Rust desktop host, `examples/rust_native_addon.rs` shows the in-process
Node-API setup. Build with `node-api-host`, provide the application's CommonJS
entry, and pass the addon's digest from trusted host metadata:

```bash
cargo run --features node-api-host --example rust-native-addon -- \
  ./app ./app/main.cjs ./app/native/example.node "$TRUSTED_SHA256"
```

The guest entry keeps using ordinary `require('./native/example.node')` or a
package's normal JavaScript wrapper. The example checks the binary before
running the entry and shuts the backend down afterward. A long-lived desktop
application should instead retain the returned runtime and call
`vm.run_event_loop_once(std::time::Duration::from_millis(16))` from its event
loop, then call `runtime.shutdown()` during application teardown. This Rust
backend supports a selected Node-API surface; use `NodeAddonOptions` when an
addon needs Node's broader ABI compatibility.

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
return graphs. Top-level `await` keeps pumping while Rust Node-API async work
is outstanding, so a `node-addon-api` `AsyncWorker` can settle an ordinary
guest-created Promise through its callback.
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

Keep the returned `NativeAddonRuntime` and call `shutdown()` on the
interpreter's owner thread when the application closes. Shutdown stops new
addon calls, asks the sidecar worker to exit cleanly, then terminates it if it
does not exit before the deadline. Repeated calls are safe.

### Experimental Rust Node-API host

Linux, macOS, and Windows desktop builds can enable the `node-api-host` Cargo
feature to load a Node-API addon directly into the Rust process, without a Node
executable. Linux is runtime-tested; the Windows GNU backend was cross-compiled
and tested under Wine. Native Windows/MSVC and macOS runtime verification
remain pending. The host uses the same guest `require()` resolver, root
restrictions, and hash allowlist:

```rust
use napi_vm::{Interpreter, ReportedNodeVersion, RustNodeApiOptions};
use std::path::PathBuf;

let app_root = PathBuf::from("./app").canonicalize().unwrap();
let addon = app_root.join("native/example.node");
let digest: [u8; 32] = trusted_manifest_digest();
let mut runtime = Interpreter::with_builtins();
runtime
    .enable_native_addons(
        RustNodeApiOptions::new([app_root.clone()])
            .allow_native_addon_with_sha256(addon, digest)
            .max_napi_version(10)
            .reported_node_version(ReportedNodeVersion::new(22, 17, 3))
            .entry(app_root.join("main.cjs")),
    )
    .unwrap();
let result = runtime.eval_source("require('./native/example.node').run();").unwrap();
```

For a package whose public API is the native addon itself, the host can select
a platform prebuild and expose it under the package's bare name:

```rust
let package_root = app_root.join("node_modules/example");
runtime.enable_native_addons(
    RustNodeApiOptions::new([app_root.clone()])
        .allow_native_prebuild("example", &package_root),
)?;
let result = runtime.eval_source("require('example').run();")?;
```

Packages whose JavaScript entry calls
`require('node-gyp-build')(__dirname)` can keep that wrapper. Configure the
selected prebuild without aliasing over the package entry:

```rust
let package_root = app_root.join("node_modules/example");
let digest: [u8; 32] = trusted_manifest_digest();
runtime.enable_native_addons(
    RustNodeApiOptions::new([app_root.clone()])
        .allow_native_package_prebuild_with_sha256(&package_root, digest),
)?;
```

The returned `NativeAddonRuntime` also supports explicit owner-thread
shutdown. The Rust backend stops async work, runs cleanup hooks and finalizers,
then releases addon libraries when no native thread still depends on them. It
rejects new addon loads and calls after shutdown.

The Rust host provides the common `node-gyp-build(dir)`, `.path(dir)`, and
`.resolve(dir)` calls, plus the package's `parseTags`, `matchTags`,
`compareTags`, `parseTuple`, `matchTuple`, and `compareTuples` helpers. Package
selection honors `PREBUILDS_ONLY`, the package-specific `<NAME>_PREBUILD`
environment variable (package name uppercased with hyphens changed to
underscores), and the nearby-prebuild fallback based on the embedding
application’s executable path. Rust hosts can override the first and last
settings with `node_gyp_build_prebuilds_only()` and
`node_gyp_build_exec_path()` on `RustNodeApiOptions`.
The package prebuild override and executable-neighbor fallback must remain
inside the configured CommonJS roots.

The Rust backend loads Node-API-compatible `.node` files only. It rejects
Node-ABI-only files and libuv-tagged builds because this backend does not
provide V8, NAN, or libuv ABI compatibility. The selected `.node` file still
passes through the configured root and digest allowlist.

Unless `PREBUILDS_ONLY` is set, the resolver checks `build/Release` and
`build/Debug` before
`prebuilds/<platform>-<arch>`. Within `prebuilds`, it selects N-API-tagged
files for the current platform and architecture. On Linux it also matches
`glibc` or `musl`; it honors `ARM_VERSION` where an arm-version tag is present.
The alias replaces
the package's JavaScript entry for that bare request, so use it when that
wrapper only returns the native addon. The selected file is still restricted
to configured roots and pinned to its SHA-256 when the runtime is configured.
Use `allow_native_prebuild_with_sha256()` to pin against a trusted manifest.

This is an early compatibility slice, not a general Node replacement. It
accepts addons requesting Node-API versions 1 through the configured maximum
(10 by default), but only implements a selected API subset rather than every
function in those versions. `max_napi_version()` also controls the value
returned by `napi_get_version`; addon registrations above that ceiling fail
before their initializer runs. Before loading, the host checks that the file is
an ELF shared object, Mach-O bundle/dylib, or PE DLL for the current
architecture and reports format, class, and architecture mismatches directly.
The v1 surface includes C callbacks and callback info, controlled synchronous
guest callback entry through `napi_call_function` and `napi_new_instance`,
local handle scopes, object
creation and named properties, `napi_get_global`, general property reads and
writes, membership, deletion and own-property checks, and
`napi_get_property_names` (including inherited enumerable names). It supports
undefined/null/boolean values, double/int32/uint32/int64 numbers, UTF-8 strings,
`napi_create_symbol`, `napi_define_properties` on ordinary object and array
targets (indexed and named data values, symbol keys, native methods, and
accessors), `napi_typeof`, array
creation/index/length operations, and `napi_define_class` with native
constructors plus static and instance data, methods, and accessors. Core error
creation and pending-exception propagation are also supported.
Node-API v5 date creation, type checks, and reads map to guest `Date` objects.
The selected Node-API v6 surface adds the `BigInt` create/read functions,
`napi_get_all_property_names` for ordinary objects, classes, functions,
arrays, errors, regular expressions, and Proxy chains with `ownKeys` traps
when called from an active guest callback, plus environment instance data.
Property collection supports own or inherited keys, attribute filters, symbols,
and numeric key conversion. The Rust host follows Node's behavior for
Proxy `writable` and `configurable` filters; Bun applies target descriptors for
those filters. Global-object key collection is available during an active
guest callback for own and inherited keys when the filter does not depend on
property attributes. Writable/enumerable/configurable filters on the global
object return a generic failure because the VM does not retain global property
descriptors. BigInt word arrays are capped at 2,048 64-bit words by the VM's
integer-size limit.
Replacing instance data overwrites the old slot without calling its finalizer;
the current finalizer runs on the owner thread during host shutdown, after
cleanup hooks.
The selected Node-API v7 surface supports ArrayBuffer detachment and detached
state checks. Detachment is idempotent, invalidates existing typed-array views,
and follows Node's `napi_is_detached_arraybuffer` result for non-ArrayBuffer
values (`napi_ok` with `false`). Bun 1.4.0 returns
`napi_arraybuffer_expected` for that same input; the differential fixture keeps
this runtime difference explicit while checking all shared behavior.
Guest `Proxy` values support `getPrototypeOf` traps. `Object.getPrototypeOf`,
`Object.prototype.__proto__`, `Object.prototype.isPrototypeOf`, and `instanceof`
follow those traps and enforce the non-extensible-target invariant.
`napi_instanceof` uses the same guest `instanceof` behavior, including custom
`Symbol.hasInstance` methods and Proxy traps on constructor and prototype
chains. A compiled addon fixture found that Node's current N-API implementation
returns `null` for a Proxy without calling its `getPrototypeOf` trap, while Bun
1.4 calls the trap. This differs from the Node-API documentation, which
describes the operation as equivalent to `Object.getPrototypeOf`
([`napi_get_prototype`](https://nodejs.org/api/n-api.html#napi_get_prototype)).
The Rust host follows the observed Node behavior, and the fixture asserts
Bun's difference explicitly.
The selected Node-API v8 slice adds object type tags, freeze/seal for ordinary
guest objects, arrays, functions, and class constructors, and asynchronous
cleanup hooks. Type tags are shared across addon environments and remain
attached to the guest object.
Because the VM has no object garbage collector, tagged objects remain retained
until host shutdown; the tag table is capped at the local-handle limit.
Array index and `length` descriptors reflect their frozen or sealed attributes.
Guest writes, deletes, length truncation, Node-API element writes, and the
implemented mutating array methods honor frozen arrays. Cleanup hooks run in
reverse registration order. Async hooks start in that order, synchronous hooks
continue, and the host waits for async completion before finalizers while
keeping addon libraries mapped. Other object representations report a generic
failure until their property metadata can enforce the same integrity rules.
The compiled fixture compares type-tag and integrity behavior with Node and
Bun, and verifies async cleanup ordering against Node. Bun 1.4.0 exits without
awaiting an asynchronous cleanup hook, so it is not used as the teardown-order
reference.
The selected Node-API v9 functions provide the global symbol registry,
SyntaxError creation and throwing, and the addon's `file://` URL.
`node_api_symbol_for` shares identity with guest `Symbol.for`, and the URL
storage remains valid for the lifetime of the addon environment.
The selected Node-API v10 functions provide external Latin-1 and UTF-16 string
creation with eager copy/finalizer handling, string property-key creation, and
zero-copy `Buffer` views over `ArrayBuffer` storage. The Node and Bun fixture
checks the documented external-string copy/finalizer contract, Unicode keys,
buffer aliasing, and out-of-range errors.
The stable v1 `napi_get_node_version` function returns the numeric compatibility
version configured with `reported_node_version`; its release name is always
`napi-vm`. The default version is `0.0.0`, so the Rust host does not identify
itself as Node.js unless the desktop application deliberately supplies a
numeric profile. This metadata does not add APIs to the compatibility surface.
`napi_get_uv_event_loop` is link-compatible but returns
`napi_generic_failure` with a null output because the Rust host does not embed a
libuv loop. The deprecated `napi_module_register` constructor path also works
for a single registered module in a shared library. Since its descriptor has
no Node-API version field, the loader treats it as a v1 registration request.
`napi_fatal_exception` enters the existing external-event queue and delivers
`process.emit('uncaughtException', error)` at the next VM checkpoint when that
handler is available; otherwise the error propagates to the Rust embedder.
`napi_fatal_error` writes its diagnostic and aborts the process as specified.
`napi_add_finalizer` supports its optional zero-count reference and runs the
finalizer on the owning thread at host shutdown, after cleanup hooks. The VM has
no guest-object garbage collector, so this does not provide collection-time
finalization.
`napi_get_last_error_info` exposes the most recent API status and a VM-neutral
message. Its returned data is valid only until the next Node-API call. Returning
a null callback value without a pending exception produces guest `undefined`.
Imports outside that subset fail when the library is loaded. Strong `napi_ref`
creation, lookup, count changes, and deletion are supported. For addons
requesting Node-API v10, references can also hold primitive values; those values
are released when the count reaches zero, and later lookup returns `NULL`.
Zero-count references to objects, externals, functions, and symbols are cleared
when runtime ownership shows no other roots at Node-API callback and reference
checkpoints. This is an ownership-count approximation, not tracing collection:
cyclic values, shared-buffer wrappers, and values held by wrap/finalizer records
can remain retained.
`napi_wrap`, `napi_unwrap`, and `napi_remove_wrap` work for VM values with
stable object identity. Wrap finalizers run once on the runtime's
owning thread when the Rust Node-API host shuts down; removing a wrap does not
call its finalizer.
There is no guest-object garbage collector, so wrap finalizers do not run at
ordinary object collection time. `napi_create_buffer`,
`napi_create_buffer_copy`, `napi_get_buffer_info`, and `napi_is_buffer` are
supported; these bytes appear to guest code as `Uint8Array` views.
`napi_create_external_buffer` and `napi_create_external_arraybuffer` expose
addon-owned memory without copying. The runtime retains their backing values
and runs their finalizers once on the owner thread during shutdown; collection
time finalization is unavailable. ArrayBuffer, typed-array, and DataView
creation, type checks, and info APIs share backing storage with guest views and
preserve byte offsets. `napi_create_promise`, deferred resolution/rejection, and
`napi_is_promise` use the VM's Promise and microtask implementation. During
module initialization, deferreds can be settled directly with primitive
resolutions or any rejection. Object and promise resolutions require an active
interpreter callback dispatcher because checking thenability can execute guest
code. `napi_create_async_work`, `napi_queue_async_work`,
`napi_cancel_async_work`, and `napi_delete_async_work` use a bounded pool of
four worker threads with a 128-item queue. Execute callbacks run off-thread;
completion callbacks enter the VM through its event queue. Execute callbacks
must not call Node-API. The Node-API v4 thread-safe function calls for create,
call, context lookup, acquire/release, and ref/unref are supported. Queue size,
queue-full, blocking producer backpressure, abort, and finalization follow the
Node-API contract; calls from the owner thread return `napi_queue_full` rather
than blocking that thread when the queue is full. Thread-safe callbacks run on
the VM owner thread through the existing host-event queue. If the host shuts
down while native producers still hold a thread-safe function, queued custom
callback data is offered to the addon with a null environment for cleanup, and
the addon libraries and ABI shim remain mapped to keep later closing/release
calls safe. Such outstanding functions do not run their finalizers during that
shutdown path.
`napi_add_env_cleanup_hook` and `napi_remove_env_cleanup_hook` are also
supported. Hooks run in reverse registration order on the VM owner thread
before thread-safe function and wrap finalizers. Duplicate registrations and
unmatched removals return `napi_invalid_arg`; Node aborts for these misuse cases,
while the Rust host reports a recoverable error to protect the embedding
process.
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
