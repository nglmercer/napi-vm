# napi-vm

An embeddable JavaScript engine written in Rust, with Node.js bindings,
WebAssembly support, an optional runtime, and editor tooling.

The engine owns parsing, bytecode execution, ECMAScript builtins, modules,
and guest values. Hosts explicitly provide filesystem, network, timers, and
other capabilities. Native addons and trusted process plugins run with host
privileges; see [safety](docs/safety.md) before choosing an integration.

## Start with Node.js

<a id="quick-start"></a>

From a checkout, install dependencies and build the local addon:

```sh
npm ci --ignore-scripts
npm run build
```

```javascript
const { Vm } = require('./index.js');

const vm = new Vm();
vm.run('let answer = 40 + 2;');
console.log(vm.run('answer;')); // 42, returned as a string
vm.dispose();
```

Node.js 22+ is required by the package. Building needs a Rust toolchain;
CI pins its toolchain in [the workflow](.github/workflows/ci.yml). Bun runs
the main JavaScript test suite, but is not required by Node consumers.
Use [getting started](docs/getting-started.md) for host callbacks, modules,
async execution, and browser setup.

## Choose an integration

<a id="rust-embedding-and-commonjs"></a>
<a id="experimental-rust-node-api-host"></a>

| Need | Entry point |
| --- | --- |
| Embed JavaScript in Node | `Vm`, `runCode`, or `AsyncSession`; [API](docs/api.md) |
| Embed the engine in Rust | `napi-vm-core`; [crate architecture](docs/crate-architecture.md) |
| Install runtime capabilities | `napi-vm-runtime`; [runtime guide](docs/optional-runtime.md) |
| Load CommonJS or native addons | [Native addon guide](docs/native-addons.md) |
| Run sandboxed guest plugins | [VM plugins](docs/plugins.md) |
| Run trusted Node/Bun/native child plugins | [Trusted process plugins](docs/trusted-plugins/README.md) |
| Integrate editors or live metadata | [Editor guide](docs/editor.md) |
| Build the browser playground | [Playground](playground/README.md) |

Guest strings retain UTF-16 code units, including lone surrogates. The
Rust engine and optional runtime are separate crates; the `napi-vm` facade
retains its default `napi` feature. See the architecture guide for feature
selection and host string boundaries.

## Validation

<a id="status"></a>

The selected, explicitly labelled **`reviewed-5112` Test262 corpus passes
102,992/102,992 variants**, with zero skips or errors, at two workers and a
ten-second deadline. It is the pinned upstream corpus plus the reviewed,
unmerged correction from PR #5112. The original corpus remains available
and contains contradictory assertions. [Report and audit](tools/test262/evidence/reviewed-5112-full-2026-11-06/README.md).

```sh
cargo build --release --locked --no-default-features --bin napi-vm-test262
cargo build --release --locked -p napi-vm-test262-runner
target/release/test262 fetch --profile reviewed-5112
target/release/test262 run --profile reviewed-5112 --jobs 2 --timeout 10 \
  --output reviewed-5112-full.json
```

[The Test262 guide](tools/test262/README.md) explains the original and
reviewed profiles, isolated workers, comparisons, and archived reports.
Conformance results apply to the recorded corpus and engine digest; they
do not imply complete Node, browser, or native-addon compatibility.

## Develop and measure

```sh
npm run test:rust
npm test
npm run lint
```

Read [development](docs/development.md) for prerequisites and targeted
checks, and [performance](docs/performance.md) for reproducible baselines,
coverage, and comparisons. Build artifacts before timing; keep correctness
checks separate from performance measurements.

## Documentation

[Documentation index](docs/README.md) · [implementation map](docs/roadmap.md)

<a id="disposing-a-vm"></a>

Dispose async VMs and sessions in `finally`; see [API lifetime rules](docs/api.md#diagnostics-and-disposal).

## License

[MIT](LICENSE).
