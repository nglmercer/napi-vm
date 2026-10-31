# Test262 progress log

Current status and remaining work: [`test262-full-pass-plan.md`](test262-full-pass-plan.md).
Commands and reports: [`tools/test262/README.md`](../tools/test262/README.md).
Earlier experiment-by-experiment notes are listed in [`test262-history.md`](test262-history.md).

## Complete runs

| Date | Source | Revision | Pass / total | Fail | Timeout | Evidence |
| --- | --- | --- | ---: | ---: | ---: | --- |
| 2026-10-25 | `259402f` | `5992dc3` | 102,760 / 102,956 | 133 | 63 | removed; see history |
| 2026-10-27 | `d1d403d` | `0b4e081` | 102,957 / 102,992 | 1 | 34 | [`full-new-revision-2026-10-27`](../tools/test262/evidence/full-new-revision-2026-10-27/) |
| 2026-10-30 | `85ba746` | `0b4e081` | 102,975 / 102,992 | 1 | 16 | [`full-interpreter-overhead-2026-10-30`](../tools/test262/evidence/full-interpreter-overhead-2026-10-30/) |
| 2026-10-30 | `9f5c446` | `0b4e081` | 102,975 / 102,992 | 1 | 16 | not archived; zero gains and losses against `85ba746` |
| 2026-10-30 | `9bc2103`, Rust runner | `0b4e081` | 102,973 / 102,992 before the CR fix | 3 | 16 | not archived; the two extra failures were fixtures with bare-CR line endings in the runner's metadata parser, fixed in the same commit |

The `85ba746` run gained 18 variants over `d1d403d` and lost none. The
`9bc2103` tree matches `85ba746` variant for variant once the metadata parser
handles bare carriage returns; that was checked with the Rust runner's
`compare`.

## Changes measured by these runs

- `85ba746`: boxed rarely used object and frame payloads, lazily allocated
  hash collections, cached `Object`/`Array`/`Date` allocation prototypes, a
  direct `new Date` path for primitive arguments, in-place writes to existing
  global data properties and numeric lexical bindings, cached closure chain
  context, and inlined instruction dispatch. RegExp literal tests dropped from
  about 5.2 to 1.1 seconds alone; URI decoding from 5.5 to about 4.2; the
  TypedArray counting sort from 5.6 to about 4.8.
- `9f5c446`: single-load call arguments written directly into argument
  registers; statement completion tracking skipped in function bodies; scalar
  overwrites without drop glue; captured bindings returned without a second
  chain walk.
- `9bc2103`: Date property objects created on first use (`new Date` about
  twice as fast; the DST tests moved from about 6.5 to about 6.1 seconds alone).

## Still failing

- `annexB/language/function-code/block-decl-func-skip-arguments.js`:
  contradicted by a staging test in the same revision (see the plan).
- `staging/sm/Date/dst-offset-caching-1..8-of-8` (script and strict): timeouts,
  about 6.1 seconds alone against a five-second limit.
