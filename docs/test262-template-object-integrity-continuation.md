# Tagged template object integrity

Fresh tagged-template cooked and raw arrays are frozen through shared intrinsic
array metadata operations. The cooked array's own raw property is non-writable,
non-enumerable, and non-configurable. Freezing does not call mutable guest
Object.freeze or other public built-ins.

AST evaluation and prepared bytecode share the same helper. The compiler emits
a FreezeTemplate instruction whose operands are register-checked by the verifier;
the VM attaches raw and freezes both arrays directly. Both execution tiers are
exercised explicitly by the regression tests.

The frozen baseline fails both guest probes: array integrity and raw property
flags. Both new tests pass in AST and bytecode execution. All 369 core tests, strict
workspace Clippy, and formatting pass. See the
[native evidence](../tools/test262/evidence/template-object-integrity-2026-10-20/). This change
does not implement template-object identity caching; same-site reuse across
function calls and realms remains an open requirement. Release conformance
measurement is pending.
