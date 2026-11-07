# Modules, packages, and native addons

The engine resolves code only through registered modules or loaders supplied by
the host. The Node binding does not forward a guest import to the host's module
loader automatically.

## Register ES module source

`defineModule(name, source)` stores lazy source. Register every source in a
cyclic graph before importing any of it. `registerModule(name, source)` is
eager: it evaluates into a fresh export record and replaces the old exports
only after successful evaluation.

```javascript verify
const { Vm } = require('./index.js');
const assert = require('node:assert/strict');
const vm = new Vm();
try {
  const parsed = vm.validateModule('export const answer = 42;');
  assert.equal(parsed.valid, true);
  vm.defineModule('answer', 'export const value = 42;');
  assert(vm.hasModule('answer'));
  assert.equal(vm.run("import { value } from 'answer'; value;"), '42');
  assert(vm.removeModule('answer'));
  assert.equal(vm.hasModule('answer'), false);
} finally {
  vm.dispose();
}
```

Eager replacement protects the export table, not global assignments, host
callbacks, or other side effects performed before a throw. Revocation stops new
resolution; an already imported value may remain reachable in guest code.

## Export host functions as a module

```javascript verify
const { Vm } = require('./index.js');
const vm = new Vm();
try {
  vm.registerHostModule('host:math', { double: n => n * 2 });
  require('node:assert/strict').equal(
    vm.run("import { double } from 'host:math'; double(21);"), '42');
  vm.removeModule('host:math');
} finally {
  vm.dispose();
}
```

`registerHostModule` creates hidden bridge globals and returns their names.
The optional `async` array marks async export names. Module removal tears down
its installed globals. The function implementation remains responsible for
permissions and input validation.

## Supply Rust loaders

Core loaders include `VirtualLoader`, `DataUrlLoader`, and `CompositeLoader`.
Implement `ModuleLoader` to resolve/load ES module bytes and source in a custom
host. CommonJS uses a separate `CommonJsModuleLoader` interface, installed
through the interpreter or `EngineBuilder::commonjs_loader`.

`FileCommonJsLoader` and runtime file/npm loaders are host integrations. Root
checks, file-size limits, dependency policies, and any transformation belong
at that boundary. Runtime npm support and TypeScript transformation require
their corresponding [features](runtime.md). Package exports do not imply that
the package's APIs are all implemented by the VM.

The TypeScript VM plugin integration has its own
[guest package loader](../plugins/npm/guest-package-loader.ts), resolver, and
compiler interface. Use the portable interfaces for browser/alternate hosts;
Node-specific loading is exported separately from `plugins/node.ts`.

## Native addons

A `.node` binary runs trusted native code. Two Rust backends are available:

| Backend | Execution location | Compatibility boundary |
| --- | --- | --- |
| `NodeAddonSidecar` | A selected Node child process | Node loads the addon; values/calls use the sidecar bridge |
| `RustNodeApiHost` | The Rust host process | The project's implemented Node-API subset; requires `node-api-host` on supported native platforms |

Use `NativeAddonPolicy` to restrict roots, entry resolution, and allowlisted
binary digests. Backend options can be configured through `NodeAddonOptions`
or `RustNodeApiOptions`; `Interpreter::enable_native_addons` installs the
selected backend. Keep expected SHA-256 values in trusted application metadata.
Calculating a digest from an untrusted binary immediately before trusting it
does not establish who authorized that binary.

The runnable [Rust native addon example](../examples/rust_native_addon.rs)
takes an application root, CommonJS entry, addon path, and trusted digest.
The [Rust napi-rs plugin workflow](../examples/plugins/rust-napi-plugin/README.md)
builds an addon and exercises the Rust VM plugin host without embedding Node.

The [vendored rdev-node probe](../examples/rdev_node_vendor.rs) exercises another
real native package. Its source documents the required distribution build and
backend selection. Desktop input/callback checks require an appropriately
isolated display and platform libraries; building the vendored crate alone
does not prove those desktop operations passed.

In-process Node-API support does not imply V8, NAN, Node C++, libuv, or every
Node-API call is implemented. Preflight checks can reject unsupported imports;
they are not a security boundary against malicious native code. Shut down the
backend on its owner thread and test callbacks, finalizers, errors, and process
exit on every supported target.
