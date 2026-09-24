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

The Node-API contract is portable; each `.node` binary is still specific to an
OS, CPU architecture, and its external native dependencies. Select a matching
prebuild or build the addon for the desktop target.

“Full compatibility” means a published compatibility matrix with differential
tests, not merely exporting every function name from `node_api.h`. Unknown or
unsupported behavior must fail with a specific error rather than silently
returning plausible but incorrect results.

## Guest API and compatibility scope

The guest uses ordinary CommonJS requests:

```js
const direct = require("./native/example.node");
const packageAddon = require("example-addon");
```

`require("module.node")` without `./` is a package request resolved through
`node_modules`; `require("./module.node")` is a file request relative to the
requiring module. Both go through napi-vm's resolver and cache. Guest
JavaScript and JSON execute in the VM, never through host `require()`. The
guest API uses no VM-specific module namespace.

The in-process target is the stable Node-API C ABI, including addons written
with `node-addon-api` or Rust bindings such as napi-rs when they only import
Node-API symbols supported by the selected version. “Full” means complete
behavior for every API napi-vm advertises, including lifecycle and error
semantics. It does not mean emulating the distinct V8, NAN, Node C++, or libuv
ABIs; those addons use the Node sidecar. Node describes Node-API as
runtime-independent and ABI-stable, while its V8, C++, and libuv interfaces do
not carry the same cross-major compatibility guarantee ([Node-API](https://nodejs.org/api/n-api.html), [C++ addons](https://nodejs.org/api/addons.html)).

The existing Rust desktop API is already usable as the plan's host entry
point:

```rust
use napi_vm::{Interpreter, RustNodeApiOptions};

let mut vm = Interpreter::with_builtins();
let native_runtime = vm.enable_native_addons(
    RustNodeApiOptions::new([app_root.clone()])
        .allow_native_addon_with_sha256(addon_path, trusted_sha256)
        .entry(app_root.join("main.cjs")),
)?;
vm.eval_source(r#"
  const addon = require("./native/example.node");
  addon.run();
"#)?;
native_runtime.shutdown()?;
```

For a pure Rust desktop build, depend on the crate with default features off
and `node-api-host` enabled. Building that feature also requires a C compiler
for the small Node-API ABI shim. Native addons remain explicitly enabled,
allowlisted, and digest-pinned because they run with the desktop process's OS
privileges.

This document is the delivery roadmap. The more detailed per-API implementation
notes and known behavior differences are tracked in
[the Node addon runtime plan](node-addon-runtime-plan.md).

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
- A compiled napi-rs addon now runs the same CommonJS entry under Node, Bun,
  and napi-vm. Its fixture covers Rust functions, classes, structured objects,
  optional arguments, callbacks, JSON values, string enums, multiword BigInts,
  typed arrays, Buffers, thrown errors, and async tasks. It verifies the
  optional module API-version getter behavior: when that getter is absent,
  napi-vm uses Node's default Node-API module version (v8) for
  `napi_register_module_v1`.
- A source audit found shim symbols for the functions in the local Node v26
  headers. Semantic support is still a subset: exporting a symbol does not
  mean its contract is implemented.
- Native callbacks use the existing VM event queue and interpreter-thread
  checkpoints. The runtime exposes `run_event_loop_once` to desktop hosts.
- `Interpreter::enable_native_addons` is the first shared configuration entry
  point. It accepts the existing `NodeAddonOptions` or `RustNodeApiOptions` and
  returns a `NativeAddonRuntime` identifying the selected backend. Backend
  installation now uses one helper, and `NativeAddonBackendHost` combines the
  existing addon loader and host event bridge. `NativeAddonRuntime::shutdown`
  now provides owner-thread, idempotent teardown for either backend. Backend
  options and backend capability checks still differ.
- Both native backends now enforce canonical roots, the `.node` extension, and
  the configured SHA-256 pin inside their own loader methods as well as in the
  CommonJS path. Direct calls through the public backend interface therefore
  cannot bypass the addon allowlist.
- Both backends use the same ELF, Mach-O, and PE header preflight to reject a
  malformed binary or a binary for another host architecture before invoking
  the native loader.
- `NativeAddonRuntime::preflight_addon(path)` exposes the root, digest, file
  format, and architecture checks to desktop hosts without invoking an addon
  initializer. Each backend uses the same check when it later loads the addon.

## Current compatibility status

| Area | Current state | Remaining work |
| --- | --- | --- |
| CommonJS `require()` | VM resolver handles JS, JSON, direct `.node`, package exports, cache, cycles, and configured native prebuild helpers. | Audit parity against the supported Node resolution contract and retain differential tests for each behavior. |
| Node sidecar | Uses a configured Node executable and provides the broadest addon compatibility. | Package and ship the selected Node runtime for desktop apps; keep this backend explicit. |
| Rust Node-API host | Opt-in `node-api-host`; broad but partial Node-API v1-v10 behavior. Missing imported symbols fail during load. | Complete the advertised stable API surface. A max-version setting is not a completeness claim. |
| Backend selection | The host explicitly chooses `NodeSidecar` or `RustNodeApi`. | Add `Auto` only after preflight can prove backend suitability without running addon initialization. |
| Platforms | Linux runtime tests pass; Windows GNU has cross-build/Wine coverage. | Add native macOS, Windows MSVC, and Windows GNU CI before advertising those targets as verified. |
| Guest module names | Plugin APIs use standard `node:` facades or package names. | Keep guest fixtures and docs free of VM-specific module specifiers. |

## Public host configuration

Keep current APIs working. `NativeAddonPolicy` shares filesystem roots, the
native addon integrity allowlist, and the application entry across backend
options; each backend wrapper retains its own runtime-specific settings. The
dispatcher accepts the existing backend-specific options directly:

```rust
runtime.enable_native_addons(
    RustNodeApiOptions::new([app_root.clone()])
        .allow_native_addon_with_sha256(addon_path, trusted_digest)
        .entry(app_root.join("main.cjs")),
)?;
```

`NodeAddonOptions` selects `NodeSidecar`; `RustNodeApiOptions` selects
`RustNodeApi`. This preserves all existing backend-specific configuration
while using the same common filesystem and integrity policy. A policy can be
passed to `NodeAddonOptions::with_policy(node_executable, policy)` or
`RustNodeApiOptions::with_policy(policy)`.

The backend choices should be:

- `RustNodeApi`: in-process support for Node-API addons, with no Node
  executable dependency.
- `NodeSidecar`: use the configured Node binary for Node-API and Node-ABI
  addons.
- `Auto`: preflight the binary and choose a compatible backend before running
  its initializer. Base this decision on explicit package metadata or static
  import/dependency inspection. Never probe by loading the library, and never
  retry in another backend after initialization starts, since library
  constructors and addon initialization can have side effects. If inspection
  is inconclusive, require an explicit backend choice.

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
- Include imported symbols from representative C, C++ `node-addon-api`, and
  napi-rs addons; distinguish stable Node-API from experimental functions and
  Node-runtime-specific imports.
- Record whether each function is stable, experimental, or Node-specific, and
  its expected behavior during initialization, callbacks, and teardown.
- Give every unsupported import or operation a diagnostic naming the API,
  requested version, selected backend, and any known alternative.

**Exit condition:** a reviewer can tell exactly which addon class and API
surface each backend accepts before running guest code.

### 2. Make backend selection and module loading one host feature

- Keep common format and architecture checks in the shared native-binary
  preflight. Extend `NativeAddonBackendHost` with backend capability checks
  before initialization. Its current supertraits cover addon initialization,
  host calls, and event polling, and its idempotent shutdown is explicit
  through `NativeAddonRuntime`.
- Share roots, the addon allowlist, and entry selection through
  `NativeAddonPolicy`; retain backend-specific settings on each options type.
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
- Keep the current checkpoint-based `Rc` ownership approximation for
  zero-count weak references clearly marked as partial. It can clear acyclic
  values with no other runtime roots, but does not collect cycles, all wrapper
  records, or shared-buffer wrappers. Do not claim full weak-reference or
  collection-time finalizer compatibility until a tracing collector or
  equivalent lifetime model can prove those cases.
- Keep finalizers deterministic at host shutdown while ordinary guest-object
  collection is unavailable, and test callback timing against Node before
  advertising stronger lifecycle compatibility.
- Define shutdown behavior when native threads retain thread-safe functions or
  handles, and keep addon libraries mapped until callbacks and finalizers can
  no longer enter them.

**Exit condition:** lifecycle behavior is covered by deterministic tests and
the compatibility matrix clearly marks any remaining GC differences.

### 5. Build the differential addon suite

- Compile small C and C++ `node-addon-api` fixtures against pinned Node
  headers, plus a small napi-rs addon, and run each same guest fixture under
  Node, Bun where applicable, and napi-vm. Keep the fixture source identical
  between runtimes.
- Compare structured results: status, stdout, values, error names/messages,
  callback order, and shutdown events. Normalize only unstable data such as
  paths and stack traces.
- Organize cases by Node-API version and behavior family. Include package
  wrappers, direct `.node` imports, conditional exports, prebuild selection,
  circular loads, nested callbacks, worker completions, and permissions.
- Include both `require("./fixture.node")` and bare package requests such as
  `require("fixture")`; test extension fallback, `exports` conditions, and
  `require.resolve()` without triggering addon initialization.
- Classify each difference as module resolution, missing API, wrong semantics,
  event-loop ordering, host bridge, sandbox policy, native ABI, or runtime
  difference. Node is the primary reference; preserve Bun differences as
  explicit observations instead of treating them as Node's contract.
- Give every discovered mismatch a regression fixture or a documented,
  runtime-specific result. Normalize paths and unstable stack details only;
  do not normalize observable API behavior.
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
- Provide a Rust desktop example that selects each backend, runs the same
  CommonJS entry, pumps external events, and shuts down deterministically.
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

Portable guest APIs and native addon loading are separate concerns. Guests use
standard JavaScript, Web APIs, or Node built-ins such as `node:fs` where needed.
Native `.node` packages use ordinary CommonJS `require()` through the Rust
loader; the guest does not import a VM-specific module specifier to reach them.
