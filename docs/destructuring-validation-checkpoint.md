# Destructuring validation follow-up

Continues assignment, declaration, and default name inference work through `f9ab3a7`.

Array patterns mark iterator records done when calling `next`, validating its result, or reading `done` or `value` fails. These failures skip IteratorClose for ordinary elements, elisions, and rest. Failure while obtaining `next` also skips closing. Target and default initializer failures still close unfinished iterators and preserve the original exception; close failures propagate on normal early completion.

Object declaration defaults in bytecode now compare strictly with `undefined`. Previously their nullish check incorrectly evaluated default initializers for `null`. Regression coverage includes null, zero, false, and empty strings as well as missing properties.

`tests/destructuring.rs` checks prepared execution and direct AST execution for observable iterator order, rest, nested and loop patterns, default name inference, and exception handling. The array elision regression permits the deliberate AST fallback for destructuring assignments. The temporary print-only probe was replaced with assertions.

Compiler and parity tests now pin the intentional AST fallback for array binding patterns and destructuring assignments, while object declarations retain bytecode coverage. The inline cache regression reflects prepared assignment writes invalidating receiver cache hints.

Validation: `cargo test --no-default-features --test destructuring --test array_elisions` passes all 17 tests. `cargo test --no-default-features --lib` passes all 40 root library tests. `cargo test --manifest-path crates/napi-vm-core/Cargo.toml --no-default-features --lib bytecode::` passes all 80 bytecode tests. Full Test262 conformance was not measured in this follow-up.
