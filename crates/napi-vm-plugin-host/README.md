# napi-vm-plugin-host

Launch and manage trusted JavaScript or native executable plugins from a Rust
application. The host owns process startup, authentication, exact contract
agreement, invocation, events, reload/state transfer, logs, and shutdown.
It is independent of the JavaScript VM.

`Host` registers host services and loads a manifest with `LoadOptions`.
`PluginHandle` exposes invocation, metadata, status/history, events, logs, and
shutdown. JavaScript entries require a declared supported Node/Bun runtime;
native entries must match the actual target and executable requirements.
Normal load does not install dependencies or compile the plugin.

Always shut down the host on error and success paths. Reload drains/quiesces,
validates state, and returns a replacement handle. Deadlines and cancellation
are cooperative; a failed call or process exit cannot undo external effects.
Plugin processes execute with their OS account's authority.

Package inventory verification checks bytes relative to a supplied lock; it is
not publisher authentication. Use trusted artifact delivery and OS containment
as required by the application.

## Develop

Run `cargo test -p napi-vm-plugin-host` at the repository root. The
`trusted-host-rust` example package provides command-line invocation and a
JSONL development adapter. Cross-language and relocated-package checks belong
to the full plugin gate.

[Hosting guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/hosting.md) ·
[Packaging guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/packaging.md)
