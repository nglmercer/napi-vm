# Vendored rdev-node loader probe

The source in [`vendor/rdev-node`](../vendor/rdev-node) is copied from
[`nglmercer/rdev-node`](https://github.com/nglmercer/rdev-node) at the revision
recorded in `vendor/rdev-node/UPSTREAM_COMMIT`. Its MIT license is included.
The vendored source is kept separate from napi-vm so the real napi-rs addon can
be rebuilt and used as a compatibility fixture.

Build its package files and native addon into the ignored `dist/rdev-node`
directory:

```sh
bash vendor/rdev-node/build-dist.sh
```

The build needs Bun, Rust, and the platform libraries listed in the vendored
README. On Linux, use a separate X11 display to test keyboard callbacks. For
example, with `xvfb-run` installed:

```sh
xvfb-run -a env XDG_SESSION_TYPE=x11 bash -c 'cd vendor/rdev-node && bun run test'
xvfb-run -a env XDG_SESSION_TYPE=x11 RDEV_NODE_INJECT_INPUT=1 \
  cargo run --no-default-features --features node-api-host --example rdev-node-vendor -- rust
xvfb-run -a env XDG_SESSION_TYPE=x11 RDEV_NODE_INJECT_INPUT=1 \
  cargo run --no-default-features --example rdev-node-vendor -- sidecar
```

The probe loads the built `.node` file through `node:module`'s `createRequire`,
checks its exports and basic calls, starts a listener, rejects a duplicate
listener, verifies an injected `KeyB` reaches the guest callback, stops the
listener, then starts and verifies it again. The 5-second event deadline and
outer `timeout` wrapper used during local testing bound failures. Without
`RDEV_NODE_INJECT_INPUT`, the probe checks loading, calls, listener start,
duplicate rejection, restart, and stop without generating input events.
An embedding application must keep calling `Interpreter::run_event_loop_once`
while listening; the VM does not run a background JavaScript event loop.

On 2026-09-24, the Linux x64 GNU build passed all 35 upstream tests on an
isolated X display. Both napi-vm backends passed the input-enabled probe,
including event delivery after listener restart. The desktop Wayland session
could load the addon and start and stop listeners, but simulated events were
not observed there; upstream also skips that loopback test on XWayland.

The generated `index.js` package wrapper imports Node's `fs` builtin. The
standalone VM does not install an unrestricted `node:fs` module, so requiring
that wrapper currently fails with `Cannot find module 'fs'`. The probe loads
the distributable `.node` file directly through the configured, allowlisted
N-API loader. Hosts that expose a suitable `fs` capability can evaluate the
wrapper separately.
