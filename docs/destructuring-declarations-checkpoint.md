# Destructuring declaration checkpoint

Continues iterator-backed destructuring from `9818ece`.

Array binding patterns, including nested array patterns inside object/rest/default patterns, now select the interpreter path when encountered in bytecode compilation. The evaluator uses `IteratorStep` and `IteratorValue`, caches the `next` method, handles holes without reading `value`, consumes rest iterators, and closes unfinished iterators on early completion or abrupt assignment/default evaluation. This brings array declarations and loop heads onto the same semantics as array destructuring assignments.

`cargo check --no-default-features` passes. Tests were not run for this checkpoint.
