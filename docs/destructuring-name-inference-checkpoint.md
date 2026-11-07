# Destructuring initializer name inference

> **Historical record.** This is a historical focused checkpoint. Its counts describe the run recorded below, not the current engine. See the [implementation map](roadmap.md) and [development guide](development.md) for current entry points.

Continues destructuring declaration work from `f401192`.

When a default initializer directly supplies an anonymous function, arrow, or class to an identifier pattern, the evaluator now applies the binding name to the value. Bytecode compilation selects the evaluator for patterns with these defaults so declarations and assignment patterns share this behavior. Array patterns already use the evaluator for iterator semantics.

`cargo check --no-default-features` passes. Tests were not run for this checkpoint.
