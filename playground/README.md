# napi-vm playground

The playground runs the Rust VM as WebAssembly in the browser. Vite serves a
Preact/TypeScript frontend and the generated wasm package; JavaScript execution
runs in the page. Completion, hover, and diagnostics use the shared Rust
[language core](../crates/napi-vm-core/src/lang/).

## Build and run

Install Node.js and the repository dependencies, Rust with the
`wasm32-unknown-unknown` target, and [wasm-pack](https://rustwasm.github.io/wasm-pack/).
From the repository root:

```sh
npm ci --ignore-scripts
rustup target add wasm32-unknown-unknown
npm run playground:build
npm run playground
```

Open the URL printed by Vite (normally `http://localhost:3000`). The build
writes generated wasm and JavaScript glue to the ignored `playground/pkg/`
directory; rebuild it after changing the Rust core. The Vite configuration
also writes production frontend output to `playground/dist/`.

For the browser API smoke test, install Bun and run `bun playground/smoke.ts`
from the repository root after building the wasm package. The broader wasm
checks are available as `npm run test:wasm`; frontend types can be checked
with `npm run lint:playground`.

## Layout

- `vite.config.ts` configures the development server and `/pkg` alias.
- `public/index.html` loads `public/src/main.tsx`; `components/`, `hooks/`,
  `styles/`, and `i18n/` contain the frontend.
- `public/src/vm.ts` initializes `WasmVm`, registers example modules and host
  functions, and converts editor UTF-16 offsets to the UTF-8 byte offsets
  used by the language service.
- `public/examples/` contains the sample programs and modules.
- `smoke.ts` exercises execution, logging, callbacks, modules, and language
  services against the generated wasm package under Bun.

The playground exposes host callbacks explicitly. A browser-hosted VM does not
provide Node.js APIs unless the host implements and exposes them.
