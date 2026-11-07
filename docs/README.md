# Documentation

Use these guides to build an integration, understand its boundaries, and verify
it against the current implementation. Commands assume the repository root
unless a guide states otherwise.

## Start running code

1. [Build and run your first program](getting-started.md).
2. Choose [Node.js](api.md), [Rust](rust.md), or
   [WebAssembly](../playground/README.md).
3. Learn [module registration and loading](modules.md) and
   [async execution and clocks](async.md).
4. Select [runtime capabilities](runtime.md), or use the [CLI](cli.md).

## Build an application

| Task | Guide |
| --- | --- |
| Set limits and decide what code to trust | [Safety](safety.md) |
| Host guest plugins in VMs | [VM plugins](plugins.md) |
| Author or host independent child-process plugins | [Trusted plugins](trusted-plugins/README.md) |
| Load CommonJS and native addons | [Modules and native code](modules.md) |
| Connect an editor to static and live metadata | [Editor integration](editor.md) |
| Implement the editor metadata connection | [Runtime metadata protocol](runtime-protocol.md) |

## Work on the project

| Task | Guide |
| --- | --- |
| Locate implementation owners and feature boundaries | [Architecture](architecture.md) |
| Build, test, lint, and regenerate outputs | [Development](development.md) |
| Interpret conformance and compatibility results | [Validation](validation.md) |
| Measure throughput and compare engines | [Performance](performance.md) |
| Generate Unicode, locale, and timezone tables | [Table generators](../tools/generators/README.md) |
| Run isolated Test262 workers | [Test262 runner](../tools/test262/README.md) |
| Maintain native build infrastructure | [Build tools](../tools/ci/README.md) |

Package READMEs explain each independently consumed package. The
[trusted-plugin contract guide](trusted-plugins/contracts.md) links generated
wire references and their source definitions.

## Documentation ownership

The current guides are written from public types, implementation, executable
examples, and recorded results. Old work plans and implementation checkpoints
and historical implementation logs, benchmark captures, and plugin validation
reports have been removed; Git history retains them. Current conformance and
performance references remain under `tools/test262/evidence/` and
`bench/baselines/`. They describe the recorded build, not an automatic claim
about a later build.

When behavior changes, update its owning guide and runnable examples. Regenerate
generated references from their source. Run `node tools/docs/check.mjs` and
`node tools/docs/verify-examples.mjs` before submitting documentation changes.
Use its `--rust` option for Rust blocks; [the tooling guide](../tools/docs/README.md)
explains build prerequisites and the CI checks.
