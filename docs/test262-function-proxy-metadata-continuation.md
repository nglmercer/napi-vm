# Function and Proxy constructor metadata

Function and its async/generator constructors install own length before name,
with the standard descriptor flags. Proxy now owns length 2 and name Proxy and
inherits Function.prototype internally, without exposing a Proxy.prototype.
This also gives Proxy the inherited native function toString behavior.

The focused debug run passes **1,622 / 1,628** variants, versus **1,612 / 1,628**
for the isolated c306dee release baseline: **10 gains, zero lost passes**.
Four failures and two timeouts remain. All 369 core tests, 89 related integration
tests, strict workspace Clippy, and formatting pass. All three new regression
tests failed before the patch and pass afterward.

These focused results do not update the full-suite score. Optimized verification
is pending. The live detached f708e0c full run excludes these changes.

Evidence: [archive](../tools/test262/evidence/function-proxy-metadata-2026-10-20/).
