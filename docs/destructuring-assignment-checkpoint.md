# Destructuring assignment checkpoint

Continues plain assignment work from `e2a07e1`.

Array destructuring now gets the source iterator and captures `next` once. Each assignment target is prepared before its iterator step; the result object's `done` property is read before its `value`, and elisions avoid reading `value`. Rest consumes the remaining iterator. Early completion closes an unfinished iterator; abrupt target/default failures close it while preserving the original error. Iterator acquisition and step/value failures skip closing (see [validation follow-up](destructuring-validation-checkpoint.md)).

Object destructuring assignment prepares member and binding references before reading the source property. Shorthand names use the same captured binding path. Default initializers now run only for `undefined`, including declaration patterns.

Assignment destructuring currently falls back from bytecode to the shared AST evaluator to preserve observable ordering. Declarations still use their existing bytecode path. `cargo check --no-default-features` passes. Tests were not run for this checkpoint.
