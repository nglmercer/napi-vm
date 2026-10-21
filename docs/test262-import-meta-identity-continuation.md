# Per-module import.meta identity

Each module record now lazily owns its import.meta object. Repeated reads and
exported functions return the same object; different modules receive distinct
objects. The initial object has a null prototype and retains the host's url/main
properties. Guest properties, prototype changes, and integrity levels persist.
The module GC marker traces this field, preserving identity even when metadata
has no exported binding or remaining guest reference.

Dynamic Function, AsyncFunction, GeneratorFunction, and AsyncGeneratorFunction
constructors parse their generated source with the Script goal. Import.meta in
parameters or bodies therefore throws SyntaxError independently of caller module
context. Valid new.target behavior remains supported.

All three new tests fail before the patch and pass after it. All 369 core tests,
33 module tests, 22 GC tests, strict workspace Clippy, and formatting pass.
The debug worker passes **27 / 27** import.meta variants, gaining **11 passes
with zero losses** against the frozen release baseline. The neighboring four
function-constructor selections preserve all 1,013 passing variants: six failures
and two timeouts remain. Those timeouts were previously failures, not lost passes.

See the [archive](../tools/test262/evidence/import-meta-identity-2026-10-20/).
These are debug measurements, separate from optimized release and full-suite
scores. The active detached full run on f708e0c excludes this patch.
