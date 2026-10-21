# Instanceof primitive operand ordering

OrdinaryHasInstance now returns false for a primitive left operand before
reading the constructor's prototype. Callable checks and bound-target delegation
still precede this check, preserving custom Symbol.hasInstance hooks. Object
operands still read and validate the prototype and reject primitive prototypes.

Three regression tests cover all primitive operand kinds, throwing prototype
getters, primitive prototypes, bound-target hooks, and object-side validation.
The primitive access test fails before the patch; both control tests pass before
and after. All **369 core tests**, **90 related integration tests**, formatting,
and strict workspace Clippy pass.

The debug instanceof and Function.prototype[Symbol.hasInstance] selection passes
**107 / 107**, versus **103 / 107** for the isolated 157ade0 release baseline:
**four gains, zero losses**. This is a focused debug measurement; optimized
verification is pending and the active f708e0c full run excludes the change.

See the [evidence archive](../tools/test262/evidence/instanceof-primitive-order-2026-10-20/).
