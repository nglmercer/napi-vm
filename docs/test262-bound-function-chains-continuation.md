# Bound function target chains

Bind retains its immediate target, receiver, and argument segment. Construction
replaces newTarget at each matching bound function instead of flattening away
intermediate identities. Calls and construction resolve chains iteratively and
combine arguments once in inner-to-outer order. The first bound receiver wins.
ShadowRealm wrapped-function dispatch remains a separate boundary.

Instanceof follows bound targets iteratively while observing each target's
Symbol.hasInstance method, including custom hooks. Intrinsic method recognition
supports both native values and native function objects. Bound functions install
length before name with standard descriptor flags.

Four new integration tests cover new.target, foreign newTarget, receiver and
argument order, metadata order, intermediate hooks, and 2,000-level call,
construction, and instanceof chains. Six worker probes include three failures
before the patch and six passes afterward. All **369 core tests**, **90 related
integration tests**, strict workspace Clippy, and formatting pass.

The debug Function/Proxy/instanceof/ShadowRealm selection passes **1,829 / 1,837**:
six failures and two timeouts. Compared with the isolated 157ade0 release worker,
the nested-newTarget variants gain **two passes** and the two built-in-function
source-string variants move from pass to timeout. There are no pass-to-fail
transitions. These timings require optimized verification; this is not a full
score update. The live detached f708e0c full run excludes this change.

Remaining semantic failures are computed-method source strings and instanceof
primitive-left-hand-side ordering. Evidence: [archive](../tools/test262/evidence/bound-function-chains-2026-10-20/).

The isolated optimized worker at a68b1a1 passes **1,831 / 1,837**, with six
failures and zero timeouts: **two gains, zero lost passes** against the same
baseline. Both debug timeout variants pass in release. The checkout was checked
clean before and after the build; its manifest and report are archived above.
