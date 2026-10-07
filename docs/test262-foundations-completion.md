# Test262 foundation completion: work in progress

The continuation starts at merged PR #23 (`fc4c875`). **Phases 1–3 are not
complete, and this continuation is not ready to merge.** The completion criteria
are semantic coverage, zero unexplained regressions, zero crashes, correct
negative-test phases, and reproducible full-corpus evidence. A percentage does
not establish phase completion.

## Internal implementation order

1. Phase 1: agents, shared-memory coordination, realm ownership, and GC roots.
2. Phase 2: grammar contexts, contextual validation, and declaration early errors.
3. Phase 3: global/declaration environments, classes/private elements, descriptors,
   shared internal operations, Proxy invariants, and exotics.
4. Phase 4: bytecode coverage through shared semantic helpers, with compiler,
   verifier, VM, differential, and focused Test262 validation before fallback removal.
5. Full pinned-corpus validation, regression fixes, and final documentation.

Phase 1 is in progress. Later implementation phases have not started. PR #23's
realm-owned module caches, async/module ownership, parameter and constructor
environments, private instance fields, AST fallback, execution limits, and
capabilities disabled by default are preserved.

## Phase 1 agent and shared-memory implementation

The Test262 host provides real `start`, `broadcast`, `receiveBroadcast`, `report`,
`getReport`, `sleep`, `leaving`, `shutdown`, and `monotonicNow` operations.
Each native worker constructs an independent interpreter on its own owner
thread. No foreign thread enters another interpreter. Native channels carry
source, primitive IDs/reports, and owned shared data blocks. Guest callbacks,
realm environments, SAB wrappers, and other `Rc` values never cross a thread.

`start` waits for worker initialization. `broadcast` waits for retrieval by
active agents, tolerating an agent leaving concurrently. IDs preserve Int32
conversion or full BigInt values. Reports preserve UTF-16 code units and FIFO
order. Worker errors propagate to the parent; they cannot silently pass a
variant. Worker count and report queues are bounded. Shutdown cancels execution,
unblocks native waits, closes queues, and joins every worker on all exit paths.

An owned SAB data block has an `Arc` lifetime separate from each guest wrapper.
The transferable `SharedMemory` handle contains no guest state or addon finalizer.
`Interpreter::shared_array_buffer_from_memory` creates the imported guest wrapper
in the receiving realm, including when that realm later escapes or is dropped.
External addon-owned storage cannot be transferred: its lifetime belongs to its
original host. VM accesses to owned memory share an access lock, including
mixed-width operations, to avoid overlapping Rust atomic-width data races.

Blocking and async waits use one native FIFO list indexed by shared data block
and byte offset. Equality checking and registration hold notify's list lock,
preventing lost wakeups. Native notifications only update native signals. The VM
owner settles promises at checkpoints and is awakened through the existing
native wake latch; notifications never manipulate guest promises on foreign threads. Timeouts, cancellation, and dropping a registration
remove stale waiters. Blocking waits observe execution deadlines and cancellation.

The embeddable engine defaults to `CanBlock=false`. The isolated conformance host
explicitly permits blocking unless Test262 metadata requires `CanBlockIsFalse`.
Agents use real-time queues; pending async completions are polled on the owner.
Broadcast callbacks enter through the existing scheduler's callback queue.

Registered callbacks are explicit host GC roots until replacement or shutdown.
Worker GC requests run only at quiescent owner boundaries. Native waiters retain
the data block without retaining the guest wrapper and realm. External storage
waiters retain their wrapper for its host-managed lifetime. The existing collector
still conservatively refuses collection over opaque suspended guest stacks.

Atomics uses shared numeric/coercion helpers for omitted/NaN indices, abrupt
coercions, ToBigInt, timeouts/counts, and `store`'s unwrapped return value. Notify
on a valid non-shared waitable view returns zero. Methods have ordinary function
metadata/descriptors and retain intrinsic realm ownership through the existing
intrinsic registry.

## Focused validation groups

`tools/test262/groups.json` defines repeatable `--group` selections:

- `agents`, `shared-memory`, `realms`, `gc-weak`
- `async-generator-grammar`, `super`, `private-names`, `eval-globals`
- `classes-private`, `object-reflect-proxy`, `typedarray-exotics`

Selections form a union and retain every selected outcome in the denominator.
Unknown groups, missing paths, and escaping paths are errors. Named groups do
not classify unselected tests as passes or silently skip unsupported features.

```sh
cargo build --locked --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py /workspace/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --group agents --group realms --group gc-weak \
  --output artifacts/test262/foundations-phase1-focused.json
python3 -m unittest discover -s tools/test262 -p 'test_*.py'
```

Every worker uses fuel 1,000,000, loop budget 100,000, call depth 128, and max jobs
10,000. Runtime capabilities remain disabled. The runner snapshots the worker
binary and records its SHA-256 before running variants.

The supplied baseline is 41,407 / 102,956, with 284 harness errors, two timeouts,
and zero crashes. A fresh full rerun of `fc4c875` passes 41,409 / 102,956, with
61,261 failures, 284 harness errors, two timeouts, zero crashes, and zero skips.
The individual rerun outcomes are retained for actual transition comparisons;
the two-pass difference from the supplied historical measurement is not attributed
to implementation changes.

The first exploratory focused comparison covered 2,030 variants: 1,357 passes,
673 failures, and zero harness errors, timeouts, crashes, or skips. Compared with
the identical baseline selection it gained 254 passes and lost zero. That worker
predates the final ownership/deadline fixes and is not final-source evidence.
Final-source checks and corpus measurements must be recorded before merge.

## Remaining implementation gates

Phase 1 still needs a complete realm/intrinsic/error/prototype audit, unification
of primary and child global identity across independent embeddings, iterator and
generator ownership coverage, and complete GC/finalization host observation.
Growable/resizable buffers and remaining shared-memory exotic semantics need
focused validation and implementation. Agents being implemented does not close
these gates.

Phase 2 still needs the complete async/generator/super/private-name grammar
contexts and declaration conflicts specified in the completion request.

Phase 3 still needs one global environment/declaration-instantiation model,
complete lexical private identities and branding for methods/accessors/static
elements, initialization ordering, and centralized descriptor/internal operations
for all Proxy traps and exotic receivers.

Phase 4 remains pending those semantics. No AST fallback has been removed.
The new Atomics differential tests assert verified bytecode execution and compare
against raw AST execution through the same native operations. They do not
establish full-corpus AST/bytecode parity.

Gaps outside Phases 1–3 must be classified from the final full triage. Existing
broader ECMAScript and runtime incompatibilities remain failures, not exclusions.
