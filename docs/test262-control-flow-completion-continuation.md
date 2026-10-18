# Control-flow completion continuation

## Implementation

Statement lists now distinguish an empty completion from an explicit
`undefined`. A `break` or `continue` carries the last nonempty completion
through enclosing blocks. Loops retain that value when consuming the signal;
labels preserve an empty result so it can inherit the surrounding statement
list's value. `if`, `with`, `try`, and `finally` apply the required
`undefined` fallback at their completion boundaries.

The bytecode compiler tracks the same distinction with a presence register.
Before a break or continue jumps over normal result moves, it merges the
intervening statement-list and control-boundary completions into its target.
Normal empty blocks and cases leave the previous value intact. Finally copies
can replace an earlier abrupt completion. A plain break skips labeled-block
contexts and targets the enclosing loop or switch.

Switch evaluation selects a matching case before executing any body. It uses
`default` only if no selector matches, stops evaluating selectors after a
match, and preserves the completion value during fall-through and abrupt
exits. All case clauses still share their lexical environment.

The generator stress tests now enter through `run_program_body`, which performs
declaration instantiation. They previously called the raw, non-hoisting
statement sequencer and failed to create their generator declarations.

## Measurement

The release worker passes all **150 / 150** statement completion variants.
The complete switch directory passes **213 / 216**, up from 181 / 216 with
32 new passes and zero losses. Its three remaining failures require proper
tail-call execution. The combined completion-and-switch selection passes
321 / 324. The neighboring control-flow regression selection passes
**3,574 / 3,624**, up from 3,503 / 3,624 with **71 gains and zero losses**.
There are no skips, timeouts, crashes, or harness errors in these selections.

Release measurements and before/after comparisons are recorded in
[`latest.json`](../tools/test262/latest.json) and
[`evidence/control-flow-completion-2026-10-18/`](../tools/test262/evidence/control-flow-completion-2026-10-18/).
The corpus remains pinned to
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, with two workers, a five-second
per-variant timeout, and no feature skips.

The regression selection covers switch, while, do-while, for, for-in, for-of,
if, try, with, block, labeled, break, and continue statements. A separate
selection includes every statement `cptn-*.js` completion test plus the complete
switch directory. Evidence includes the measured source patch and worker hash;
these focused results do not change the last completed full-corpus score of
101,806 / 102,956 (98.8830%) on the later `d8d1652` source snapshot, which
includes this implementation and tail-call execution.

## Validation and remaining work

The completion integration tests check AST results against prepared execution,
and direct completion checks require the bytecode tier. Neighboring iterator,
async-generator, generator-stress, and Test262-worker integration suites are
also exercised: **93 integration tests pass**. Strict workspace Clippy,
formatting, and diff checks pass. The bytecode compiler, verifier, VM, and
parity selection passes **79 tests**, with
the previously failing
`super_destructuring_tags_and_delete_keep_tier_parity` test excluded.

The subsequent [tail-call continuation](test262-tail-calls-continuation.md)
implements strict tail-call execution. Its worker passes 35 / 35 tail-call
variants, all 216 / 216 switch variants, and 3,591 / 3,624 neighboring control-flow
variants. The earlier measurements above retain this completion checkpoint's
worker identity and counts. The completed full-corpus refresh uses
the later `d8d1652` worker snapshot and includes this continuation, static module
attributes, and strict tail calls. Full conformance and full-corpus
AST/bytecode differential validation remain incomplete.
