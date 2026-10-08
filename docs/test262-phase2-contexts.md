# Phase 2 contextual grammar and static semantics

This is the next internal implementation phase in draft PR #24. Phase 1's
broader realm/GC audit remains open; Phases 3–4 remain pending. Phase 2 is in
progress, not complete.

## Implemented coverage

The shared static-semantics validator carries strictness, Script/Module goal,
ordinary/arrow/async/generator/async-generator function kind, parameter context,
await/yield permissions, super-call/property permissions, new.target permission,
lexical private-name scopes, class-initializer arguments restrictions, and
loop/switch/label context. Parsing separately tracks the grammar's Yield/Await
parameters so sloppy contextual identifiers remain valid.

- Ordinary functions reset async/generator and lexical super permissions.
  Arrows inherit super/new.target and private scope, while their own async kind
  controls await. Parameter defaults reject direct await/yield and permit nested
  functions with their own grammar.
- Super calls require a derived constructor or an arrow inheriting that context.
  Super properties require a method or class initializer. Bare super, super
  private access, optional super access, and new super are rejected.
- Private references and `#name in object` require a lexically visible name.
  Nested classes inherit and can shadow outer declarations. Duplicate private
  elements are rejected except one getter/setter pair with matching staticness.
  Private constructor names and deletion of private elements are rejected.
- Constructors must be unique ordinary instance methods. Constructor fields and
  accessors, async/generator constructors, and named static prototype elements
  are rejected. Accessor arity/rest restrictions apply to classes and objects.
- Block/module functions participate in lexical conflicts; sloppy block duplicate
  ordinary functions retain the Annex B exception. Imports participate in lexical
  conflicts. Duplicate exports/defaults and exports of undeclared local bindings
  fail static semantics. Catch lexical conflicts and switch-wide var/lexical
  conflicts are checked.
- Required delimiters, semicolon insertion boundaries, line terminators after
  throw/before arrows, return/yield line boundaries, async declarations, and
  postfix updates are checked. Malformed parameter tokens no longer disappear
  during recovery into an executable tree.
- Direct eval receives strictness, new.target, private-name visibility, and super
  context from the existing environments. Ordinary this environments stop super
  inheritance; private scope remains lexical. Indirect eval uses the global scope.

Class field/static-block contexts reject direct await/yield, return in static
blocks, and arguments references. Computed class names evaluate in the enclosing
grammar context. Class heritage and bodies use strict static semantics.

No runtime private identities were changed. Source-name sets are used only for
lexical validation; runtime private fields retain PR #23's symbol identities.
Bytecode class templates carry the same lexical declaration metadata for eval.
Unsupported new syntax retains AST fallback; no fallback was removed and no new
bytecode feature coverage was introduced. Two pre-existing parity cases
using invalid super syntax now assert parse errors, and the module collision test
expects SyntaxError before linking can instantiate declarations.

## Full-corpus regression repairs

The Script lexer accepts Annex B HTML comments while the Module lexer rejects
them. ECMAScript whitespace and line separators participate in ASI, including
inside comments. Leading-dot decimal literals and separated exponent digits are
recognized. Do-while uses its special semicolon insertion rule; break/continue
labels do not cross a line terminator. Private `in` observes the loop head's
NoIn grammar and rejects an unparenthesized arrow as its right operand. Nested
computed names, calls, literals and the conditional middle branch restore In.
Arrow expression bodies inherit In; arrow block bodies reset it. Expression
for-in heads retain their assignment target for static validation and AST
execution, with bytecode fallback retained.

Named function expressions create an immutable lexical self-binding. Declarations
and inferred method names do not create that scope. AST creation and bytecode
function templates use the same environment helpers; differential tests pin the
compiled tier and cover recursion, scope leakage, sloppy assignment, strict
assignment, compound assignment, and declaration reassignment.

AsyncFunction, GeneratorFunction, and AsyncGeneratorFunction constructors are
realm-owned intrinsics reached through the corresponding function prototypes,
without new global bindings. One dynamic-function parser selects the correct
function kind and validates parameter/body source independently. Syntax failures
precede access to newTarget.prototype for every dynamic function kind.

The AST represents `with`, resource declarations, resource loop heads, import
attributes (including re-exports), and source/defer import phases. Attributes
are retained and duplicate keys/non-string values are rejected. Unsupported
resource disposal and non-evaluation import execution report explicit runtime
errors; parsing does not execute or discard those constructs. Resource initializers
and loop source evaluation observe their lexical TDZ before the unsupported
execution boundary. These newer runtime features are outside the requested
Phase 1–3 foundation scope and remain unsupported.

`with` reads, writes, updates, deletion, callable receivers, lexical shadowing,
and Symbol.unscopables use an object-backed environment and the existing object
operations. Abrupt completion restores the previous scope. Captured receiver
objects are GC roots, with a dedicated retention/collection test. Strict code
rejects `with` during static semantics. Complete reference capture/order and
bytecode coverage remain follow-up work.

The explicitly selected conformance host now supplies setTimeout/clearTimeout
through the existing owner-thread job queue and real-time clock. Callback roots,
capacity, cancellation, and dispatch use that queue; native threads never enter
an interpreter. This avoids the harness's Promise-based busy-loop timer shim
exhausting the unchanged job budget. General runtime capabilities remain disabled.

## Validation

Twenty-one parser/context integration tests cover valid and invalid forms, including
direct eval and Annex B controls. The isolated worker also checks that contextual
errors precede harness execution and are classified as parse/SyntaxError.

The focused selection is the union of `async-generator-grammar`, `super`,
`private-names`, and `eval-globals`, containing 15,247 variants. Its baseline is
extracted by exact test/variant identity from the retained complete Phase 1 run.
The first focused run gained 1,442 passes and lost 18. Those losses exposed
rest-pattern, function-name, private-eval declaration, and static-block await
restrictions. The subsequent focused run gained 1,448 passes and lost zero.
The first full Phase 2 run gained 2,605 passes and lost 119 against Phase 1.
It exposed lexer, newer-syntax, function-kind, named-function-expression, and
host-timer gaps. The second full run gained 3,134 passes and lost 43 against Phase 1,
with 44,833 passes, 58,075 failures, 46 harness errors, two timeouts and zero
crashes. It exposed NoIn context boundaries, import-call restrictions, resource
declaration positions, numeric literal validation and dynamic constructor
ordering. These are exploratory measurements; validation of the subsequent
repairs is still in progress.

## Remaining Phase 2 gates

The follow-up below implements parentheses and cover metadata, Unicode and escaped
identifiers, RegExp grammar validation, legacy literal metadata and class-initializer
eval context. Parser-controlled lexical goals, remaining Annex B and contextual
positions, and the full binding/assignment-pattern audit remain completion gates.
Global declarations across separate
scripts/eval invocations require Phase 3 declaration instantiation. Function-kind
intrinsic descriptor/prototype completeness and cross-realm construction remain
part of the broader Phase 1/3 audits. Import attributes are retained for future
module-request/cache semantics; JSON modules and source/defer import execution
are not implemented. Resource disposal and its async continuation semantics are
not implemented. Ordinary for-in/of still needs lexical iteration environments;
binding metadata is now retained as described below.

Runtime private methods/accessors/static initialization and global declaration
instantiation remain Phase 3 work. Their execution failures are retained rather
than treated as parser conformance or skipped tests.

At a48a3df, the two regression selections pass 120/120 and 44/44. All required
checks pass: 748 workspace Rust tests (four existing ignored), 158 minimal Rust
tests, 73 Node tests, 14 WASM tests and nine runner/tooling tests; fmt and Clippy
are clean. Its full run remains in progress. Two further losses in the
for-in bare-initializer negative fixture motivated a subsequent parser fix.
That correction and lexical declaration statement-position coverage pass the
17-test grammar suite; their full-source checks remain pending.

Computed object methods/accessors now have distinct AST metadata. Their shared
static validation uses method context, while ordinary computed function-valued
properties retain ordinary function context. Named/computed methods and accessors
share one AST callable allocation helper. Computed methods retain bytecode
fallback. Eighteen grammar tests cover their early errors, computed accessor
execution, Symbol keys and inferred names. Method home-object semantics remain
a Phase 3 gate. The a48a3df full run also identified two covered import-call
constructor cases; direct import calls are rejected as new callees while
parenthesized import expressions remain valid. Validation is in progress.

The third exploratory full run at a48a3df passed 45,313/102,956: 3,588 new
passes and 17 lost passes versus Phase 1, with 57,595 failures, 46 harness
errors, two timeouts and zero crashes/skips. All outcomes and transitions are
retained. Its 17 losses motivated general rest-pattern comma metadata, lexical
loop-binding restrictions, labelled-function statement-position checks and the
covered-import distinction. The subsequent 754ed80 source passes 19 grammar
tests; required checks and exact regression selections are rerunning before
its full-corpus measurement. Array/object trailing commas are syntax metadata
and do not change ordinary literal evaluation or bytecode behavior.

Frozen 754ed80 full evidence: **45,459/102,956 passed, 4,052 new passes,
zero lost passes versus PR #23**. Versus Phase 1: 3,717 new passes and zero
losses. Remaining outcomes: 57,449 failures, 46 harness errors, two timeouts,
zero crashes and zero skips. Its focused run passes 6,353/15,247, with 1,543
new passes and zero losses. All required checks pass at that revision: 751
workspace Rust tests (four existing ignored), 158 minimal Rust tests, 73 Node,
14 WASM and nine tooling tests; fmt and Clippy are clean. Repository
differential fixtures show zero mismatches; full-corpus differential is not
measured. This establishes a zero-regression milestone, not Phase 2 completion.

The subsequent d8afb76 parser source records failed parameter-default parses,
checks actual pattern-bound names for duplicate parameters, rejects rest-binding
initializers, carries Annex B declaration-position metadata into static semantics,
and distinguishes braceless function declarations from StatementList declarations.
Switch cases and nested blocks reset the statement-position context. Twenty-one
grammar tests pass; required checks and Test262 validation are in progress.

At d8afb76, all required checks pass: 753 workspace Rust tests (four existing
ignored), 158 minimal Rust tests, 73 Node, 14 WASM and nine tooling tests; fmt
and Clippy are clean. The expanded statement-position/parameter-pattern selection
contains 28,799 variants and passes 14,110: 248 new passes and zero lost passes
versus its exact subset of the 754ed80 full report. It has no harness errors,
timeouts or crashes. This revision has no independent full-corpus measurement.

The subsequent catch-pattern source (09498f5) represents a catch binding as an
optional Pattern rather than a string-only parameter. Bound names participate
in duplicate, lexical and var conflict validation. Catch bindings and let/const
patterns share one initialization helper, preserving TDZ and restoring scope
when getters/defaults throw. Existing catch-identifier bytecode uses lexical
bindings through the shared declaration operation; pattern catches retain AST
fallback. Twenty-three grammar tests and one pinned-tier differential test pass.
Required checks and the catch-focused corpus are in progress.

All required checks pass at 09498f5: 756 workspace Rust tests (four existing
ignored), 158 minimal Rust tests, 73 Node, 14 WASM and nine tooling tests; fmt
and Clippy are clean. Its catch/eval selection passes 1,563/2,071: 95 gains and
zero losses versus an independently rerun d8afb76 baseline, with zero crashes,
timeouts or harness errors. Complete outcomes and transitions are retained.

The subsequent iteration-binding change retains declaration kind, full binding
pattern, optional Annex B initializer, or an assignment target in a single
`ForBinding` model. Static semantics reject lexical/body var conflicts,
duplicate lexical binding names, invalid assignment heads and prohibited
initializers. Classic const/destructuring heads require initializers. AST
execution handles identifier/member/destructuring assignment heads and closes
for-of iterators when assignment throws. Shared var-name collection includes
var loop declarations. Assignment destructuring preserves TDZ rather than
initializing an uninitialized binding. Existing compiled declaration heads keep
their bytecode path; new forms retain AST fallback. Per-iteration lexical
environments and complete iterator-based destructuring remain Phase 3 gaps.
At 5eb88fe, required checks pass: 759 workspace Rust tests (four existing
ignored), 158 minimal Rust tests, 73 Node tests, 14 WASM tests and nine tooling
tests; fmt and Clippy are clean. Its 4,837-variant exploratory selection gains
446 passes but loses four parse-negative variants. The retained report records
all losses. Source 63f16b6 repairs the for-of RHS AssignmentExpression boundary
and the bare `async of` restriction. Its focused selection passes 2,240/4,837:
450 new passes and zero lost passes, with no harness errors, timeouts or crashes.
All 25 grammar tests pass. Full-corpus validation runs against this frozen
revision; required checks for the subsequent source are in progress.

Source 35c9653 accepts sloppy `static` bindings/references and parses full `let`
expression statements, while retaining strict-mode rejection and the `let [`
ExpressionStatement lookahead restriction. Failed statement parsing in a block,
and a missing control-flow body, now records a parse error instead of silently
producing an empty statement list. Source 39c9c02 distinguishes an identifier
catch binding from an ordinary lexical binding in shared AST/bytecode binding
metadata. Sloppy direct-eval var/function redeclarations may cross an identifier
catch binding under Annex B; patterns and intervening lexical bindings still
reject the conflict. All 27 grammar tests pass; required checks are in progress.
The exploratory full run at 63f16b6 exposed the catch/eval regression and is
retained independently of these repairs.

The exploratory 63f16b6 full corpus finishes at 46,320/102,956: 4,916 gains and
three losses versus PR #23, with 56,588 failures, 46 harness errors, two
timeouts, zero crashes and zero skips. Two losses expose the simple-catch
Annex B eval rule; the third exposes var iteration patterns initialized as
lexical bindings. The subsequent 39c9c02 and 165dddf repairs distinguish catch
bindings and assign hoisted var-pattern names without creating lexical bindings.
The full run is retained with all exact transitions. Required checks and a
repaired full run remain pending.

At 165dddf, all required checks pass: fmt, Clippy with warnings denied, 761
workspace Rust tests (four existing ignored), 158 minimal Rust tests, 73 Node
tests, 14 WASM tests and nine Test262 tooling tests. The repository differential
fixtures have zero observed mismatches; direct eval and new loop binding forms
retain AST fallback. Full-corpus AST/bytecode differential is not measured.
The frozen worker is measuring the combined iteration/catch/contextual/eval
selection and the full corpus before any zero-regression claim for this source.

The repaired 165dddf focused union passes 4,131/7,528, gaining 11 variants and
losing zero against the identical 63f16b6 selection. It has no harness errors,
timeouts, crashes or skips. The four variants covering the three exploratory
full regressions pass 4/4 in a separate selection. The repaired full-corpus run
remains in progress; focused results do not establish full-source completion.

The frozen 165dddf full corpus passes **46,342/102,956**, with **4,935 new passes
and zero lost passes versus PR #23**; it gains 883 variants and loses zero
against the 754ed80 milestone. Remaining outcomes: 56,566 failures, 46 harness
errors, two timeouts, zero crashes and zero skips. Configuration is the pinned
5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 revision, four workers, 5s timeout,
fuel 1,000,000, loop budget 100,000, call depth 128 and max jobs 10,000. The
worker SHA256 is
`7b0d2d7ed6f2eb192ec72bf316405048b38758acb44ecfc0633f2a3e81169d6a`.
The complete compressed reports, exact transitions, checks and checksums are
retained in `tools/test262/evidence/foundations-phase2/165dddf-summary.json`.
This source is a zero-regression milestone; Phase 2 remains incomplete for the
known gates above. Repository differential tests observe zero mismatches;
full-corpus AST/bytecode differential remains unmeasured, and fallback stays.

## Literal and cover grammar follow-up

The follow-up preserves Unicode identifier escapes and legacy literal spellings
through static semantics. RegExp syntax validation now shares the ECMAScript
grammar validator with runtime construction while retaining the existing matcher.
Parentheses retain reference identity for AST and bytecode calls, updates, delete,
typeof and direct eval, while rejecting parenthesized destructuring targets.
Object assignment defaults use explicit cover metadata; ordinary object literals
reject cover names and duplicate prototype setters. Optional-chain assignment,
constructor and template-tag boundaries, exponentiation and coalescing restrictions
are checked before execution. Tagged invalid template escapes produce undefined
cooked entries; untagged invalid escapes and unterminated templates fail parsing.
Classic and iteration loop heads share lexical conflict validation. Class
initializer eval carries the arguments restriction across lexical environments;
ordinary functions reset it. Token end lines distinguish internal continuations
from ASI boundaries.

The expanded grammar suite passes 38 tests, with comment termination and newline
handling added to the final check run. Full required checks and a new pinned
corpus comparison are pending for this follow-up. Remaining Phase 2 completion
gates include parser-controlled RegExp lexical goals, the remaining binding and
assignment-pattern audit, declaration/contextual grammar audit, and classification
of all parse-negative corpus failures. Phase 2 is not yet declared complete.

### Focused corpus audit and repairs

The fe65c74 exploratory focused run selects 16,619 variants across literals,
RegExp, cover grammar, iteration bindings and eval/globals. It records 10,238
passes, 6,360 failures, 21 timeouts, zero crashes and zero harness errors:
1,769 gains and 14 losses against the exact selection projected from 165dddf.
All losses involve keyword accessor names or contextual await/yield shorthand.
The subsequent repair centralizes IdentifierName classification, retaining
context-sensitive binding restrictions. Immutable private-name sets are shared
across static contexts instead of being copied for every class element; this
addresses quadratic validation of newly accepted large Unicode classes.

The parse-negative audit of that selection finds 16 remaining failures. Repairs
require const initializers, reject numeric literals immediately followed by
identifier starts, restrict spread expressions to arrays/argument lists, and
share function/arrow parameter grammar with balanced-head lookahead. Arrows
now accept rest binding patterns, and object binding rest rejects trailing
commas. The expanded grammar suite passes 42 tests. Required checks and focused
corpus validation must be rerun on these repairs before the full corpus.

### Full parser classification audit

A compile-only replay uses the public compile-with-goal API and the same Script,
strict Script and Module variants as the runner. All 102,956 corpus variants have
valid metadata: 8,659 expect a parse error and 94,297 require source acceptance.
At db44aac, 113 parse-negative variants remain accepted. Subsequent repairs
lex private identifiers as contiguous tokens, distinguish private member names
from quoted public hash properties, constrain class heritage, reject constructor
fields, enforce module declaration positions, restrict export-default expression
grammar and validate resource declaration scopes. The complete negative replay
now rejects all 8,659 variants. Forty-five grammar tests and the public/private
hash-property AST/bytecode differential test pass.

This audit does not count parser acceptance as an ECMAScript execution pass.
The positive-source replay still rejects 2,775 variants, predominantly nested
aggregate binding defaults and import option grammar, plus lexical-goal and
Unicode 17 RegExp property-name gaps. These remain Phase 2 work; decorators are
a separately classified feature outside the requested foundation list. Final
required checks and full-corpus execution remain pending for these repairs.
