# TypedArray index investigation

Direct TypedArray reads and same-receiver writes now recognize canonical integer
index strings without reparsing/reformatting them as property names. Other keys,
proxies and different receivers retain the existing path. Five regressions cover
signed zero, ordinary numeric-looking names, coercion, detachment, BigInt and
proxy trap keys.

The final **isolated** comparison uses distinct worker hashes and the same
release settings (LTO disabled, one codegen unit), two workers and five-second
deadlines. All ten selected variants time out in both versions. **No additional
timeout fixes are confirmed.** The PR remains experimental. The 371 core tests,
five native integration tests, 12 runner tests, formatting and core Clippy pass.

Earlier shared-target comparisons accidentally reused the same binary for the
two variants; those comparisons are invalid and excluded. The retained reports
and manifests identify the actual final workers and their source trees.
