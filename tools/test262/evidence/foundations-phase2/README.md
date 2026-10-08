# Phase 2 exploratory evidence

Phase 2 remains incomplete. These reports retain all outcomes, including
regressions; they do not certify final-source completion.

Corpus revision: `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`.
All selections use four workers, 5s timeout, instruction fuel 1,000,000,
loop budget 100,000, maximum call depth 128 and maximum jobs 10,000.
No variants are skipped. Each report records the worker SHA256.

| Selection | Source | Pass | Fail | Harness | Timeout | Crash | Lost vs Phase 1 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| First full | d434ffe | 44,228 | 58,680 | 46 | 2 | 0 | 119 |
| Second full | 0faed45 | 44,833 | 58,075 | 46 | 2 | 0 | 43 |
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
