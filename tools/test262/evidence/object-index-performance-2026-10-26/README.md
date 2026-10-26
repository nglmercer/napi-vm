# Object index and TypedArray timing follow-up — 2026-10-26

The pinned Object, Array, TypedArray, buffer, Proxy, Reflect, arguments, and
assignment selection passes **22,058 / 22,058** variants, gaining **eight
passes and zero losses** against the latest complete-report projection. Six
are the previously timing-out TypedArray copyWithin variants; two are earlier
DataView gains. The six TypedArray variants and six standard timing variants
also pass together in a repeated two-worker, five-second run. The latest full
score remains **102,839 / 102,956** until a complete run confirms these gains.

Large ordinary objects now maintain a property-key index, invalidated on raw
slot mutation and permanently disabled if the underlying RefCell escapes via
Deref. Guarded numeric writes preserve inherited setters, proxies, descriptors,
and extensibility. Direct numeric TypedArray reads/writes retain coercion,
detachment, immutable storage, proxies, and alternate-receiver behavior.
Array.from accelerates a fresh intrinsic Array result only for a dense intrinsic
Array iterator with no mapper; custom constructors, iterators, accessors,
mutations, and holes retain the full protocol. Differential tests cover each
boundary, including retained raw slot references.

A baseline 30-second diagnostic still timed out both variants. Isolating the
10,000-property array-like factory measured approximately 4.42 seconds before
indexing and 0.10 seconds after indexing, for one constructor; diagnostics are
not corpus passes. Candidates v1-v3 remain archived: each passed checks but
all six target variants still timed out. V4 passes the broad and timing runs;
v5 additionally disables the index when raw slots escape, and repeats the broad
and timing runs with zero losses versus v4. The source and worker hashes were
revalidated after measurement.

Validation passed **423 core tests**, five worker tests, all-target core Clippy
with warnings denied, root all-target checking, formatting, whitespace, and a
locked release build. Source diffs, reports, comparisons, diagnostics, timing,
and check logs are retained.
