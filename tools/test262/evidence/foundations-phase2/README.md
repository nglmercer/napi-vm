# Phase 2 validation evidence

The current frozen source is **313146b**. Foundation grammar and early-error
coverage is implemented; the broader Phase 1 audit and Phases 3–4 remain open.
The PR stays draft.

| Outcome | Count |
| --- | ---: |
| Pass | 50,926 / 102,956 |
| New passes versus PR #23 | 9,519 |
| Lost passes versus PR #23 / 165dddf / 885a8c7 | 0 / 0 / 0 |
| Fail | 51,982 |
| Harness errors | 46 |
| Timeouts | 2 |
| Crashes / skips | 0 / 0 |
| Parse-negative phase/error checks | 8,659 / 8,659 |

Required checks pass: 793 workspace tests (four existing ignored), 158 minimal,
73 Node, 14 WASM, nine tooling, fmt and strict Clippy. Repository forced-tier
fixtures observe zero mismatches; full-corpus AST/bytecode differential is not
measured. No AST fallback was removed.

`313146b-summary.json` records configuration, identities, digests and every
compressed report. `313146b-full.json.gz` retains all outcomes; comparisons against
PR #23 and both earlier Phase 2 milestones retain every transition. The focused
projection covers 38,822 variants, with 23,054 passes, 15,768 failures and zero
harness errors/timeouts/crashes/skips. It is extracted from the full run by exact
variant identity. The independent repaired-regression selection passes 2/2.

The compile-only source audit rejects all 8,659 parse negatives and accepts
94,143/94,297 sources requiring acceptance. Its 154 remaining rejections cover
88 deferred imports, 22 resource-management variants, 42 decorators/auto-accessors
and two preserved parse-depth limits. Acceptance is not an execution pass.

Reproduce the comparison:

```sh
python3 tools/test262/compare.py \
  tools/test262/evidence/foundations-phase1/baseline-final-full.json.gz \
  tools/test262/evidence/foundations-phase2/313146b-full.json.gz \
  --output artifacts/test262/phase2-comparison.json
```

See [implementation and reproduction](../../../../docs/test262-phase2-contexts.md)
for build, full execution and source-audit commands. All execution uses revision
`5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, four workers, 5s timeout, fuel
1,000,000, loop budget 100,000, call depth 128 and job budget 10,000.

The 885a8c7 reports retain the two intermediate losses restored by 313146b.
They are historical evidence, not the final implementation's regression count.

---

# Earlier Phase 2 validation evidence

At the 165dddf milestone, Phase 2 remained incomplete. That frozen source passes 46,342/102,956:
4,935 new passes and zero lost passes versus PR #23, with zero crashes.
The exploratory reports retain all outcomes, including regressions. Subsequent
parser changes require their own validation.

Corpus revision: `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.
All selections use four workers, 5s timeout, instruction fuel 1,000,000,
loop budget 100,000, maximum call depth 128 and maximum jobs 10,000.
No variants are skipped. Each report records the worker SHA256.

| Selection | Source | Pass | Fail | Harness | Timeout | Crash | Lost vs Phase 1 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| First full | d434ffe | 44,228 | 58,680 | 46 | 2 | 0 | 119 |
| Second full | 0faed45 | 44,833 | 58,075 | 46 | 2 | 0 | 43 |
| Third full | a48a3df | 45,313 | 57,595 | 46 | 2 | 0 | 17 |
| Frozen milestone | 754ed80 | 45,459 | 57,449 | 46 | 2 | 0 | 0 |
| First regression paths | a48a3df | 120 | 0 | 0 | 0 | 0 | 0 |
| Second regression paths | a48a3df | 44 | 0 | 0 | 0 | 0 | 0 |

The focused baseline is extracted by exact variant identity from the Phase 1
full report. The selection is the union of async-generator-grammar, super,
private-names and eval-globals (15,247 variants). The initial run gained 1,442
passes and lost 18; d434ffe gained 1,448 and lost zero. These focused results
are not a substitute for full validation of subsequent source changes.

Reproduce the first comparison from the repository root:

```sh
python3 tools/test262/compare.py \
  tools/test262/evidence/foundations-phase1/current-full.json.gz \
  tools/test262/evidence/foundations-phase2/exploratory-first-full.json.gz \
  --output artifacts/test262/phase2-first-comparison.json
```

For the second comparison, substitute `exploratory-second-full.json.gz`.
The retained transitions include every newly passing and formerly passing
variant. `exploratory-summary.json` records source revisions, worker/report
checksums, configuration, and required checks at a48a3df. Full AST/bytecode
differential testing is not measured; AST fallback remains enabled.

The frozen milestone gains 3,717 passes versus Phase 1. Its focused selection
passes 6,353/15,247: 1,543 gains and zero losses versus Phase 1. Required checks
pass at this source: fmt, Clippy, 751 workspace Rust tests (four existing ignored),
158 minimal Rust tests, 73 Node tests, 14 WASM tests and nine tooling tests.
`754ed80-summary.json` records checksums and exact source/worker identities.
Use `754ed80-full.json.gz` for the milestone comparison and
`../foundations-phase1/baseline-final-full.json.gz` as the PR #23 baseline.

Iteration bindings at 5eb88fe are exploratory: 2,232/4,837 pass, with 446 gains
and four losses against the identical 09498f5 selection. All required checks
pass, with 759 workspace tests and four existing ignored tests. The losses are
for-of RHS comma-expression and bare `async of` grammar boundaries; 63f16b6
repairs them. Reports retain every outcome and phase classification; no tests
are skipped. The repaired 63f16b6 selection passes 2,240/4,837: 450 gains and
zero losses, with zero crashes/timeouts/harness errors. Its full corpus and the
required checks for subsequent source are still in progress.

The exploratory full run at 63f16b6 passes 46,320/102,956 with 4,916 gains and
three losses versus PR #23, 46 harness errors, two timeouts, zero crashes and
zero skips. The catch/eval and var-pattern binding regressions are repaired in
39c9c02 and 165dddf; measurements for the repaired source are pending.
`63f16b6-summary.json` records all retained report checksums and worker identity.

At repaired source 165dddf, all required checks pass (761 workspace Rust tests,
four existing ignored; 158 minimal; 73 Node; 14 WASM; nine tooling; fmt/Clippy).
Its 7,528-variant iteration/catch/contextual/eval selection passes 4,131: 11 gains
and zero losses against 63f16b6, with zero harness errors/timeouts/crashes/skips.
The separate regression selection passes 4/4. The full run remains in progress.

The final measured source for this increment, 165dddf, passes 46,342/102,956:
4,935 gains and zero losses versus PR #23; 883 gains and zero losses versus
754ed80. Remaining outcomes are 56,566 failures, 46 harness errors, two
timeouts, zero crashes and zero skips. This is not Phase 2 completion.
`165dddf-summary.json` records source/worker identity, configuration, required
checks and all compressed-report checksums. Full-corpus differential remains
unmeasured; repository forced-tier fixtures have zero observed mismatches.

Reproduce the comparison:

```sh
python3 tools/test262/compare.py \
  tools/test262/evidence/foundations-phase1/baseline-final-full.json.gz \
  tools/test262/evidence/foundations-phase2/165dddf-full.json.gz \
  --output artifacts/test262/phase2-165dddf-comparison.json
```

To reproduce execution, build source 165dddf with Rust 1.98.1 using
`cargo build --release --bin napi-vm-test262`, freeze the executable, then run
`tools/test262/run.py /workspace/test262 --engine <frozen-worker> --revision
5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 --jobs 4 --timeout 5 --output <report>`.
The worker fixes fuel/loop/call-depth/job limits to the configuration above.

The fresh `1005a4c-syntax-*` evidence repeats all 102,956 source-grammar outcomes
with zero acceptance transitions from `313146b` and no invalid syntax accepted.
`372c20b-companion-link-driver-failure.log.gz` is the failed tooling attempt,
not a Test262 conformance report; `1005a4c` fixes its Cargo dependency search path.
