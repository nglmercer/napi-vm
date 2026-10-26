# Complete runtime follow-up — 2026-10-26

102,819 / 102,956 passes (99.8669%): 78 failures and 59 timeouts, with zero
skips, crashes, or harness errors. There are 137 remaining variants: 42 parse
failures, 36 runtime failures, and 59 timeouts.

Comparison against the preceding complete worker 0714d545… records 36 gains
and zero losses: all 33 focused semantic gains plus three timeout recoveries
(toSpliced-dense script and both BMP casing variants). Against the clean d942b04
reference it gains 74 passes with zero losses. Timing recovery in this single
full run still needs stability checks before the final full-pass gate.

The full report, both comparisons, triage, and exact residual inventory are
retained. source.json and source.diff identify the measured uncommitted source
snapshot based on 42ae715; their hashes were checked against the current source
and worker after the run. The run used the pinned corpus, two workers,
five-second deadlines, no skips, and no resumed outcomes.

Checks passed 391 core library tests, five worker tests, all-target Clippy with
warnings denied, the root all-target check, formatting, and diff whitespace.
The original checkpoint remains at
artifacts/test262/full-runtime-followup-2026-10-26.checkpoint.jsonl.
The final acceptance gate requires 102,956 passes on clean committed source.
