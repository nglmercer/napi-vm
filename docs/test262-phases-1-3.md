# Test262 phases 1–3: foundation implementation

These phases are started, not complete. The goal remains a full pinned-corpus pass with no filtered failures. Runtime capabilities remain optional.

## Phase 1: conformance harness

Implemented:
- Real `$262.detachArrayBuffer` using the engine byte store, rejecting non-buffers, shared buffers and mismatched detachment keys.
- `$262.gc` and `gc` requests deferred to a safe quiescent host boundary. Collection never runs over invisible live guest frames; this hook does not promise synchronous reclamation or finalization.
- An intrinsic registry independent of mutable global constructor bindings, traced and cleared by the cycle collector. It preserves default prototypes when guest code replaces or deletes global constructor properties.

Continuation implemented fresh child globals/intrinsics, retained realm ownership, `$262.createRealm`, detached realm host functions and cross-realm calls. Escaped object/array/function values keep their realm after the child interpreter is dropped, and realm edges participate in GC tracing. GC requests use host state rather than a guest-visible sentinel.

Remaining: agents/shared-memory coordination, broader cross-realm exotic-object coverage, separate module caches per realm, primary interpreter global identity across independent embeddings, and more complete GC/finalization observation. The existing `Realm` type remains an agent execution-state handle; `Interpreter::create_realm` constructs fresh child globals/intrinsics on that agent.

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

Continuation implemented assignment/update-target validation, destructuring assignment-target checks, `var`/lexical conflicts and explicit `ParseGoal::{Script, Module, Auto}` with separate caches. Script eval rejects module declarations and import.meta; Module validation enforces strict semantics even without import/export declarations. Each goal cache has its own 1,024-program / 8 MiB bound.

Remaining: comprehensive declaration-instantiation and binding-conflict rules, full async/generator/super/private-name grammar contexts, and remaining lexical/grammar conformance.

## Phase 3: object/function foundations

Implemented:
- `new.target` across ordinary calls, constructors, bound constructors, Reflect.construct, derived constructor calls and lexical arrows. Constructor state is private environment metadata and participates in GC tracing.
- A verified bytecode NEW_TARGET instruction and top-level direct-eval instructions. Direct eval requiring register-local visibility keeps the AST fallback until complete lexical frame materialization exists.
- Direct eval inherits lexical scope; indirect eval uses the persistent global scope and cannot inherit a caller's new.target. Replaced eval functions are invoked normally, based on intrinsic identity rather than their name.
- Global builtin data descriptors, non-enumerability (including for-in enumeration), declaration attributes, protected explicit writes, and configurable deletion without exposing an older builtin shadow. Top-level lexical bindings are excluded from global object own-property enumeration.
- Intrinsic Object/Function/Array default prototypes survive global replacement and deletion.
- Top-level script `this` resolves to the global object; numeric unary operations perform object coercion and reject inappropriate Symbol/BigInt conversions.
- Super property reads and Reflect.get preserve the explicit receiver through getters and Proxy traps.

Continuation captures strictness on AST and bytecode functions, scopes Script/module/eval execution contexts, normalizes sloppy `this` (including primitive boxing), and retains strict receivers for ordinary, generator and async execution. Direct strict eval isolates declarations; indirect eval clears caller strictness and uses its owning realm. Strict writes reject readonly object properties, getter-only properties, nonextensible objects and false Proxy set results. Class creation no longer leaks strictness; instance getter/setter pairs work in either declaration order. Ordinary and collection constructor fallback prototypes use the new.target realm, including bound constructors. Generator next/throw/close and async continuations select their defining allocation realm on each resume; abandoned stacks restore the host allocation context.

Remaining: full global object storage with lexical/property coexistence, accessors and symbols, complete descriptor mutations, complete strict write/delete/update behavior for every exotic receiver, eval declaration instantiation, class-field initialization contexts, coercion, exotic objects and Proxy invariants. Global inherited property reads and numeric parser function aliases also preserve their identities. Private reads reject missing receiver members in both tiers; lexical private-name identity and complete branding remain unfinished. The exploratory global-storage migration is deferred; environment-backed global descriptors are retained in this batch.

## Foundation baseline validation

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

1. Complete global object storage and declaration instantiation: global lexical/property coexistence, accessor/symbol descriptors, strict writes/deletes/updates for all receiver kinds, and eval variable/lexical environment separation.
2. Complete realm coverage: realm-owned exotic instances and Reflect construction, per-realm module caches, primary global identity. The UTF-16 host evalScript path is implemented in the parameter continuation below.
3. Implement real Test262 agents with shared-memory transport and scheduler ownership; external threads must never enter an interpreter.
4. Expand contextual grammar/early errors for async, generators, super and private names, then use complete-corpus failure clusters to drive the next fixes.

Phase exit criteria remain unmet: phase 1 requires agents and full realm/GC host behavior; phase 2 requires comprehensive grammar and early-error coverage; phase 3 requires complete descriptors, declaration instantiation, exotic objects and Proxy invariants. Runtime features remain disabled by default.

Public API additions include `ParseGoal`, `compile_with_goal`, `create_realm`, `realm_global_object`, `eval_in_realm`, and `native_function_in_realm`. Exhaustive `Value` matches must handle `RealmGlobal`; direct initializers of `FunctionData` and `BytecodeFunction` must supply `strict`, and `AssignOutcome` matches must handle `ReadOnly`. Foreign realm globals are experimental at native-addon boundaries. Bytecode `Instr::GetProp` now carries a `private` flag; custom instruction producers must supply it.

## Continuation validation

The final continuation passes **38,733 / 102,956 variants (37.62%)**, up from 36,874 (35.82%) in the phase foundation batch. There are **1,859 newly passing variants and zero regressions relative to that batch**. The complete report retains 63,935 failures, two timeouts, two String subclassing crashes, 284 harness errors and zero skips. This remains far below the stable-engine compatibility target.

Compared with merged main (34,736 passes), this draft has 4,009 newly passing variants and **12 formerly passing variants that remain nonpassing**, reduced from 79 in the foundation batch. They concern Proxy getOwnPropertyDescriptors behavior, eval declaration instantiation and arguments bindings, and eval var scope in loops/switches. Their complete rows remain in `artifacts/test262/phases-1-3-continuation-regressions.json`; this is not a merge-readiness or phase-completion claim.

The final focused run passes **3,009 / 4,991 variants (60.29%)**, with 191 new passes and zero regressions relative to the phase foundation selection. There are 1,982 failures and no timeouts, crashes, harness errors or skips. WeakMap passes 281/281 and WeakSet 170/170 in that selection. All six variants covering the four intermediate continuation regressions pass after the final fixes.

Both complete and focused reports use release worker SHA-256 `0f900a18269295f2f6fee60dcd38fb931614e56eb10df2ff8d1cbb370733a6cf`, the same pinned corpus, four jobs and a five-second timeout. Generated evidence is in `artifacts/test262/phases-1-3-continuation-full.json`, `phases-1-3-continuation-focused.json`, the transition report, triage report and compatibility dashboard. Compact results, including both baseline comparisons, are versioned in `tools/test262/latest.json`.

Final verification: **679 Rust workspace tests pass** with all features (four ignored), 73 native Node tests, 14 WebAssembly tests, eight no-default-feature worker tests and four Python runner/triage tests pass. Formatting and all-target/all-feature workspace Clippy with warnings denied pass. The scoped Bun suite records 1,356 passes and the same 80 failing test names as the phase foundation baseline. Bun's existing failures remain unresolved and are not described as a passing suite.

## Eval and descriptor continuation

This continuation separates variable environments from lexical blocks for both
AST and bytecode function frames. Sloppy eval uses a fresh lexical environment
while hoisting its variables and top-level functions into the enclosing variable
environment; strict eval keeps both local. Declaration checks reject intervening
lexical conflicts and non-definable global functions before creating bindings.
Global builtin values survive bare `var` redeclarations. Class static blocks
retain independent strict variable environments and cannot leak `var` bindings
into the surrounding scope.

`Object.getOwnPropertyDescriptor` now invokes proxy descriptor traps, propagates
getter/trap errors, reads inherited descriptor fields in specification order,
completes descriptors, and checks protected target properties without mutating
them. `Object.getOwnPropertyDescriptors` omits keys whose descriptor is undefined.
Descriptor completion preserves undefined accessor fields and callable names.
Reflect delegates to the same operation. Proxy revocation and broader exotic and
symbol invariants still require implementation.

The expanded focused run covers 2,130 variants and gains 66 passes with no lost
passes against the previous complete-corpus report. Its selection differs from
the previous focused report, so their percentages are not directly comparable.
The final release-worker measurement uses the same pinned corpus
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, four workers and a five-second
per-variant timeout. Worker SHA-256:
`db902525f920314f36668e8e88c32dc4c986913a40581db779dc92eff908f4ba`.

| Result | Count |
| --- | ---: |
| All selected variants | 102,956 |
| Pass | 38,816 (37.7015%) |
| Fail | 63,852 |
| Timeout | 2 |
| Crash | 2 |
| Harness error | 284 |
| Skip | 0 |

Compared with the previous continuation report, 83 variants gained a pass and
zero previously passing variants became nonpassing. Compared with merged main,
4,081 gained a pass and one became nonpassing: the default-parameter eval case
below. This fixes 11 of the 12 previously recorded merged-main regressions.
The remaining errors and crashes stay in the denominator.

Reproduce the report with `tools/test262/run.py` and the pinned corpus, using
`--jobs 4 --timeout 5` and the release minimal worker. Complete reports and
transition rows are generated under `artifacts/test262/phases-1-3-completion-*`;
`tools/test262/latest.json` stores the compact measurement. Validation passes
684 workspace Rust tests (four ignored), 73 native Node tests, 14 WebAssembly
tests, eight minimal worker tests, four Python tests, formatting and Clippy.
The scoped Bun suite remains at 1,356 passes and the same 80 baseline failures,
with no changed failing test names.

At the measurement above, parameter defaults still shared a desugared
function-body statement list. The parameter-initialization continuation below
addresses this gap. Complete global object
storage, realm/agent coverage and contextual grammar remain open; these changes
do not complete phases 1–3.

## Parameter initialization and UTF-16 realm sources

Non-simple parameter lists now retain an explicit `ParameterInitialization`
AST boundary. Parameter expressions execute before body declaration
instantiation, using a declarative parameter environment inside the function
environment. Self and later parameter reads observe their temporal dead zones;
destructured bindings initialize in order. Implicit `arguments` bindings are
created for non-simple ordinary functions, while formal `arguments` parameters
replace that binding and arrows retain inherited `arguments` behavior. Eval
checks parameter bindings before creating variables in the function environment.

The body receives its own variable environment and copies parameter values into
body variable bindings. Closures created during defaults retain the parameter
scope. Generators initialize parameters when called, propagate initialization
errors at that point, and reuse the prepared body scope on resume. Base-class constructors with non-simple parameter lists
initialize instance fields in their defining scope before parameter defaults.
Derived-class field and `super()` ordering still require further work. Function
length stops before the first default or rest parameter.

Bytecode programs retain AST function fallback for non-simple parameter
initialization (`separate parameter environment`); the compiler must implement
these environments before removing that fallback. The regression tests exercise
both AST and compiled entry paths. Parameter execution retains guest-execution
accounting so collection cannot run over active, unpublished guest frames.

`compile_utf16_with_goal` and `eval_in_realm_utf16` preserve unpaired surrogates
while enforcing the selected grammar goal. The Test262 realm host uses the
UTF-16 source path. UTF-8 inputs retain the parse cache; rendered diagnostic
source text remains a host-facing UTF-8 view.

Public AST consumers must handle `Statement::ParameterInitialization`, and direct
`GeneratorInner` initializers must supply `parameters_initialized`. Phases 1–3
remain incomplete: global object storage, additional descriptors and Proxy
invariants, realm-owned exotic objects and independent module caches, agents,
and contextual grammar still require work.

Final release measurement on the pinned corpus
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`:

| Result | Count |
| --- | ---: |
| All variants | 102,956 |
| Pass | 40,107 (38.9555%) |
| Fail | 62,561 |
| Timeout | 2 |
| Crash | 2 |
| Harness error | 284 |
| Skip | 0 |

The worker SHA-256 is
`3b5b5515385e22da7da424e7404b61d4e47c0ccba432e27ac1e65444b06cc58f`.
The complete run gains 1,291 passes with zero lost passes against the previous
eval/descriptor continuation. Against merged main, 5,371 gain a pass and zero
lose a pass, resolving the previously recorded merged-main regressions. The
expanded focused selection passes 5,903 of 13,273 variants, gaining 629 with
zero lost passes against the previous complete report; the existing two String
subclassing crashes remain in that focused denominator. All full reports use
four workers, five-second per-variant timeouts and no feature skips.

Generated evidence is under `artifacts/test262/phases-1-3-parameters-*`, and the
compact result is in `tools/test262/latest.json`. Validation passes 691 workspace
Rust tests (four ignored), 73 native Node tests, 14 WebAssembly tests, eight
minimal worker tests, four Python tests, formatting and Clippy with warnings
denied. These results are a continuation measurement, not phase completion or
full ECMAScript conformance.

The scoped Bun suite retains 1,356 passes and the same 80 baseline failures,
with unchanged failing test names.


## Constructor and descriptor continuation

Class construction now resolves the prototype from `newTarget`, including
`Reflect.construct` with a different constructor and intrinsic fallback.
Proxy construction rejects nonconstructible targets before reading the handler,
propagates trap getter exceptions, rejects noncallable traps, accepts null or
undefined as an absent trap, and requires an object result. Descriptor conversion
rejects an invalid getter immediately, before reading the setter. Regression
tests cover compiled entry and AST execution. No public APIs change in this batch.

The complete pinned corpus passes **40,145 / 102,956 variants (38.9924%)**,
with **38 newly passing variants and zero lost passes** against the parameter
continuation. Against merged main, 5,409 variants gain a pass and zero lose one.
Remaining outcomes are 62,523 failures, 284 harness errors, two crashes and two
timeouts, with zero skips. The focused class/Reflect/Proxy selection passes
2,937 / 8,782 variants, gaining 32 with zero lost passes; both existing String
subclassing crashes remain included. This selection differs from earlier focused
runs, so their percentages are not directly comparable.

Both reports use release worker SHA-256
`892b7a95b6fc0c9086745d1e2a6096febf813d176fa46deacf7886bd0fae4617`,
the same pinned corpus, four workers and five-second timeouts. Complete reports,
transitions and triage are under `artifacts/test262/phases-1-3-constructors-*`;
compact results are versioned in `tools/test262/latest.json`.

Validation passes 694 workspace Rust tests (four ignored), 73 native Node tests,
14 WebAssembly tests, eight minimal worker tests, four Python tests, formatting
and Clippy with warnings denied. **Phases 1–3 and full conformance remain
incomplete.** The next constructor work needs a proper activation record:
uninitialized derived `this`, superclass object returns, derived fields after
successful `super()`, base fields in the defining scope for simple parameters,
DefineField rather than assignment, and lexical private brands. The existing
constructor still preallocates derived receivers; the prototype fix does not
claim to implement those rules.

The scoped Bun suite retains 1,356 passes and the same 80 baseline failures,
with unchanged failing test names and multiplicities.
