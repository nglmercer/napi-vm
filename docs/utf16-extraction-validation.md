# UTF-16 migration and crate extraction validation

Validation performed on Linux x86_64, with local Node and wasm builds. This is
implementation evidence, not a stable ECMAScript or runtime compatibility claim.

| Check | Result |
| --- | --- |
| `cargo test --profile ci --workspace --all-features --locked` | 606 passed, 4 ignored |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `node --test tests/node/*.test.js` with runtime compiled | 69 passed |
| wasm32 build with `wasm,runtime`, then `node --test tests/wasm/*.test.mjs` | 13 passed |
| `npm run lint:ts` | Passed |
| Standalone core without default features | 311 passed |
| Minimal runtime | 11 passed |
| Facade without default features | 136 passed |
| Default facade | 143 passed, 1 ignored |
| Runtime CLI without bindings | Compiles |
| Test262 runner unit tests | 3 passed |

Standalone core, minimal runtime, facade without default features, and runtime
CLI without bindings are checked separately by the feature matrix. Runtime
features remain absent from the facade default and capabilities require explicit
installation. Core has no runtime transport, npm, TypeScript or N-API dependency.
Trusted native-addon backends remain optional core host integrations.

Regression cases cover AST and bytecode execution, raw surrogate source, string
coercion and iteration, lexical ordering, property assignment/enumeration, warmed
inline caches, JSON, symbol descriptions, errors, regex offsets and Unicode
quantifiers, N-API values/keys/callbacks/async rejections, and wasm host callbacks.
Host-facing diagnostic/display strings intentionally use text conversion. Module
identifiers and source-file contracts require valid Unicode and reject unpaired
surrogates; JavaScript Unicode escapes remain supported in those files.

## Existing failures

The Bun suite was compared using both the merged PR #18 native binary and the
migrated binary under the same environment: both recorded 1,356 passes and the
same 80 failing test names. Many legacy tests assume ambient runtime globals,
which PR #18 deliberately made opt-in; others exercise incomplete engine
behavior such as `Function.prototype.toString`. They remain visible.

A pinned Test262 selection at revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81` selected
`String.prototype.charCodeAt`, `String.prototype.codePointAt`,
`String.fromCharCode` and `String.fromCodePoint`: 98 of 138 variants passed,
40 failed, with no skips, crashes, timeouts or harness errors. Remaining failures
include builtin metadata and coercion/constructor semantics. This selection is
not an overall ECMAScript percentage. The historical full 31.43% baseline has
not been rerun for this branch. Web Platform Tests, Node compatibility suites
and the npm package corpus still need separate measurements.
