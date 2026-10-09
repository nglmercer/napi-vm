# Phase 1 host, realms, and GC audit

This audit accompanies source `372c20b` in draft PR #24. It records implemented
ownership and host behavior, plus dependencies that still prevent declaring every
Phase 1 realm case complete. It does not classify all remaining conformance
failures as outside the foundation work.

## Agent ownership and shared memory

`src/bin/test262/agents.rs` implements start, broadcast, receiveBroadcast, report,
getReport, sleep, leaving, and shutdown. Each worker constructs and executes its
own Interpreter on its owner thread. Channels carry source, primitive reports,
and transferable SharedMemory blocks; they never carry guest Values or callbacks.
Broadcast callbacks and their GC pins remain local to their worker. Shutdown
cancels execution, wakes bounded waits, joins workers, and releases local roots.
The primary and worker VMs use the same host and scheduler ownership rules.

Owned shared allocations use Arc, a memory-access mutex, and native waiter
signals. The mutex covers ordinary byte accesses and atomic accesses of different
widths. Atomics.wait/notify coordinate through the same allocation identity;
transferring a data block creates a fresh realm-local SAB wrapper. No foreign
thread enters an already-created Interpreter. External shared-memory hosts must
honor their documented atomic-access contract; external wrappers are not sent to
Test262 agents.

The follow-up adds shared ToBigInt conversion for Atomics and guest typed-element
writes, preserves observable conversion hooks, and accepts iterable or array-like
typed-array constructor inputs in the specified order. ArrayBuffer and SAB slice
share SpeciesConstructor and allocation logic, including conversion order,
callee-realm defaults, result validation, same-data-block rejection, and copying
into larger custom-species results.

## Root audit

| Root category | Reachability path |
| --- | --- |
| Primary and child realms | Interpreter roots, realm-global Environment edges, intrinsic property metadata |
| Closures and bound functions | Function closure, properties, bound target/receiver/arguments |
| Modules and namespace objects | Live module maps, exports, namespace cells, module scopes, realm-owned graph/cache |
| Module promises and dynamic imports | Module graph values and owner-thread job queues |
| Generators | Closure, arguments, results, owned realm module/job roots, async request state |
| Async continuations | Task owner environment and result promise; pending scheduler jobs |
| Async-from-sync adapters | Typed metadata slots, iterator record, awaiting reaction and completion values |
| WeakMap and WeakSet | Weak keys with fixed-point ephemeron marking of values |
| WeakRef | Weak targets plus kept-object roots until the job checkpoint ends |
| FinalizationRegistry | Strong callback/held values, weak targets/tokens, owner queue cleanup jobs |
| Host handles | HostBridge.trace_roots, explicit host pins, reachable host-call tracing |
| Private instance fields | Typed property metadata values, independent of ordinary property names |
| TypedArray and DataView | Realm-local property cells and their backing-buffer wrapper |
| Agent/shared memory | Worker-local callback pin and realm wrapper; transferable Arc allocation has no guest edges |

Marking is iterative. A borrowed reachable cell aborts collection before weak
pruning or sweeping. Guest execution and suspended opaque coroutine stacks keep
the conservative collection barrier: Rust temporaries on those stacks are not
explicitly traced. This prevents unsound collection but can delay reclamation.
The Test262 host now retains deferred collection requests when a checkpoint is
unsafe and retries on a later owner boundary. It clears requests between isolated
test requests. GC does not promise immediate finalization or collection while a
live coroutine is suspended.

## Realm coverage and open dependencies

Ordinary/exotic property cells retain defining realms. Shared constructor helpers
apply newTarget prototype fallback and constructor error realms. Promise jobs,
module imports, native generators, and async-generator request queues restore their
captured owner context. String receiver-brand errors and RegExp intrinsic getter
errors now belong to the executing function/accessor realm.

The following known cases remain visible and must not be called completed merely
because basic constructor realm tests pass:

- Proxy-backed species result definitions still need the shared Phase 3
  DefineOwnProperty operation; ordinary custom constructors now determine
  allocation realm for Array.from/of and all seven implemented species methods.
- Realm-global property/exotic operations and private method/accessor branding
  depend on the pending Phase 3 global/private/internal-operation work.
- RegExp legacy statics and missing standard-library algorithms, Intl, ShadowRealm,
  disposable-stack proposals, and stack accessor proposals have separate failure
  groups. Their failures are not evidence of completed implementations.
- Full-corpus AST/bytecode differential measurement remains a Phase 4 gate; AST
  fallback and execution limits remain enabled.

Phase 2 source-grammar coverage and its pinned compile-only audit are documented
in [test262-phase2-contexts.md](test262-phase2-contexts.md). Validation for this
follow-up must record its own source and immutable worker identities; previous
full reports do not measure these changes.

## Follow-up source 8314510

Thenable jobs capture and trace the realm of their `then` function. Resolving
functions use the existing bound-native function representation, including
Function branding, non-constructibility, standard metadata, and realm-owned
Function.prototype. No interpreter crosses an owner-thread boundary.

Array.map/filter now share ArraySpeciesCreate and descriptor-based result writes.
Foreign intrinsic Array constructors are ignored before observing Symbol.species;
custom species constructors allocate before callbacks and preserve sparse input
indices. Array's species accessor has owned native metadata. All 38 realm tests
and all required checks passed. The frozen worker passed 2,637/2,945 focused
variants with 170 new passes, zero lost passes, and zero special outcomes against
the matching 7c0a9ce projection. This is scoped evidence, not a full-corpus result.

This source does not close the open dependencies above. Array.from/of, other
species methods, and Proxy-backed result definitions still need shared internal
operations. An additional runtime gap was reproduced: array literal elisions are
currently represented as present undefined values by the AST evaluator. That is
an execution issue, not a parser acceptance/early-error defect; sparse-result
regression tests create holes by deleting indices.

## Current allocation and lifecycle follow-up

Proxy revocation now clears target and handler references, preserves callable and
constructible classification, and validates GetFunctionRealm. GC regressions
cover release of revoked references. The earlier 564efab focused run recorded
one lost Atomics waitAsync pass due to instruction fuel; isolated repeats passed,
but those repeats do not replace the failed gate. Agent broadcast now acknowledges
publication into the worker owner's queue before returning and yields after all
acknowledgements. A fresh focused run must validate this change.

Array.from/of and map/filter/slice/splice/concat/flat/flatMap use shared allocation
helpers. TypedArray.from/of and map/filter/slice/subarray now validate actual
typed-array constructor results, respect content type and custom species, and
preserve shared backing stores for subarray. Regression tests cover foreign
constructor prototypes, callback/source identity, allocation ordering, short
species results, and iterator consumption before TypedArray.from allocation.
Native constructor prototype lookup enters the constructor's realm, including
Reflect.construct and Proxy delegation, while derived-constructor post-return
errors retain their specified caller realm. These source changes require fresh
full checks and Test262 evidence before they can be treated as validated.

Source `0577fc2` passes all required checks: 884 workspace tests (four existing
ignored), 203 minimal tests, 73 Node tests, 14 WASM tests, and ten tooling tests,
plus formatting and strict Clippy. Its immutable worker SHA256 is
`ccbc78ac0d0a12a32dbc781db9bbe291bf9c9d5b529c11d65c588eef622498e7`.
The expanded 5,267-variant focused selection passes 4,796, gains 1,282 and loses
zero against the matching 7c0a9ce full projection, with zero special outcomes.
The earlier f9aa579 two-loss report remains archived. Full-corpus validation is
pending; known global-object/private/internal-operation dependencies remain open.
