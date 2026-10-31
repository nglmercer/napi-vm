# Native-addon snapshot reference order

Two synchronization defects caused invalid guest object references during
native-to-guest callbacks.

Object snapshots now serialize own properties before the prototype, matching
the Node decoder's order. Previously the encoder serialized the prototype
first. If a new child was shared by a data property and the prototype graph,
the property could refer to a node Node had not decoded yet. A callback
regression creates this shared structure and verifies native strict equality
through both paths. It failed before the order correction.

Snapshot roots now use the same persistent-registry lookup as property
references. A callback may retain several older wire graphs. Previously a root
absent from its local map was skipped even when the persistent registry held
it; newly encoded children in that skipped snapshot were then missing from
later snapshots. A direct protocol regression executes the actual decoder,
node-registration, and snapshot functions with a persistent root and shared
child. It fails before the fix and passes after it. Unknown roots still raise
an error.

All **369 core-library tests pass without exclusions**, including the complete
native-addon fixture. Three additional isolated addon runs pass. Strict
workspace Clippy and formatting pass. Failing/passing regression logs, full core
output, repeat output, and the source patch are archived in
snapshot-order evidence (removed in the 2026-10-30 Test262 cleanup; see [Test262 history](test262-history.md)).
This host-only fix makes no Test262 score claim and is outside the preceding
default-export naming worker.
