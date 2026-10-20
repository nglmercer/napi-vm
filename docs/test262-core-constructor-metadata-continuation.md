# Core constructor metadata and Array unscopables

Object, Array, String, Number, and Boolean share installation of their initial
own `length` and `name` properties. Length is 1, name matches the constructor,
and both properties are non-writable, non-enumerable, and configurable. They
are installed before additional public properties, in length/name order.
Number previously had the right values and flags but installed metadata after
its statics and in the opposite order. Object retains its existing behavior
through the shared helper; Function.prototype inheritance remains intact.

Metadata deletion or redefinition does not control calling or construction:
those operations use internal slots. Deleting own metadata exposes the inherited
Function.prototype values, and ordinary calls, primitive boxing, and Array
subclass construction retain their behavior.

Array.prototype[Symbol.unscopables] no longer owns `with`, matching the pinned
Test262 list. The `with` method remains on Array.prototype. The unscopables
object retains its null prototype, the outer property's flags, and the copy
methods' writable, enumerable, configurable true-valued entries.

Three new regression tests fail before the change and pass after it. A fourth
call/construction test passes before and after. The frozen release worker
confirms the same three failing guest scenarios and one already-passing case.
All four new tests and 103 neighboring integration tests pass, together with
all 369 core-library tests, strict workspace Clippy, and formatting.

The pinned selection covers Array, String, Number, Boolean, Object.defineProperty,
Object.getOwnPropertyDescriptor, Reflect.defineProperty, and Reflect.ownKeys.
Its baseline is **12,241 / 12,261**, with 20 failures and no other non-pass
outcomes. The final release passes **12,255 / 12,261**, gaining **14 passes with zero losses**.
There are 6 failures and 0 timeouts, with no skips, crashes, or harness errors.
All **6,117 / 6,117** Array variants pass. The remaining failures are the
script/strict cases for String split regexp escapes, startsWith arity, and
String.raw Symbol length coercion.
See the [archived evidence](../tools/test262/evidence/core-constructor-metadata-2026-10-20/summary.json).

The active full run on `daadb9b` excludes this continuation as well as the earlier
sparse-index and logical-length implementations. The latest completed full
score remains separate from these focused measurements.
