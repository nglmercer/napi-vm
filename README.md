# napi-vm

Run JavaScript inside a Rust engine that you can embed in Node.js, Rust, or
WebAssembly. The project provides its own parser, interpreter, bytecode engine,
ECMAScript builtins, module system, and language service.

Choose which host functions and runtime capabilities guest code can use.
Filesystem access, networking, timers, and native code are explicit integration
choices. Read the [trust and resource model](docs/safety.md) before running
untrusted programs.

## Try it from a checkout

You need Node.js 22+, npm, Rust, and a native compiler/linker. Run these commands
at the repository root:

```sh
npm ci --ignore-scripts
npm run build
```

```javascript verify
const { Vm } = require('./index.js');
const vm = new Vm();
try {
  vm.setGlobal('price', 21);
  const result = vm.run('price * 2');
  console.log(result); // "42": run() returns formatted text.
  require('node:assert/strict').equal(result, '42');
} finally {
  vm.dispose();
}
```

For structured results, define a guest function and use `callFunction`.
For asynchronous host callbacks, use `AsyncSession` or `Vm.runAsync`.
The [first-program guide](docs/getting-started.md) walks through both.

## Choose your entry point

| What you want to build | Start here |
| --- | --- |
| A Node.js application embedding guest JavaScript | [Node API](docs/api.md) |
| A Rust application embedding the engine | [Rust embedding](docs/rust.md) |
| An application with timers, I/O, or package loading | [Optional runtime](docs/runtime.md) and [CLI](docs/cli.md) |
| A browser integration | [WebAssembly and playground](playground/README.md) |
| Plugins evaluated inside separate VMs | [VM plugins](docs/plugins.md) |
| Trusted plugins running in Node, Bun, or native child processes | [Trusted process plugins](docs/trusted-plugins/README.md) |
| Editor completion, hover, and diagnostics | [Editor integration](docs/editor.md) |
| A contribution to the engine | [Architecture](docs/architecture.md) and [development](docs/development.md) |

## Understand the current evidence

The retained `reviewed-5112` Test262 run passes **102,992 variants**, with zero
skips or non-passing results. That profile applies one explicitly identified
correction to the pinned upstream corpus. Results identify their engine binary
and corpus; they do not establish complete Node.js or browser compatibility.
See [validation and compatibility](docs/validation.md).

Benchmarks compare ten synchronous workloads with rquickjs 0.14.0 and retain
separate startup, source-evaluation, and direct-call results. Several measured
paths have been optimized; rquickjs remains faster across the retained warm-call
workloads. See [performance and reproducibility](docs/performance.md).

## Find the rest

[Documentation](docs/README.md) · [Examples](examples/) ·
[Generated Node types](index.d.ts) · [MIT license](LICENSE)
