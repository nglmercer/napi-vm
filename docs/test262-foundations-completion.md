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

Phase 1 remains incomplete. Phase 2 contextual grammar/static semantics is now
in progress; Phases 3–4 have not started. PR #23's
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
These exploratory results are superseded by the final-source evidence below.

The final-source focused rerun at `1a8162a` covered the same 2,030 variants:
1,435 passes, 595 failures, and zero harness errors, timeouts, crashes, or skips.
The exact outcome comparison gained 332 passes and lost zero; all gains are in
Atomics. Its worker SHA-256 is
`7c572cffae742ab4d231bcb5acc22fd934a6a03d1a9780f28e5b473ad4ca0cdb`.
Full outcome files and transitions are retained under `artifacts/test262/`.
This focused result does not establish phase completion.

## Full-corpus evidence for this Phase 1 commit

The final pinned run and an additional PR #23 baseline run used four workers,
five-second timeouts, and the unchanged budgets above, without competing builds.

| Outcome | PR #23 baseline | Phase 1 continuation |
| --- | ---: | ---: |
| Pass | 41,407 | 41,742 |
| Fail | 61,263 | 61,152 |
| Harness error | 284 | 60 |
| Timeout | 2 | 2 |
| Crash | 0 | 0 |
| Skip | 0 | 0 |
| Total | 102,956 | 102,956 |

The exact comparison gains **335 passes and loses zero**. The pass rate is
40.5435%; this is development evidence, not foundation completion or conformance.
The first baseline run passed 41,409 and produced two apparent losses. Both are
variants of `language/expressions/dynamic-import/await-import-evaluation.js`:
its fixture spins for 100 ms and can exhaust the unchanged fuel budget when the
host executes more iterations in that interval. The first baseline ran alongside
builds; the later idle baseline and final implementation both fail these variants
with instruction-fuel exhaustion. Both baseline reports are retained, including
the first comparison's two losses. No outcomes were edited or excluded.

The remaining harness errors are 46 missing-`EvalError` failures and 14 staging
Set harness parse failures. Both timeouts are variants of the existing deep
WeakMap fixture. Non-passing outcomes are reported as 53,666 runtime, 7,440 parse,
46 resolution, 60 harness, and two timeout outcomes. These are observed runner
phases; complete parser/static-semantics correctness remains pending Phase 2.

Required checks passed: formatting, strict all-features Clippy, workspace tests
(728 passed, four ignored), no-default tests (156 passed), Node (73 passed), WASM
(14 passed), and runner/tooling tests (nine passed). A real-worker checkpoint
smoke test preserves all 14 outcomes across resume. The Atomics differential test
compares two raw-AST/verified-bytecode fixtures with zero mismatches; full-corpus
AST/bytecode mismatch measurement remains pending.

Compressed full outcome reports, digests, configuration, checks, and remaining
gates are versioned in
[`tools/test262/evidence/foundations-phase1`](../tools/test262/evidence/foundations-phase1/README.md).
The runner's durable checkpoint journal preserves completed outcomes across
execution-server disconnects. The successful final run completed without resume.

## Remaining implementation gates

Phase 1 still needs a complete realm/intrinsic/error/prototype audit, unification
of primary and child global identity across independent embeddings, iterator and
generator ownership coverage, and complete GC/finalization host observation.
Growable/resizable buffers and remaining shared-memory exotic semantics need
focused validation and implementation. Agents being implemented does not close
these gates.

Phase 2 now validates async/generator/arrow boundaries, parameter restrictions,
super/new.target contexts, lexical private-name scope, class-element early errors,
and import/export/block/catch declaration conflicts. It also validates semicolon
insertion boundaries and parameter/call delimiters. This is not complete Phase 2:
see [context implementation status](test262-phase2-contexts.md) for coverage,
validation evidence, and the remaining parser representation gaps.

Phase 3 still needs one global environment/declaration-instantiation model,
complete lexical private identities and branding for methods/accessors/static
elements, initialization ordering, and centralized descriptor/internal operations
for all Proxy traps and exotic receivers.

Phase 4 remains pending those semantics. No AST fallback has been removed.
The new Atomics differential tests assert verified bytecode execution and compare
against raw AST execution through the same native operations. They do not
establish full-corpus AST/bytecode parity.

Outside the requested foundation work, Temporal and Temporal Intl algorithms
remain unsupported: the final triage records 9,210 non-passing built-in Temporal
variants and 4,058 Intl Temporal variants. Broader RegExp, Array, String, and
Iterator failures also remain; these clusters mix algorithm and foundation
failures and require individual classification. They remain in the denominator.
