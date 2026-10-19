# Promise capability host work continuation

Promise.prototype.then constructs its result through a species capability.
Previously host-work propagation marked the internal reaction placeholder,
rather than the capability's actual returned promise. A top-level await of a
native promise chain could therefore report that no host event could settle
it, before the native settlement was received.

The actual promise returned by the capability now inherits the source's
pending external-work marker. The shared settlement operation clears this
marker when the promise settles. Species results that are not VM promises
remain untouched.

A regression covers then, catch, finally, a Promise subclass, and an ordinary
pending chain that must retain its lack of external work. The regression and
strict workspace Clippy pass. The native addon test now passes its previously
failing native promise chain, when it reaches that section. Its fixture also
installs scheduling helpers explicitly, as required by its queueMicrotask
callback assertions.

The full native-addon test remains failing: some runs fail while synchronizing
class snapshots with a prototype redefinition error; a later run reached native
Counter object enumeration and failed with a null/undefined receiver. Temporary
instrumentation localized the prototype failure to applyGuestSnapshots applying
an object snapshot to a function's nonconfigurable prototype. The instrumentation
was removed. These failures still require investigation; no full-core pass is
claimed. This follow-up has not been measured by a release Test262 worker.

Subsequent status: [native descriptor forwarding](test262-node-addon-descriptors-continuation.md)
resolves the Counter enumeration failure. The full native-addon fixture and
366 core-library tests now pass; the excessive-depth stack-overflow test remains
excluded. Earlier failure observations above describe the state at that stage.
