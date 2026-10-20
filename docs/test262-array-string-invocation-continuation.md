# Array string invocation and intrinsic fallback

Array.prototype.toString now boxes its receiver before reading join through
the interpreter's ordinary property path. Accessor results and callable proxies
are invoked with that same receiver and no arguments. Nullish receivers reject
before property access. Getter and proxy exceptions propagate unchanged.

When join is not callable, the implementation calls the Object toString
intrinsic directly. Replacing the Object global or deleting/replacing the public
Object.prototype.toString property does not intercept the fallback.

The shared intrinsic tag helper now forwards only IsArray and IsCallable through
proxies. Date, RegExp, Error, arguments, and boxed-primitive internal slots do
not transfer to proxies. Modern ArrayBuffer, DataView, and typed-array tags
come from Symbol.toStringTag; removing that tag produces Object as the default.
Revoked proxies retain the required IsArray error behavior.

This follows [Array.prototype.toString](https://tc39.es/ecma262/multipage/indexed-collections.html#sec-array.prototype.tostring)
and [Object.prototype.toString](https://tc39.es/ecma262/multipage/fundamental-objects.html#sec-object.prototype.tostring).

All five regressions fail before the change and pass after it. Both existing
Object constructor tests, both Object locale-string tests, and all 83 realm
semantics tests pass. All 369 core-library tests pass without exclusions, as do strict workspace
Clippy and formatting. The release passes **755 / 763** selected variants, gaining **two passes
with zero losses**. All eight remaining failures are unchanged Proxy cases.
All selected Array, Object, join, and typed-array string cases pass. See the
[archived evidence](../tools/test262/evidence/array-string-invocation-2026-10-20/summary.json). The baseline passes 753 / 763 selected variants;
the active full snapshot 1bffdb8 excludes this continuation.

Full-run tracking update: the completed `1bffdb8` run excludes this change.
The active frozen `daadb9b` full run includes it; its final score is pending.
