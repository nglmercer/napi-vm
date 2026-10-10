# Foundations follow-up after merged PR #24

The branch starts at merge 9c6559e21302694074ee5b4967c0dfd83d0b34dc.
Its independent full-corpus baseline is c82f617: 63,673/102,956 passes,
zero lost passes against PR #23, zero crashes/harness errors and two existing
script/strict deep-WeakMap timeouts. Merging PR #24 did not close every foundation
dependency.

## Shared key and class semantics

ToPropertyKey is a single owner-thread helper, shared by class key conversion,
object literals and descriptor/Proxy operations. It invokes ToPrimitive with
the string hint and preserves Symbol values. Class methods, accessors and fields
retain these values; property storage keeps the existing symbol identity table.
Computed names convert once in source order. Symbol method/accessor names follow
SetFunctionName's description convention. Getter descriptor lookup accepts the
observable Symbol name without replacing its stored identity.

Class prototype methods/accessors are non-enumerable. Later methods replace
previous methods or accessors. Computed constructor members remain ordinary
members. Public instance fields use the shared DefineOwnProperty operation,
including Proxy traps and rejected definitions. Anonymous function/arrow/class
field values receive inferred names; anonymous class names are set before their
static initializers without introducing a lexical class binding.

Heritage checks use the shared IsConstructor and observable Get operations.
Errors from reading prototype propagate. Prototype values must be objects or
null. Base prototypes inherit the owning realm's Object.prototype; extends-null
prototypes are null while the constructor inherits Function.prototype.

## Lexical home objects and reference evaluation

Classes and object-literal methods capture home objects in their existing lexical
environments. Super references read the home's current prototype through the
shared GetPrototypeOf operation and retain the actual receiver. Extracted methods
and arrows keep their defining home. Static fields/blocks use the class home;
direct eval inherits the appropriate super context.

Super calls capture their constructor/method before evaluating arguments.
Assignments, compound/logical assignments, updates, destructuring targets,
tagged calls and delete use shared reference semantics. Updates perform observable
numeric conversion and retain BigInt. Delete evaluates the reference and throws
ReferenceError. Getter/setter access and Proxy get/set traps share ordinary
internal operations. Prototype changes during a right-hand side do not replace
the already captured base.

Home objects and captured field keys are normal Environment values; the existing
collector traces them, breaks their cycles and preserves escaped foreign methods.
No foreign thread enters an Interpreter, and module caches, scheduler ownership,
constructor/parameter environments, execution limits and capability defaults are
preserved.

## Bytecode and validation boundary

Bytecode retains converted String/Symbol keys. Explicit super-reference,
constructor lookup, receiver get/set and numeric-update instructions call the
same semantic helpers as AST evaluation. Compiler tests require supported method
bodies to compile; verifier tests reject invalid operands; runtime assertions and
forced AST/bytecode differential fixtures cover the changed semantics. Existing
fallback remains enabled.

Source 7a71cca passed all required checks: formatting, strict Clippy, 970 workspace
tests (four existing ignored), 203 minimal-feature tests, 73 Node tests, 15 WASM
tests and ten Test262 tooling tests. Check logs and the immutable worker checksum
are archived under tools/test262/evidence/foundations-followup/7a71cca*. Its
expanded focused run completed: 34,782/42,569 passes, 1,881 new passes and 48
lost passes against the matching merged baseline, with zero crashes, timeouts or
harness errors. Function-valued computed keys exposed inconsistent conversion
in member reads and String construction. The regression gate blocked the full
run; the unsuccessful evidence is retained. Follow-up corrections and their
validation are pending. Previous measurements do not validate these changes.

## Object-literal follow-up

The uncomputed colon form `__proto__: value` now sets the literal prototype
through the shared internal operation; primitives are ignored. Computed keys,
shorthand and methods remain ordinary properties. Bytecode uses the result of
ToPropertyKey rather than the original register, and static numeric keys use
ECMAScript number formatting. Member reads and String construction use observable
string-hint conversion, resolving the function-key regression at its shared
conversion boundary. All 52 bytecode parity tests pass. Source ccedb3d passed all required checks:
971 workspace tests (four existing ignored), 203 minimal-feature tests, 73 Node
tests, 15 WASM tests, ten tooling tests, formatting and strict Clippy. Its frozen
worker completed the expanded focused corpus, including String conversion:
36,223/44,330 passes, 1,935 new passes and eight lost passes against c82f617,
with no crashes, timeouts, harness errors or skips. All 48 earlier function-key
losses recovered; the remaining losses exposed null receiver/coercion ordering
and the immutable-prototype internal operation. The failed gate is archived.
The intermediate full run was blocked while these dependencies and call-reference
ordering are addressed;
the final source still requires the full pinned corpus. The failed 496883d
workspace gate is archived: the legacy prototype fixture's debug formatter
recursed through Object.prototype's constructor cycle. The corrected fixture
asserts guest-visible prototype and property behavior.

## Call references, spread and internal-operation corrections

AST and bytecode capture ordinary/optional callees and their receivers before
arguments. Nullish calls skip argument evaluation, optional method calls retain
their receiver, and direct eval retains intrinsic identity checks. Constructor
expressions run before arguments; constructor spread follows the shared iterator
protocol. Bytecode snapshots each spread at its argument/array-element position,
retains the snapshot in ordinary traced registers and avoids iterating it twice.
Compiler, verifier and forced differential fixtures cover the new operations.

Reads reject null/undefined receivers before observable key coercion. Canonical
String/Symbol keys retain existing shared lookup diagnostics. Immutable-prototype
metadata is applied to each realm's Object.prototype; the common SetPrototypeOf
operation accepts its current prototype and rejects other prototypes. Proxy
forwarding and Object/Reflect use that same operation.

Source 05d590c passed 378 core unit tests and every required check: 975 workspace
tests (four existing ignored), 203 minimal-feature tests, 73 Node tests, 15 WASM
tests, ten tooling tests, formatting and strict Clippy. Exact check logs and the
immutable worker digest are archived under foundations-followup/05d590c*.
The expanded focused run completed: 36,671/44,796 passes, 1,995 new passes and
two lost passes against c82f617, with no crashes, timeouts, harness errors or skips.
All earlier semantic losses recovered. The two ownkeys-linear variants exhausted
fuel after the compiler introduced an unconditional default-result instruction on
ordinary calls. The unsuccessful report is retained; follow-up code emits that
instruction only for optional calls. Fuel and other limits remain unchanged.
Another frozen gate and a full pinned corpus remain required.

Source b8c9ed1 retains the call/reference semantics while avoiding the ordinary
call's extra charged instruction. All required checks pass: 975 workspace tests
(four existing ignored), 203 minimal-feature tests, 73 Node tests, 15 WASM tests,
ten tooling tests, formatting and strict Clippy. The release worker is immutable
and its digest/check logs are archived. Its expanded focused gate includes the
host/realm/GC, grammar/private-name/class, Object/Reflect/Proxy and typed-array
groups before the full pinned corpus. No limits or feature skips were changed.
The focused run passed 42,348/52,734 variants, with 1,995 new and zero lost passes
against the matching c82f617 selection. The completed full corpus passed
65,744/102,956 variants: 24,359 new and 22 lost passes against PR #23; 2,093 new
and the same 22 lost passes against c82f617. It recorded 37,210 failures, two
timeouts, zero crashes, zero harness errors and zero skips. All losses are
compound-assignment key-conversion variants outside that focused selection.
The exact full report and transitions are retained. This run fails the zero-loss
gate. Follow-up shared compound-property logic captures the canonical property
key once for both its read and write; forced AST/bytecode tests cover all eleven
compound operators. New required checks and pinned validation are required.

## Remaining work

Complete descriptor/internal-operation coverage remains open, including replacing
accessor-role recognition through callable names with typed property storage,
remaining assignment/update reference coercion and optional-chain propagation,
remaining class definition-order
restrictions, and the remaining realm/builtin dependencies. Private-write and
other unsupported bytecode bodies retain fallback. Full-corpus AST/bytecode
mismatches are not measured yet. This follow-up does not classify all remaining
Test262 failures as outside Phases 1–3 and is not a completion/merge-readiness claim.
