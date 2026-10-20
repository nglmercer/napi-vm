# TypedArray constructors require new

Concrete TypedArray constructors now use the existing require-new handler for
ordinary calls and their allocation function for construction. Calling a
constructor with no arguments, a length, an object, a buffer, or another typed
array throws TypeError before inspecting or coercing any argument.

The regression covers all eleven numeric and BigInt concrete constructors,
including a proxy argument that records any premature access. It fails before
the change and passes afterward. The construction control passes before and
after, covering subclass construction, Reflect.construct with a different
newTarget, numeric wrapping, BigInt elements, and empty allocation.

Both constructor tests and both numeric-key tests pass. All 369 core tests and 83 realm tests pass, as do strict workspace Clippy
and formatting. Native evidence is in the
[archive](../tools/test262/evidence/typed-array-constructor-calls-2026-10-20/).
Release Test262 measurement for this change is pending; the earlier numeric-key
release comparison excludes this constructor-call change.

Combined optimized release verification on `7e762ff` passes **4,322 / 4,336**,
gaining **32 passes with zero losses** versus the numeric-key worker. Eight
failures and six unchanged timeouts remain; there are no other non-pass outcomes.
See the [release evidence](../tools/test262/evidence/typed-array-calls-buffer-2026-10-20/summary.json).
