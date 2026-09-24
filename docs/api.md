# API reference

## VM

| Function | Description |
|----------|-------------|
| `runCode(code)` | Execute code statelessly and return the stringified result |
| `new Vm()` | Create an isolated, stateful VM |
| `vm.run(code)` | Execute synchronously while preserving VM state |
| `vm.runAsync(code)` | Execute asynchronously and return `Promise<string>` |
| `vm.callFunction(name, args)` | Call a VM-defined function and return a live value |
| `vm.setGlobal(name, value)` | Add a structured value to the guest global scope |
| `vm.getGlobal(name)` | Read a global as a string |
| `vm.hasGlobal(name)` | Check whether a global exists |
| `vm.removeGlobal(name)` | Remove a global, including an exposed host function |
| `vm.exposeFunction(name, fn)` | Expose a synchronous host callback |
| `vm.exposeAsyncFunction(name, fn)` | Expose an asynchronous host callback |
| `vm.registerModule(name, code)` | Register an importable ES module |
| `vm.registerHostModule(name, exports, opts?)` | Register a module whose exports are host functions |
| `vm.removeModule(name)` | Remove a registered module |
| `vm.hasModule(name)` | Check whether a module is registered |
| `vm.listModules()` | List registered module names |
| `vm.setLoopLimit(n)` | Set the per-execution loop budget |
| `vm.setImportMetaMain(bool)` | Set `import.meta.main` |
| `vm.dispose()` | Release host handles so the process can exit |
| `debugParse(code)` | Parse source and return its AST string |

### dispose

A VM that has run `runAsync` holds a native handle for dispatching host calls,
and that handle keeps the Node process alive. Call `dispose()` when finished
with such a VM, or the script will not exit:

```javascript
const vm = new Vm();
vm.exposeAsyncFunction("fetchRow", async (id) => db.get(id));
await vm.runAsync(`await fetchRow(1);`);
vm.dispose();
```

`dispose()` is idempotent and safe to call while an async worker is still in
flight — handles are marked retired and the last in-flight callback releases
them. After it returns, host functions are no longer callable from guest code;
plain `run()` still works. VMs that never call `runAsync` do not need it.

### Modules and revocation

`registerModule` is transactional over the module's export table. The body is
evaluated into a fresh export record that replaces the previous one only on
success, so:

- a body that throws leaves the previously registered version untouched, and
  registers nothing if there was no previous version;
- re-registering with fewer exports **drops** the ones the new source removed,
  rather than merging into the old record.

The transaction covers exports, not global side effects: a body that assigns a
global and then throws leaves that global set. Use a fresh `Vm` if you need
that isolation.

`removeModule` is the revocation primitive. After it returns, `import` of that
name fails with `Module not found`, `hasModule` reports `false`, and any bridge
globals a `registerHostModule` created are revoked. It never disagrees with
what `import` can resolve. Bindings a previous execution already imported into
a global keep the value they captured — revocation applies to resolution, not
to references the guest already holds.

### registerHostModule

The generic form of `exposeFunction` + `registerModule`: the core bridges each
export and generates the wrapper module, and returns the global names it
created so the host can remove them alongside the module.

```javascript
const globals = vm.registerHostModule(
  "node:fs",
  { readFileSync: restrictedReadFileSync, writeFileSync: restrictedWriteFileSync },
  { async: [] },              // export names the guest may `await`
);

vm.run(`import { readFileSync } from "node:fs"; readFileSync("./config.json", "utf8");`);

vm.removeModule("node:fs");   // also revokes the module's bridge globals
```

Revocation is tracked per module: `removeModule` revokes the globals the
module created, and re-registering with fewer exports revokes the ones that
disappeared — a dropped export cannot stay callable through its old global.
Module names are encoded injectively, so `a:b` and `a/b` never share a
namespace. Registration is transactional: callbacks are bridged, then the
globals installed, then the wrapper module evaluated, and a failure at any
point rolls every step back — including bindings this call had already
replaced. The previous registration keeps working, handles included, because
replaced handles are retired only once the swap commits.

Export names must be usable as binding identifiers and every value must be a
function. Validation asks the lexer directly, so every reserved word is
rejected — including `null`, `true`, `async`, `from`, `of`, `get` and `set`,
which look like plain identifiers but are keywords here. The
core stays generic on purpose: permission checks, path resolution and policy
belong to the host functions themselves — see
[Plugins](plugins.md) for a full capability host built this way.

`runAsync` should be reserved for genuinely long-running or asynchronous
work. It spawns one OS thread per call and must not run concurrently with
another operation on the same VM.

### Generators

Generators suspend on a stackful coroutine that runs on the calling thread, so
`yield` inside loops, conditionals and `try`/`finally` all behave as specified,
and infinite generators are fine.

Leaving a `for...of` early — `break`, `return`, or a `throw` from the body —
closes the generator, so its `finally` blocks run:

```javascript
function* g() { try { yield 1; yield 2; } finally { cleanup(); } }
for (const v of g()) { break; }   // cleanup() runs
```

A generator that is merely dropped without being closed does *not* run its
`finally`, matching JavaScript, where a generator collected by the GC never
resumes. Calling `next()` on a generator from inside its own body throws
`TypeError: Generator is already running`.

## LanguageService

| Function | Description |
|----------|-------------|
| `new LanguageService()` | Create an analysis service |
| `open(uri, source)` | Add a document |
| `update(uri, source)` | Replace a document |
| `close(uri)` | Remove a document |
| `registerModule(name, source)` | Add module exports to analysis context |
| `registerHostFunction(name, params, returns, docs, async)` | Add typed host metadata |
| `complete(uri, offset)` | Return completion items |
| `hover(uri, offset)` | Return hover information |
| `diagnostics(uri)` | Return diagnostics |

## VmSession

`runtime/session.cjs` exposes:

- `new VmSession({ workspace, vm, sessionId })`
- `start()` / `stop()`
- `attach(vm, { modules })` / `detach()`
- `exposeFunction` / `exposeAsyncFunction`
- `registerModule` / `removeModule`
- `observeHandler(name, value)` — publish a JSON-derived property shape and a
  bounded snapshot of the latest value for the handler's first parameter. The
  live LSP uses the shape for nested completion and shows the latest value in
  parameter hover information.
- `snapshot()`

The session is opt-in. Constructing or starting no session means no runtime
locator is created.
