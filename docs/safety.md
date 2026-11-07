# Safety and operational limits

## Guest isolation and host trust

A bare engine does not expose Node `require`, the filesystem, network or
process globals. The host explicitly installs capabilities and is responsible
for permission checks and validation at every privileged operation.

The in-process VM is not an operating-system security boundary. Synchronous
guest code blocks its calling thread, host callbacks can block outside guest
loop accounting, and per-allocation caps are not a total heap quota. Use a
supervised child process with external CPU, memory and wall-time limits when
those guarantees are required.

Native addons and trusted process plugins execute with host/account privileges.
An addon allowlist chooses code that may load; it does not sandbox that code.
[Native addons](native-addons.md) and [trusted plugins](trusted-plugins/README.md)
have separate trust and shutdown contracts.

## Engine guards

| Area | Guard and boundary |
| --- | --- |
| Guest recursion | Call-depth limit; failures become guest errors |
| Parsing | Nesting/depth limits |
| Loops and execution | Loop/fuel budgets; cancellation and deadline mechanisms at execution checkpoints |
| Arrays | Bounded materialized storage; large logical lengths may remain sparse |
| Strings | Bounded construction paths, measured in UTF-16 units |
| Cycles | Cycle-aware formatting and bounded JSON paths |
| Teardown | Iterative value dropping and quiescent cycle collection |
| Generators | Same-thread stackful coroutine lifecycle and reentry checks |

The constants and exact checks live in
[`value.rs`](../crates/napi-vm-core/src/value.rs) and
[`interpreter`](../crates/napi-vm-core/src/interpreter/mod.rs). A JavaScript
array's logical length is not the same as its allocated storage. Do not
infer an aggregate memory quota from either an array or string cap.

Run `bun examples/crash.ts` for the executable safety catalogue and use the
targeted tests listed in [development](development.md) after changing guards.
A successful old run does not certify a new code change or host integration.

## Capability hosts

The legacy plugin host resolves paths against granted roots and patterns;
its manifest requests capabilities rather than granting them. The Node file
backend also bounds reads/writes and uses descriptor-based checks plus
`O_NOFOLLOW` where supported. Parent-directory replacement races remain a
boundary of that Node filesystem implementation. Review [the plugin guide](plugins.md)
and its backend code for platform-specific behavior.

Keep privileged callbacks small, bounded and explicit. Check permissions on
each operation so a previously retained guest reference cannot bypass a
revoked policy. Separate guest execution, native-addon lifecycle, and trusted
child-process supervision in operational monitoring.

[Runtime ownership and collection](runtime-improvements.md) ·
[Event-loop scheduling](event-loop.md)
