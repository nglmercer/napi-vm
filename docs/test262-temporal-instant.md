# Temporal Instant continuation

Adds privately branded Temporal.Instant values using temporal_rs for epoch
bounds, exact nanosecond arithmetic, string parsing/formatting, differences,
and rounding. Constructor/factory coercions use guest ToPrimitive, ToBigInt,
and ToNumber operations. Prototype accessors reject incompatible receivers.
The from/fromEpoch factories produce intrinsic instances; construction honors
new.target. Existing runner limits, corpus, denominator, and skips are unchanged.

The pinned Instant selection passes **886 / 934 variants (94.8608%)**.
Comparison against the preceding complete Temporal report has **878 gained
passes, zero lost passes**, and eight retained passes. There are no skips,
timeouts, crashes, or harness errors. Remaining Instant failures depend on
ZonedDateTime, including toZonedDateTimeISO, which is still unimplemented.
This focused percentage is not the overall pass rate.

Evidence is in tools/test262/evidence/temporal-instant. The measured debug
worker SHA-256 is
2910cd202bb839274233dddf3fc8696d74633a5aa4ba0cf9b61f764909f6448e.
The measured.patch file reconstructs the measured snapshot on source-base.txt.
After measurement, one Clippy fix changes TimeZone::try_from_str's argument
from an allocated string to s.as_ref(); this does not change guest behavior.

Validation: all 72 selected Rust tests pass across temporal, json_reviver,
builtin_quick_wins, utf16, and test262_worker. The two new Instant regressions
independently assert AST and verified bytecode execution. Strict Clippy for
the library, worker, and Temporal regression test passes.

A full pinned run was started with the frozen measured debug worker:

```sh
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/debug/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 \
  --checkpoint artifacts/test262/temporal-instant-full.checkpoint.jsonl \
  --output artifacts/test262/temporal-instant-full.json
```

At checkpoint writing, this run is still active (exec session 11633). Inspect
its live process/session before considering restart/resume. The latest completed
full report remains temporal-time-duration-full.json at 79,243 / 102,956
(76.9678%), before raw JSON and Instant. The overall 90% goal remains active
and needs at least 92,661 passing variants. Do not add focused gains to the
previous full count and present that sum as a measured full result.
