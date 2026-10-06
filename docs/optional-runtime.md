# Optional runtime foundations

The default Cargo feature set is still `default = ["napi"]`. Neither
`Interpreter::new()` nor `Interpreter::with_builtins()` installs console,
timers, queueMicrotask, Web globals, Buffer, process, require, or placeholder
browser APIs. ECMAScript builtins, Promise jobs, modules, bytecode and the AST
fallback remain available in the engine.

This is the first implementation stage of [the runtime plan](runtime-plan.md),
not the runtime preview milestone or a stable ECMAScript engine release.

## Rust embedding

The former interpreter configuration `RuntimeBuilder` is now `EngineBuilder`:

```rust
use napi_vm::runtime::EngineBuilder;
let mut engine = EngineBuilder::new().loop_budget(10_000).build()?;
engine.eval_source("1 + 2")?;
# Ok::<(), napi_vm::VmErr>(())
```

Enable `runtime` for `RuntimeBuilder`, `Runtime`, the bounded external completion
queue, real-time scheduling, and permission policy. Add `runtime-fs` for file
operations, `runtime-web` for the implemented Web wrappers and crypto APIs, `runtime-net` for native
transports, `runtime-npm` for package resolution, `runtime-typescript` for Oxc
transformation, and `runtime-node` for the implemented Node module subset. Compilation does not install globals or grant access.

```rust,ignore
use napi_vm::runtime::{RuntimeBuilder, permissions::Permissions};

// The directory must already exist. Writes remain denied.
let permissions = Permissions::new().allow_read("./data")?;
let mut runtime = RuntimeBuilder::new()
    .console()
    .timers()
    .filesystem(permissions)
    .build()?;
runtime.eval("console.log(napiVm.readTextFile('./data/input.txt'))")?;
runtime.run_event_loop()?;
```

`.build()` returns the runtime; `.build_runtime()` is an equivalent explicit
alias. `Runtime::eval()` runs script code and its microtasks without waiting for
future timers. `run_module()` evaluates a module through a configured loader.
`poll()` and `run_event_loop_once()` perform bounded nonblocking turns;
`run_until_idle()` drains currently runnable work; `run_event_loop()` waits for
registered external operations and future timers. An unresolved guest Promise
alone does not keep the runtime alive.

Console, timers (including repeating intervals), environment lookup, file
operations, computational Web APIs and Buffer each require explicit builder
selection. The computational Web subset includes TextEncoder, TextDecoder,
URLSearchParams and structuredClone. Runtime wrappers add Headers, Request,
Response, AbortController/AbortSignal and crypto. Fetch, WebSocket, TCP and an
HTTP server additionally require `.network(permissions)` and `runtime-net`.
Node compatibility remains partial; no complete Node environment is advertised. `interpreter_mut()` is a trusted host escape hatch; custom loaders
and host bridges are explicit capabilities supplied by the embedding host.

## External completions

Register a Promise on the VM owner with `runtime.external_promise()`. Pass only
its numeric ID and `runtime.external_sender()` to producer threads. Producers
call `sender.complete(id, Ok(json_payload))` or provide an error string. VM
values never cross the queue; conversion and settlement happen on the owner.

A full queue returns `TrySendError::Full` with the completion for retry. Dropping
the owner disconnects senders. Cancellation rejects the Promise and ignores
late completions. The queue stages at most one additional completion on the
owner to report readiness across bounded ingress batches. Outstanding promises
are included in GC roots. `build()` owns the host bridge; use `EngineBuilder`
for an independent custom bridge.

## Permission-checked IO and limits

`Permissions::new()` denies read, write, network, environment, process and FFI.
File grants are directory based. Unix file access traverses directory handles
with `openat` and `O_NOFOLLOW`, rejects parent components, symlinks, nonregular
files and multiply linked files, and checks file sizes. Granting read never
permits write. Secure file access fails closed on other platforms pending a
platform-specific implementation.

`napiVm.readTextFile(path)`, `napiVm.writeTextFile(path, text)` and
`napiVm.env(name)` are synchronous operations. Environment grants are per name.
Network policy supports exact hosts with optional ports; process and FFI policy
checks exist, but the new runtime does not yet install those operations.
Existing native addon embedding remains a separate, explicitly configured host
capability with its existing allowlist and integrity checks. `node-api-host`
now implies `runtime-node`; explicitly installing a Node addon backend installs
Buffer for its guest wrappers. This does not change core initialization.

`RuntimeLimits` currently enforces instruction fuel, call depth, executed jobs,
timer count (including executing intervals and Atomics timeouts), external queue
capacity, outstanding external operation count, file byte limits, and an
optional elapsed deadline spanning pending external work. In-process heap
accounting is not implemented. `--max-time` measures elapsed time. On Unix the
CLI supervisor implements `--isolate=process`, `--max-memory=128M` (address-space
limit via RLIMIT_AS), `--max-cpu=5s` (OS CPU accounting rounded up to whole
seconds), and a wall-clock watchdog. Supplying memory or CPU limits automatically
selects process isolation. Isolation supports run/eval; other platforms fail
explicitly pending platform-specific resource controls.

## CLI

```sh
cargo run --no-default-features --features runtime-cli --bin napi-vm -- eval '1 + 2'
cargo run --no-default-features --features runtime-cli --bin napi-vm -- repl
cargo run --no-default-features --features runtime-cli --bin napi-vm -- info
cargo run --no-default-features --features runtime-cli,runtime-fs --bin napi-vm -- \
  run --allow-read=./data --allow-write=./cache --max-time=5s app.js
```

The CLI explicitly selects console, timers and environment lookup; file APIs
are installed when `runtime-fs` is compiled. The entry source is a host-selected
file with a 16 MiB cap. This does not grant the guest access to its directory.
Imported files and guest file operations require `--allow-read`/`--allow-write`.
Flags also include `--allow-env`, `--allow-net`, `--allow-run`, `--allow-ffi`,
`--max-fuel`, `--max-stack`, `--max-jobs`, `--max-timers`, `--max-io` and
`--max-file-bytes`. Unknown flags fail explicitly. TypeScript/TSX entry points run when
`runtime-typescript` is compiled. `--compat=node` selects Buffer and the current
Node modules. `install` requires `runtime-npm` plus `runtime-net`, with explicit
read, write and registry network grants. `info` reports enabled features and the
historical initial Test262 baseline; it does not claim a current compatibility
measurement.


## HTTP, sockets and crypto

```rust,ignore
let mut runtime = RuntimeBuilder::new()
    .web_apis()
    .network(Permissions::new().allow_net("api.example.com", Some(443)))
    .build()?;
runtime.eval("let result; fetch('https://api.example.com/data').then(r => r.json()).then(v => result = v)")?;
runtime.run_event_loop()?;
```

HTTP workers drive async Reqwest through Tokio. Abort requests cancel header and
body waits; runtime teardown also signals cancellation. Every redirect checks
network permission again, and cross-origin redirects remove credentials. Host,
connection and proxy transport headers cannot override the granted authority;
ambient proxy settings are not used. Bodies and command queues are bounded.
Only IDs, JSON and byte payloads cross threads. Promise settlement and guest
callbacks always run on the VM owner.

WebSocket supports text/binary messages, protocols, open/message/error/close
callbacks, close codes and bounded messages. TCP is exposed as
`napiVm.connect({hostname, port})`, returning an object with async read/write and
close. HTTP is `napiVm.serve({hostname, port}, handler)`, returning a Promise for
`{addr, finished, shutdown}`. The handler receives a Request and returns a
Response or Promise for one. Binding a listening socket requires a network
grant for that address/port; port zero is an explicit ephemeral-port grant.
Network transports are currently native-only. Cryptographic randomness uses
OS entropy; WebCrypto digest supports SHA-256, SHA-384 and SHA-512. Broader
WebCrypto, WPT semantics and protocol conformance remain open.

## Modules, npm and TypeScript

CompositeLoader routes explicitly selected schemes; DataUrlLoader accepts bounded
JavaScript data URLs, and HttpLoader enforces network policy on HTTP imports and
redirects. Redirected module dependencies resolve against the final URL while
retaining the requested module identity. HTTP module loading is synchronous at
the loader boundary and also works from an embedding Tokio owner.

NpmLoader implements installed-package lookup, scoped packages, package subpaths,
exports/imports maps, wildcard targets, ordered import/require/node/default
conditions, main/module/type, CommonJS and default ESM imports of CommonJS. Read
permissions cover every package manifest and source. Native addons fail closed
without an explicit provider, including when FFI is granted.

NpmInstaller selects registry versions, installs nested dependency graphs,
verifies SHA-512 or SHA-256 integrity, validates archives before source writes,
caches verified archives and records exact artifacts in napi-vm.lock. Locked
replay uses cached artifacts and rechecks integrity. Archives cannot create
symlinks, hardlinks, devices, parent escapes or duplicate files. Package graphs,
archive sizes and entry counts are bounded. Install scripts are not executed.
Installation requires read and write grants for the application/cache directory
and network grants for both metadata and artifact hosts. Registry fetching is
available only with runtime-net. Peer/optional dependencies, complete npm range
syntax and transactional installation are not yet implemented.

```sh
# Build with runtime-cli,runtime-npm,runtime-net.
napi-vm install --allow-read=. --allow-write=. \
  --allow-net=registry.npmjs.org npm:zod
napi-vm install --locked --allow-read=. --allow-write=. \
  --allow-net=registry.npmjs.org npm:zod
```

Node compatibility currently implements subsets of node:path (POSIX), node:events,
node:assert, node:util and node:buffer. These work through ESM and CommonJS after
explicit node selection. Relative path.resolve requires an explicitly supplied
absolute base; no ambient cwd is exposed. Other Node modules fail resolution.

Oxc erases types and transforms enums/TSX before engine parsing. TypeScriptLoader
wraps another explicit loader and retains generated source maps for hosts. CLI
entry points and imported .ts/.tsx/.mts files use this pipeline. This is not a
TypeScript type checker; `check` and automatic stack-frame remapping remain open.

## N-API and browser adapters

A new `Vm` has engine globals. Native builds with runtime features can explicitly
call `vm.enableRuntime({ console: true, timers: true, webApis: true,
nodeCompat: true })`. Missing compiled features reject the request before any
globals are installed. This installs the implemented global subset; it does
not grant IO or replace the Node adapter's historical timer scheduling policy.
Legacy JavaScript tests and applications that assumed runtime globals must
request them explicitly and use a native build with the relevant features.
The browser adapter explicitly creates its own captured console namespace;
this capability is owned by the browser host rather than core builtin setup.

## Compatibility evidence

The pinned Test262 runner and report generator live in
[`tools/test262`](../tools/test262/README.md). The first full development baseline
passed **32,359 / 102,956 variants (31.43%)**, with 70,099 failures, 8 timeouts,
20 crashes and 470 harness errors. Runner limitations are recorded with the
measurement. The percentage is not a stable-engine claim. Web API, Node and npm
compatibility measurements remain separate and unmeasured by those corpora.

## Remaining roadmap work

- Extract actual `napi-vm-core` and `napi-vm-runtime` crates; the current split is
  feature-gated modules within the existing package.
- Replace scalar strings with a lossless UTF-16 representation, including lexer,
  regex, JSON, property keys and bridge semantics. Unicode escapes remain a gap.
- Complete Test262 host hooks and module linking/negative-phase classification;
  improve bytecode coverage, weak reachability and finalization.
- Complete Web Platform Test coverage: streaming bodies, Blob/FormData, the full
  URL API, EventTarget semantics and remaining WebCrypto algorithms. Current
  HTTP server support is sequential HTTP/1.x with Content-Length, bounded
  headers/bodies, no chunked requests or server TLS.
- Complete npm semantics (peer/optional dependencies, aliases, all npm range
  syntax, installation rollback/upgrades and complete ESM/CJS interop), the
  remaining Node tier APIs, and permission-aware native-addon runtime wiring.
- Add TypeScript type checking and source-map integration into guest stack
  traces. Oxc transformation generates source maps, but generating a map does
  not yet remap interpreter frames.
- Implement in-process heap accounting and non-Unix secure file/process limits;
  expand the remaining CLI commands and compatibility corpora.

The runtime preview checklist remains open. Feature names and passing targeted
integration tests do not establish Web, Node or npm corpus compatibility.
