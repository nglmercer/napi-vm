# Phase 1 host, realms, and GC audit

This audit records the staged implementation in draft PR #24. The latest
completed full measurement is `287bddc`: 62,671/102,956 passed, with 21,264
new passes and zero lost passes against PR #23. It records implemented
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

## Shared assignment and descriptor follow-up

Assignment and Reflect.set now share a receiver-preserving Set operation. Proxy
traps receive the original receiver and enforce protected-property invariants.
Integer-indexed writes distinguish the typed-array target from another receiver:
invalid indices on another receiver return without coercing the value, while
writes to the typed array perform conversion before checking index validity.
Object-literal getter/setter pairs remain visible through descriptor lookup.

Object.defineProperty, Reflect.defineProperty, and array result-element creation
use a shared DefineOwnProperty operation and descriptor compatibility checks.
Public descriptors undergo observable ToPropertyDescriptor reads in specification
order; symbol keys retain identity through Proxy traps and descriptor queries.
Array species results can therefore use Proxy-backed definition operations.

The subsequent full validation must still cover these changes. This does not
complete realm-global ObjectRecord/DeclarativeRecord semantics or the pending
private-element and exotic descriptor dependencies. Full AST/bytecode mismatch
measurement remains outstanding.

## Canonical keys and Array length

Typed-array Get and HasProperty now share the canonical numeric-index rules used
by Set and DefineOwnProperty. Guest number strings use ECMAScript shortest-rounding
rules, while host inspection retains its signed-zero rendering. Proxy HasProperty
and Object.getOwnPropertyDescriptor retain symbol identities and observe key
conversion once.

Array assignment and descriptor definition share ArraySetLength conversion through
DefineOwnProperty. ToUint32 and ToNumber each observe the original value, invalid
lengths raise RangeError, and a failed shrink still applies a requested non-writable
length. Regression coverage includes conversion side effects and non-configurable
array elements. The 54 realm tests pass; fresh required checks and focused/full
worker evidence are necessary for these source changes.

## Current validation: 6456b53

All required checks pass: formatting, strict Clippy, 891 workspace tests (four
existing ignored), 203 minimal-feature tests, 73 Node tests, 14 WASM tests and
10 Test262 tooling tests. The expanded focus passes 8,122/8,845 variants with
1,946 new passes and zero lost passes or special outcomes versus the matching
7c0a9ce projection.

The fresh full pinned corpus passes 62,107/102,956 variants: 20,700 new passes,
zero lost passes, 40,847 failures, zero harness errors, two known deep-WeakMap
timeouts, zero crashes and zero skips versus PR #23. It gains 9,061 and loses zero
versus 7c0a9ce. The frozen worker digest is
`a3b96f79678538a9a67bbae3d848d75149d9fd3d6fda4ab5a1f404b0945e17df`.
The required revision, four workers, five-second timeout and all execution limits
are recorded in the exact [full summary](../tools/test262/evidence/foundations-realms-memory/6456b53-full-summary.json).

The earlier 7dce903 full run also finishes with zero losses after checkpoint
resumption, and all earlier failed gates remain archived. These results close the
reported regressions, not the remaining realm-global/private/descriptor/Proxy
foundation dependencies. Phase 1 is still open and full-corpus AST/bytecode
measurement is outstanding.

## Realm-global records and descriptor follow-up: b28ddd0

Primary and child realms now share a GlobalEnvironment with an ordinary object
record and an independent declarative record. Global object properties use the
same descriptor, symbol-key, prototype, extensibility, accessor, and Proxy paths
as ordinary objects. Lexical declarations remain off the global object. Reads and
writes retain the realm-global receiver; bare global calls retain their required
undefined receiver. The object record is an actual traced GC edge, with its
realm-owner back-edge traced through ordinary object metadata.

Script, eval, and bytecode global declaration paths share preflight checks and
var/function binding helpers. Configurable properties created by eval can coexist
with later lexical bindings, as required by the pinned corpus. Protected script
properties still reject conflicting declarations. Host UTF-16 names and explicit
removal preserve the existing N-API contract.

Lexical for-in/of heads have a TDZ environment for the RHS and a fresh environment
per iteration, including empty binding patterns. Compiler scopes now track whether
they own a runtime environment separately from whether they contain boxed names.
Compiler, verifier, VM, and AST/bytecode differential fixtures cover these paths.

Internal descriptor records have no prototype. Guest-defined properties named
get, set, value, or writable on Object.prototype cannot alter internal records.
Object/Reflect descriptor results and Proxy defineProperty trap arguments still
receive ordinary descriptor objects. This closes record normalization pollution;
it does not finish the remaining accessor-storage/private-element dependencies.

Validation: formatting and strict Clippy passed; workspace tests passed 916 with
4 pre-existing ignored tests; minimal tests passed 203; Node passed 73; WASM passed
15; runner/tooling passed 10. The frozen worker SHA-256 is
`b02073c11eee38cbe361cf52cf95ef444790095d9ab3388acbfdd4251e9942b8`.

The expanded focused selection passed 12,978/14,662 variants, with 1,684 failures
and zero skips, harness errors, timeouts, or crashes. Against the matching 6456b53
full-report projection: 279 new passes and zero lost passes. Against 7c0a9ce:
2,290 new passes and zero lost passes. All 47 intermediate losses were recovered;
the failed runs and checks are archived alongside the successful evidence.

The 236aafc full run was interrupted by an environment restart. Its incomplete
checkpoint is archived without claiming full totals. The b28ddd0 full run completed
with the pinned revision and required limits: 62,596/102,956 passed, 40,347 failed,
13 timed out, and zero skips, harness errors, or crashes. Against PR #23 there are
21,210 new passes and 21 lost passes; against 6456b53 there are 522 new and 33 lost.
The complete reports, checkpoints, and comparisons are archived. This result is
not a passing regression gate. The losses identify callable global data being
mistaken for getters, boxed-string enumeration, property-update null-check order,
inherited exotic accessor receiver validation, and enumeration/shape performance.
These require regression fixes and a new independent full run.

Full-corpus AST/bytecode mismatch counts remain unmeasured. Known private-element,
accessor storage, and Proxy/exotic dependencies remain open, so this is not a
merge-ready Phase 1 completion claim.


## Full-run regression corrections (b0dc280)

The follow-up preserves callable data in global records rather than invoking it
because its function name resembles a getter. Intrinsic Symbol/Map/Set accessors
carry accessor metadata. Property updates require a coercible base after evaluating
the reference operands and before converting the key. For-in enumeration uses the
shared own-key/descriptor layer, including boxed-string virtual indices.

Missing inherited exotic properties preserve the original receiver when invoking
prototype accessors. Direct buffer/view reads retain the existing tracking and
out-of-bounds behavior. Proxy ownKeys required-key checks reuse the identity set
already used for duplicate rejection. Large shape rebuilds use a flat layout and
stop retaining every large prefix, preserving small-object canonical shapes.

Compiler/verifier/VM and AST/bytecode differential fixtures cover the shared
reference operations; realm regressions also cover poisoned function prototypes,
boxed-string keys, and inherited buffer/view accessor brand checks. The required
checks passed: 921 workspace tests (four existing ignored), 203 minimal tests,
73 Node tests, 15 WASM tests, ten runner tests, formatting, and strict Clippy.
These checks do not replace a new focused/full corpus regression gate or complete
the remaining private-element and descriptor model work.


The expanded b0dc280 focused run passed 14,203/16,389 variants with 2,186 failures
and zero skips, harness errors, timeouts, or crashes. Against the matching 6456b53
full projection: 309 new passes and zero losses. All 38 variants in the 19 files
containing the preceding full run's regressions passed under the original limits.
These focused results measure the immutable b0dc280 worker, not subsequent edits.


## Proxy private-field roots and receivers (436fb19)

Proxy receivers now have lazily allocated private storage belonging to the Proxy,
separate from the target and handler. Shared private-field get/set/initialization
operations use it without invoking traps or checking revocation. Revocation removes
target/handler references while retaining the Proxy's own private values.

The collector traces a real edge to the private storage cell, including on revoked
Proxies. Iterative destruction drains private values even after revocation, so
10,000-element private-field chains do not recurse on the native stack. GC tests
cover live private values, dead cycles, wrong/duplicate brands, escaped foreign
constructors and allocation realms. Compiler/verifier/VM/differential fixtures
cover stamping, updates, revoked receivers and duplicate initialization. Existing
instance-field identities and constructor environments remain intact.

This closes Proxy receiver storage/root coverage for the existing private-field
foundation. Private methods/accessors/static elements still require the unified
lexical identity and branding model; this section does not claim their completion.


The 436fb19 required checks passed: formatting, strict workspace/all-target/all-feature
Clippy, 925 workspace tests (four pre-existing ignored), 203 minimal-feature tests,
73 Node tests, 15 WASM tests and ten runner/tooling tests. The subsequent frozen
worker corpus results must be recorded separately from the earlier b0dc280 focus.


The expanded class/host selection for the frozen 436fb19 worker passed
25,751/33,063 variants, with 7,312 failures and zero skips, harness errors,
timeouts or crashes. Against the matching 6456b53 full projection: 327 new passes
and zero losses. Exact logs, checkpoints, comparisons and worker digests are
archived under foundations-realms-memory.

A subsequent root audit removes the hidden storage cell's incidental allocation
realm edge. Stored guest values retain their own realms through their existing
metadata; a primitive private value does not retain an otherwise dead foreign
constructor realm. The WeakRef fixture proves that the foreign constructor can
be collected while the stamped Proxy stays alive. The 287bddc correction passed every required check (926 workspace tests, 203
minimal-feature tests, 73 Node tests, 15 WASM tests and ten tooling tests). Its
expanded focus passed 25,751/33,063 variants with zero losses, timeouts, crashes
or harness errors. The full pinned corpus passed 62,671/102,956: 40,283 failures,
two timeouts, zero crashes, zero harness errors and zero skips. All baseline
passes remain passing. Full-corpus AST/bytecode mismatches are not measured.
Exact checks, checkpoints, comparisons and worker hashes are archived alongside
the previous results. These measurements apply to 287bddc, before the subsequent
typed private-element implementation.
