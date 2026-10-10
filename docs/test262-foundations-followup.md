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
expanded focused/full pinned corpus validation is running separately. Previous
measurements do not validate these changes.

## Remaining work

Complete descriptor/internal-operation coverage remains open, including replacing
accessor-role recognition through callable names with typed property storage,
ordinary/optional call reference ordering, remaining class definition-order
restrictions, and the remaining realm/builtin dependencies. Private-write and
other unsupported bytecode bodies retain fallback. Full-corpus AST/bytecode
mismatches are not measured yet. This follow-up does not classify all remaining
Test262 failures as outside Phases 1–3 and is not a completion/merge-readiness claim.
