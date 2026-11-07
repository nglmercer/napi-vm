# Test trusted plugins and interoperability

Use three levels of verification: business logic, contract/lifecycle harness,
and actual packaged child-process execution. Each proves a different boundary.

## Build the checkout fixtures

Install Node.js 22+, Bun, Rust, and workspace dependencies, then run:

```sh
npm run plugins:generate
npm run plugins:build
```

JavaScript/TypeScript examples and SDK packages are emitted before tests load
them. Rust fixtures build through Cargo. Generated outputs are committed, but
generated JavaScript build products are not a substitute for building the
current source.

## Run the full plugin gate

```sh
npm run plugins:check
```

This checks generation/assets, builds both language stacks, typechecks/lints,
runs Node/Bun/Rust suites, exercises interop, and verifies packaged consumers.
Use the individual scripts to reproduce a failure:

| Command | Boundary |
| --- | --- |
| `plugins:test:node`, `plugins:test:bun` | Protocol, SDK, host, generator, CLI, and example JS suites |
| `plugins:test:rust` | Rust protocol/SDK/host/conformance and example crates |
| `plugins:test:interop`, `plugins:test:rust-interop` | Real processes across supported host/plugin language combinations |
| `plugins:test:packaged` | Relocation, integrity rejection, independent executable artifacts |
| `plugins:test:consumers`, `plugins:test:rust-consumer` | External consumer paths |
| `plugins:check-generated`, `plugins:check-assets` | Deterministic contracts, metadata, scaffold copies |
| `plugins:test:cargo-package` | Rust package construction/consumer prerequisites |

Scripts set up their own fixtures where implemented. Inspect their prerequisites
before running on a machine without Bun, Cargo, native compilers, or platform
runtime dependencies. An explicitly skipped unavailable runtime is different
from a passing test of that runtime.

## Test a plugin you authored

Run scaffold unit/harness tests, build the selected format, then run
`napi-vm-plugin test DIRECTORY --artifact`. `plugin.test.json` supplies explicit
scenarios and expected data. Native artifacts require `--host-executable`.
When testing a distributed directory, include its scenario file explicitly;
packaging does not include `plugin.test.json` automatically. Testing a built
source project without a lock packages it temporarily before execution.
Test cancellation, deadlines, invalid inputs/results, listener errors, bounded
queues, missing services, reload/state incompatibility, crashes, and shutdown.

For state migration, compare the externally observed contract before and after
replacement; do not claim migration works merely because a snapshot parses.
For native dependencies, run on the target platform/ABI rather than inferring
runtime support from cross-compilation alone.

## Recorded evidence and measurements

The [evidence index](evidence/README.md) links retained interoperability,
package, compatibility, and benchmark captures. These records identify a past
run; regenerate them when behavior or artifacts change. The workflow
[trusted-plugins.yml](../../.github/workflows/trusted-plugins.yml) defines the
current CI matrix.

`npm run plugins:bench` and `npm run plugins:bench:rust` measure process-plugin
paths. The [general performance guide](../performance.md) also links the
isolated event-queue optimization. In-process harness delivery, TCP calls,
startup, and native artifact throughput are separate units of work.
