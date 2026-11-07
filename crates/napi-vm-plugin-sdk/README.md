# napi-vm-plugin-sdk

Implement trusted native process plugins in Rust. The SDK provides registries,
call contexts, lifecycle, subscriptions, tracked resources, a process session,
and an in-process harness. It does not embed Node, Bun, or the JavaScript VM.

Generate a Rust contract adapter, implement its `Handler` trait, register handlers
in a `Registry`, and construct a `Plugin`. Keep the process entry separate from
business logic. The CLI's Rust scaffold supplies a complete buildable example.

`CallContext` propagates cancellation, deadlines, host-service invocation,
events, logs, and declared resources. Track background tasks/cleanup resources
before quiescing and snapshotting. Cancellation is cooperative and does not
roll back completed external effects. Lifecycle hooks cover initialization,
quiesce, snapshot, and shutdown.

Use `Harness` for contract/lifecycle tests without transport, then exercise a
packaged executable under a real Rust host. Executables need no JavaScript
runtime unless their own implementation adds that dependency. They run with the
launching account's OS privileges; protocol authentication is not a sandbox.

## Develop

Run `cargo test -p napi-vm-plugin-sdk` from the repository root. The independent
Rust scaffold includes synchronized licensed copies of this crate and its
protocol dependency; update the originals and synchronize assets instead of
editing copied files.

[Authoring guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/authoring.md) ·
[Testing guide](https://github.com/nglmercer/napi-vm/blob/main/docs/trusted-plugins/testing.md)
