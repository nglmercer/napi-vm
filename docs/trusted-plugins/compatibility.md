# Compatibility and evidence

The trusted process plugin system is independent of the VM's legacy plugin
host. It runs code with the launching account's OS privileges and does not
provide a sandbox. The package's declared runtime and artifact inventory must
match the machine; the host does not retry execution under another runtime.

| Host | JavaScript artifact | Native executable |
| --- | --- | --- |
| Node or Bun host | Node or Bun, when declared by the package | Rejected |
| Rust host | Node or Bun, when declared by the package | Supported for matching target and declared format |

A portable JavaScript package excludes native dependencies. An
`external-runtime` package may declare specific native dependencies and their
target, runtime and ABI; this is not a promise that arbitrary addons work. A
Bun standalone executable is target-specific and requires the Rust host.
HTTP/MCP declarations expose host-supplied connection settings; they are not a
complete MCP client implementation. Only the Rust host can opt into managed
native service processes.

One business handler per endpoint runs at a time; overlapping calls fail rather
than queue. Cancellation is cooperative, and a timeout does not undo external
effects. State replacement is non-atomic, and shutdown supervises direct
children; see [migration](migration-ts-to-rust.md) and
[architecture](architecture.md) for lifecycle details.

[Testing](testing.md) describes the local commands and configured CI scope.
Historical Linux runtime and packaging results, their source revisions, and
known limitations are retained in [evidence](evidence/README.md). They do not
certify the current checkout or establish that remote CI ran. A build or a
listed platform alone is not runtime validation on that platform.
