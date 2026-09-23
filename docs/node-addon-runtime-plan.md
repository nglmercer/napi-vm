# Rust Desktop Node Addon Runtime Plan

## Goal

Allow a Rust desktop application embedding `napi-vm` to run guest CommonJS
source such as:

```js
const native = require("./module.node");
native.run();
```

The option should load trusted native packages, preserve CommonJS caching and
error behavior, and expose a clearly documented compatibility level. Package
resolution belongs in Rust; addon code must never be executed by host
`require()` as a shortcut around the guest loader.

## Compatibility boundary

`.node` is a native shared library format, not a self-describing JavaScript
module. Node-API addons use an opaque C ABI and are designed to remain stable
across Node versions. Addons using V8, NAN, Node's C++ APIs, or libuv directly
have a different and less stable ABI. Therefore the first in-process backend
must target **Node-API-only addons**. It cannot promise compatibility with every
file named `.node`.

Keep these backend choices distinct:

| Backend | Compatibility target | Runtime dependency |
| --- | --- | --- |
| Node sidecar (current) | Addons accepted by the selected Node installation | Bundled or configured Node executable |
| Rust Node-API host (experimental) | Selected Node-API v1-v4 calls; Linux runtime-tested, macOS path awaiting native verification | `napi-vm`, a C compiler at build time, and the platform dynamic loader |

Direct V8/NAN/Node C++ addons stay on the sidecar backend. If users require
those addons without a child process, evaluate embedding Node itself as a
separate product option; do not emulate a Node/V8 ABI in the Rust host.

Loading a `.node` file runs native code with the desktop process's operating
system privileges. The VM permission system cannot sandbox that code. Require
an explicit native-addon option, configured roots, and an integrity allowlist
for both backends. Never load arbitrary package files just because guest code
calls `require()`.

## Public configuration

Preserve the current sidecar API and introduce an explicit backend selector
when the Rust host is ready. The eventual shape could be:

```rust
Vm::builder()
    .native_addons(NativeAddonOptions::rust_node_api()
        .allowed_roots(roots)
        .allow_sha256(addon_path, digest)
        .max_napi_version(8));
```

The exact Rust names can follow the existing `NodeAddonOptions`; the important
parts are backend choice, allowed filesystem roots, integrity pins, supported
Node-API version, and a stable compatibility report. A guest `require()` call
still goes through the VM's CommonJS resolver and module cache.

The current implementation is available behind Cargo feature `node-api-host`:
`Interpreter::enable_rust_node_api_addons(RustNodeApiOptions)` uses the
existing CommonJS resolver and allowlist. On Linux it accepts addons requesting
Node-API versions 1 through 4 and currently covers scoped handles, callback
info, synchronous C callbacks, global-object access, named and general property
operations, inherited enumerable property-name enumeration, primitive values,
numbers, UTF-8, Latin-1, and well-formed UTF-16 string conversion, boolean
coercion, symbol creation, and `napi_define_properties` for ordinary object
targets (data values, symbol keys, native methods, and accessors),
`napi_define_class` with native constructors, static descriptors, and
prototype descriptors, `napi_typeof`, and array creation/index/length
operations. Class static properties share object descriptor metadata and
accessor behavior.
`napi_get_value_int64` truncates finite Numbers toward zero, clamps values
outside the signed 64-bit range, and converts NaN and infinities to zero.
`napi_get_prototype` preserves explicit prototypes and the realm's
`Object.prototype` identity for ordinary objects. Prototype queries for VM
values whose built-in prototype is not materialized (such as arrays and
proxies) return a generic Node-API failure until that prototype model is
implemented.
`napi_call_function` and `napi_new_instance` enter guest code through the
interpreter's paused host-call callback handler; nested native calls and
pending guest exceptions stay on that controlled call path.
It also creates Node-style errors, tracks pending exceptions, and transfers
native callback throws into guest `try`/`catch`. `napi_get_last_error_info`
reports the most recent Node-API status and a VM-neutral message. Its returned
data remains valid only until the next Node-API call. A null callback result
with no pending exception maps to guest `undefined`. Handle entries are reclaimed
when local scopes close; opaque handles do not retain one heap allocation
apiece. Strong N-API references support creation, lookup, count changes, and
deletion. The VM has no tracing garbage collector, so a zero-count reference
does not clear until it is deleted. `napi_wrap`, `napi_unwrap`, and
`napi_remove_wrap` work for values with stable VM object identity. Finalizers
run once on the owning thread during Rust host shutdown, before addon libraries
unload; removing a wrap skips its finalizer. Without guest-object collection,
these finalizers do not run at normal object collection time. `napi_create_buffer`,
`napi_create_buffer_copy`, `napi_get_buffer_info`, and `napi_is_buffer` are also
supported. N-API buffers are surfaced as guest `Uint8Array` views; external
buffers are not implemented. ArrayBuffer, typed-array, and DataView creation,
type checks, and info APIs share storage with guest views and preserve byte
offsets. `napi_create_promise`, deferred resolution/rejection, and
`napi_is_promise` use the VM's Promise and microtask implementation. During
module initialization, deferreds can be settled directly with primitive
resolutions or any rejection. Object and promise resolutions require an active
interpreter callback dispatcher because checking thenability can execute guest
code. `napi_create_async_work`, `napi_queue_async_work`,
`napi_cancel_async_work`, and `napi_delete_async_work` use a bounded pool of
four worker threads and a queue of 128 work items. Execute callbacks run off
the interpreter thread and must not call Node-API. Completion callbacks are
queued as VM external events and run on the interpreter thread. On host
shutdown, queued work is canceled, running work is joined, and completion
callbacks for finished work run before addon libraries unload; guest callback
dispatch is unavailable during this final cleanup. Selected Node-API v4
thread-safe function calls are supported through the existing external-event
queue. Node-API v3 environment cleanup hooks run in reverse registration order
on the owner thread before thread-safe function and wrap finalizers. Duplicate
hook registrations and unmatched removals return `napi_invalid_arg` rather than
aborting the embedding process. This remains an incomplete compatibility
backend, and unimplemented imported symbols fail at load time. The macOS Mach-O
build and loading path uses the same shim and fixture, but needs execution on a
macOS host before it is claimed as verified. Windows currently uses the sidecar
backend; its DLL import and symbol-export model needs a separate implementation.
The VM's Rust `String` representation cannot preserve isolated UTF-16 surrogate
code units, so `napi_create_string_utf16` currently returns
`napi_generic_failure` for malformed UTF-16 instead of replacing or dropping
those code units.

## Implementation phases

### 1. Freeze the module-loading contract

- Audit the existing CommonJS resolver, `.node` allowlist, integrity checks,
  package `exports`, and cache behavior.
- Specify resolution for exact `.node` paths, extension omission, package
  `node-addons` conditions, platform/architecture prebuild directories, and
  missing or incompatible binaries.
- Keep package selection separate from binary loading. The resolver returns a
  canonical guest module ID and a verified native library path.
- Make startup diagnostics identify the selected backend, OS/architecture,
  addon digest, and supported Node-API version.

### 2. Define the Node-API host ABI and handle model

- Add an opt-in Cargo feature and a per-VM `NativeAddonHost`; do not change the
  default guest runtime.
- Load shared libraries through a small platform abstraction for Linux,
  macOS, and Windows. Export or provide the Node-API C symbols expected by
  modules, and validate the module registration entry point before executing
  initialization code.
- Represent each `napi_value` as an opaque, generation-checked handle into a
  VM-owned arena. Never expose Rust `Value` pointers as C handles.
- Implement handle scopes, references, wrap/finalizer lifetime, callback info,
  status codes, last-error state, and pending exceptions before broad API
  coverage. Stale handles and cross-environment handles must fail safely.
- Map Node-API values to VM values with identity preservation for objects,
  functions, symbols, buffers, typed arrays, and promises.

### 3. Build synchronous Node-API compatibility

Implement the C API in versioned groups and record each function as
`supported`, `unsupported`, or `not applicable`:

1. Environment and error APIs; undefined, null, booleans, numbers, strings,
   symbols, BigInts, and type checks.
2. Objects, arrays, property keys, descriptors, prototypes, equality,
   coercion, and property enumeration.
3. Functions, callback data, `this`, callback arguments, construction, and
   guest callback invocation.
4. References, wrap/unwrap, finalizers, external values, buffers, typed arrays,
   array buffers, and data views.
5. Promises, deferred settlement, exception propagation, and rejection
   behavior.
6. Environment cleanup hooks and shutdown ordering relative to other
   finalizers.

The first release should declare a conservative maximum Node-API version and
return a categorized compatibility error for APIs outside that version. Do
not return success with a partial or fabricated result.

### 4. Make guest callback entry safe

- Route `napi_call_function` and constructors that invoke guest callbacks
  through a controlled interpreter callback boundary.
- Do not call guest code through an arbitrary raw pointer while the interpreter
  is already mutably executing. Refactor host-call dispatch so it can suspend
  at a defined checkpoint, enter the callback, drain the established
  microtask queue, and resume the native call.
- Add re-entrancy guards and tests for nested addon calls from guest callbacks.
- Native worker threads may enqueue completion records only. They must never
  execute guest code directly.

### 5. Integrate asynchronous Node-API APIs with the VM event loop

- [x] Implement `napi_async_work` using a bounded host worker pool. Run execute
  work off the interpreter thread and completion callbacks as queued VM
  external events.
- [x] Implement `napi_threadsafe_function` with queue limits, acquire/release
  accounting, abort/close behavior, and delivery on the VM owner thread.
- [x] Deliver async-work completion and deferred Promise settlement through the
  existing job/event infrastructure; do not create a second guest event loop.
- [x] Specify shutdown behavior for async work: cancel queued work, join
  running workers, invoke completion callbacks for finished work, then unload
  addon libraries.
- [x] For thread-safe functions, close new calls at host shutdown and offer
  queued custom-callback data with a null environment for cleanup. If native
  producers still hold a function, keep addon libraries and the ABI shim mapped
  because this backend cannot join arbitrary addon-owned threads safely.

### 6. Add package and native binary support

- Keep native path resolution in the Rust package loader. Support common
  package export conditions and platform prebuild layouts without running
  package install scripts.
- Require OS, architecture, and binary-format matches. Report the exact reason
  for a rejected file: missing allowlist entry, digest mismatch, wrong
  architecture, unsupported ABI, missing symbol, or initialization failure.
- Cache initialized native exports by canonical module ID. Preserve Node-like
  cycle and failed-initialization behavior in the CommonJS loader.

### 7. Prove compatibility differentially

- Build small C fixtures against selected Node-API versions. Each fixture
  should exercise one API family and run with the same JS wrapper under Node,
  Bun where supported, and `napi-vm`.
- Compare structured outcomes: exported values, side effects, callback order,
  promise/event ordering, error name/message where stable, and finalizer count.
- Add fixtures for Node-API C and `node-addon-api` C++ wrappers. Keep fixtures
  using direct V8, NAN, Node C++ APIs, or raw libuv in a separate sidecar-only
  category.
- Run the native fixture matrix on Linux, macOS, and Windows, including
  supported CPU architectures. Add malformed-library and adversarial-handle
  tests before release.

## Release gates

1. The sidecar backend remains explicit, allowlisted, integrity checked, and
   covered by the existing real-addon tests.
2. The Rust backend loads an allowlisted Node-API-only fixture with plain
   `require("./fixture.node")`, caches it, and rejects an untrusted fixture
   before initialization.
3. Every Node-API function in the declared version matrix has a conformance
   test or an explicit unsupported result.
4. Synchronous guest callbacks, nested native calls, async work, and
   thread-safe function callbacks, and cleanup hooks run on documented VM
   checkpoints without unsafe interpreter re-entry. Cleanup hooks run before
   N-API finalizers.
5. Documentation clearly states that native addons execute with host process
   privileges and that direct V8/NAN/Node C++ addons require the Node backend.

## Reference documentation

- [Node-API](https://nodejs.org/api/n-api.html) describes the opaque `napi_value`
  interface and the ABI stability boundary.
- [Node.js C++ addons](https://nodejs.org/api/addons.html) distinguishes
  Node-API, NAN, and direct V8/Node/libuv addon styles.
- [Node.js CommonJS modules](https://nodejs.org/api/modules.html) documents
  `.node` resolution and loading through `process.dlopen()`.
