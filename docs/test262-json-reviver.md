# JSON reviver continuation toward 90% Test262

The full-corpus goal remains open. The preceding full report is 77,085 / 102,956
(74.8718%). This change implements JSON.parse reviver traversal, including the
source context argument; it does not substitute JSON-only results for that goal.

The pinned JSON selection has 298 / 330 passes (previously 264 / 330): 34 gained
passes, zero lost passes, zero skips, timeouts, crashes, or harness errors. Every
selected JSON.parse variant passes. Remaining JSON failures concern raw JSON
support. Compressed focused reports and a comparison against the preceding full
report's JSON projection are in tools/test262/evidence/json-reviver/.

The implementation uses observable property reads, array identification, length
coercion, own-key enumeration, property descriptors, deletion, and definition.
Children are visited before their parent. Failed property changes are ignored
while abrupt completions propagate. Source records retain the original value
and are invalidated when an earlier callback replaces a container or value.
Duplicate keys keep the final parse record; UTF-16 source spelling is preserved,
including literal unpaired surrogates. Existing depth and allocation limits remain.

Four regressions run independently with AST execution and verified bytecode;
all pass. The 22 existing builtin_quick_wins regressions also pass. Strict Clippy
for the library, worker, and new integration suite passes, as do all 12 Test262
tooling tests. Formatting and diff whitespace checks pass.

Focused worker: debug build SHA-256
7ab61c951fcffd9b28cdacd835f85cb02af9c28bf955086dd5069e075c4e54b0.
The final source subsequently received two Clippy-only cleanups. The full run
uses the same frozen debug worker, four jobs, five-second timeout, no skips, and
corpus 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81. It was started with a resumable
journal at artifacts/test262/json-reviver-full.checkpoint.jsonl; its final output
is artifacts/test262/json-reviver-full.json. The debug run was interrupted deliberately after its slower RegExp execution
produced numerous timeouts. Its journal remains available; it is not used for
comparison. A subsequent release-worker full run completed with 77,289 / 102,956
passes (75.0699%), 25,665 failures, two timeouts, and no skips, crashes or harness
errors. The worker digest is
eb2a605bd0e2166b38e4dfa180f7967fc638fa3e3e74388bdbbb8dfd5b2df04c.
Its full report and comparison are archived beside the focused reports.
Relative to the older latest.json report it gains 204 passes and loses none:
34 JSON gains from this change and 170 gains in other areas from the already
merged observable-array work in HEAD db07251. Those 170 are not attributed to
this JSON implementation. The 90% full-suite goal remains open.

Reproduce focused execution:

```sh
cargo build --no-default-features --bin napi-vm-test262 \
  --config profile.dev.package.napi-vm-core.codegen-units=1
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/debug/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 --path built-ins/JSON \
  --output artifacts/test262/json-reviver-focused.json
cargo test --no-default-features --test json_reviver --test builtin_quick_wins \
  --config profile.dev.package.napi-vm-core.codegen-units=1
```

Failure-group inspection of the preceding full report identifies Temporal as
13,268 nonpassing variants across built-ins and intl402. Major additional groups
include RegExp (1,892), TypedArray (986), and Promise (718). Reaching 90% requires
substantial additional feature implementation, beyond this JSON batch.
