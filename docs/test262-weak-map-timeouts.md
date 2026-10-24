# Historical timeout investigation

The weak-collection changes remove two quadratic operations behind the deep
WeakMap fixture: key searches scan an identity hash index instead of all entries,
and GC wakes ephemerons as keys become reachable instead of repeatedly scanning
all maps. Weak handles and key equality checks remain in place. Weak collections
have no guest-observable iteration order.

On the pinned corpus `5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81`, both variants of
`staging/sm/regress/regress-1507322-deep-weakmap.js` now pass under the unchanged
five-second deadline. The separate weak collection regression passes 605/605,
including those two variants; the baseline passes the other 603/603.

## Measurement

All 137 files containing the previous full report's 225 timeout variants were
selected. This also runs 49 neighboring variants that previously passed.

| Outcome | Exploratory baseline | Patched worker |
| --- | ---: | ---: |
| Pass | 212 | 225 |
| Timeout | 62 | 49 |
| Other outcomes | 0 | 0 |
| Total | 274 | 274 |

Of the **original 225 timeout variants**, **176 pass and 49 still time out**.
The selected comparison has 13 gains and zero losses. This is not a claim that
this patch fixes all 176: current main already includes many earlier fixes, and
the exploratory baseline overlapped builds and a separate diagnostic probe.
Timing gains outside the deep-WeakMap fixture are not attributed to this patch.
The patched historical-timeout run had no competing builds or diagnostic probes.

Remaining cases include the eight exhaustive Date staging files (16 variants),
large regexp-literal loops, six TypedArray copyWithin variants, URI loops,
shared-buffer TypedArray workloads, and a handful of other large fixtures.
The exact 49 variants remain listed in `summary.json`. They have not been skipped,
reclassified, or assigned longer deadlines. **The request to pass all timeouts
remains incomplete.** No overall Test262 score is updated.

## Validation and reproduction

- 371 core-library tests, without exclusions.
- Five integration tests: weak-key identity/deletion, a 10,000-link ephemeron
  chain, ephemeron self-retention prevention, and the existing GC lifecycle tests.
- 12 Python runner tests; formatting and core Clippy with warnings denied.
- Release optimization, LTO disabled, 16 codegen units for both measured workers.
  The installed Rust toolchain failed to link the original thin-LTO build.

Build and run the directly fixed fixture:

```sh
CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
  cargo build --locked --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py /path/to/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 2 --timeout 5 \
  --path staging/sm/regress/regress-1507322-deep-weakmap.js \
  --output artifacts/test262/deep-weakmap.json
```

[Evidence](../tools/test262/evidence/weak-map-timeouts-2026-10-23/) contains full
selected outcomes, the remaining-case inventory, source/worker identities, build
settings and validation logs. Local commit `34fe66b` and published commit
`8d875d6` have the identical measured source tree.
