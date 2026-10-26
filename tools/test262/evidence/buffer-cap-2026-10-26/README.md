# Buffer allocation and TypedArray index follow-up — 2026-10-26

The pinned buffer selection passes **4,658 / 4,664** variants: both DataView
variants gain passes, no previous passes are lost, and the same six TypedArray
copyWithin variants remain timed out relative to the latest full projection.
The standard six-variant timing slice passes. The latest complete score remains
**102,839 / 102,956** pending another complete run.

ArrayBuffer and SharedArrayBuffer backing stores now use a 64 MiB byte limit,
independent of the dense array slot limit, allowing the DataView test's 20 MiB
allocation while preserving bounded allocation. The same byte bound applies
to resizable and immutable backing stores. Integer-indexed TypedArray reads now
stop at the exotic object for all canonical numeric keys, including invalid,
detached, and out-of-bounds indices; they cannot fall through to a prototype.

Validation passed **414 core tests**, five worker tests, all-target core Clippy
with warnings denied, root all-target checking, formatting, whitespace, and a
locked release build. AST/bytecode differential tests cover the byte limit,
resizing, shared buffers, and numeric prototype lookup. Source and worker
hashes were revalidated after the broad run. The first candidate exposed two
TypedArray regressions from the earlier inherited-accessor change; its exact
source, report and comparison are retained. The second candidate repairs both.
