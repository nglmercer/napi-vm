# Trust, permissions, and resource limits

Choose an integration based on the code that will execute and the authority it
must receive. The engine restricts guest access through its language/host
boundary; it does not turn arbitrary host callbacks or native libraries into
unprivileged code.

## Trust boundaries

| Integration | What executes | What the host must control |
| --- | --- | --- |
| Bare VM | Guest JavaScript inside the interpreter | Exposed globals, callbacks, loaders, execution limits |
| Optional runtime | Guest code plus selected host services | Capability installation, grants, deadlines, and resource capacities |
| VM plugin host | Per-plugin guest VM | The selected host's manifest, grants, filesystem backend, and capability rules |
| Native addon | Native library in a Rust process or Node sidecar | Binary provenance, digest allowlist, platform/ABI compatibility, OS authority |
| Trusted process plugin | Node, Bun, or native child application | Whether the application is trusted, package integrity, services, lifecycle, OS controls |

Trusted process plugins are not sandboxed by their RPC protocol. A loopback
connection and session token authenticate a connection; they do not revoke the
child process's filesystem, network, or process privileges.

## Install capabilities explicitly

The core does not automatically install ambient filesystem or networking APIs.
Enabling a Cargo feature makes an implementation available. Builder calls or
binding methods select capabilities. Permission grants authorize particular
operations. Keep these steps separate in an application's configuration.

A manifest must be evaluated under the selected host's policy rules. The Rust
VM host intersects file/path requests with host grants. The TypeScript VM host
instead uses manifest file patterns within unconditional plugin-root confinement
and a manifest-only path-helper gate; an empty dynamic capability policy does
not deny those file/path requests. Review them or inject a restrictive filesystem
backend. See [VM plugin policy details](plugins.md#requests-grants-and-runtime-controls).
File/path checks must be performed against resolved paths; do not
replace the project's path checks with string-prefix comparisons. Native or
user-defined host callbacks remain trusted code and can bypass guest policies
if their implementation does so.

## Bound work and memory

The engine includes guards for loops, instruction fuel, call depth, queued
jobs, strings, buffers, and conversions. The optional runtime adds timer,
external-event, I/O, file-size, and elapsed-time limits. Protocol peers have
independent frame, depth, queue, call, and timeout limits.

These are different boundaries. A loop limit is not an OS CPU quota, and a
guest allocation guard is not a hard process RSS cap. A trusted host callback
can block or allocate independently. Use the CLI's supported process-isolation
limits or application-level OS containment when hard process bounds are needed.
See [CLI limits](cli.md#execution-and-resource-limits).

## Avoid accidental authority transfer

Expose small operations with validated arguments, rather than arbitrary host
objects, unrestricted `require`, or a general filesystem/process dispatcher.
Do not treat formatted output as trusted JSON. Module removal revokes future
resolution and installed bridge globals, but guest references already captured
can survive. Unloading a plugin or cancelling an evaluation cannot roll back
external effects already completed.

Native buffer exports are owned copies. Host shared-backed typed arrays are
rejected by the native marshaller. These rules protect the conversion boundary;
they do not establish safe arbitrary shared-memory concurrency for guest graphs.

## Preserve ownership and teardown

Keep guest `Rc` values on their owner, pin guest values captured by opaque Rust
callbacks, and collect cycles at quiescent boundaries. Dispose async Node
handles, stop metadata sessions, shut down native backends, and drain or stop
trusted child processes. Every application should exercise its actual shutdown
path, including errors and cancellation.

## Interpret validation narrowly

Test262 checks a specified ECMAScript corpus against a recorded engine binary.
It does not prove absence of security bugs, full Node/browser behavior, or
native-addon safety. Platform CI and boundary tests are separate evidence.
See [validation](validation.md) and [performance](performance.md) for provenance
and measurement limits.
