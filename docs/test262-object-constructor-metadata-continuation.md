# Object constructor metadata

Object now has its own length (1) and name (Object) properties, inserted before
prototype and static methods. Both descriptors are nonwritable, nonenumerable,
and configurable. Once Function.prototype exists, Object's internal prototype
is set to it. Object.prototype and its constructor back-reference retain their
existing identity, and the call/construct implementation is unchanged.

The metadata regression fails before the change; its paired behavior test
passes before and after. Tests cover property order, flags, Function.prototype
inheritance, ordinary calls, construction preserving an existing object's
identity, boxing, and subclass construction ignoring the value argument.

Both integration tests, all 369 core-library tests without exclusions, strict
workspace Clippy, and formatting pass. The release comparison passes **7,984 / 8,001** selected Object, Function, and
Reflect variants, gaining **12 passes with zero losses**. See the
[archived evidence](../tools/test262/evidence/object-constructor-metadata-2026-10-20/summary.json). The completed full snapshot `1bffdb8` excludes this continuation; its score is
102,079 / 102,956.

See [Properties of the Object Constructor](https://tc39.es/ecma262/multipage/fundamental-objects.html#sec-properties-of-the-object-constructor).

Full-run tracking: the completed `daadb9b` snapshot includes this change.
It passes 101,978 / 102,956 variants, with 753 failures and 225 timeouts.
All 155 lost passes against `1bffdb8` are measured timeouts, retained without
reclassification. See the [current checkpoint](test262-checkpoint-status.md).
