# Realm and buffer continuation evidence

Phases 1–3 remain incomplete and PR #24 remains draft. Each report is tied to
its exact source commit and frozen worker digest; focused and exploratory
results are not completion evidence.

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

Later GC/host-coercion changes require their own checks and corpus evidence;
this full result must not be attributed to those later commits.
