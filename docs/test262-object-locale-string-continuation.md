# Object locale-string accessor invocation

Object.prototype.toLocaleString now performs an ordinary property read of
`toString` before invoking it. The previous raw lookup returned accessor
functions instead of executing them. Getters now see the original primitive
or object receiver, and their results supply the method to call with no
arguments. Getter and proxy exceptions propagate without replacement.

This follows [Object.prototype.toLocaleString](https://tc39.es/ecma262/multipage/fundamental-objects.html#sec-object.prototype.tolocalestring).

Both new regressions fail before the change and pass after it. They cover
Boolean, Number, String, Symbol, and BigInt primitive receivers, getter and
method this identity, exactly one getter invocation, zero forwarded arguments,
and accessor/proxy error identity. All 369 core-library tests, strict workspace
Clippy, and formatting pass without exclusions. The release passes **122 / 122** selected locale-string variants, gaining
**two passes with zero losses**: Object and Array primitive-receiver getter
cases. The baseline passes 120 / 122. See the
[archived evidence](../tools/test262/evidence/object-locale-string-2026-10-20/summary.json). The completed full snapshot `1bffdb8` excludes this change.

Full-run tracking: the completed `daadb9b` snapshot includes this change.
It passes 101,978 / 102,956 variants, with 753 failures and 225 timeouts.
All 155 lost passes against `1bffdb8` are measured timeouts, retained without
reclassification. See the [current checkpoint](test262-checkpoint-status.md).
