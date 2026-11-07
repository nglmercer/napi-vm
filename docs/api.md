# Node API

[index.d.ts](../index.d.ts) is the generated signature reference. This page
explains lifetimes, execution, and return-value differences. Rust ownership
is documented in [crate architecture](crate-architecture.md).

## Execution and values

<a id="vm"></a>

| API | Result and lifetime |
| --- | --- |
| `runCode(source)` | Display string from a fresh VM execution |
| `new Vm()` / `createVm()` | Stateful guest VM |
| `vm.run(source)` | Synchronous execution; returns a display string |
| `vm.runAsync(source)` | Promise of a display string; uses the VM executor |
| `vm.callFunction(name, args)` | Marshalled guest return value, including supported live bridge values |
| `debugParse(source)` | Diagnostic AST string; does not execute source |
| `vm.validateModule(source)` | Parse-only `{ valid, diagnostics }`; no registration or evaluation |

`run` and `getGlobal` do not return arbitrary JavaScript objects. `setGlobal`
and `callFunction` use the host bridge's supported value conversions. Prefer
explicit host functions for privileged behavior, rather than transferring
host objects with implicit capabilities.

Native `ArrayBuffer` and typed-array transfers copy bytes into owned storage;
typed arrays copy only their visible range. Empty and detached host buffers
and typed arrays import as empty copies. Typed arrays backed by a host
`SharedArrayBuffer` are rejected because concurrent writes cannot be copied
safely through this bridge. Guest shared buffers and views are also rejected.

VMs retain state between calls and are isolated from one another. Overlapping
operations on a `Vm` are rejected by its admission guard. Await `runAsync`
before calling another operation on that instance. It uses a lazily created
persistent executor, rather than a new OS thread for every call.

## Globals and callbacks

| Method | Purpose |
| --- | --- |
| `setGlobal(name, value)` | Install a supported structured value |
| `getGlobal(name)` | Read a global as a display string |
| `hasGlobal(name)` | Test presence |
| `removeGlobal(name)` | Remove a binding or exposed function |
| `exposeFunction(name, fn)` | Register a synchronous callback |
| `exposeAsyncFunction(name, fn)` | Register a callback for asynchronous host work |
| `setLoopLimit(n)` | Set the per-execution loop-iteration budget |
| `setImportMetaMain(value)` | Configure `import.meta.main` |

Host exceptions are propagated through the bridge. The host owns argument
validation and permissions. Limits on guest loops do not preempt a blocking
host callback. [Safety and limits](safety.md).

## Modules

<a id="modules-and-revocation"></a>
<a id="registerhostmodule"></a>

| Method | Evaluation |
| --- | --- |
| `defineModule(name, source)` | Store source; evaluate on first import |
| `registerModule(name, source)` | Evaluate eagerly and replace exports on success |
| `registerHostModule(name, exports, options?)` | Bridge named host functions and return their generated global names |
| `removeModule(name)` | Remove resolution entry and retire its generated host bridges |
| `hasModule(name)` / `listModules()` | Inspect registered names |

Define all members of a cyclic module graph before importing one. Eager
registration is transactional over exports: failed evaluation leaves the
previous export record in place, and successful replacement drops removed
exports. Guest global side effects are outside that transaction.

Host-module exports must be functions with accepted binding-identifier names.
`{ async: ['name'] }` marks asynchronous exports. The host functions enforce
permissions; generated bridge names are a naming convention, not a security
boundary. Existing guest references are not a general revocation mechanism:
keep permission checks at the host operation itself.

## Runtime capabilities

`vm.enableRuntime({ console, timers, webApis, nodeCompat })` installs selected
globals only when the corresponding Cargo features were compiled:

| Option | Required feature |
| --- | --- |
| `console`, `timers` | `runtime` |
| `webApis` | `runtime-web` |
| `nodeCompat` (Buffer) | `runtime-node` |

This call does not grant filesystem, network, process, environment or native
addon permission. See [optional runtime](optional-runtime.md).

## Diagnostics and disposal

<a id="dispose"></a>

`evaluationStats()` and `heapStats()` return JSON text describing engine
counters; `collectCycles()` collects at a quiescent boundary and returns a
count. Do not interpret individual allocation caps as an aggregate heap quota.

Call `dispose()` in `finally` after using async callbacks or `runAsync`.
It is idempotent and retires host resources so native handles do not keep
Node alive. Outstanding work and bridge retirement are coordinated internally;
disposal is not a reset operation that makes a VM fresh again.

## Generators

On the native target, generators suspend on a stackful coroutine running on
the calling thread. Explicit closing, including leaving `for...of` early,
runs the generator's `finally` handlers. Dropping an abandoned generator
does not resume guest handlers. Reentrant `next()` is rejected. Test
coroutine lifecycle changes on each target rather than assuming the native
and WASM implementations have identical machinery.

## AsyncSession

`new AsyncSession(options?)` provides separate asynchronous ownership. Its
options include bounded command capacity, clock selection (`legacy`, `virtual`,
`real-time`), fairness (`external-first`, `alternate`), maximum jobs per turn,
auto-polling, and explicitly installed timers when compiled with runtime support.

| Methods | Purpose |
| --- | --- |
| `run(source)`, `evaluate(source)` | Queue source execution; return `Promise<string>` |
| `setGlobal`, `getGlobal`, `exposeFunction`, `defineModule` | Queue host/guest state operations |
| `advanceClock(milliseconds)` | Advance virtual time |
| `pollEventLoop(maxJobs)` | Return bounded turn status and next deadline |
| `setExecutionLimits(fuel, timeoutMs?)` | Configure execution limits |
| `cancel()` | Signal cancellation |
| `evaluationStats()`, `collectCycles()`, `wakeups()` | Inspect or collect session state |
| `dispose()` | Close the session and retire its host resources |

Use [the event-loop guide](event-loop.md) for scheduling and shutdown rules.
Do not mix `Vm` and `AsyncSession` lifetimes or assume that asynchronous
methods allow reentrant calls from a callback into its own pending operation.

## LanguageService

The shared analysis service supports `open`, `update`, `close`, module
registration, typed host-function metadata, runtime shapes, completion,
hover, and diagnostics. `registerHostFunction` takes parameter metadata with
`name` and `typeName`; generated declarations provide the exact types.
`registerRuntimeShape` takes JSON metadata, not a live guest object.
[Editor integration](editor.md) describes URI and position conventions.

## VmSession

`runtime/session.cjs` exposes an opt-in `VmSession` for live metadata:
`start`/`stop`, `attach`/`detach`, callback and module registration,
`observeHandler`, and `snapshot`. Without a started session, no runtime
locator is published. See [runtime protocol](runtime-protocol.md) for the
transport contract and [editor integration](editor.md) for consumers.
