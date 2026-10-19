# Class property layout continuation (2026-10-18)

This continuation starts from `63a4dda`. It corrects shared class assembly so
prototype `constructor` and constructor `length`, `name`, and `prototype`
properties retain their initial insertion positions when user methods replace
configurable properties. User methods retain their own property attributes.

Computed public static method and accessor keys named `prototype` throw before
later computed keys execute. Both execution tiers enforce this boundary. Static
fields continue to evaluate their initializer before property definition fails;
symbol keys and private static `#prototype` methods remain valid.

Seven regression tests in `tests/class_property_layout.rs` require verified
bytecode execution and independently execute the AST. They cover insertion
order, computed constructor overrides, name/length attributes, key conversion
and abrupt completion, method kinds, static field timing, symbols, and private
static methods.

The release selection passes **18,986 / 19,037** variants, gaining **38 passes
with zero losses** against 18,948 baseline passes. All 96 computed-name and
2,252 object-expression variants pass. The 51 remaining failures include 38
parse failures and 13 runtime failures. There are zero skips, timeouts, crashes,
or harness errors. A frozen baseline worker from `63a4dda` and a fresh release
worker run identical selections. Reports, comparison, worker identity, and the
source patch are archived in [class layout evidence](../tools/test262/evidence/class-property-layout-2026-10-18/).
This does not replace the latest archived full-suite result (101,889 / 102,956
on `997b476`); the separate full run on `a528b02` remains active.

The current ECMAScript delete algorithm rejects super references before key
conversion. Two stale tests were corrected separately in `3705f74`, allowing
all 80 bytecode tests and all 11 class home-object tests to pass without
exclusions. See [ECMA-262 13.5.1.2](https://tc39.es/ecma262/2026/multipage/ecmascript-language-expressions.html#sec-delete-operator-runtime-semantics-evaluation).
Strict workspace Clippy and formatting pass.

A broader core-library run passed 363 tests and failed the native-addon bridge
test; the known excessive-JSON-depth stack-overflow test was excluded. The
bridge failure was not stable: the broad run reported a prototype redefinition,
while an isolated rerun passed the class section and later failed awaiting a
native promise. This remains unresolved and is not represented as a green
core-library gate.

A subsequent promise-capability host-work propagation fix passes the native
promise section and exposes a later missing `queueMicrotask` error. That
follow-up is outside the worker and implementation patch archived here.

Subsequent status: [native descriptor forwarding](test262-node-addon-descriptors-continuation.md)
resolves the Counter enumeration failure. The full native-addon fixture and
366 core-library tests now pass; the excessive-depth stack-overflow test remains
excluded. Earlier failure observations above describe the state at that stage.
