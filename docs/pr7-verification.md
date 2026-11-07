# PR #7 verification

> **Historical record.** This is a historical PR verification record. Results, limitations and follow-up status describe the revisions tested at the time. See the [implementation map](roadmap.md) and [development guide](development.md) for current entry points.

Reviewed head: `ba1443705bb80d2e4eafeb2783e74b2cb1ad40b3` (also the remote head at the start of this task). Final implementation and regression-test revision: `b918b38`.

## Reproductions

The checked-in addon was stale and did not export `AsyncSession`. A fresh isolated build of the reviewed source reproduced all three reported failures:

- The throwing-microtask example threw `boom`, then `Vm.run("42;")` failed with `resume the pending microtask checkpoint before evaluating new guest code`.
- `background(); 42;` returned `42`, but the next command failed with `async session is awaiting Node`.
- Awaiting a 50 ms real-time timer failed with `cannot synchronously await a pending Promise: no VM or host event can settle it`.

Reproduction build: `git archive ba1443705bb80d2e4eafeb2783e74b2cb1ad40b3` into `/tmp/napi-vm-reviewed`, then `cargo build --manifest-path /tmp/napi-vm-reviewed/Cargo.toml --target-dir /tmp/napi-vm-reviewed-target --lib`; its library was copied to the platform addon filename before running the supplied examples with Node.

A controlled-clock ordering regression also fails against `4fa8b9a`, before the one-event await fix, and passes in the final implementation. It forces both timers to become due through the wait path and requires code after await to run before the second timer. That comparison used a separate target directory to avoid sharing compiled artifacts across source trees.

## Changes

- Drain guards reconcile checkpoint state on every exit. Queued jobs retain their roots. Legacy `Vm.run` finishes a pending microtask checkpoint with its old hard budget before starting another program.
- Actual Node waits control session admission independently of stored results. A quiescent root walk keeps result handles reachable through globals, closures, promises and queued jobs; opaque suspended stacks conservatively retain handles. Abandoned receivers and Node promise references retire at owner/main-thread lifecycle boundaries. Settlement remains one-shot.
- Native real-time top-level await waits for future timers and resumes after one event and its microtask checkpoint. Virtual/browser awaits report that host-driven progress is required, retain timers/promises, and never sleep or implicitly advance time.
- Latched condition-variable waits cap time by the caller timeout, timer and execution deadline. Cancellation wakes sleeping owners. Stale/spurious wakes recheck readiness. Node replies and sidecar responses use wake signals instead of response polling; sidecar EOF becomes visible before notification. Interrupted sidecar requests retire their connection to avoid stale replies.
- Timer IDs reside alongside callbacks in the ordered tree, eliminating the reverse index. Ordering, cancellation and GC roots remain covered by tests.
- Full Bun testing exposed an existing deep-array teardown stack overflow. Heap registry weak references prevented `Rc::get_mut` from enabling iterative destruction. Sole-owner arrays/objects now drain through their cells; a small-stack regression drops 20,000-deep tracked graphs.
- WASM compilation no longer references the native-only `url` dependency. Bun package/CI commands explicitly use `./tests`, excluding vendored Node-only tests that call Bun's unimplemented `node:test` `t.skip`.

## Exact local checks

Run against the final implementation on Linux x86_64, Rust 1.97.1, Node 26.10.0, Bun 1.4.0 (the installed executable identifies itself as a canary build in crash diagnostics):

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --all-features` | 455 passed, 1 ignored; includes Linux native-addon fixtures and 21 scheduler tests |
| `cargo test --no-default-features` | 422 passed, 1 ignored |
| `bun test ./tests --timeout 30000` | 1,477 passed with debug addon |
| `bun test ./tests` | 1,477 passed with final release addon |
| `node --test tests/node/*.test.js` | 36 passed |
| `npm run lint:ts` | Passed |
| `npm run lint:playground` | Passed |
| `cargo check --no-default-features --features wasm --target wasm32-unknown-unknown` | Passed after native-dependency guard |
| `npm run test:wasm` | WASM release build, playground types and 9 browser tests passed |
| `npx napi build --platform --release --esm --js index.mjs` then `npx napi build --platform --release --js index.js` | Passed; regenerated bindings unchanged |
| `git diff --exit-code -- index.js index.mjs index.d.ts` | No public N-API binding changes |
| `bench/run-scheduler.sh` | Original baseline versus final implementation; see measured report and raw records |

Builds, addon replacement, tests and final benchmark timing are serialized. Earlier exploratory Bun processes also collided with addon replacement and are excluded from final results. The independently reproduced reviewed-head full-suite stack overflow was investigated under GDB, which showed recursive `ArrayCell` teardown. The final scoped repository suite passes. Unscoped `bun test` includes vendored input-device tests and fails because Bun cannot implement their Node-only skip call; it is not the repository test command after this change.

GitHub Actions was disabled (`GET /repos/nglmercer/napi-vm/actions/permissions` returned `enabled: false`). It was enabled before the final push so the pull-request workflow can run. The CI workflow also accepts manual dispatch and pushes to the PR branch, allowing an explicit final-head run when a synchronize event does not schedule one. Check [the PR's current checks](https://github.com/nglmercer/napi-vm/pull/7/checks) for the final documentation descendant of the implementation revision; local checks do not establish remote CI success.

CI was explicitly triggered for `4e0c271f8843ddf51a414329637217f76595efec` through both push and manual dispatch. [Manual run 36674668473](https://github.com/nglmercer/napi-vm/actions/runs/36674668473) failed before any job step ran. Quality and Linux check annotations both state: “The job was not started because your account is locked due to a billing issue.” Remote CI is **unavailable**, not a passing validation. Resolve the account billing lock and rerun the workflow on the final PR head before merging. The final documentation push also triggers that workflow.

## Remaining limitations / checks not run locally

- macOS, Windows, ARM native execution, cross-platform addon ABI behavior and Node-version matrices require CI; they were not run locally.
- Awaiting a Node host completion does not pump unrelated guest timers. Arbitrarily blocking in-process native functions cannot be preempted.
- A background auto-poll hard error stops session scheduling; recreate the session. Legacy checkpoint recovery does not refill exhausted hard budgets.
- Virtual/browser future-timer await is a nonblocking error with host-driven recovery through a stored promise, not resumable arbitrary top-level code.
- Opaque coroutine stacks conservatively retain pending results until their lifecycle ends. Live stored results remain bounded by the existing 1,024-handle limit and are consumed by await as before.
- Custom threaded bridges need wake notifications for prompt scheduler wakeups and must implement the optional execution-context hook to interrupt their own blocking operations. Sidecar startup/shutdown process-management polling remains separate from guest event waits.
- Benchmark Node allocations/retained bytes and browser/other-platform idle CPU were not measured. Rust records allocation/reallocation counts for entire batches, not retained memory or bytes. Single-host samples are not production guarantees.

See [event-loop behavior](event-loop.md) and [benchmarks](scheduler-benchmarks.md).

## Follow-up: remaining lifecycle and bridge issues (2026-09-30)

This section supersedes the earlier admission/lifecycle claims for the remaining
issues. Inspected PR head and reproduction baseline:
`e1467c1707a981c999b1489f66eb115c8b190b3c`. Implementation commits:
`7c6731d` (execution-scoped Node dependencies and deadlines) and `2fe37ba`
(explicit bridge wait capabilities and blocking-poll compatibility).
No workflow, account, billing, or permission changes were made. Remote CI was
not queried or run, as requested.

### Reproductions before fixes

- `node --test --test-name-pattern='dispatched host dependency|completed deadlines' tests/node/async-session.test.js`
  failed both added tests on the original addon. The dispatch barrier admitted
  reentry before guest await; cancellation used to release the would-be cycle
  produced `Guest execution cancelled` instead of an admission error. Idle
  metadata after a completed 50 ms execution failed with `Guest execution
  deadline exceeded`.
- `cargo test --no-default-features --test scheduler blocking_only_bridge`
  failed the added bridge regression: `blocking-only bridge was never given a
  blocking poll`. The barrier timed out and cancellation woke the old private
  wait, so the reproduction did not leave a hanging owner.

The final tests additionally cover delayed callback continuations, immediate
await (existing reentry tests), unrelated sessions, unawaited completed and
unresolved results, later awaiting saved results, rejection, cancellation and
disposal. Injectable execution clocks verify retirement and preservation of
original deadlines for future timers, unfinished checkpoints and rooted async
continuations. Channel barriers cover blocking-only event delivery,
cancellation, deadline expiry, no-event timeout, shutdown and notifications
latched between readiness checking and sleep. Existing hard-budget, GC-root,
legacy ordering and native-addon tests remain enabled.

### Final local validation

Linux x86_64; Rust 1.97.1, Node 26.10.0, Bun 1.4.0 (canary executable). These
checks cover the final implementation; the subsequent Rustdoc correction and
this verification record have no runtime changes.

| Exact command | Result |
| --- | --- |
| `npm run lint:rust` | Formatting and Clippy, all targets/all features, warnings denied: passed |
| `npm run lint:ts` | Passed |
| `cargo test --release --all-features` | 460 passed, 1 ignored; 371 unit tests and 24 scheduler integration tests; Linux native-addon fixtures included |
| `cargo test --no-default-features` | 427 passed, 0 ignored |
| `npm run build:all` | Release ESM and CommonJS native addons built successfully |
| `git diff --exit-code -- index.js index.mjs index.d.ts` | Passed; regenerated public N-API surfaces unchanged |
| `npm run test:node` | 40 passed, 0 failed |
| `npm test` | Bun: 1,481 passed across 64 files, 0 failed, 2,423 assertions |
| `npm run test:wasm` | Release wasm-pack build, playground TypeScript and 9 Node-hosted WASM tests passed |
| `cargo fmt --all -- --check` | Passed again after the Rustdoc correction |
| `git diff --check` | Passed |

Local logs are `/tmp/pr7-followup-{release,core-final,lint-rust,ts,build,node-final,bun,wasm}.log`.
WASM reports the existing unused native-addon digest helper warning; the build
and tests succeed. The ignored test is
`tokio_loop_delivers_real_rdev_node_events_on_both_backends`, requiring a built
`dist/rdev-node`, isolated X display and `RDEV_NODE_TEST_LOOPBACK=1`.

### Idle waiting smoke check and limitations

With the final release addon, ten AsyncSessions first completed `run('42;')`;
then a Node `setTimeout` held the process for one second while `process.cpuUsage`
and each session's `wakeups()` were sampled. Measured wall time **1000.488 ms**,
process CPU **1.203 ms**, and **zero additional wakeups in all ten owners**.
This is a single local smoke measurement, not a comparative benchmark or a
speedup claim. The notifier and legacy-poll tests also verify that notifier
polls remain nonblocking and legacy no-event polls are bounded rather than
busy-spinning.

Reproduce the idle measurement after building the addon:

```sh
node <<'JS'
const {AsyncSession}=require('./index.js');
(async()=>{
  const sessions=Array.from({length:10},()=>new AsyncSession());
  try {
    await Promise.all(sessions.map(s=>s.run('42;')));
    const before=sessions.map(s=>s.wakeups());
    const cpu=process.cpuUsage(), start=performance.now();
    await new Promise(r=>setTimeout(r,1000));
    const elapsedMs=performance.now()-start, used=process.cpuUsage(cpu);
    console.log({elapsedMs,cpuMs:(used.user+used.system)/1000,
      wakeups:sessions.map((s,i)=>s.wakeups()-before[i])});
  } finally { sessions.forEach(s=>s.dispose()); }
})().catch(e=>{console.error(e);process.exitCode=1;});
JS
```

Blocking-only bridges must honor timeout arguments. Their cancellation latency
can include one 10 ms poll slice plus scheduling overhead; a bridge that ignores
its timeout cannot be forcibly interrupted safely. Admission conservatively
rejects commands while an active execution has unsettled Node dependencies;
stored unawaited results alone do not lock an idle session. Actual background
failures remain terminal. macOS/Windows native execution, real browser GUI
execution and the isolated X-display test were not run. Remote CI remains
explicitly outside this follow-up's scope.

## Cross-execution reentry follow-up (2026-09-30)

Review baseline: `05e1cf0657cffeb264748455ee45a9c54f30b4ee`. The earlier
execution-epoch admission claim was incomplete: an unresolved result saved by
A could be awaited by B while A's callback queued and awaited a command behind
B. Fixed in `1131fc4f97519f1b453e4b61d0079a9041cb808b`.

### Reproduction and fix

Before the fix:

- `node --test --test-name-pattern='saved host result protects' tests/node/async-session.test.js`
  failed on the original release addon after 2009 ms with `Guest execution
  deadline exceeded`. A stored `saved=background()` and completed; B was
  admitted before the Node continuation barrier was released. B executed a
  pure guest prelude before `await saved`. The reentrant command was admitted
  and the execution deadline served only as an escape from the cycle.
- `cargo test --all-features dependency_lifecycle_tests` failed at
  `assert!(bridge.owner_waiting_for_node())`. This test creates no Node handles
  and explicitly transitions A to inactive and then starts B with A's
  dependency still unresolved, deterministically exposing the epoch reset.

The dependency registry is now an immutable session-lifetime `Arc`. Ending an
execution changes its active flag; it cannot replace the registry and orphan
old leases. Admission consults every unsettled Node call while guest execution
is active or admitted. A guest command reserves admission before enqueueing
and keeps its RAII lease through completion, so an old callback cannot queue
behind B even before B starts. Failed submissions, cancelled/removed commands,
panics and shutdown drop the reservation through the same ownership path.
`await_host` retains its existing wait guard and stored-result ownership.

This does not lock idle sessions based on the number of stored results: while
no guest execution is active/admitted, unresolved unawaited results permit new
commands. Settled stored results have released their Node dependency. Later
await, settlement/rejection, abandonment and cancellation keep their existing
cleanup and GC-root behavior. Deadlines, scheduler waits, public APIs and hard
budgets are unchanged by this fix. The restriction is conservative during guest
work: guest code may await an old unresolved result later in that execution.

After the fix, the native regression rejects the callback's reentry, B returns
`7`, and subsequent work succeeds, without cancelling B. The Rust test covers
both B-admitted and B-active phases and verifies release on settlement. Existing
same-execution, idle callback reentry, unresolved/completed saved results,
rejection, cancellation, disposal and unrelated-session tests remain enabled.

### Exact final local checks

| Command | Result on the final implementation |
| --- | --- |
| `npm run lint:rust` | Formatting and Clippy all targets/all features with warnings denied: passed |
| `npm run lint:ts` | Passed |
| `cargo check --all-features` | Passed |
| `cargo test --release --all-features` | 461 passed, 1 ignored; includes native-addon fixtures and the new deterministic dependency test |
| `cargo test --no-default-features` | 427 passed, 0 ignored |
| `npm run build:all` | Release ESM and CommonJS native-addon builds passed |
| `git diff --exit-code -- index.js index.mjs index.d.ts` | Passed; regenerated bindings unchanged |
| `npm run test:node` | 41 passed with release addon, 0 failed |
| `npm test` | Release addon: 1,482 passed across 64 files, 0 failed, 2,423 assertions |
| `npm run test:wasm` | Release WASM build, playground types, and 9 Node-hosted WASM tests passed |
| `git diff --check` | Passed |

The initial debug-addon `npm test` run had 1,481 passes and one timeout:
`dynamic global creation is bounded through all global aliases` exceeded Bun's
5000 ms default (6583 ms recorded). The unchanged test passed with release in
1152 ms, and the full release suite passed in 8.61 s. The default-timeout debug
suite is therefore **not claimed to pass**. The debug Node suite passed all 41
tests; the final release regression passed in Bun in 41 ms.

Logs: `/tmp/pr7-cross-epoch-{repro,rust-repro,release,core,lint-final,ts,build,node,bun,wasm}.log`.
The ignored X-display/native-input test and unrun macOS/Windows and real-browser
GUI checks retain the limitations documented above. No remote CI was queried,
no workflows/account/billing/permission settings changed, and no merge was
performed. No performance speedup is claimed.

## Integration with current main before merging PR #7

Integrated base `0fa987d8860d620cd1008a84f2a17c9b67c495cd` into PR head
`0c8f3a8b97902c69ba24504174dc667bb4ac10e8` to resolve merge conflicts.
Kept main's broader iterative Value teardown helpers and deep array/object/proxy
test, together with the PR's small-stack weak-registry regression. Retained the
native-only plugin file-URL guard and the working `bun test ./tests` package
command. Workflow files match current main exactly; no workflow/account/billing
or permission changes were introduced relative to the merge base.

Post-resolution local validation:

| Command | Result |
| --- | --- |
| `cargo test --all-features` | 462 passed, 1 ignored; includes native-addon fixtures and both teardown regressions |
| `npm run lint:rust` | Formatting and Clippy all targets/all features: passed |
| `npm run lint:ts` | Passed |
| `npm run build:all` | Release ESM/CommonJS addons built; generated bindings unchanged |
| `npm run test:node` | 41 passed |
| `npm test` | Release addon: 1,482 passed across 64 files, 0 failed |
| `npm run test:wasm` | Release WASM build, playground types, 9 Node-hosted WASM tests passed |
| `git diff --cached --check` and `git diff --check` | Passed |
| `git diff origin/main -- .github/workflows` | Empty: workflows unchanged from current main |

Logs are `/tmp/pr7-merge-{rust,lint,ts,build,node,bun,wasm}.log`. Remote CI was not
inspected. The ignored isolated-X-display test and unrun macOS/Windows and
real-browser GUI checks retain the limitations listed above.
