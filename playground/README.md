# Browser VM and playground

Run the Rust engine as WebAssembly and explore it through a Preact/TypeScript
editor. Guest execution happens in the page. Completion, hover, and diagnostics
use the shared Rust language core.

## Build and open

Install workspace dependencies, Rust, wasm-pack, and the browser target. From
the repository root:

```sh
npm ci --ignore-scripts
rustup target add wasm32-unknown-unknown
npm run playground:build
npm run playground
```

Open Vite's printed URL, normally `http://localhost:3000`. The WASM build writes
`playground/pkg`; rebuild it after changing Rust. Vite's production frontend
output is `playground/dist`. The configured development server listens on all
interfaces, so choose local network exposure deliberately.

## Embed the browser binding

```javascript
import init, { WasmVm } from './pkg/napi_vm.js';
await init();
const vm = new WasmVm();
try {
  const result = vm.run('6 * 7');
  if (!result.ok) throw new Error(result.error);
  console.log(result.value);
} finally {
  vm.free();
}
```

Serve the generated JavaScript and `.wasm` file together. This is the web-target
binding, not the native Node `Vm` API. `run` returns a result object with success,
value/error, and logs; errors need checking. `run_file` supplies a file/module
context for relative imports. Host functions and modules are registered
explicitly through the binding.

Timers require the runtime-enabled build and `enable_timers()`. Select a clock
with `set_clock` and drive `advance_clock`/`poll_event_loop` as appropriate.
The VM does not automatically expose Node APIs or arbitrary browser globals.
Language-service offsets use UTF-8 bytes; the frontend converts editor UTF-16
indices before analysis calls.

## Verify and locate code

```sh
npm run test:wasm
bun playground/smoke.ts
```

`test:wasm` rebuilds the WASM package, typechecks the frontend, and runs browser
binding regressions under Node. The Bun smoke also requires a built package.
These checks exercise the API, not visual browser rendering.

`public/src/vm.ts` owns initialization and host registration; `components/`,
`hooks/`, `styles/`, and `i18n/` own the UI. `public/examples/` contains programs.
See [editor integration](../docs/editor.md) and [architecture](../docs/architecture.md).
