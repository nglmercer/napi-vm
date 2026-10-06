# Test262 phases 1–3: foundation implementation

These phases are started, not complete. The goal remains a full pinned-corpus pass with no filtered failures. Runtime capabilities remain optional.

## Phase 1: conformance harness

Implemented:
- Real `$262.detachArrayBuffer` using the engine byte store, rejecting non-buffers, shared buffers and mismatched detachment keys.
- `$262.gc` and `gc` requests deferred to a safe quiescent host boundary. Collection never runs over invisible live guest frames; this hook does not promise synchronous reclamation or finalization.
- An intrinsic registry independent of mutable global constructor bindings, traced and cleared by the cycle collector. It preserves default prototypes when guest code replaces or deletes global constructor properties.

Remaining: isolated realm/global identity and cross-realm calls, `$262.createRealm`, agents/shared-memory coordination, and more complete GC/finalization observation. The current `Realm` type shares execution state; it does not implement separate ECMAScript realms.

## Phase 2: parser and early errors

Implemented:
- A shared static-semantics validation pass for compilation, cached parsing and eval.
- Strict binding/assignment names, deletion of unqualified identifiers, duplicate parameters, non-simple parameter strict directives and direct lexical collisions.
- Return/break/continue placement, label resolution and duplicate switch defaults.
- Escaped literal accessor keys preserve function names.
- Source-aware strict directives: escaped strings cannot enable strict mode, and parentheses do not create a directive. Escaped literals still preserve UTF-16 code units and remain in the directive prologue.
- Explicit array-pattern elisions, without scratch bindings or premature rest consumption. Rest syntax rejects a trailing comma or following element; exhausted inputs no longer panic when sliced.
- `new.target` syntax, lexical-context early errors and rejection as an assignment/update target.
- Dynamic Function construction parses parameters and body separately, rejects delimiter injection, and validates their combined grammar and strict semantics.

Remaining: comprehensive declaration-instantiation and binding-conflict rules, assignment-target validation, explicit script/module parse goals, full async/generator/super/private-name grammar contexts, and remaining lexical/grammar conformance.

## Phase 3: object/function foundations

Implemented:
- `new.target` across ordinary calls, constructors, bound constructors, Reflect.construct, derived constructor calls and lexical arrows. Constructor state is private environment metadata and participates in GC tracing.
- A verified bytecode NEW_TARGET instruction and top-level direct-eval instructions. Direct eval requiring register-local visibility keeps the AST fallback until complete lexical frame materialization exists.
- Direct eval inherits lexical scope; indirect eval uses the persistent global scope and cannot inherit a caller's new.target. Replaced eval functions are invoked normally, based on intrinsic identity rather than their name.
- Global builtin data descriptors, non-enumerability (including for-in enumeration), declaration attributes, protected explicit writes, and configurable deletion without exposing an older builtin shadow. Top-level lexical bindings are excluded from global object own-property enumeration.
- Intrinsic Object/Function/Array default prototypes survive global replacement and deletion.
- Top-level script `this` resolves to the global object; numeric unary operations perform object coercion and reject inappropriate Symbol/BigInt conversions.
- Super property reads and Reflect.get preserve the explicit receiver through getters and Proxy traps.

Remaining: full global object storage with lexical/property coexistence, accessors and symbols, complete descriptor mutations, strict runtime writes and this binding, cross-realm construction, class-field initialization contexts, coercion, exotic objects and Proxy invariants.

## Validation

The selected and complete Test262 measurements use the pinned corpus `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`. Results include every selected variant and all failures, timeouts, crashes and harness errors. Subset percentages are not overall compatibility claims.

Local verification: 665 Rust workspace tests passed (four ignored), including AST/bytecode regression cases; 73 native Node tests, 14 WebAssembly tests and four Python runner/triage tests passed. Formatting and workspace Clippy with warnings denied passed. Bun recorded 1,356 passes and the same 80 failing test names as the merged baseline.

The final complete run passed **36,874 / 102,956 variants (35.82%)**, compared with 34,736 (33.74%) on the merged baseline. It recorded 65,794 failures, 2 timeouts, 2 crashes, 284 harness errors and zero skips. Both measurements used release workers, four jobs and a five-second timeout. The final worker SHA-256 is `ed9dc3ee79a84b078508c2b1966aa92f37f65e8a86d6f4cff836a887aaaadffb`; the focused report uses the same hash.

There were 2,217 newly passing variants and 79 formerly passing variants that now fail. Complete transition rows are retained in `artifacts/test262/phases-1-3-regressions.json`. The remaining crashes are both variants of String subclassing; all six baseline destructuring crashes are gone. Full outcomes, the dashboard and triage remain under `artifacts/test262/`; compact evidence is versioned in `tools/test262/latest.json`.

Rust consumers matching public syntax enums exhaustively must handle `Expr::NewTarget`, `Expr::EscapedString`, `Pattern::Elision`, and `Token::EscapedString`. `Interpreter::un_op` now requires mutable access to execute guest coercion methods.

The final focused selection passed **2,818 / 4,991 variants (56.46%)**, compared with 2,495 on the merged baseline. There were 2,173 failures and no crashes, timeouts, harness errors or skips. All 28 new.target variants passed; WeakMap passed 279/281 and WeakSet 168/170, with only realm-dependent variants failing in those collections.

| Family | Baseline passes | Current passes / total |
| --- | ---: | ---: |
| language/expressions/new.target | 4 | 28 / 28 |
| language/function-code | 242 | 216 / 281 |
| language/expressions/arrow-function | 368 | 430 / 643 |
| language/statements/break | 14 | 38 / 40 |
| language/statements/continue | 20 | 46 / 48 |
| language/statements/return | 8 | 28 / 31 |
| language/statements/let | 153 | 163 / 287 |
| language/statements/const | 124 | 134 / 271 |
| language/eval-code | 110 | 133 / 454 |
| built-ins/Function | 477 | 523 / 893 |
| built-ins/WeakMap | 277 | 279 / 281 |
| built-ins/WeakSet | 166 | 168 / 170 |
| built-ins/ArrayBuffer | 78 | 84 / 442 |
| built-ins/DataView | 454 | 548 / 1122 |

The focused comparison contains 45 formerly passing variants that now fail. Correct top-level script `this` exposes missing sloppy-function this binding; visible global descriptors expose missing eval declaration checks. Other transitions include lexical eval/arguments behavior. These remain failures in the denominator, rather than being filtered. This batch is a draft foundation change, not a phase-completion or merge-readiness claim.

## Next implementation order

1. Resolve the recorded transition failures: strict/sloppy function this binding, lexical arrows and eval declaration instantiation; introduce explicit script/module/eval goals and strict execution metadata.
2. Complete global object descriptor storage and lexical/property coexistence.
3. Isolated realm identity and realm-owned globals/intrinsics, followed by real createRealm and cross-realm constructor/default-prototype tests.
4. Expand early-error validation against the failure clusters from the complete report.
