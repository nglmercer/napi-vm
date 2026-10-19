# Native-addon snapshot continuation

Repeated synchronous guest callbacks retain several bridge graphs. The same
guest reference can have different local mirror objects across those graphs.
Reapplying an immutable data descriptor with another local mirror throws even
though the guest slot cannot change. Snapshot application now preserves the
original mirror reference for an existing nonconfigurable, nonwritable reference
slot with matching enumerability. Writable and configurable slots still refresh;
invalid immutable primitive changes still throw. The referenced objects retain
their own snapshots for content updates.

Callback results are now decoded before snapshots are applied. This registers
new returned graph objects before snapshots can reference them. Applying snapshots
first could report an invalid guest object reference.

A Node regression executes the actual embedded snapshot function and checks
immutable reference identity, writable updates, and immutable primitive rejection.
The native addon fixture also constructs and reads a guest class eight additional
times to exercise graphs retained across calls. These class checks and the native
promise checks pass; the full addon fixture still fails later in Counter object
enumeration with a null/undefined receiver error. No full-addon or full-core
pass is claimed. Strict workspace Clippy and formatting pass. No ECMAScript
Test262 score changes are claimed for this host-bridge-only continuation.

The broader duplicate mirror identity behavior remains a bridge limitation;
this change preserves immutable slots during synchronization and does not claim
to unify all copies across independent calls.
