# Immutable byte modules

The engine now implements `with { type: 'bytes' }` as a synthetic module whose
only export is a Uint8Array backed by an immutable ArrayBuffer. Bytes are not
parsed as JavaScript or decoded as UTF-8. The view and buffer use realm-owned
intrinsics, so replacing the public constructors does not intercept loading.
Writes through typed-array methods, indexed assignment, or DataView reject or
return false as appropriate; the immutable buffer cannot be transferred.

Static imports, re-exports, and dynamic imports share the cached value for the
same canonical resource and normalized attributes. Different attribute sets
remain separate module identities, consistent with the existing loader policy.
Script-level import declarations now preserve attributes as well.

Hosts opt in through `ModuleLoader::load_bytes(id, attributes)`, returning
`ModuleBytes { id, bytes }`. The default implementation rejects byte loading.
Canonical identity is checked before publication. VirtualLoader supports
`insert_bytes` and can also expose the exact UTF-8 bytes of stored text.
CompositeLoader forwards binary and attribute-aware text requests. The Test262
file loader uses its existing root containment and size checks with a raw file
read. Engine buffers retain the existing maximum allocation limit of
`MAX_ARRAY_LEN * 8` bytes (2 MiB).

These are the synthetic-module semantics described by the
[import-bytes proposal](https://github.com/tc39/proposal-import-bytes).

Four dedicated byte-module tests, 33 module integration tests, 369 core-library
tests without exclusions, 20 worker tests, strict workspace Clippy, and formatting
pass. The release comparison passes **4,334 / 4,341** selected variants, gaining
**five passes with zero losses**. All **5 / 5** import-bytes variants pass.
The same two ArrayBuffer timeouts remain in both runs without reclassification.
See the [archived evidence](../tools/test262/evidence/byte-modules-2026-10-20/summary.json). The completed full snapshot
ffc10c7 excludes this continuation.

Runtime file, HTTP, package, and TypeScript adapters now implement the binary
hook. File and package reads retain permission and size checks. HTTP byte
loading reuses the same checked fetch, redirect policy, status validation, and
response limit as text loading. TypeScript wrappers forward raw byte requests
without transformation. Package candidate resolution checks bytes rather than
decoding text, allowing non-UTF-8 resources to resolve.

Validation: two binary-adapter tests (including a local HTTP server, denied
file/network access, size limits, immutable imports, and TypeScript bypass),
six package tests, and two TypeScript tests pass with all runtime features.
All-feature runtime Clippy and formatting pass. This host integration does not
change the Test262 worker score. See
[runtime adapter evidence](../tools/test262/evidence/runtime-byte-adapters-2026-10-20/summary.json).

Static deferred-import declarations now retain their attributes: parsing uses
the common attribute tail once, avoiding an empty-list overwrite. Six byte and
attribute tests and 33 module tests pass, with strict workspace Clippy and
formatting. A host-selection regression also verifies that the selected body
remains deferred until namespace access. No Test262 gain is claimed. See
[deferred attribute evidence](../tools/test262/evidence/defer-attributes-2026-10-20/summary.json).
