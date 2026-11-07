# Node.js API

The binding is exported through [index.js](../index.js) and
[index.mjs](../index.mjs). [index.d.ts](../index.d.ts) is the generated signature
reference. Examples here run from a checkout; replace `./index.js` with
`napi-vm` when importing an installed package.

## Evaluate code

| Entry point | Result | Lifetime |
| --- | --- | --- |
| `new Vm()` / `createVm()` | VM handle | Persistent guest globals and modules |
| `runCode(source)` | Formatted string | Fresh evaluation |
| `vm.run(source)` | Formatted string | Uses the existing VM |
| `vm.runAsync(source)` | `Promise<string>` | Uses the existing VM's async bridge |
| `vm.callFunction(name, args)` | Converted host value | Looks up and invokes a guest function |

`VM` is a type alias for `Vm`. A failed evaluation throws at the binding boundary;
an asynchronous failure rejects its promise. Guest mutations performed before
an error can remain visible. Use a fresh VM when an evaluation needs separate
state.

## Globals and host callbacks

| Method | Behavior |
| --- | --- |
| `setGlobal(name, value)` | Copies/converts a host value into a global property |
| `getGlobal(name)` | Returns the formatted guest value as a string |
| `hasGlobal(name)` / `removeGlobal(name)` | Inspect/remove the host-visible global entry |
| `exposeFunction(name, callback)` | Installs a synchronous host callback |
| `exposeAsyncFunction(name, callback)` | Installs an async callback for `runAsync` |

Global lexical declarations and global object properties are distinct. Removing
a global property is not a general way to remove a guest `let` or `const`
binding. Each callback is trusted application code. Its implementation owns
argument checks, access policy, and external effects.

## Value conversion

Strings preserve JavaScript UTF-16 code units, including lone surrogates.
Ordinary JSON-compatible values are appropriate for application messages.
The native bridge also handles selected richer values such as BigInts, dates,
regular expressions, collections, buffers, and typed views; this is not a
general structured-clone contract.

ArrayBuffer/typed-array transfers produce owned copies. Exporting a view copies
its effective visible range, rather than exposing the guest's backing memory.
Host typed arrays backed by SharedArrayBuffer are rejected, including empty
views and Node Buffers over shared memory. Conversion has depth/size guards;
do not depend on arbitrary cycles or repeated Map/Set references retaining
their identity. See [marshal.rs](../src/bindings/marshal.rs) and its
[boundary regressions](../tests/node/typed-array-subviews.test.js).

```javascript verify
const { Vm } = require('./index.js');
const assert = require('node:assert/strict');
const vm = new Vm();
try {
  vm.run('function bytes() { return new Uint8Array([2, 4, 6]); }');
  const result = vm.callFunction('bytes', []);
  assert(result instanceof Uint8Array);
  assert.deepEqual([...result], [2, 4, 6]);
} finally {
  vm.dispose();
}
```

## Modules

| Method | Behavior |
| --- | --- |
| `validateModule(source)` | Parse-only `{ valid, diagnostics }`; does not register, resolve imports, or execute code |
| `defineModule(name, source)` | Registers lazy source |
| `registerModule(name, source)` | Evaluates eagerly and replaces exports only after success |
| `registerHostModule(name, exports, { async? })` | Wraps host functions; returns generated global names |
| `hasModule(name)`, `listModules()`, `removeModule(name)` | Inspect or revoke module resolution |
| `setImportMetaMain(boolean)` | Selects the main-module flag for guest `import.meta` |

Eager replacement is transactional over exports, not arbitrary guest/global/host
side effects. Removing a module prevents subsequent resolution and removes its
installed bridge globals; previously captured guest references can survive.
See [modules](modules.md) for complete lifecycle guidance.

## AsyncSession

`new AsyncSession(options?)` provides an async-only owner worker and bounded
command channel. Its `run` and `evaluate` return `Promise<string>`. Methods
`exposeFunction`, `getGlobal`, `setGlobal`, `defineModule`, `advanceClock`,
`pollEventLoop`, `setExecutionLimits`, `collectCycles`, and `evaluationStats`
return promises. `cancel()`, `dispose()`, and `wakeups()` are synchronous.

Options are `commandCapacity`, `timers`, `clock`, `fairness`, `maxJobsPerTurn`,
and `autoPoll`. Clocks accept `legacy`, `virtual`, or `real-time`; fairness
accepts `external-first` or `alternate`. Timers require the runtime feature.
Use [async execution](async.md) for runnable examples and scheduler semantics.

## Limits, diagnostics, and disposal

`Vm.setLoopLimit(n)` caps loop iterations. It is not an OS memory or CPU limit.
`evaluationStats()` and `heapStats()` return diagnostic JSON strings.
`collectCycles()` performs collection at a quiescent VM boundary and returns a
count. These diagnostics do not grant safe concurrent access to guest values.

Call `dispose()` in `finally`, especially after `runAsync` or async host-function
registration. Async dispatch handles can otherwise keep Node's environment
alive. Disposal is idempotent and retires bridge handles; do not use it as a
transaction rollback or assume it reverses callbacks already running.

## Optional runtime globals

`vm.enableRuntime({ console, timers, webApis, nodeCompat })` selects globals
that were compiled into the addon. It does not grant filesystem, networking,
environment, process, or addon permissions. Missing build features are reported
as errors. See [runtime feature selection](runtime.md).

## LanguageService

`new LanguageService()` analyzes documents without executing their source.
`open(uri, source)`, `update(uri, source)`, and `close(uri)` manage documents.
`complete(uri, offset)`, `hover(uri, offset)`, and `diagnostics(uri)` query them.
Offsets are UTF-8 byte offsets in the Rust source, not JavaScript UTF-16 indices.

`registerModule`, `registerHostFunction`, and `registerRuntimeShape` add trusted
metadata. They do not execute a runtime function. The [editor guide](editor.md)
explains offsets, the standalone LSP, and live-session discovery.

`debugParse(source)` exposes parser debugging text; it is not a stable AST wire
format. Use `validateModule` when a host needs a structured syntax check.
