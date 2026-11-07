# Getting started

## Build from a checkout

Use Node.js 22+, npm, and a Rust toolchain compatible with the pinned CI
version in [ci.yml](../.github/workflows/ci.yml). Bun is needed for the main
JavaScript regression suite. Install `wasm-pack` separately for a browser build.

```sh
npm ci --ignore-scripts
npm run build
```

Build the local addon after changing Rust code. A platform binary already in
the checkout may come from an older source revision. Generated binding files
and native artifacts are checked separately in CI.

## Keep state in a VM

```javascript
const { Vm, runCode } = require('./index.js');

console.log(runCode('2 + 2;')); // '4'
const vm = new Vm();
vm.run('let count = 0;');
console.log(vm.run('++count;')); // '1'
console.log(vm.run('++count;')); // '2'
vm.dispose();
```

`runCode` creates a fresh execution context. A `Vm` retains its globals and
modules across calls. `run` returns a display string; use `callFunction` to
obtain a marshalled guest result. [API details](api.md).

## Expose a host function

```javascript
const { Vm } = require('./index.js');
const vm = new Vm();
try {
  vm.exposeFunction('add', (a, b) => a + b);
  console.log(vm.run('add(20, 22);')); // '42'
} finally {
  vm.dispose();
}
```

The host decides what the callback may access. Validate arguments, paths,
request sizes and permissions inside privileged callbacks. Guest isolation
is not an operating-system sandbox; read [safety](safety.md).

## Await host work

```javascript
const { Vm } = require('./index.js');

async function main() {
  const vm = new Vm();
  try {
    vm.exposeAsyncFunction('lookup', async (id) => ({ id, name: 'example' }));
    console.log(await vm.runAsync(`
      async function main() { return (await lookup(7)).name; }
      main();
    `));
  } finally {
    vm.dispose();
  }
}
main().catch((error) => { console.error(error); process.exitCode = 1; });
```

`Vm.runAsync` lazily creates a persistent executor for that VM. Await each
operation before starting another operation on the same VM; busy admission
checks reject overlapping use. Dispose the VM to retire host resources.
For an explicitly async-only owner with queued commands, clocks and bounded
event turns, use `AsyncSession` and the [event-loop guide](event-loop.md).

## Define modules before importing

```javascript
const { Vm } = require('./index.js');
const vm = new Vm();
try {
  vm.defineModule('math', 'export function double(n) { return n * 2; }');
  console.log(vm.run("import { double } from 'math'; double(21);"));
} finally {
  vm.dispose();
}
```

`defineModule` stores source for deferred evaluation and supports defining a
cyclic graph before its first import. `registerModule` evaluates eagerly;
a failed registration preserves the previous export table, but does not
undo arbitrary global side effects. [Module API](api.md#modules).

## Add capabilities deliberately

A bare engine has no Node `require`, filesystem, network, console or timers.
Runtime Cargo features make implementations available; the host still has to
install capabilities and grant permissions. See [the runtime guide](optional-runtime.md)
and [native addons](native-addons.md) for the two separate host boundaries.

## Browser and editor tooling

```sh
npm run playground:build
npm run playground
```

The browser build generates `playground/pkg` before Vite starts. See
[the playground README](../playground/README.md) for its UI and
[editor integration](editor.md) for the shared language service, native LSP,
Zed extension and opt-in live metadata sessions.
