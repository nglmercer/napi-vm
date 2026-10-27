# Full-run regression repairs — 2026-10-26

The pinned affected-area selection passes **17,680 / 17,680** variants,
recovering all four losses from the latest complete report with zero new
losses. Both strict TypedArray prototype-chain assignments and both Proxy
ownKeys timing variants pass. The two Proxy cases, six previously slow
TypedArray cases, and the standard six-case timing slice pass together in a
repeated two-worker, five-second run. The latest complete score remains
**102,906 / 102,956** until another complete run confirms these focused gains.

The numeric object-write fast path now checks non-extensibility only after
checking the prototype chain, allowing an invalid TypedArray prototype index
to return success without creating a receiver property or coercing the value.
A verified-new ordinary data property is appended while maintaining the object
index rather than invalidating it and rebuilding it for every definition.
Existing descriptors and accessor conversions retain the general path.

Validation passed **423 core tests**, five worker tests, all-target core Clippy
with warnings denied, root all-target checking, formatting, whitespace, and a
locked release build. A differential regression covers non-extensible receivers
with TypedArray prototypes. Exact source diff, worker identity, checks, reports,
projection, comparison, and timing are archived; source and worker hashes were
revalidated after the broad run.
