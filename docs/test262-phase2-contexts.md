# Phase 2 contextual grammar and static semantics

Phase 2 foundation grammar and early-error coverage is implemented in merged
PR #24. Frozen source `313146b` passes the required checks and pinned corpus validation below. The broader Phase 1
realm/GC audit and Phases 3–4 remain open; this is not completion of the entire PR.

## Grammar context

The shared validator carries strictness, Script/Module goal, ordinary/arrow/
async/generator/async-generator kind, parameter context, class/constructor/
derived-constructor context, await/yield permissions, super-call/property
permissions, new.target permissions, lexical private-name scopes, class-initializer
arguments restrictions, and loop/switch/label state. Parsing tracks the grammar's
Yield/Await/In parameters independently of execution.

Ordinary functions reset lexical super and async/generator permissions. Arrows
inherit super/new.target and private-name scope while their own kind controls
await/yield. Parameter defaults reject direct await/yield but allow nested
functions with their own grammar. Class initializers and static blocks retain
arguments restrictions across arrows and direct eval; ordinary nested functions
establish their own arguments context.

Private references and `#name in object` require a visible declaration. Nested
class scopes can shadow outer names. Class declarations share immutable private
name sets during validation, avoiding quadratic copies for large Unicode tests.
Contiguous private identifiers have a dedicated token and `MemberName::Private`
AST metadata. Quoted public `"#x"` properties do not declare private names or
initialize private slots. Runtime private identity, methods/accessors and branding
completion remain Phase 3 work.

The lexer tracks statement delimiters, function/class expressions, callable
contexts and template substitutions to distinguish regular expressions from
division. Contextual await/yield identifiers, keyword property names and async
line-terminator restrictions have explicit regression coverage. Template raw and
cooked values normalize CR/CRLF according to the language; tagged invalid escapes
produce undefined cooked entries, while untagged invalid escapes fail parsing.

## Early errors and retained syntax

- Script/Module declaration positions, module-only import.meta, duplicate exports,
  unresolved local exports, quoted ModuleExportNames and import binding rules.
- Lexical/var/function/class/parameter/catch/import conflicts, including function
  body lexical declarations versus parameters and loop head versus body var names.
- Duplicate constructors, invalid constructor kinds/fields, static prototype
  elements, getter/setter arity, and duplicate private elements. One getter/setter
  pair is allowed only with matching staticness.
- Super calls/properties, bare or optional super, new.target, undeclared private
  references, private deletion, strict identifier deletion and strict bindings.
- Assignment/update targets, optional chains, rest/default/trailing-comma rules,
  CoverInitializedName, duplicate __proto__ setters, exponentiation and nullish
  operator grouping. Parenthesized nodes retain grammar boundaries while shared
  reference evaluation preserves receivers and direct eval.
- Annex B sloppy call targets are accepted for ordinary assignment/update and
  iteration heads, and evaluate the call before ReferenceError. They remain early
  errors in strict code, logical assignments, tagged targets and destructuring.
- Unicode identifier escapes and reserved-word spellings; numeric separators,
  BigInt/property names, legacy number/string restrictions and unterminated input.
  RegExp literals use the same OXC syntax validator as construction, now with
  Unicode 17 property data. The native matching algorithm is unchanged.
- Nested aggregate binding defaults, NoIn loop heads with nested +In grammar,
  contextual let heads, resource declaration placement, accessor modifiers and
  complete list separators. Retained source/deferred import and resource metadata
  never silently executes ordinary import/loop semantics.

Global declaration instantiation across previously executed sources, descriptors,
Proxy invariants, iteration/destructuring algorithms and class initialization
ordering remain Phase 3 responsibilities. Eval and dynamic function constructors
reuse static validation with their actual enclosing context; parameter and body
source are validated independently before constructor prototype access.

## Shared execution helpers

Catch and iteration binding metadata preserves simple-catch Annex B eval behavior,
parameter/body environments, TDZ initialization and hoisted var assignment. Named
function expression self-bindings are immutable. Direct eval marks functions as
needing an arguments object. `undefined` resolves bindings rather than bypassing
shadowing in either execution tier.

Dynamic import retains its options expression and evaluates source/options before
specifier conversion. The existing owner-thread module scheduler and realm-owned
cache remain in use. Option/attribute type checks and getter exceptions reject the
promise; expression evaluation can throw synchronously. Empty attributes are
supported. Nonempty attributes and non-evaluation import phases remain explicit
unsupported runtime features. AST fallback remains enabled for new constructs;
Phase 4 must add compiler/verifier/VM/differential/focused evidence before removal.

## Validation

The pinned corpus is `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.
Source `313146b` passes all required checks: fmt, strict workspace/all-target/
all-feature Clippy, 793 workspace tests (four existing ignored), 158 minimal
feature tests, 73 Node tests, 14 WASM tests and nine Test262 tooling tests.
The grammar and static-semantics suites contain 56 and 40 passing tests.
Repository forced-tier fixtures have zero observed mismatches; full-corpus
AST/bytecode differential is not measured.

The compile-only audit covers all 102,956 variants without harness execution:
all 8,659 parse-negative variants are rejected, and 94,143 of 94,297 variants
requiring acceptance compile. The 154 remaining rejections are 88 deferred import
proposal variants, 22 resource-management loop variants, 42 decorator/auto-accessor
variants and two preserved parse-depth limits. These remain visible in the audit
and execution denominator. Syntax acceptance is not a Test262 execution pass.

Reproduce the source audit after selecting this implementation source:

```sh
python3 tools/test262/audit_syntax.py /workspace/test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --output artifacts/test262/phase2-syntax.json
```

The command builds the compile-only Rust companion and retains every source
outcome, including valid sources rejected for unsupported syntax. The report
records the source revision and companion digest separately from execution.

The fresh full run passes **50,926/102,956**, with 51,982 failures, 46 harness
errors, two timeouts, zero crashes and zero skips. Exact variant comparison records
**9,519 new passes and zero losses** versus merged PR #23, and 4,584 gains with zero
losses versus `165dddf`. Both intermediate regressions in `885a8c7` are restored;
the explicit regression selection passes 2/2. All 8,659 parse-negative variants
also pass the execution runner's phase/error-type checks.

The focused group union, extracted by exact identity from this full run, passes
23,054/38,822 with 15,768 failures and zero harness errors, timeouts, crashes or
skips. This is a projection of the full run, not an independent rerun. Complete
source/worker identities, compressed outcomes, comparisons and checksums are in
[313146b-summary.json](../tools/test262/evidence/foundations-phase2/313146b-summary.json).
The two timeouts are the Script/strict variants of the existing deep-WeakMap
staging test. Broader Phase 1 GC behavior remains under audit.

This update used Rust 1.98.1. The Unicode 17 validator dependency requires Rust
1.92 or newer; runtime TypeScript tooling retains its separate OXC dependency.

Reproduce execution with four workers, 5s timeout, fuel 1,000,000, loop budget
100,000, maximum call depth 128 and maximum jobs 10,000:

```sh
cargo build --release --bin napi-vm-test262
cp target/release/napi-vm-test262 artifacts/test262/engines/phase2-worker
python3 tools/test262/run.py /workspace/test262 \
  --engine artifacts/test262/engines/phase2-worker \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --output artifacts/test262/phase2-full.json
```

Focused groups include async-generator-grammar, super, private-names, eval-globals,
parameter-patterns, catch-bindings, iteration-bindings, contextual-identifiers,
literal-grammar, regexp-grammar and cover-grammar. The full run includes every
variant from these groups. Compressed outcome reports and comparisons are retained
in [validation evidence](../tools/test262/evidence/foundations-phase2/README.md).

## Current source audit (`1005a4c`)

The complete pinned source audit was repeated after the Phase 1 follow-up. Across
102,956 variants it rejects all 8,659 parse negatives and accepts no invalid
sources. It accepts 94,143 of 94,297 variants requiring acceptance. Every source
acceptance outcome matches `313146b`; the same 154 documented proposal/limit
rejections remain visible. This closes the Phase 2 foundation grammar audit;
proposal syntax and the preserved execution/parse limits remain explicit.

The companion digest is
`fb73a2c6be3df0de8cd6a4c07e28e36f051ad2f35b4ed2483bc89468550a9201`.
Exact outcomes and checksums are in
[1005a4c-syntax-summary.json](../tools/test262/evidence/foundations-phase2/1005a4c-syntax-summary.json).
The first attempt failed to link its companion because its dependency search
path omitted Cargo's `deps` directory. That driver failure is retained separately;
no corpus outcomes were produced by it. The fixed tooling passes all ten tests.
This compile-only result does not establish runtime conformance, Phase 1 realm
completion, or full-corpus tier parity.
