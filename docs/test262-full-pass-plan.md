# Test262 status and plan

Corpus: `0b4e0812022b18236b5276d2804948fdd9f71603` (`tools/test262/corpus.lock.json`),
102,992 variants. Measured with the isolated worker, two jobs, five-second
deadline per variant.

## Current status

Latest complete run: source `85ba746`, revision `0b4e081`.

| Outcome | Count |
| --- | ---: |
| Pass | 102,975 |
| Fail | 1 |
| Timeout | 16 |
| Crash, harness error, skip | 0 |

Against the previous complete run (`d1d403d`, 102,957 passing) it gains 18 and
loses none. A later complete run of the Rust runner's tree matched that
report variant for variant except for two bare-carriage-return fixtures that
the runner's metadata parser had missed; those are fixed.
Evidence: [`full-interpreter-overhead-2026-10-30`](../tools/test262/evidence/full-interpreter-overhead-2026-10-30/).

## What blocks a full pass

1. **The failure cannot be fixed in the engine.**
   `annexB/language/function-code/block-decl-func-skip-arguments.js` requires
   `arguments` to remain the Arguments object after a block-level
   `function arguments(){}`. `staging/sm/lexical-environment/block-scoped-functions-annex-b-arguments.js`
   requires it to become the function. Both are sloppy-mode functions over the
   same construct; no behaviour satisfies both
   ([tc39/test262#5113](https://github.com/tc39/test262/issues/5113)).
   A literal full pass requires editing or excluding one of them, which this
   project does not do.

2. **Sixteen Date timeouts** (`staging/sm/Date/dst-offset-caching-1..8-of-8`,
   script and strict). Each part takes about 6.5 seconds alone; the limit is
   five seconds, and a two-job run needs about 4.4 seconds for a reliable pass.
   The profile is flat: no source line exceeds about 2% of samples. Each part
   performs about 3 million `tzOffsetFromUnixTimestamp` calls, each doing a
   JavaScript function call, `new Date`, `setTime`, and `getTimezoneOffset`.

## Measured costs (per operation, one million iterations)

| Operation | Time |
| --- | ---: |
| Empty loop iteration | 0.10 µs |
| JavaScript function call | 0.42 µs |
| `new Date(NaN)` | 0.27 µs |
| Native method call (`d.setTime`, `Math.abs`) | 0.29 µs |
| `assert.sameValue` | 1.5 µs |

Changes already made, checked by the core tests and by complete runs of the committed trees: object metadata and
frame payloads boxed or lazily allocated; cached allocation prototypes; direct
`new Date` for primitive arguments; in-place writes to existing global and
numeric lexical bindings; cached closure chain context; inlined per-instruction
dispatch; compile-time elimination of single-load argument copies and of
completion tracking in function bodies; lazily created Date property objects.

## Next steps

- **Reduce per-call cost** (the only remaining lever for the DST tests). The
  candidates are frame and environment reuse across calls, and a cheaper path
  for native methods on Date receivers. Each needs a full run before commit.
- **Reword the goal** to "every test except the upstream contradiction" if the
  Date timeouts are not fixed, so progress is measured against an attainable
  target.
- **Engine startup** (about 10 ms per worker process) and the 1,226
  Unicode-property-escape variants (about 1.1 s each) dominate full-run time.
  They do not affect results, only how long validation takes.

## Validation policy

Iterate with `test262 run --group smoke` and `test262 retest`. Before committing
an engine change, run the complete corpus with `test262 run --output full.json`
and compare it with the last complete report:
`test262 compare <previous.json.gz> full.json`. Commit only when that comparison
shows no losses and no unexplained additions or removals.
