# Run the command-line runtime

Build the executable with the capabilities your application needs. A minimal
runtime CLI includes console, timers, and permission-checked environment access:

```sh
cargo build --release --locked --no-default-features --features runtime-cli --bin napi-vm
target/release/napi-vm info
target/release/napi-vm eval '6 * 7'
```

Add `runtime-fs`, `runtime-net`, `runtime-web`, `runtime-npm`, or `runtime-node`
to the comma-separated feature list when needed. Compiled I/O remains subject
to explicit grants. See [runtime setup](runtime.md).

## Commands

| Command | Purpose |
| --- | --- |
| `napi-vm run [options] app.js` | Evaluate the host-selected entry file |
| `napi-vm eval [options] 'source'` | Evaluate supplied source |
| `napi-vm repl [options]` | Interactive evaluation |
| `napi-vm info` | Print build/runtime information |
| `napi-vm install [options] [--locked] npm:package` | Install a package with the npm feature and required permissions |

The CLI reads an explicitly selected entry as host input. Guest file access and
dependency loading still require permission. Use `--` before operands that
would otherwise be parsed as flags.

## Permission flags

| Flag | Scope |
| --- | --- |
| `--allow-read=DIR` | Read within a granted filesystem root |
| `--allow-write=DIR` | Write within a granted filesystem root |
| `--allow-net=HOST[:PORT]` | Connect to the specified host, optionally restricted to a port |
| `--allow-env=NAME` | Read one environment variable |
| `--allow-ffi` | Authorize native integration where available |
| `--allow-run` | Authorize process operations where implemented |
| `--compat=node` | Select implemented Node compatibility, requiring `runtime-node` |

For a local file application, build with `runtime-cli,runtime-fs`, then use:

```sh
target/release/napi-vm run --allow-read=./app ./app/main.js
```

Granting a permission does not install a missing implementation. A manifest,
package, or guest program cannot grant itself access.

## Execution and resource limits

`--max-time=5s` sets an elapsed execution deadline. Other runtime flags are
`--max-jobs=N`, `--max-stack=N`, `--max-fuel=N`, `--max-io=N`,
`--max-timers=N`, and `--max-file-bytes=N`. Time values accept `ms` or `s`.

`--isolate=process` uses the CLI's child-process isolation adapter. Associated
flags include `--max-memory=128M` and `--max-cpu=5s`. Platform support is enforced
by [isolation.rs](../crates/napi-vm-runtime/src/cli/isolation.rs); do not assume
an OS limit is available merely because an in-process timeout exists.

The CLI is an explicit runtime host. It is not a replacement for Node's full
CLI, package manager, standard library, debugger, or native-addon ABI. Use
[native-addon integration](modules.md#native-addons) for those trust decisions.
