# Rust desktop native addon loader plan

## Goal

Let a Rust desktop application run guest code such as:

```js
const addon = require("example");
const direct = require("./native/example.node");
```

Keep package resolution, CommonJS caching, and the guest-facing `require()` API
inside napi-vm. Let the host choose how native code runs. Node-API addons should
be portable across supported host platforms; addons that depend on Node's V8,
NAN, Node C++ APIs, or libuv ABI need a compatible Node runtime.

“Full compatibility” means a published compatibility matrix with differential
tests, not merely exporting every function name from `node_api.h`. Unknown or
unsupported behavior must fail with a specific error rather than silently
returning plausible but incorrect results.

## Existing foundation

- `FileCommonJsLoader` resolves guest JavaScript and JSON within configured
  roots. Guest modules are never evaluated with host `require()`.
- `NodeAddonSidecar` loads allowlisted addons with a real Node-API environment
  in a Node child process. This is the broad compatibility backend for addons
  that need Node's ABI.
- The experimental `node-api-host` feature loads Node-API `.node` libraries in
  the Rust process, with root checks, SHA-256 pinning, format checks, a version
  ceiling, and a `node-gyp-build` prebuild selector.
- The Rust backend already covers substantial Node-API behavior, including
  values and callbacks, handles, classes, binary data, Promises, async work,
  thread-safe functions, cleanup, and selected APIs through v10.
- A source audit found shim symbols for the functions in the local Node v26
  headers. Semantic support is still a subset: exporting a symbol does not
  mean its contract is implemented.
- Native callbacks use the existing VM event queue and interpreter-thread
  checkpoints. The runtime exposes `run_event_loop_once` to desktop hosts.

## Public host configuration

Keep current APIs working and converge on a small backend choice in a future
builder API. For example:

```rust
let options = NativeAddonOptions::new([app_root.clone()])
    .backend(NativeAddonBackend::RustNodeApi)
    .allow_native_addon_with_sha256(addon_path, trusted_digest)
    .entry(app_root.join("main.cjs"));

runtime.enable_native_addons(options)?;
```

The backend choices should be:

- `RustNodeApi`: in-process support for Node-API addons, with no Node
  executable dependency.
- `NodeSidecar`: use the configured Node binary for Node-API and Node-ABI
  addons.
- `Auto`: preflight the binary and choose a compatible backend before running
  its initializer. Never retry in another backend after addon initialization
  has started, since initialization can have side effects.

The guest keeps using `require()`. Backend choice, host process, integrity
checks, and transport stay in Rust host configuration. Keep native loading
disabled unless explicitly enabled and allowlisted.

## Work plan

### 1. Freeze the loader contract and compatibility inventory

- Document the resolution order for direct `.node` paths, bare packages,
  `exports` conditions, `node-addons`, `main`, and JavaScript fallback.
- Separate guest JavaScript resolution from native binary loading. Never use
  host `require()` for arbitrary package JavaScript.
- Maintain a checked-in Node-API inventory by version and function, with
  statuses `implemented`, `partial`, `unsupported`, and `not applicable`.
- Record whether each function is stable, experimental, or Node-specific, and
  its expected behavior during initialization, callbacks, and teardown.
- Give every unsupported import or operation a diagnostic naming the API,
  requested version, selected backend, and any known alternative.

**Exit condition:** a reviewer can tell exactly which addon class and API
surface each backend accepts before running guest code.

### 2. Make backend selection and module loading one host feature

- Introduce a backend trait around load, initialize, call, event pump, and
  shutdown. Adapt `NodeAddonSidecar` and the Rust Node-API host behind it.
- Route `.node` requests through that host feature while leaving JavaScript and
  JSON modules in the existing guest loader and cache.
- Preserve provisional CommonJS exports, failed-initialization rollback,
  circular require behavior, package conditions, `require.resolve`, and
  `node-gyp-build` selection.
- Preflight platform, architecture, addon format, Node-API version, integrity,
  and backend capability before calling an initializer.
- Keep allowlisted paths inside canonical application roots and verify the
  configured digest at setup and load time.

**Exit condition:** one guest fixture using `require("example")` works with
each configured backend without changing its source.

### 3. Complete core Node-API semantics in the Rust backend

Use the versioned function inventory to finish and test the stable APIs in
dependency order:

1. Handles, scopes, callback info, property keys, descriptors, exceptions,
   status codes, and last-error information.
2. Primitives, objects, arrays, functions, symbols, errors, classes, and
   prototype behavior.
3. References, wraps, type tags, instance data, finalizers, and cleanup hooks.
4. Buffers, ArrayBuffers, typed arrays, DataViews, BigInts, Dates, and
   regular expressions.
5. Promises, deferreds, async work, thread-safe functions, and callback
   lifetime rules.
6. Newer stable Node-API versions and Node-specific APIs, each gated by the
   configured reported version.

Match Node's observable return statuses, pending-exception behavior, property
attributes, coercions, callback receiver/arguments, and teardown ordering.
Add a regression fixture whenever an incompatibility is found. Do not mark an
API implemented merely because a C shim symbol exists.

**Exit condition:** every API reported as supported has passing compiled-addon
fixtures for normal behavior, invalid arguments, pending exceptions, and
relevant teardown paths.

### 4. Resolve object lifetime and thread-affinity gaps

- Specify owner-thread rules for all Node-API calls and ensure native worker
  callbacks only enqueue events. Guest callbacks run at VM checkpoints, then
  drain the VM's existing microtask queue.
- Exercise nested synchronous callbacks and addon re-entry without mutably
  re-entering an executing interpreter.
- Decide the heap strategy required for weak references and collection-time
  finalizers. The current VM retains values and can run finalizers at shutdown,
  which is not equivalent to Node garbage collection. If full N-API lifecycle
  compatibility is required, implement tracing/collection or an equivalent
  lifetime model before claiming weak refs and ordinary finalizers are
  compatible.
- Define shutdown behavior when native threads retain thread-safe functions or
  handles, and keep addon libraries mapped until callbacks and finalizers can
  no longer enter them.

**Exit condition:** lifecycle behavior is covered by deterministic tests and
the compatibility matrix clearly marks any remaining GC differences.

### 5. Build the differential addon suite

- Compile small C/C++ addon fixtures against pinned Node headers and run each
  same guest fixture under Node, Bun where applicable, and napi-vm.
- Compare structured results: status, stdout, values, error names/messages,
  callback order, and shutdown events. Normalize only unstable data such as
  paths and stack traces.
- Organize cases by Node-API version and behavior family. Include package
  wrappers, direct `.node` imports, conditional exports, prebuild selection,
  circular loads, nested callbacks, worker completions, and permissions.
- Classify each difference as module resolution, missing API, wrong semantics,
  event-loop ordering, host bridge, sandbox policy, native ABI, or runtime
  difference. Node is the primary reference; preserve Bun differences as
  explicit observations instead of treating them as Node's contract.
- Run security cases for traversal, symlinks, changed hashes, unlisted binaries,
  unsupported ABI files, malformed exports, and initialization failure.

**Exit condition:** CI reports per-case compatibility status and category, and
every claimed supported API family has a passing Node differential fixture.

### 6. Verify desktop targets and packaging

- Add native CI jobs for Linux, macOS, Windows MSVC, and Windows GNU. Keep
  cross-compilation separate from runtime validation.
- Test the C ABI shim, dynamic library loading, symbol visibility, unload and
  shutdown behavior, and architecture checks on every supported target.
- Document build-time C compiler requirements, required runtime files, Node
  sidecar packaging, and how applications supply trusted SHA-256 manifests.
- Publish a support table for OS, architecture, backend, Node-API version, and
  known gaps.

**Exit condition:** every advertised desktop target has a native load-and-call
smoke test and a documented deployment recipe.

## Compatibility boundary

A `.node` filename does not identify a portable ABI. The Rust backend can
provide a portable Node-API environment, but it cannot transparently emulate
the full V8, NAN, Node C++, or libuv ABI. The Node sidecar is the compatibility
path for those addons. Both backends execute trusted native code with host
process privileges; root restrictions and hashes identify the code that may
load, but do not sandbox it. Applications that need isolation must put native
addons in a separately restricted process.

The guest-facing `napi:*` namespace is a separate concern from loading native
`.node` files. Portable application APIs should remain standard JavaScript or
Web APIs where those exist; Node-API loader internals belong to the Rust host.
