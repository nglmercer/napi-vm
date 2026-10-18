# Object copy semantics and fresh full-corpus evidence

Historical implementation checkpoint. The full-suite counts below describe
that source snapshot; see [latest.json](../tools/test262/latest.json) for the
current complete-corpus result.

This is incremental work toward the full-conformance goal. It does not close
the remaining foundation, standard-library, proposal, native-addon, or bytecode
coverage gates.

## Implemented semantics

Object literal spread and object rest share CopyDataProperties. They preserve
symbol identities, box primitive sources, support array/function/exotic own
properties, snapshot own keys, and then interleave descriptor checks with value
reads. Deleted or newly non-enumerable properties are skipped; properties added
after the key snapshot are excluded. Excluded rest keys are normalized once
and skipped before descriptor lookup or Get.

The bytecode compiler materializes each spread source at its source position,
before later property expressions. Its private data-property snapshot makes
final object assembly unobservable to source getters and proxies. Abrupt copies
stop subsequent expressions. Copied methods retain their original home objects.

Object rest enumerates after named bindings, so mutations from named getters
are visible. The AST evaluator and bytecode preserve computed string/symbol
keys for both Get and exclusion. The obsolete early key snapshot was removed
from the RestObject instruction, with matching compiler, VM, and verifier changes.

Object.assign boxes its target and sources, rejects nullish targets, skips
nullish sources, preserves symbols, and interleaves each Get with a throwing Set.
Setters may therefore change later source descriptors. Rejected writes throw
even when the caller is sloppy. Assignment uses Set; literal/rest construction
creates ordinary writable/enumerable/configurable data properties.

No AST fallback was removed. Full-corpus tier comparison remains unmeasured.

## Full pinned run

The working-tree snapshot is based on `9e4f017ee76ce538d9ac076fa180ac6d12ddd104`.
Its archived source patch SHA-256 is
`2fdf6f1e6af3589363645d53eda5180910bd31f5f2de85285f6f4100f081db92`.
That patch was frozen before the evidence-only documentation/latest.json updates.
Worker SHA-256 is
`54ae4204bf68771dada024933081e44e4bb8d30e2895280933fa131a489a3cd4`.

The corpus is pinned to `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`. The complete
run selected all 102,956 variants with no skips or substituted isolated outcomes:

| Outcome | Archived be8ebf3 repeat | Current snapshot |
| --- | ---: | ---: |
| Pass | 66,208 | 66,826 |
| Fail | 36,746 | 36,128 |
| Timeout | 2 | 2 |
| Crash | 0 | 0 |
| Harness error | 0 | 0 |
| Skip | 0 | 0 |

The pass percentage is **64.9073%**. Exact comparisons gain **618 passes** and
lose **zero**. This full delta includes the preceding array-elision/search work;
it is not attributed entirely to object copy. Both timeouts are the Script and
strict variants of `staging/sm/regress/regress-1507322-deep-weakmap.js`.
All 8,659 execution parse-negative variants pass phase/error-type checks.

The run retains four workers, five-second timeouts, fuel 1,000,000, loop budget
100,000, call depth 128, and job budget 10,000. It used a frozen worker, a verified
corpus, a complete checkpoint journal, and zero restored files.

The focused union of object/array literals, assignment destructuring, Object.assign,
and variable/const/let statements passes 3,145/3,939 variants, with **60 new passes
and zero losses** against the matching archived full-run projection. Its 794
failures remain explicit; no special outcomes or skips occurred. The earlier
spread-only exploratory report passes 2,447/3,072; it is superseded by the wider
final focus and full run.

Exact outcomes, comparisons, triage, source patch, check logs, and archive hashes
are retained in [the evidence summary](../tools/test262/evidence/object-copy-continuation/summary.json).
The saved immutable worker and checkpoint are available locally under
`artifacts/test262/object-copy-worker` and
`artifacts/test262/object-copy-full.checkpoint.jsonl`.

```sh
cargo build --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 \
  --checkpoint artifacts/test262/object-copy-full.checkpoint.jsonl \
  --output artifacts/test262/object-copy-full.json
```

## Validation and remaining work

All 11 object-copy regression tests assert verified bytecode execution and
compare raw AST outcomes. They cover symbols, primitive/array/function copying,
property mutation, Proxy trap order, source-position copying, abrupt completions,
rest exclusions, single computed-key conversion, throwing writes, and setter
interleaving. All pass, as do the six preceding array tests, four engine-boundary
tests, 17 worker tests, and 12 Python tooling tests. Formatting, whitespace checks,
and strict workspace/all-target/all-feature Clippy pass.

The minimal core unit suite passes 354 tests with the known sidecar integration
fixture explicitly filtered. This is a targeted check, not an unfiltered green
workspace gate. The preceding workspace run stopped at core with 381 passes and
five native-addon failures; those failures remain open and that workspace gate
was not repeated here. Details are preserved in the array continuation checkpoint.

The fresh full triage still records 13,268 Temporal/Intl Temporal failures, 1,892
RegExp failures, 2,441 class statement/expression failures, 1,130 Iterator failures,
1,016 Array failures, 988 TypedArray failures, and 855 Object failures. These
counts include unsupported algorithms and foundation dependencies; they do not
classify every individual failure. Runtime failures total 35,945, parse failures
154, and resolution failures 29, plus the two timeouts.

Typed accessor/descriptor storage, remaining class/reference/global/realm/GC
semantics, broader algorithms and proposals, the native-addon test gates, and
full-corpus AST/bytecode parity still require implementation and verification.
The original full-conformance goal remains active.
