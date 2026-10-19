# Native object descriptor continuation

Native objects returned through the Node sidecar are represented by guest
proxies. Previously these proxies exposed own keys without own-property
descriptors, so Object.keys, values, entries, object spread, and Object.assign
filtered out all native properties. Counter fixture enumeration then attempted
to index an absent first entry.

The sidecar now forwards getOwnPropertyDescriptor and uses Reflect.ownKeys.
Descriptors cross as ordinary descriptor records with native values and
accessor functions. Nonconfigurable descriptors are mirrored on the proxy's
local target before returning, preserving proxy invariant checks. Property
operations encode string or symbol keys through the ordinary wire format,
so symbol reads, writes, membership, and deletion retain symbol identity.

The native fixture tests enumeration, spread, assign, for-in, descriptor flags,
missing descriptors, repeated nonconfigurable data/accessor queries, hidden
properties, and symbol operations. Its full test passes, including earlier
class callback, native promise, and threadsafe callback checks.

Validation: 366 core-library tests pass with the known
from_json_rejects_excessive_depth stack-overflow test excluded. Strict workspace
Clippy and formatting pass. The full native-addon test also passes in isolation.
No release Test262 gain is claimed for these host-bridge changes.
