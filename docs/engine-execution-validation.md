# Engine execution and collection changes

Module linking now discovers dependencies and creates live bindings before executing bodies. Missing and ambiguous exports fail during resolution; cyclic reads enforce the temporal dead zone. Module evaluation caches success and the original thrown value. Dynamic imports run as jobs, and concurrent importers share evaluation. Module namespaces use sorted UTF-16 export names, a null prototype, and reject mutation. Imported bindings cannot overwrite local declarations. Module functions retain their defining module as the resolution context.

`Interpreter::link_module` links without executing bodies. `Interpreter::import_module` returns a Promise; pending module evaluation progresses through the existing event loop. Native stackful execution can suspend dependency evaluation at top-level await and resume after its Promise settles.

Bytecode has a verified Await instruction. Supported async bodies execute bytecode, with the AST evaluator retained for unsupported constructs. `PreparedProgram::fallback_reason()` explains a compiler decline. Thenable assimilation and implicit async/generator return values are corrected.

WeakMap and WeakSet store weak keys. Collection traces ephemerons to a fixed point. WeakRef keeps dereferenced targets alive until the job checkpoint. FinalizationRegistry retains holdings and queues cleanup outside collection, supports unregister tokens, and does not retain dead targets. The collector traces current shared module maps and refuses collection when reachable cells are mutably borrowed by the host.

## Validation

- Rust workspace, all features: 635 passed, 4 ignored.
- Native Node binding: 72 passed, including pending dependency evaluation, early import rejection, and finalization.
- WebAssembly binding: 14 passed, including thenable await and namespace mutation rejection.
- Test262 runner unit tests: 3 passed.
- Broad Bun suite: 1,356 passed and 80 failed; failing test names match the preceding baseline after updating missing-export expectations.
- Rust formatting and all-target, all-feature Clippy checks pass.
- Focused Test262 revision `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`: 158 passed and 133 failed out of 291 selected variants, with zero skips, crashes, timeouts, or harness errors. Selection: `language/module-code/ambiguous-export-bindings` and `built-ins/WeakMap`. This selection is not an overall ECMAScript compatibility score.

## Remaining gaps

Collection still pauses while stackful async tasks or generators are suspended: opaque Rust stacks do not provide traceable roots. Pending top-level await on platforms without stackful execution remains unsupported; settled Promises and thenables are tested on WebAssembly.

The AST fallback remains necessary. Weak collection constructor behavior, descriptors, iterator protocol handling, and subclass behavior need further Test262 work. WeakRef and FinalizationRegistry constructor metadata and new-target semantics are incomplete. Module namespace exotic behavior, including Symbol.toStringTag, is incomplete. The full Test262 corpus has not been rerun and no stable-engine compatibility claim is made.

The broad Bun suite retains failures from the preceding implementation, including tests that assume ambient runtime globals despite the capability-based runtime design. These are tracked separately from the passing native and WebAssembly regression suites.
