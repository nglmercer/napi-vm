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

Full-run tracking update: the completed `1bffdb8` run excludes this change.
The active frozen `daadb9b` full run includes it; its final score is pending.
