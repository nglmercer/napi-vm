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

## Phase 1 iterator ownership follow-up

Source 5d07a46 passes all required checks: formatting, strict Clippy, 976 workspace
tests (four existing ignored), 203 minimal-feature tests, 73 Node tests, 15 WASM
tests and ten tooling tests. Its expanded independent focus includes all compound
assignment variants: 43,035/53,520 passes, 1,995 new and zero lost passes against
c82f617; zero crashes, harness errors, timeouts and skips. Exact evidence is
archived under foundations-followup/5d07a46*. Its full run was explicitly deferred
to the subsequent iterator/realm source; no full result is claimed for 5d07a46.

Each realm now installs the abstract Iterator constructor on its existing
%IteratorPrototype%, using shared constructor/newTarget prototype fallback.
Constructor and Symbol.toStringTag accessors retain their home intrinsic and
implement receiver checks and own-property creation through the common property
operations. Primitive call and direct construction reject; subclasses construct.
Symbol accessor names use symbol descriptions in trusted intrinsic installation.

Bootstrap conversion of native methods now covers array elements and named
properties through the same helper as ordinary object cells. Aliases retain
identity, escaped functions retain their realm through existing property metadata,
and array iterators allocate in the method's realm. Nullish array-iterator receivers
reject in that realm. No additional GC representation or scheduler is introduced.
Tests cover escaped methods after GC, foreign prototypes/errors, replacing globals,
subclass construction, accessor setters and AST/bytecode behavior. Full validation
of this later source is required before declaring these changes regression-free.
Iterator helper algorithms, broader descriptor dependencies and other builtin
algorithms remain explicit gaps; installing the abstract constructor does not
claim those algorithms are implemented.

## Phase 2 module request and declaration grammar follow-up

ModuleSpecifier values remain UTF-16 through parsing, compilation and linked
request records. The shared host-loader check rejects unsupported strings during
linking, without aliasing a replacement-character source or executing module bodies.
Module source caches and ownership remain intact. ModuleExportName strings retain
their well-formed Unicode early error. Named imports share one parser; `from`, `as`
and list separators are mandatory, and quoted names require local aliases in both
ordinary and default-plus-named forms. AST/bytecode consume the same helpers.
The combined core suite passes 655 tests; later frozen validation is still required.
The first required workspace check for 80afd97 found the plugin preflight loader
still expecting UTF-8 AST strings. That failure is retained. The corrected loader
decodes requests at its host-resolution boundary and rejects unsupported strings
as load errors, preserving UTF-16 syntax acceptance and avoiding path aliases.
Strict workspace Clippy now passes; the corrected source requires fresh full checks.

Source 421fd6d passed all required checks and its fresh full syntax audit. Exact
logs/digests are retained; focused/full execution was explicitly deferred to the
combined Phase 1/2 source after review found these additional parser gaps. No
execution corpus result is claimed for 421fd6d.

## Contextual default import follow-up

Manual review plus a 532-case Node syntax differential found eight valid contextual
default import bindings rejected by cac9c3a. Default imports now use the existing
BindingIdentifier parser rather than accepting only plain identifier tokens.
Tests cover the eight contextual names, default-plus-named/namespace forms,
reserved bindings and actual module evaluation. The combined core suite passes
657 tests. The reproducible syntax differential and outcome-completeness tooling
have twelve passing tests. Its results are distinct from execution tier parity.

Source cac9c3a passed all required checks: 986 workspace tests (four existing
ignored), 204 minimal-feature tests, 73 Node tests, 15 WASM tests, ten tooling
tests, formatting and strict Clippy. Its pinned source audit still has zero
accepted-invalid variants and 154 proposal/depth-limit rejections. Its full/focused
execution was deferred to the contextual grammar correction; no execution result
is claimed for cac9c3a. All logs and supplemental mismatches are archived.

## Remaining work

Complete descriptor/internal-operation coverage remains open, including replacing
accessor-role recognition through callable names with typed property storage,
remaining assignment/update reference coercion and optional-chain propagation,
remaining class definition-order
restrictions, and the remaining realm/builtin dependencies. Private-write and
other unsupported bytecode bodies retain fallback. Full-corpus AST/bytecode
mismatches are not measured yet. This follow-up does not classify all remaining
Test262 failures as outside Phases 1–3 and is not a completion/merge-readiness claim.

The contextual-import source `9753d69` failed strict Clippy because a new
grammar test used an unnecessary `format!`; its failed check log is retained
in the evidence directory. The follow-up also makes final-owner collection
available without N-API and invokes it after an agent worker's VM and host
callback roots drop. A native worker lifecycle test verifies that callback
and realm cycles leave no tracked heap cells. This does not establish full
Phase 1 completion or whole-corpus regression clearance.

Source `32a469b` passes formatting, strict workspace Clippy, 989 workspace
tests (four existing ignored), 205 minimal-feature tests, 73 Node tests,
15 WASM tests and 12 tooling tests. Its full source audit rejects all 8,659
parse-negative variants and retains the same 154 valid proposal/depth-limit
rejections, with no newly rejected valid variants. All 532 contextual-binding
cases match Node; an additional 1,144-case function/class/context probe also
finds no syntax differences. The probe script, syntax reports, required-check
logs and checksums are archived. These are syntax comparisons, not execution
or AST/bytecode parity measurements. Expanded focused and full runtime
validation remain pending for this source.

The expanded `32a469b` focus completed 59,714 variants with 46,775 passes
and 12,939 failures. Compared with the matching merged-PR-24 projection
(`c82f617`), it gains 2,228 passes and loses none. Crashes, timeouts, harness
errors and skips are all zero. Exact focused reports/checkpoints/transitions
are archived. The full pinned execution is running separately; the focus
does not establish a full-corpus result or Phase 1/2 completion.

The complete pinned `32a469b` run finishes at 66,035/102,956 passes, with
36,919 failures, two timeouts and zero crashes, harness errors or skips.
Against PR #23 it gains 24,628 passes and loses none; against merged PR #24
(`c82f617`) it gains 2,362 and loses none. Both timeouts remain the script
and strict variants of `staging/sm/regress/regress-1507322-deep-weakmap.js`.
All 8,659 parse-negative variants pass execution-phase/error classification.
Exact full reports, checkpoints, transitions, phase audit and digests are
archived. Full-corpus AST/bytecode differential remains unmeasured. This
result does not close remaining realm/internal-operation dependencies,
including the JSON replacer/Proxy realm failures; the PR stays a draft.

The next realm dependency uses one iterative JSON serializer, shared by
native calls from AST and bytecode. Replacer functions and property lists,
`toJSON`, boxed values, spacing, live property reads, descriptor enumeration,
Proxy behavior and UTF-16 keys now use shared operations. `Array.isArray`
and serialization share the existing iterative Proxy-target operation.
Explicit frames preserve the depth limit as a catchable error and share
property-list storage rather than cloning it at each nesting level.
Ten semantic tests cover callback order, root replacement, inherited
properties, proxies, cycles, exotic properties, depth limits and borrowed
foreign JSON holder/error realms after GC; strict Clippy passes. Forced
AST/bytecode fixtures exercise the same native helper without removing fallback.
The recursive draft's stack-overflow probe and lint failure are retained as
development evidence; they are corrected before runtime source freezing.
The pre-existing array-literal elision gap remains open. Required checks and
JSON-expanded focused/full Test262 must be measured on the new source;
`32a469b` full results do not validate this serializer.

Source `193998b` passes all required checks: 1,000 workspace tests (four
existing ignored), 205 minimal-feature tests, 73 Node tests, 15 WASM tests
and 12 tooling tests, with fmt and strict Clippy green. Its fresh pinned
source audit keeps all 8,659 negatives rejected and the same 154 valid
proposal/depth-limit rejections; the 532-case Node comparison matches.
The JSON-expanded focus passes 46,973/60,040 variants, gaining 80 and losing
none against the matching `32a469b` projection, with zero special outcomes.
The full pinned run passes 66,151/102,956 with 36,803 failures, two known
deep-WeakMap timeouts and zero crashes, harness errors or skips. It gains
24,744 passes with zero losses against PR #23, and 116 with zero losses
against `32a469b`. Full-corpus AST/bytecode differential remains unmeasured.
Both revoked-Proxy JSON replacer realm variants now pass. The supported
constructor diagnostic reaches index 28 (missing Intl.Collator), after the
first 28 constructors correctly invoke the revoking prototype trap. This
diagnostic does not turn the failing staging test into a pass. Shared typed
constructor static inheritance remains a separate genuine dependency.

The `193998b` execution report also verifies all 8,659 parse-negative phase/
error classifications, with zero nonpasses; this audit is archived separately.

The next Phase 1 fix moves `%TypedArray%.from` and `.of` from duplicate
concrete-constructor properties onto the common realm-owned abstract intrinsic.
Concrete constructors and shared-memory subclasses inherit one method identity
per realm, with the standard method lengths and property attributes. Existing
generic construction/mapper helpers are reused. Five focused realm tests and
forced AST/bytecode parity pass, including a SAB-backed subclass. New required
checks and focused/full execution are pending for this source.

Source `be8ebf3` passes all required checks: fmt, strict workspace/all-targets/
all-features Clippy, 1,003 workspace tests (four existing ignored), 205
minimal-feature tests, 73 Node tests, 15 WASM tests and 12 tooling tests.
The fresh source audit rejects all 8,659 parse-negative variants, retains
154 previously classified proposal/depth-limit rejections, and matches the
532-case contextual-binding Node comparison. The focus passes 47,017/60,040,
with 44 new passes and zero losses against matching `193998b` variants;
there are no crashes, timeouts, harness errors or skips in that focus.
Both shared-memory TypedArray `from_realms.js` variants now pass.

The first full run passes 66,207/102,956, with one lost pass caused by a new
five-second timeout in the script variant of `staging/sm/Proxy/ownkeys-linear.js`.
That failed regression gate is preserved. Both old and current workers pass
both variants in isolated runs; eight timing probes take 2.48–3.23 seconds.
The complete repeat, using the same immutable worker and required configuration,
passes 66,208/102,956, with 36,746 failures, two known deep-WeakMap timeouts,
and zero crashes, harness errors or skips. It gains 24,801 passes and loses
none against PR #23; it gains 57 and loses none against `193998b`.
All 8,659 parse-negative execution-phase/error classifications pass.
Original and repeat reports, diagnostic timings, transitions, source audits,
required-check logs and archive digests are retained in `be8ebf3-summary.json`.
No isolated outcome is substituted into either full report.

This remains a draft foundation implementation. Phase 1–2 completion is not
claimed: conservative GC barriers for opaque suspended continuations and
remaining realm/internal-operation dependencies still require work. Missing
RegExp symbol algorithms and iterator helpers must be distinguished from
working constructor-realm ownership. Descriptor/accessor storage, array literal
elisions, class definition ordering and general reference/optional-chain
semantics remain open dependencies. Full-corpus AST/bytecode mismatches are
unmeasured; targeted differential fixtures pass. The source syntax audit alone
does not certify every static-semantics rule or classify all remaining failures
as outside Phase 1–3.
