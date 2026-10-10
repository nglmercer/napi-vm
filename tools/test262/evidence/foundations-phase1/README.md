# Phase 1 continuation evidence

This evidence validates the agent/shared-memory commit `1a8162a`, not completion
of Phases 1–3. The PR remains draft. See [implementation status](../../../../docs/test262-foundations-completion.md).

`summary.json` records configuration, worker digests, source commits, required
checks, outcome counts, differential scope, and remaining gates. The compressed
reports retain every individual outcome from the pinned 102,956-variant corpus:

- `current-full.json.gz`: final implementation, 41,742 passes.
- `baseline-final-full.json.gz`: PR #23 without competing builds, 41,407 passes.
- `baseline-first-full.json.gz`: earlier PR #23 measurement, 41,409 passes.

Reproduce both exact comparisons from the repository root:

```sh
python3 tools/test262/compare.py \
  tools/test262/evidence/foundations-phase1/baseline-final-full.json.gz \
  tools/test262/evidence/foundations-phase1/current-full.json.gz \
  --output artifacts/test262/reproduced-final-transitions.json
python3 tools/test262/compare.py \
  tools/test262/evidence/foundations-phase1/baseline-first-full.json.gz \
  tools/test262/evidence/foundations-phase1/current-full.json.gz \
  --output artifacts/test262/reproduced-first-transitions.json
```

The final comparison has 335 new passes and zero lost passes. The first comparison
has 335 new passes and two lost passes: both variants of the 100 ms busy-loop
dynamic-import fixture. Idle baseline and final implementation exhaust unchanged
instruction fuel; the earlier baseline under competing builds passes. This
load-sensitive difference is retained rather than removed from evidence.

The reports were generated before the runner's static `limitations` description
became worker-neutral. That description does not probe worker capabilities:
PR #23 lacks agents despite the inherited report wording. Source commits and
worker digests identify the implementation actually measured.

Full-corpus AST/bytecode differential results are not available. Zero mismatches
in `summary.json` refers only to the two Atomics fixtures. AST fallback remains.
