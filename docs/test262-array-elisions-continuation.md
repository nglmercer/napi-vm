# Array elisions and observable searches: incremental conformance work

Full conformance remains incomplete. This working-tree continuation preserves
array elisions as `Expr::Elision`, rather than treating them as explicit
`undefined` values. AST and bytecode array construction retain index presence;
spread values are materialized as present elements. Assignment destructuring
recognizes the same elision representation. Bytecode verification rejects
elisions in call templates and incompatible spread flags. No fallback was removed.

`indexOf`, `lastIndexOf`, and `includes` now share ordinary property operations
for generic array-like receivers, observable index conversions, inherited
properties, getters, proxies, and live element reads. The first two perform
HasProperty before Get; includes performs Get for every visited position.
Length is captured before index conversion. Native searches retain the existing
array-length bound and poll cancellation/deadlines without charging the guest
loop counter for each native position.

## Pinned focused evidence

The final release worker SHA-256 is
`4bd90cc2d2bfa0958d07e65081e0cacb1a0b81f0c76815f585dd260507b7f77a`.
The corpus revision remains
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`. Four workers, five-second timeouts,
and the standard fuel/loop/call/job budgets were retained. Nothing was skipped.

The union of `language/expressions/array`, `built-ins/Array`, and
`language/expressions/assignment/dstr` passes **5,649/6,861** variants, with
1,212 failures and no crashes, timeouts, harness errors, or skips. Against the
matching rows of the archived `be8ebf3` full repeat, it gains **468 passes** and
loses **zero**. That baseline projection passes 5,181/6,861.

The gains include 185 indexOf, 181 lastIndexOf, and 28 includes variants; the
remaining 74 cover sparse callbacks, reductions, and sorting. These are focused
measurements, not a new full-corpus percentage. At this checkpoint, the last
archived full repeat was 66,208/102,956 and this worker had not had a full-corpus
run. The subsequent [object-copy continuation](test262-object-copy-continuation.md)
includes these changes and establishes a fresh full measurement of
66,826/102,956, with 618 new passes and zero losses versus that full repeat.

Local individual outcomes and exact transitions are retained in
`artifacts/test262/array-elisions-searches-final.json` and
`artifacts/test262/array-elisions-searches-final-transitions.json`.
The two exploratory reports remain available: the first gained 78 and lost four
inherited-accessor variants; the second gained 468 and lost four sparse-search
variants to the newly introduced native loop-budget charge. Both regression
families were corrected before the final complete focused run. No isolated
outcomes were substituted into a report.

```sh
cargo build --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 \
  --path language/expressions/array --path built-ins/Array \
  --path language/expressions/assignment/dstr \
  --output artifacts/test262/array-elisions-searches-final.json
```

## Other checks and remaining failures

Six integration tests assert verified bytecode execution and compare raw AST
results for holes, spreads, inherited elements, destructuring, observable
searches, and generic receivers. All pass. Engine-boundary tests pass 4/4 and
worker tests pass 17/17. Python runner tests pass 12/12. A core minimal-feature
unit run passed 354 tests with one native-addon test explicitly filtered;
unfiltered runs exposed that test's failures, so this is not a green core gate.
Formatting, strict workspace/all-target/all-feature Clippy, and diff whitespace
checks pass.

The workspace/all-feature run stopped at the core suite with 381 passing and
five failing tests. Its complete log is
`artifacts/test262/array-elisions-workspace-tests.log`. Failures concern:

- Node sidecar class prototype redefinition, observed intermittently.
- Experimental shared-memory N-API fixture requiring missing structuredClone.
- Experimental finalizer fixture whose native host was not dropped.
- N-API v8 cleanup fixture encountering a missing file.
- N-API v7 fixture requiring missing queueMicrotask.

The untouched HEAD worktree also failed the sidecar test on redeclaring a global
`const addon`. Independent sidecar fixture scripts now use block scopes, and
guest Buffer assertions explicitly respect the existing node-api-host feature.
Those corrections do not resolve the intermittent class bridge error; no
workspace-wide passing claim is made.

Descriptor/accessor representation, remaining class/reference/realm/GC
dependencies, standard-library algorithms and proposals, and full-corpus
AST/bytecode comparison remain part of the original completion objective.
