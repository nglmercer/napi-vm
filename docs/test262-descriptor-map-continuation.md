# Descriptor map continuation

Object.defineProperties and Object.create now share the runtime descriptor
operations used by Object.defineProperty. Descriptor maps are boxed, their
own string and symbol keys are snapshotted, and each key's current enumerable
descriptor is checked before its value is read. All ToPropertyDescriptor
conversions complete before the target receives the first property.
Definitions then use the target's internal DefineOwnProperty operation,
including Proxy traps and exotic-object behavior. Rejected definitions throw.
Object.create rejects a null descriptor map; an omitted or undefined map
still creates an object without additional definitions.

Three additional integration tests execute through verified bytecode and raw
AST evaluation. They cover conversion failure without target mutation, symbol
and Proxy operation ordering, primitive boxing, and nullish errors. All eleven
enumeration tests and eleven object-copy tests pass. The minimal core suite
passes 354 tests with the known sidecar native-addon test explicitly excluded.
Python tooling passes twelve tests; workspace Clippy with all targets/features
and `-D warnings`, formatting, and whitespace checks pass. Existing workspace
native-addon failures remain unresolved.

The focused Object/Reflect selection passes 6,591 / 7,108 variants, gaining
290 passes with zero losses relative to the matching projection of the
preceding object-enumeration full run. This projection is a comparison
artifact rather than an independent baseline execution.

The frozen release worker SHA-256 is
`1333a3c87c8df524a62f4f020cc0f7d6ac8558795ccf392065e32a9d7b7629d4`.
The source patch SHA-256 is
`b5f8332a26855930a8e54f48e7abb571465c88ff16f35a041986c2a4fe8bc1c1`,
based on commit `9e4f017ee76ce538d9ac076fa180ac6d12ddd104` plus working-tree
changes. The source was frozen before this checkpoint documentation.

Full-corpus verification uses the pinned Test262 revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, four workers, a five-second
timeout, no feature skips, and unchanged resource budgets. The full run passes 67,192 / 102,956 variants (65.2628%), with 35,762
failures, two existing deep-WeakMap timeouts, and no skips, crashes, or harness
errors. It gains 298 passes with zero losses against the preceding full run.
Individual outcomes, comparisons, triage, logs, patch, and artifact hashes are
archived in [the evidence summary](../tools/test262/evidence/descriptor-map-continuation/summary.json).
Subsequent Object/Math metadata changes are outside this frozen source snapshot.

Full conformance remains unfinished. Remaining descriptor-map failures include
built-in metadata/enumerability and Arguments object tagging. Broader Temporal,
RegExp, classes, environments, agents/shared memory, exotics, and bytecode parity
requirements remain open.
