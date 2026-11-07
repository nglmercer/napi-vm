# Build and run your first program

This guide uses the Node.js binding from a local checkout. For a Rust-only host,
go directly to [Rust embedding](rust.md). For a browser host, use the
[playground setup](../playground/README.md).

## Build the local addon

Install Node.js 22+, npm, a Rust toolchain supporting edition 2024, and a native
C/C++ build toolchain. CI's selected Rust version is recorded in
[ci.yml](../.github/workflows/ci.yml). Bun is needed for the main JavaScript test
suite, but a Node consumer does not need it.

```sh
npm ci --ignore-scripts
npm run build
```

`npm run build` produces a platform-specific `.node` library and the CommonJS
loader. `npm run build:all` refreshes CommonJS, ESM, and TypeScript declarations
together. Rebuild after changing Rust code; an existing platform binary may
belong to another revision.

## Keep state between evaluations

```javascript verify
const { Vm } = require('./index.js');
const assert = require('node:assert/strict');
const vm = new Vm();
try {
  vm.run('let count = 0;');
  assert.equal(vm.run('++count'), '1');
  assert.equal(vm.run('++count'), '2');
} finally {
  vm.dispose();
}
```

`Vm.run` returns a printable string. Reuse the VM for persistent declarations;
use `runCode(source)` for a single fresh evaluation. For object and array
results, call a named guest function instead of parsing display text.

## Exchange data and functions

```javascript verify
const { Vm } = require('./index.js');
const assert = require('node:assert/strict');
const vm = new Vm();
try {
  vm.setGlobal('settings', { multiplier: 3 });
  vm.exposeFunction('hostAdd', (a, b) => a + b);
  vm.run(`
    function calculate(input) {
      return { total: hostAdd(input.value, 2) * settings.multiplier };
    }
  `);
  assert.deepEqual(vm.callFunction('calculate', [{ value: 4 }]), { total: 18 });
} finally {
  vm.dispose();
}
```

Every exposed callback runs with the host application's authority. Validate its
arguments and enforce any access policy inside the callback. The guest does not
automatically gain access to `process`, host modules, or the host global object.
See [conversion rules](api.md#value-conversion).

## Register modules before importing them

```javascript verify
const { Vm } = require('./index.js');
const vm = new Vm();
try {
  vm.defineModule('math', 'export const double = x => x * 2;');
  require('node:assert/strict').equal(
    vm.run("import { double } from 'math'; double(21);"), '42');
} finally {
  vm.dispose();
}
```

`defineModule` delays evaluation until import. Register all sources first when
the module graph contains cycles. [Module loading](modules.md) also explains
eager registration, host modules, file loaders, and native addons.

## Await a host callback

```javascript verify
const { AsyncSession } = require('./index.js');
const session = new AsyncSession();
try {
  await session.exposeFunction('loadPrice', async () => 21, true);
  const result = await session.run('await loadPrice() * 2');
  require('node:assert/strict').equal(result, '42');
} finally {
  session.dispose();
}
```

This block belongs inside an async function when used in a CommonJS file.
`AsyncSession` serializes commands on a persistent owner worker. See
[async execution](async.md) for cancellation, clocks, queue limits, and the
separate `Vm.runAsync` API.

## Diagnose setup problems

| Symptom | Check |
| --- | --- |
| Native module cannot be found | Run the native build on this OS/architecture; confirm `index.js` and its platform `.node` file are together. |
| A runtime capability is unavailable | Compile its Cargo feature, then explicitly enable it. See [runtime setup](runtime.md). |
| A module cannot be resolved | Define the source before importing, or install the appropriate host loader. |
| An async example keeps the process alive | Dispose the VM/session in `finally`; stop any live metadata session separately. |
| An expression returns unexpected text | Use `callFunction` for structured output; `run` is a display API. |

Continue with the [Node reference](api.md) and [safety model](safety.md).
