# Async execution and clocks

The engine owns guest jobs and promises. The embedding host decides how timers,
external completions, cancellation, and scheduling make progress. A promise
alone does not provide an OS event loop.

## Choose a Node API

Use `Vm.runAsync` when an existing `Vm` needs to await host callbacks:

```javascript verify
const { Vm } = require('./index.js');
const vm = new Vm();
try {
  vm.exposeAsyncFunction('lookup', async key => ({ key, value: 42 }));
  const result = await vm.runAsync("(await lookup('answer')).value");
  require('node:assert/strict').equal(result, '42');
} finally {
  vm.dispose();
}
```

Run this inside an async function in CommonJS. Await each evaluation before
starting dependent work on the same VM. Disposal releases the native async
dispatch handles that can otherwise keep Node alive.

Use `AsyncSession` for a persistent owner worker and an entirely asynchronous
command API:

```javascript verify
const { AsyncSession } = require('./index.js');
const assert = require('node:assert/strict');
const session = new AsyncSession({ commandCapacity: 32, maxJobsPerTurn: 64 });
try {
  await session.setGlobal('base', 20);
  await session.exposeFunction('increment', async n => n + 1, true);
  assert.equal(await session.run('await increment(base) * 2'), '42');
} finally {
  session.dispose();
}
```

The command queue is bounded; producers must handle rejected work rather than
assuming unlimited buffering. `setExecutionLimits(fuel, timeoutMs?)` configures
session execution. `cancel()` signals cancellation without waiting for a queued
command. Cancellation does not undo host side effects and cannot forcibly
interrupt arbitrary blocking code inside a trusted host callback.

## Drive timers deliberately

Timer globals require the runtime feature and explicit installation. For
`AsyncSession`, request `timers: true`; for `Vm`, call
`enableRuntime({ timers: true })`. A core interpreter has scheduler machinery
but does not install `setTimeout` automatically.

| Clock mode | What advances time |
| --- | --- |
| `legacy` | The compatibility scheduler's timer progression |
| `virtual` | Explicit host calls to advance the clock |
| `real-time` | Elapsed monotonic host time |

For deterministic tests, create a virtual-clock session with `autoPoll: false`,
schedule guest work, call `advanceClock(milliseconds)`, and call
`pollEventLoop(maxJobs)` until the expected result is observed. Do not await a
guest timer that the same host task still needs to advance.

The poll result includes `executedJobs`, `runnable`, `checkpointPending`,
`yieldReason`, and `nextDeadline`. A future deadline is not runnable work.
`checkpointPending` means a bounded turn stopped with work still queued;
continue driving it according to the application's scheduling policy.

## Rust and WASM hosts

Rust embedders configure `EventLoopOptions`, a `TurnBudget`, and a clock on
the interpreter. External producers send completion records through a host
bridge; guest objects remain on their owner. `Runtime::poll` performs a bounded
turn, `run_until_idle` stops when no work is immediately runnable, and
`run_event_loop` can wait for timers or external completion. Inspect
[the scheduler](../crates/napi-vm-core/src/interpreter/scheduler.rs) and
[the OS adapter](../crates/napi-vm-runtime/src/runtime/optional.rs) when
implementing a custom wake mechanism.

Browser bindings expose `enable_timers`, `set_clock`, `advance_clock`, and
`poll_event_loop`. The browser host must drive polling; running a guest
evaluation does not install a background Node-style event loop.

## Check lifecycle behavior

The [scheduler benchmark scripts](../bench/run-scheduler.sh) and
[Node scheduler tests](../tests/node/) cover different host boundaries.
When adding a scheduling feature, verify bounded turns, cancellation, shutdown,
and process exit in addition to successful promise resolution.
