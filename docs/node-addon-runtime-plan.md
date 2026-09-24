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
| Rust Node-API host (experimental) | Selected Node-API v1-v10 calls; Linux runtime-tested, Windows GNU path cross-compiled and Wine-tested, macOS and native Windows verification pending | `napi-vm`, a C compiler at build time, and the platform dynamic loader |

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
        .max_napi_version(10));
```

The exact Rust names can follow the existing `NodeAddonOptions`; the important
parts are backend choice, allowed filesystem roots, integrity pins, supported
Node-API version, and a stable compatibility report. A guest `require()` call
still goes through the VM's CommonJS resolver and module cache. Module-local
`require.resolve(specifier)` uses that same resolver to return a filename
without executing JavaScript or initializing a native addon.

The current implementation is available behind Cargo feature `node-api-host`:
`Interpreter::enable_rust_node_api_addons(RustNodeApiOptions)` uses the
existing CommonJS resolver and allowlist. Its `max_napi_version` option
defaults to 10, controls `napi_get_version`, and rejects registrations above
the configured ceiling before calling their initializer. This is a version
ceiling rather than a claim that every function in that Node-API version is
implemented. Before opening an addon, the in-process loader checks ELF headers
on Linux, Mach-O headers on macOS, and PE headers on Windows for library type,
class, and host architecture. The Windows backend builds a DLL image named
`node.exe` to satisfy Node-API import libraries and adds its private directory
only to flagged addon loads. Its GNU target compiled and loaded a fixture under
Wine; native Windows and MSVC execution remain unverified. On Linux it currently
covers scoped handles, callback
info, synchronous C callbacks, global-object access, named and general property
operations, inherited enumerable property-name enumeration, primitive values,
numbers, UTF-8, Latin-1, and well-formed UTF-16 string conversion, boolean,
number, string, and object coercion, symbol creation, and `napi_define_properties`
for ordinary object, array, class, and ordinary function targets (indexed and
named array properties, data values, symbol keys, native methods, and
accessors),
`napi_define_class` with native constructors, static descriptors, and
prototype descriptors, `napi_typeof`, and array creation/index/length
operations including `napi_delete_element`. Escapable handle scopes can
promote one local handle into the parent scope. Class static properties share
object descriptor metadata and accessor behavior.
`napi_get_value_int64` truncates finite Numbers toward zero, clamps values
outside the signed 64-bit range, and converts NaN and infinities to zero.
`napi_coerce_to_number` and `napi_coerce_to_string` perform guest-side
conversion through the existing callback dispatcher. They honor
`Symbol.toPrimitive`, `valueOf`, and `toString`; Symbol-to-string and
Symbol/BigInt-to-number conversions raise guest `TypeError`s. These APIs do
not call guest code directly from an arbitrary native context and require an
active interpreter callback dispatcher.
`napi_coerce_to_object` preserves existing object identity, rejects `null` and
`undefined` with a pending guest `TypeError`, and returns boxed primitive
objects with working `valueOf()` and basic `toString()` behavior. This
conversion does not re-enter the interpreter because it does not invoke guest
code. The guest `Object(value)` constructor shares the wrapper representation.
Boxed Boolean, Number, String, Symbol, and BigInt values use the same
materialized constructor prototypes in guest reflection and `napi_get_prototype`.
`new Boolean`, `new Number`, and `new String` create boxed values; `Symbol` and
`BigInt` reject construction.
`napi_get_prototype` preserves explicit prototypes and the realm's
`Object.prototype` identity for ordinary objects, and returns the shared
`Function.prototype` for ordinary guest functions, class constructors, and
native callback functions. Arrays use the realm's shared `Array.prototype`,
which is itself an array and inherits from `Object.prototype`. Promise values
use a shared `Promise.prototype` with the standard constructor and non-enumerable
`then`, `catch`, and `finally` methods. Date values use a shared
`Date.prototype` with the implemented date methods and standard non-enumerable
descriptors. Queries for values whose built-in prototype is not represented
(including proxies) return a generic Node-API failure.
`napi_instanceof` handles VM class constructors and ordinary function
constructors, inherited prototypes, VM error classes, and the shared
`Function.prototype[Symbol.hasInstance]` intrinsic. Guest-defined
`Symbol.hasInstance` methods run through the active paused-callback dispatcher;
they receive the constructor as `this`, and their results follow JavaScript
truthiness. Ordinary functions share a lazily created own
`prototype` object with constructed instances and inherit from a shared
callable `Function.prototype`; function `name`, `length`, and `prototype`
descriptors participate in guest and Node-API property reflection. The current
Function.prototype method surface includes `call`, `apply`, `bind`, and
`[Symbol.hasInstance]`.
`apply` accepts array-like arguments, and bound functions preserve call,
construction, prototype, `name`, and `length` behavior for supported targets.
Source-aware `toString` behavior and callable proxies remain incomplete.
`napi_call_function` and `napi_new_instance` enter guest code through the
interpreter's paused host-call callback handler; nested native calls and
pending guest exceptions stay on that controlled call path. `napi_run_script`
uses the same dispatcher to execute source in the VM and leaves queued jobs for
the normal event-loop checkpoint. `napi_async_init` and `napi_async_destroy`
track native async-context lifetimes, `napi_open_callback_scope` and
`napi_close_callback_scope` validate nested scope lifetimes, and
`napi_make_callback` uses the same controlled guest-callback dispatcher.
Async-context resource metadata is retained until destroy, but Node's
`async_hooks` and `AsyncLocalStorage` propagation are not implemented.
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
supported. `napi_create_external` and `napi_get_value_external` preserve an
external's native pointer and distinct `napi_typeof` tag. In guest JavaScript,
an external behaves like a non-extensible, null-prototype object with no own
properties. Its finalizer runs during host shutdown because this VM does not
collect guest objects. N-API buffers are surfaced as guest `Uint8Array` views.
`napi_create_external_arraybuffer` and `napi_create_external_buffer` expose
addon-owned memory without copying, so guest typed-array writes are visible to
the native allocation. The environment retains these values and invokes their
finalizers once on the owner thread at host shutdown; finalization at ordinary
guest garbage-collection time is unavailable. ArrayBuffer, typed-array, and
DataView creation, type checks, and info APIs share storage with guest views
and preserve byte offsets. `napi_adjust_external_memory` keeps a checked per-environment
total and returns the updated value; the VM has no garbage collector to tune.
Node-API v5 date creation, type checks, and value reads use the VM's Date
objects. `napi_add_finalizer` supports optional zero-count references and runs
registered callbacks on the owner thread during environment shutdown, after
cleanup hooks. As with wraps, ordinary guest-object collection is unavailable.
The selected Node-API v6 surface includes BigInt creation and extraction,
`napi_get_all_property_names` on ordinary objects, classes, ordinary
functions, arrays, errors, and regular expressions, plus Proxy chains with
`ownKeys` traps when the addon call has an active guest callback dispatcher,
and environment instance data. Property collection supports own-only or
prototype-chain keys, writable/enumerable/configurable filters, string and
symbol keys, and numeric key conversion. For Proxy `ownKeys` results, the Rust
host follows Node's behavior: enumerable filtering consults target property
descriptors, while writable/configurable filters preserve the returned keys.
Bun applies target descriptors to all three filters; the v7 differential
fixture records this runtime difference. Global-object reflection supports
own and prototype-chain key collection plus string/symbol filtering during an
active guest callback. The VM's shared `Object.prototype` supplies the standard
Node/Bun string-key method set, including `toString`, `valueOf`, `isPrototypeOf`,
accessor helpers, and the `__proto__` accessor. Writable/enumerable/configurable
filters on the global object fail because global property attributes are not
represented by the VM. BigInt word arrays are limited to 2,048 words by the
VM's BigInt allocation cap. Replacing environment instance data overwrites the
previous slot without calling its finalizer; the active finalizer runs on the
owner thread during shutdown after cleanup hooks.
The selected Node-API v7 surface supports idempotent ArrayBuffer detachment,
updates existing typed-array views, and exposes detached-state checks. The VM
matches Node when `napi_is_detached_arraybuffer` receives a non-ArrayBuffer
value (`napi_ok`, false); Bun 1.4.0 returns `napi_arraybuffer_expected` for
that input, which the differential fixture records as a runtime difference.
The selected Node-API v8 slice adds type tags for identity-bearing guest
objects, freeze/seal for ordinary objects, arrays, ordinary functions, and
class constructors, and async cleanup hooks. Array index and `length`
descriptors participate in guest writes, deletion, truncation, Node-API
element writes, and property reflection. Implemented mutating array methods
reject writes to frozen arrays. Function integrity operations first materialize
the standard own `name`, `length`, and `prototype` properties so the resulting
descriptors are frozen or sealed. Hooks start in reverse
registration order; asynchronous hooks
start without blocking the remaining hooks, and shutdown waits for all of them
to remove their handles before running finalizers. Addon libraries remain
mapped through cleanup. Tagged values stay retained until host shutdown because
the VM has no object garbage collector, and the tag table is capped. Other
object representations do not yet expose the property metadata needed by
freeze/seal and return a generic failure. The v8 fixture compares tag and
integrity results with Node and Bun; async shutdown is compared with Node
because Bun 1.4.0 exits without awaiting async cleanup hooks.
The selected Node-API v9 functions provide the global symbol registry,
SyntaxError creation and throwing, and the addon's `file://` URL.
`node_api_symbol_for` shares identity with guest `Symbol.for`; module URL
storage remains valid for the lifetime of its addon environment.
The selected Node-API v10 functions provide external Latin-1 and UTF-16 string
creation with eager copy/finalizer handling, string property-key creation, and
zero-copy `Buffer` views over `ArrayBuffer` storage. Node and Bun fixtures check
the external-string copy/finalizer contract, Unicode keys, buffer aliasing, and
out-of-range errors.
The experimental `node_api_set_prototype` and
`node_api_create_object_with_properties` entry points are available to addons
compiled with `NAPI_EXPERIMENTAL`; they update prototype metadata or create an
ordinary object with ordered data properties. Prototype mutation is supported
for ordinary VM objects, ordinary guest functions, and class constructors,
and object creation accepts ordinary object/function/class prototypes or
null. Specialized object representations
whose prototype model is not implemented fail explicitly. These functions do
not raise the stable Node-API version reported by the host. Their fixture
compares prototype identity, inherited behavior, and property values with
Node. Bun 1.4 does not export these experimental symbols, so that comparison is
recorded as unsupported there.
`node_api_post_finalizer` queues a native finalizer as an external event for
the interpreter's owner thread. The callback can use Node-API after the queued
event runs; it never executes directly on the posting thread. During host
shutdown, new posts are closed and already queued finalizers run before addon
libraries unload. Guest callback dispatch is unavailable during this final
shutdown drain, so finalizers that need to enter JavaScript must be processed
while the runtime event loop is still active.
The experimental `node_api_create_sharedarraybuffer`,
`node_api_create_external_sharedarraybuffer`, and
`node_api_is_sharedarraybuffer` entry points are exported. Owned shared byte
stores have aligned backing memory and are distinct from ordinary
`ArrayBuffer` values. Guest `SharedArrayBuffer`, typed-array, and `DataView`
accesses use atomic byte operations, and structured cloning creates a new
shared-buffer object over the same data block. External shared buffers preserve
the `node_api_noenv_finalize` callback signature and run that callback on the
owner thread during host shutdown. The guest `Atomics` object supports
`isLockFree`, `load`, `store`, arithmetic and bitwise read-modify-write calls,
`exchange`, and `compareExchange` for integer typed arrays. `waitAsync` and
`notify` use the existing shared job queue: notification settles waiting
promises and timeout jobs resolve them at timer checkpoints. `Atomics.wait`
handles immediate `not-equal` and zero-timeout results, then reports a clear
unsupported-operation error when it would block because napi-vm does not yet
provide guest worker agents. Timeouts follow napi-vm's deterministic timer
ordering, not a wall clock. Full blocking wait behavior remains a
`MISSING_METHOD` compatibility gap until worker agents can notify the runtime
without re-entering a running interpreter. The Node differential fixture also
checks that `Atomics.notify` through a structured-cloned
`SharedArrayBuffer` wakes the original wait list. Bun 1.4.0 currently reports
zero waiters and times out in that case; the test records this as a
`WRONG_SEMANTICS` difference instead of normalizing it.
The stable v1 `napi_get_node_version` function returns a numeric compatibility
profile from `RustNodeApiOptions::reported_node_version`; the default is
`0.0.0`, and the release name is `napi-vm`. This reports metadata only and does
not claim that every API available in the configured Node version is supported.
Both symbol-based `NAPI_MODULE` registration and the deprecated
`napi_module_register` static-constructor path load through the same allowlisted
CommonJS route. `napi_get_uv_event_loop` resolves at link time but returns
`napi_generic_failure` and a null output because this host does not embed libuv.
The deprecated registration descriptor has no Node-API version field, so its
single-module registration path is conservatively treated as v1. The stable
`napi_fatal_exception` path uses the VM's existing external-event queue, offers
the error to `process.emit('uncaughtException', error)`, and otherwise returns
the uncaught throw to the Rust embedder. `napi_fatal_error` reports its message
and terminates the process.
`napi_create_promise`, deferred resolution/rejection, and
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
macOS host before it is claimed as verified. Windows addons that import
`node.exe` use the generated PE DLL provider and Windows DLL search flags; the
GNU target was cross-compiled and exercised under Wine, while native Windows
and MSVC execution still need CI verification. The checked-in
`tests/fixtures/node-api/node-api-smoke.c` exercises the platform import paths;
native CI builds it against the generated `node.exe` import library on Windows
and with dynamic symbol lookup on macOS, then runs the same
`require('./fixture.node')` check.
The VM's Rust `String` representation cannot preserve isolated UTF-16 surrogate
code units, so `napi_create_string_utf16` currently returns
`napi_generic_failure` for malformed UTF-16 instead of replacing or dropping
those code units.

## Implementation phases

### 1. Freeze the module-loading contract

- Audit the existing CommonJS resolver, `.node` allowlist, integrity checks,
  package `exports`, and cache behavior.
- The filesystem resolver treats `node-addons` as an active package condition
  only when a native addon provider is configured. Without a provider, it skips
  that branch and may resolve the JavaScript fallback, matching Node's
  `--no-addons` mode. A chosen `.node` file still requires its own allowlist
  entry and integrity check.
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

- [x] Resolve a package's `build/Release`, `build/Debug`, or
  `prebuilds/<platform>-<arch>` Node-API binary. Prebuild selection matches
  platform, architecture, `glibc`/`musl`, and `armv` tags; it rejects files
  carrying a Node ABI or libuv tag.
- [x] Let `RustNodeApiOptions::allow_native_prebuild()` map a bare guest
  package request to the selected addon while preserving the normal
  `require()` cache, root check, and digest allowlist. This host alias replaces
  the package's JavaScript entry for that request; use it when the native
  exports are the package API.
- [x] Publish an initial CommonJS exports object before invoking the Rust
  Node-API initializer, replace it with the returned exports on success, and
  remove the entry after initialization failure so the next `require()` can
  retry.
- [ ] The adapter does not implement the complete `node-gyp-build` JavaScript
  API or its `EXEC_PATH`, `PREBUILDS_ONLY`, and runtime-specific Node ABI/uv
  selection behavior.
- Keep native path resolution in the Rust package loader. Support common
  package export conditions and platform prebuild layouts without running
  package install scripts.
- Require OS, architecture, and binary-format matches. Report the exact reason
  for a rejected file: missing allowlist entry, digest mismatch, wrong
  architecture, unsupported ABI, missing symbol, or initialization failure.
- Cache native exports by canonical module ID. Full circular initialization
  behavior still depends on a safe guest callback entry point during addon
  initialization.

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
  interface, the ABI stability boundary, and experimental
  [`node_api_set_prototype`](https://nodejs.org/api/n-api.html#node_api_set_prototype) and
  [`node_api_create_object_with_properties`](https://nodejs.org/api/n-api.html#node_api_create_object_with_properties) and
  [`node_api_post_finalizer`](https://nodejs.org/api/n-api.html#node_api_post_finalizer),
  [`node_api_create_sharedarraybuffer`](https://nodejs.org/api/n-api.html#node_api_create_sharedarraybuffer),
  and [`node_api_create_external_sharedarraybuffer`](https://nodejs.org/api/n-api.html#node_api_create_external_sharedarraybuffer).
- [Node.js C++ addons](https://nodejs.org/api/addons.html) distinguishes
  Node-API, NAN, and direct V8/Node/libuv addon styles.
- [Node.js CommonJS modules](https://nodejs.org/api/modules.html) documents
  `.node` resolution and loading through `process.dlopen()`.
