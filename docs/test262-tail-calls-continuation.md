# Tail-call execution continuation

## Implementation

Strict ordinary functions now transfer tail calls through a shared trampoline.
The caller's AST or bytecode activation is released before the target runs.
The transfer owns the evaluated callee, receiver, arguments, originating
realm, and module referrer, so evaluation order, method receivers, and error allocation remain
observable in their original order. Bound functions and proxy dispatch forward
through the same single-call path. `Function.prototype.call`, `apply`, and
`Reflect.apply` transfer their final internal call without growing a native
trampoline stack.

The AST evaluator propagates tail position through parentheses, the selected
conditional branch, the final comma operand, and the right operand of logical
and nullish operators. Calls and tagged templates can transfer; genuine direct
eval keeps its context behavior. An ordinary target named `eval` can transfer.
Bytecode recognizes return paths containing only register moves and
unconditional joins, including branches whose final call result flows into
`Return`. It stops at conditional tests and observable operations.

Tail position is restricted to strict ordinary function contexts. Async and
generator bodies, protected try bodies, catch bodies requiring finally,
for-of bodies, and calls after a
using declaration retain their required continuations. A using declaration
blocks tail position even when it holds null and registers no disposer.
Finally bodies can have their own tail calls. Constructor return validation
still runs after the transferred call settles. These rules follow
[ECMAScript tail-position semantics](https://tc39.es/ecma262/multipage/ecmascript-language-functions-and-classes.html#sec-tail-position-calls).

## Evidence

The pinned corpus is `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.
All Test262 files declaring `tail-call-optimization` are selected, with no
feature skips, two workers, and a five-second per-variant timeout. Reports,
comparisons, worker identity, and the implementation patch are archived under
[`tail-calls-2026-10-18`](../tools/test262/evidence/tail-calls-2026-10-18/).
The release worker passes **35 / 35** variants, up from **1 / 35**, with
**34 gains and zero losses**. The neighboring control-flow selection passes
**3,591 / 3,624**, gaining **17 passes with zero losses** over the preceding
completion worker. All **216 / 216** switch variants pass. Native call/apply,
bind, and proxy-apply regressions retain **420 / 424** passes with zero losses.
The combined import-attributes/deferred-import selection remains **170 / 170**.
There are no skips, timeouts, crashes, or harness errors in these release runs.

Strict workspace Clippy and formatting pass, as do **104 integration tests**,
**83 realm tests**, and **79 bytecode compiler/verifier/VM/parity tests**. The
known unrelated `super_destructuring_tags_and_delete_keep_tier_parity` failure
is excluded from the bytecode selection.

Regression tests cover 20,000-call mutual recursion through both tiers, mixed
AST/bytecode functions, bound and proxy calls, native call/apply forwarding,
receivers, spreads, logical/conditional paths, finally behavior, using-null
boundaries, derived-constructor validation, and retained guards for sloppy or
non-tail recursion. Neighboring generator, iterator, worker, realm, compiler,
verifier, and bytecode parity tests are also exercised.

## Full-corpus state

The latest completed full run passes 101,633 / 102,956 (98.7150%) on source
`8333a38`. It excludes later static module attributes, control-flow completion,
and this tail-call implementation. Its 169 gained passes and 24 lost passes
are recorded without altering timeout outcomes; see the
[full refresh evidence](../tools/test262/evidence/full-refresh-2026-10-18/).
Focused gains are not added to that full-run score. Full conformance remains
incomplete, including remaining loop/environment and collection iteration gaps.
