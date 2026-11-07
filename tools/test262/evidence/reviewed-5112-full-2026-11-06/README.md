# Reviewed Test262 profile: complete pass

All **102,992 variants in 53,615 files passed**, with zero failures, skips,
timeouts, crashes, or harness errors. The run used two workers and the
existing CI deadline of ten seconds per variant; elapsed time was 1,458
seconds. All 16 DST Date variants passed (maximum 6.046 seconds).

This is the separately labelled `reviewed-5112` corpus selected by the user.
It contains the pinned Test262 revision plus the exact one-file correction
from reviewed, unmerged [PR #5112](https://github.com/tc39/test262/pull/5112).
The original corpus remains unchanged. These results establish a full pass
of this corrected profile.

## Provenance

- Base corpus: `0b4e0812022b18236b5276d2804948fdd9f71603`.
- Reviewed PR head: `1f3d994e2658c2934a64781ffbb7a9b3782d3310`.
- Derived corpus: `cff513bc172b79b97963046e83078e114b2a20ff`.
- Only changed file: `test/annexB/language/function-code/block-decl-func-skip-arguments.js`.
- Corrected Git blob: `33d4c24b0d0479e0c2199f7d629b9cc77ddd2d6d`.
- Engine SHA-256: `cedda1803516ed6d0b3f9e46b624e36d7dc05aec459ff4561d63dc4b7a8dcad9`.
- [Exact patch](../date-native-fast-path-2026-11-06/upstream-5112.patch).

## Evidence

- `report.json.gz`: complete report, including corpus provenance and every outcome.
- `summary.json`: counts, grouped outcomes, and provenance.
- `comparison.json`: explicit comparison with the original five-second baseline,
  permitting the corpus and timeout configuration changes. It has the same
  variant keys, 17 gains and zero losses; gains include the reviewed correction
  and completion of the Date workload under the CI deadline.
- `audit.json`: completion checks, including exact variant-set equality,
  clean original and reviewed checkouts, and the current engine digest.

The final core unit suite passed all 445 tests. All 11 runner unit tests
passed, including corpus cleanliness, profile separation, and checkpoint
provenance checks. Workflow YAML and profile selection were checked locally;
the remote GitHub Actions workflow was not triggered.

## Reproduce

```sh
cargo build --release --locked --no-default-features --bin napi-vm-test262
cargo build --release --locked -p napi-vm-test262-runner
target/release/test262 fetch --profile reviewed-5112
target/release/test262 run --profile reviewed-5112 --jobs 2 --timeout 10 \
  --output reviewed-5112-full.json --checkpoint reviewed-5112.checkpoint
```

Blaze was attempted for development, but its frontend could not compile ICU
provider types in this workspace. Cargo produced the worker used for this run.
