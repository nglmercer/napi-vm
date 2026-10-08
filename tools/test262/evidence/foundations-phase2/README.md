# Phase 2 exploratory evidence

Phase 2 remains incomplete. The frozen 754ed80 source passes 45,459/102,956:
4,052 new passes and zero lost passes versus PR #23, with zero crashes.
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
