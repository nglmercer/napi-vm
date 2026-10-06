# Optional runtime implementation plan

Target: **Rust-native embeddable JavaScript engine by default, with an optional
sandboxed runtime supporting Web APIs, npm, Node compatibility and TypeScript.**

This document describes intended architecture and acceptance gates, not shipped
capabilities. See [the implementation tracker](roadmap.md) for current coverage.
No engine stability or compatibility percentage is claimed without measurements.

## Implementation status

The first foundation is implemented in feature-gated modules: core-only globals,
explicit runtime construction, real-time timers and bounded external completions,
permission-checked Unix files and environment lookup, virtual/file module loaders,
CLI run/eval/repl/info, and a pinned Test262 runner and dashboard. Subsequent
implementation adds HTTP fetch, Web request/response and abort wrappers,
WebSocket/TCP/HTTP server transports, SHA digest/random crypto, data/HTTP/npm
loaders, registry installation with cache/lockfile/integrity, Oxc TS/TSX
transformation, Unix process resource limits and initial Node module subsets. See
[the implementation and remaining work](optional-runtime.md). The initial full
Test262 development baseline is 31.43%; the runtime preview and stable engine
gates are not met. Lossless UTF-16 strings and actual core/runtime crate
extraction are implemented; later roadmap milestones remain pending.

## Repository baseline before implementation

The `napi-vm` facade defaults to `napi`; `--no-default-features` removes the
Node binding layer. Engine implementation and configuration live in
`crates/napi-vm-core`; capabilities and OS adapters live in the optional
`crates/napi-vm-runtime`. Core initialization exposes only engine builtins.

Reuse the existing bytecode engine and AST fallback, promises, module graph,
CommonJS resolver, scheduler clocks and bounded turns, host-event bridge, heap
collector, and plugin capability policies. Their presence does not establish
Test262, Web Platform Test, Node, or npm ecosystem conformance.

Guest strings now use lossless UTF-16 `JsString` values, including unpaired
surrogates. AST and bytecode execution, JSON, regex offsets and structured
host bridges share this representation. See [crate architecture](crate-architecture.md).

## Architecture and feature contract

Extract `napi-vm-core` for parser, AST, bytecode VM, ECMAScript builtins,
promises, modules, and engine sandbox limits. Create `napi-vm-runtime` for the
event loop adapter, filesystem, networking, Web APIs, permissions, npm,
Node compatibility, and CLI. Retain the existing package as the binding and
compatibility facade during migration; avoid duplicate interpreter ownership.

The facade's intended feature graph is:

```toml
default = ["napi"]
runtime = []
runtime-cli = ["runtime"]
runtime-web = ["runtime"]
runtime-fs = ["runtime"]
runtime-net = ["runtime"]
runtime-npm = ["runtime"]
runtime-node = ["runtime", "runtime-npm"]
runtime-typescript = ["runtime"]
runtime-native-addons = ["runtime-node"]
```

These feature names are implemented in the facade; optional dependencies are
activated by their owning features. Crate extraction remains pending.
Compile-time availability never grants guest permission. `Interpreter::new()`
and core builtin setup must not install runtime globals or OS-backed loaders.
Keep Promise jobs in core; move timers and console installation to runtime.
Audit computational Web APIs and inert placeholders as part of the split.

An explicit capability builder should support:

```rust,ignore
let runtime = RuntimeBuilder::new()
    .web_apis()
    .timers()
    .filesystem(read_permissions)
    .network(net_permissions)
    .build()?;
```

## Ordered milestones

For the concrete sequence after the current implementation, see
[next-implementations.md](next-implementations.md).

### M0 — P0 engine/runtime boundary and permissions

Inventory every global installer, loader, host bridge, native addon backend,
and OS dependency. Separate core initialization from capability installation
before extracting crates. Migrate existing bindings, plugins, LSP and wasm
call sites explicitly, documenting changed default globals.

Create a shared permission policy denying filesystem reads/writes, network,
environment, process spawning, FFI and native addons by default. Reuse existing
plugin policies where possible. Enforce permissions at operations, including
module loading, redirects and native addon loading, rather than only at API
installation. Handle path canonicalization, symlinks and race-safe file access;
define host/port rules and recheck redirects and connection destinations.

Acceptance: default and no-default builds expose no runtime globals or ambient
OS access; individual features and all-features builds compile; enabling a
feature alone grants no permissions; wasm and existing embedding adapters still
build. Include guest-level denied-access and explicit-grant integration checks.

### M1 — P0 engine compatibility evidence

Add a pinned Test262 runner with metadata parsing, harness includes, strict and
non-strict variants, module mode, negative phase/type checks, asynchronous
completion, timeouts and machine-readable results. Report failures, unsupported
features, skips and timeouts separately; publish revision, test selection and
denominator with every percentage. Use bounded execution or process isolation
so a hung test cannot stall the corpus.

Introduce a code-unit string representation before adjusting individual string
methods. Audit lexer escapes, length/indexing, slicing/searching, comparison,
property keys, JSON, regex offsets, iteration, bytecode constants and N-API/wasm
conversion. Preserve lone surrogates; iteration must still combine valid pairs.
Test BMP, astral, lone-surrogate and mixed strings through both execution paths.

Improve bytecode coverage using AST/bytecode differential tests while retaining
fallback. Audit roots across suspended jobs and host handles, weak collections,
WeakRef and FinalizationRegistry. Validate weak reachability and cleanup job
scheduling without promising deterministic GC timing.

Acceptance: reproducible dashboard and baseline; UTF-16 regressions covered;
no stable engine claim below **90% Test262**, with **95%** the long-term target.

### M2 — P0 OS-backed scheduler

Adapt the existing scheduler rather than creating a competing Promise queue:

```text
OS / Tokio -> ExternalEventQueue -> EventLoop -> VM owner thread
                                     | microtasks / Promise jobs
                                     | timers / IO completion
```

Expose `run_event_loop()`, `run_event_loop_once()`, `run_until_idle()` and
`poll()`. Specify blocking behavior, readiness, deadlines, outstanding external
work and shutdown for each. `poll()` and one-turn execution must be nonblocking;
idle means no currently runnable work, not necessarily no pending IO.

External producers send bounded, thread-safe completion messages and wake the
owner. They must never enter the interpreter or send thread-local VM values.
Use IDs and transferable payloads; convert values and settle promises only on
the owner thread. Define overflow, cancellation, late completion, fairness and
teardown behavior. Preserve microtask checkpoint ordering across bounded turns.

Acceptance: deterministic clock tests, real IO wake tests, Promise/timer
ordering, cancellation, queue saturation, no lost wakeups and clean shutdown.

### M3 — P0 runtime APIs and initial CLI

Install console and timers explicitly, then filesystem/environment, fetch with
Request/Response/Headers and AbortController, WebSocket, TCP, HTTP server and
crypto. Share permissions, cancellation, resource accounting and the external
completion queue. Test operational behavior rather than global-name presence.

Ship `run`, `eval`, `repl` and `info` behind `runtime-cli`, with grants such as:

```sh
napi-vm run --allow-read=./data --allow-write=./cache \
  --allow-net=api.example.com app.js
```

Acceptance: default-denied file/network/environment access, scoped grants,
abort behavior, IO errors and resource cleanup exercised through the CLI and
Rust embedding API.

### M4 — P1 generic modules and npm

Define a `ModuleLoader` contract separating resolution, loading and source
transformation, with canonical identities, referrer context and shared caching.
Implement FileLoader, VirtualLoader, DataUrlLoader, NpmLoader, HttpLoader and
NodeBuiltinLoader. Network/file loaders require the same runtime permissions
as ordinary IO; HTTP imports may remain optional for preview.

Support relative imports, package names/subpaths, `npm:zod`, `node:path`, and
HTTPS module URLs. Extend the existing CommonJS resolver for package.json
`exports`, `imports`, `main`, `module`, `type`, conditional exports, semver,
dependency resolution, ESM/CJS interop, cache, lockfile and integrity hashes.
Specify condition precedence and compatibility treatment of the nonstandard
`module` field. Cache presence must not bypass permissions or integrity checks.

Acceptance: reproducible locked installs, export-map edge cases, cyclic ESM/CJS
graphs, cache tampering rejection and a version-pinned real-package corpus.

### M5 — P1 optional Node and TypeScript

Enable Node behavior explicitly with `--compat=node` and implement tiers:

| Tier | Modules |
| --- | --- |
| 1 | path, url, util, events, assert, buffer, process, os |
| 2 | fs, stream, crypto, http, https |
| 3 | net, tls, dns, zlib, child_process, worker_threads |

All IO remains permission checked. Native Node-API addons require
`runtime-native-addons` and explicit `--allow-ffi`; process-backed addon
providers also require process authorization. Document that native code needs
stronger isolation when it cannot obey in-process sandbox restrictions.

Transform TS/TSX through Oxc or SWC into JavaScript before engine parsing.
Support source maps for stack traces. `run app.ts` transpiles; `check app.ts`
must use a type-checking implementation, since transpilation alone is not
type checking. Node compatibility, addons and TypeScript are optional preview
extensions.

### M6 — security limits and P2 CLI expansion

Unify heap, CPU time, stack depth, job count, timers and IO resource limits.
Loop/fuel budgets already exist but do not prove heap or wall/CPU enforcement.
Define accounting and interruption across native callbacks, regex, modules and
async work. Support `--max-memory=128M --max-cpu=5s` with documented units and
failure behavior.

Optional `--isolate=process` uses a supervisor with OS CPU/memory limits,
watchdog and a child runtime. State platform guarantees and terminate/reap the
child on expiry. Expand CLI later with test, bench, task, install, check, fmt
and lint.

## Compatibility dashboard and preview gate

Track independently, without a combined percentage:

| Domain | Evidence |
| --- | --- |
| ECMAScript | Test262 revision, selected variants, outcomes and percentage |
| Web APIs | Web Platform Tests revision and supported subsets |
| Node | Node compatibility tests and implemented module tiers |
| npm | Passing packages / version-pinned corpus size |

The initial full Test262 development baseline is 32,359 / 102,956 variants
(31.43%). This is a historical engine measurement, not a stable-engine claim.
Web, Node and npm corpus measurements remain pending.

Runtime preview requires UTF-16 correctness, a Test262 runner, bytecode with AST
fallback, ES modules, Promises and async/await; an event loop and external queue,
timers, filesystem, fetch, WebSocket, TCP and permissions; ESM, CommonJS,
package.json and npm resolution; and CLI run/eval/repl. Preview does not imply a
stable engine claim. Node compatibility, native addons, TypeScript and HTTP
imports remain optional. The implemented HTTP server, environment and crypto subsets are documented in
[optional-runtime.md](optional-runtime.md); their complete compatibility coverage
remains outside this minimum preview checklist.

Begin implementation with M0: prove a capability-free engine boundary before
adding OS-backed APIs or advertising the optional runtime.
