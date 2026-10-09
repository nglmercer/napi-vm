# Realm and buffer continuation evidence

Phases 1–3 remain incomplete and PR #24 remains draft. Each report is tied to
its exact source commit and frozen worker digest; focused and exploratory
results are not completion evidence.

The `e193e23` async-generator continuation passes 4,192/5,186 variants in the
agents/realms/gc-weak/generators/async-generators union. It gains 1,084 passes and
loses zero against the matching selection projected from the `7c0a9ce` full
report, with zero harness errors, timeouts, crashes, and skips. The projection is
not an independent baseline rerun. `e193e23-summary.json` ties the worker digest,
reports, transitions, and all required passing check logs to the exact source.
No full-corpus result or phase-completion claim is made for this continuation.

The 2,030 variants have 1,640 passes and 390 failures, with no harness errors,
timeouts, crashes, or skips. Against the identical `313146b` selection, the
comparison has 139 new passes and zero lost passes. Source configuration, worker
hash, required-check logs, and archive digests are in `f078fcb-summary.json`.
All required checks pass: 805 workspace tests (four existing ignored), 158 minimal
tests, Node, WASM, formatting, strict Clippy, and nine runner/tooling tests.
Two new verified-bytecode/AST buffer-growth fixtures agree. Full-corpus tier
mismatch measurement remains pending.

The earlier `df3d691` exploratory run is retained: 1,520 passes, 510 failures,
23 new passes and four lost passes. The losses were constructor post-return
errors allocated in the callee realm; `f078fcb` corrects them to the caller realm.
No exploratory outcomes were removed or edited.

Reproduce the focused comparison from the repository root:

```sh
python3 tools/test262/compare.py \
  tools/test262/evidence/foundations-realms-memory/313146b-baseline-focused.json.gz \
  tools/test262/evidence/foundations-realms-memory/f078fcb-focused.json.gz \
  --output artifacts/test262/reproduced-realm-memory-transitions.json
```

The later `b5ae20f` focused run passes 1,654/2,030 variants, with 153 new passes
and no lost passes against the matching Phase 2 selection.

Two subsequent full runs are retained as exploratory results. `388bd73` passes
52,527 variants but loses 310 previously passing Phase 2 variants. `2c92d3b`
restores those losses and passes 53,019 variants, but introduces 25 other losses.
Their aggregate improvements do not satisfy the zero-regression gate. Both
reports, exact transitions and required-check logs are archived here.

`7c0a9ce` restores all 335 variants lost across those exploratory runs: its
regression selection has 335 passes, no failures, harness errors, timeouts,
crashes or skips. All required checks pass for that source revision (815 workspace
tests, four existing ignored; 158 minimal tests; Node, WASM and runner tests).

Its full pinned run passes **53,046/102,956** variants: **11,639 new passes and
zero lost passes** versus the exact PR #23 baseline of 41,407 passes. Against
Phase 2 `313146b`, it has 2,120 new passes and zero losses. The remaining outcomes
are 49,908 failures, two timeouts, zero harness errors, zero crashes and zero
skips. Both timeouts are the script/strict variants of the deep-WeakMap staging
regression. The worker digest and required limits are recorded in
`7c0a9ce-summary.json`. Full-corpus AST/bytecode mismatch measurement remains
pending; no phase-completion claim follows from these results.

Reproduce the full comparison:

```sh
python3 tools/test262/compare.py \
  tools/test262/evidence/foundations-phase1/baseline-final-full.json.gz \
  tools/test262/evidence/foundations-realms-memory/7c0a9ce-full.json.gz \
  --output artifacts/test262/reproduced-realm-full-transitions.json
```

The later `bbca33c` GC/host-coercion snapshot explicitly traces completed async
tasks' result promises and performs guest ToString on the owner thread before
queue locking or source/report transfer. All required checks pass: 819 workspace
tests (four existing ignored), 161 minimal tests, 73 Node tests, 14 WASM tests,
formatting and strict Clippy. Its focused union passes 1,712/2,030 variants, with
318 failures and no special outcomes: zero losses versus the matching `7c0a9ce`
full-run projection, and 211 new passes with zero losses versus Phase 2.
This is focused evidence; the `7c0a9ce` full result must not be attributed to it.

The initial `bbca33c` worker copy lacked executable permissions. The archived
driver-error report is not valid variant evidence and cannot be compared against
the focused denominator. After correcting permissions, the focused run above
completed normally. `c94ede0` adds preflight validation so future invalid worker
setups fail before corpus outcomes are created; all ten tooling tests pass.
The source/check/archive details are in `bbca33c-summary.json`.

`7c0a9ce-triage.json` retains every outcome by path and engine phase. Non-passing
phases are runtime (49,725), parse (154), resolution (29), and timeout (2).
These buckets do not claim all remaining failures are outside Phase 1–3.

The `372c20b-*` follow-up records all required checks passing (872 workspace,
203 minimal, 73 Node, 14 WASM, ten tooling tests; four existing workspace ignored).
Its exploratory 2,044-variant agent/realm/GC/regression selection has 1,852 passes,
192 failures, 130 new passes, four lost passes, and zero special outcomes versus
the explicitly labeled matching `7c0a9ce` full-report projection. The full run
was blocked by the regression gate. The four losses expose missing metadata on
Promise resolving functions after their constructibility fix; follow-up changes
must be separately revalidated. These files do not establish Phase 1 completion.

### Source 8314510: expanded Array species follow-up

The immutable worker passed 2,637/2,945 variants across agents, realms, GC/WeakRef,
all map/filter tests, and the 14 earlier regression cases. The matching independent
7c0a9ce projection shows 170 new passes and zero lost passes. No crashes, timeouts,
harness errors, or skips occurred. All required checks passed; see
`8314510-summary.json` and archived logs. This source does not complete Phase 1;
Proxy revocation and remaining Array allocation dependencies are still open.
No full corpus or full AST/bytecode differential measurement is claimed.

### Source 564efab: exploratory Proxy lifecycle validation

All required checks passed. The wider 3,721-variant selection passed 3,289, failed
432, and produced no special outcomes. Against the matching 7c0a9ce projection
it adds 359 passes and loses one: Atomics/waitAsync/bigint/was-woken-before-timeout,
script, instruction fuel exhausted. This failed gate is retained in the archives;
it did not trigger a full run. Thirty isolated repetitions passed and are archived
as diagnostics, not as replacement conformance evidence. A newer source corrects
broadcast acknowledgement ordering and yields after queue publication. Validation
of that source must include another complete focused gate.

Source `f9aa579` passes all required checks (884 workspace tests, four existing
ignored; 203 minimal; 73 Node; 14 WASM; 10 tooling). Its expanded focused selection
passes 4,794/5,267 variants, gaining 1,282 and losing two against the independent
7c0a9ce projection, with no special outcomes. Both losses are foreign
non-constructor TypeError realm regressions introduced by construction-context
entry. This failed gate blocks a full run until corrected. The prior agent
waitAsync fuel regression passes in this selection; no failed report is replaced.

Source `0577fc2` restores both construction eligibility regressions by checking
IsConstructor in the evaluating caller's realm before entering construction.
All required checks pass with the same totals as f9aa579. The identical expanded
selection passes 4,796/5,267 variants: 1,282 new passes, zero losses, and zero
harness errors, timeouts, crashes or skips against the matching independent
7c0a9ce projection. An environment restart interrupted this run after 1,136
files; the runner verified the exact configuration and frozen worker digest and
resumed the ordered checkpoint. Its final report retains every completed result.
The full pinned corpus is pending; this focused result does not close Phase 1's
remaining global-object/private/internal-operation dependencies.

The subsequent `0577fc2` full run is exploratory: 61,379 passes, 41,575 failures,
two timeouts, zero harness errors/crashes/skips across all 102,956 variants. It
gains 19,981 and loses nine versus PR #23; against 7c0a9ce it gains 8,345 and loses
12. Eight losses involve TypedArray [[Set]] with another receiver and invalid
indices. Other losses involve primitive string iterator deletion and inherited
global/with deletion resolution. These are regressions to fix, not silently
excluded variants. The positive focused gate does not override this failed full
gate. Exact full outcomes, both comparisons and the worker/check identities are
archived in `0577fc2-full-*` and `0577fc2-summary.json`.

Source `7dce903` shares receiver-preserving assignment/Reflect.set and normalized
Object/Reflect/Proxy descriptor definitions, including Proxy-backed array species
results. Its expanded focused gate passes 7,134/7,709 variants: 1,730 new passes,
zero lost passes, and zero harness errors, timeouts, crashes or skips against the
matching independent 7c0a9ce projection. All 12 variants lost by the previous
0577fc2 full run are selected and restored. Required checks pass: 888 workspace
(four existing ignored), 203 minimal-feature, 73 Node, 14 WASM and 10 tooling tests;
formatting and strict Clippy pass. The frozen worker digest is
`58190860d349fb11c3155920328fe98c732402bb3c9df901c4bcd6f86d46d0ec`.
Exact focused outcomes, transitions, checkpoint and check logs are archived in
`7dce903-*`. The full pinned run is in progress; this focused result is not full
merge evidence and does not close pending realm-global/private/exotic semantics.

Source `6456b53` extends shared canonical-index handling to typed-array Get and
HasProperty, preserves symbols through descriptor queries, and shares Array length
conversion/definition with assignment. Its focused gate passes 8,122/8,845 variants:
1,946 new passes, zero losses, and zero harness errors, timeouts, crashes or skips
against the matching independent 7c0a9ce projection. Required checks pass: 891
workspace (four existing ignored), 203 minimal-feature, 73 Node, 14 WASM, 10 tooling,
formatting and strict Clippy. Worker SHA256:
`a3b96f79678538a9a67bbae3d848d75149d9fd3d6fda4ab5a1f404b0945e17df`.
The initial minimal build encountered a disk-full linker failure; its log is
retained, stale compiled executables were removed, and the entire check pipeline
passed on rerun. This does not close the remaining foundation dependencies.

The `7dce903` full run was interrupted after 21,288 files without a final report.
Its ordered checkpoint is preserved for resumption with the same worker digest,
revision and configuration. A partial checkpoint is not full-corpus evidence.

Both subsequent full pinned runs pass the regression gate. Source `7dce903`
finishes its verified checkpoint with 61,995/102,956 passes, 20,588 new passes,
zero lost passes versus PR #23, 40,959 failures, two known deep-WeakMap timeouts,
and zero harness errors, crashes or skips. It also loses zero passes versus
7c0a9ce; interrupted/resumed logs remain archived with the final ordered outcomes.

The current source `6456b53` completes a fresh full run with **62,107/102,956
passes**, **20,700 new passes and zero lost passes** versus PR #23. It has **40,847
failures, two timeouts, zero harness errors, zero crashes and zero skips**. Versus
7c0a9ce it gains 9,061 and loses zero. Exact outcomes, checkpoints, comparisons,
configuration, worker hashes and log digests are in the two `*-full-summary.json`
files and their referenced archives. Both timeouts remain the Script/strict
`staging/sm/regress/regress-1507322-deep-weakmap.js` variants.

These passing regression gates do not declare Phase 1–3 semantics complete.
Realm-global records, complete private elements, accessor descriptors and remaining
Proxy/exotic operations remain open. Full-corpus AST/bytecode mismatches are not
measured; fallback remains enabled. Temporal/Intl, broader RegExp/standard-library
algorithms, deferred/JSON imports, resource management and decorators/auto-accessors
remain unsupported or incomplete outside the foundation scope and in the denominator.
