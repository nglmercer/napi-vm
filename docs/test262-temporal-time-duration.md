# Temporal PlainTime and Duration continuation

The user goal remains at least 90% passes over the complete pinned corpus, with
all variants in the denominator. This batch adds Temporal.PlainTime and
Temporal.Duration bindings backed by temporal_rs 0.2.6. No corpus, test assertion,
feature skip, timeout, runner setting, or resource limit changes are made.

JavaScript property reads, coercion order, receiver branding, descriptors,
constructor/newTarget handling, and realm prototypes remain VM operations.
Arithmetic and parsing use temporal_rs, including bundled timezone data for
relativeTo strings and property bags. The float64_representable_durations
feature preserves ECMAScript representable-number behavior. No system clock or
host local timezone access is enabled.

Supported operations include time construction/from/compare, accessors, add,
subtract, with, until, since, round, equals, and formatting; duration
construction/from/compare, accessors, add, subtract, with, abs, negated, round,
total, and formatting. Source slots are private metadata rather than visible
properties. Other Temporal types are not yet installed; their tests still fail.
This batch is not a claim of complete Temporal or Intl support.

## Measured results

The release focused run passes **1,844 / 2,066** selected variants (89.2546%).
Against the same selection projected from the preceding full report, it gains
1,844 passes and loses none. There are 222 failures, zero skips, timeouts,
crashes, or harness errors. Debug and release builds independently reproduce
these counts. Reports, comparison, source patch, and summary are archived in
`tools/test262/evidence/temporal-time-duration/`.

Worker SHA-256:
`02213b69197d3f1d8501cd550dd90a6f2941cfae389c45676811ae7263025edc`.
Source base: HEAD db07251 plus the archived patch, which includes the preceding
JSON reviver changes. The preceding complete release measurement is
77,289 / 102,956 (75.0699%); see test262-json-reviver.md. Do not add focused gains
to this count and call that a measured overall result.

The full run of the frozen release worker completed with **79,243 / 102,956
passes (76.9678%)**, 23,711 failures, two timeouts, and zero skips, crashes,
or harness errors. Comparison against the preceding complete JSON release run
shows **1,954 gained passes and zero lost passes**. This includes 110 gains
outside the focused PlainTime/Duration selection. The compressed full report
and transitions are archived alongside the focused evidence. The 90% goal
remains open; it requires at least 92,661 passing variants.

## Validation

All eight new regressions pass independently in raw AST execution and verified
bytecode. They cover exact nanosecond arithmetic, field/option coercion order,
private branding and proxy rejection, mixed-sign duration rejection, calendar
relativeTo arithmetic, null-prototype shorthand options, string-hint option
conversion, and 23-hour/25-hour DST days. All four JSON reviver and 22 existing
builtin_quick_wins regressions pass as well (34 total).

Strict Clippy for the library, worker, and Temporal test passes. Formatting and
diff whitespace checks pass. The WASM-feature host check passes; this does not
claim actual wasm32 execution. Core library testing passes 354 tests and fails
the historically problematic native-addon fixture test
`loads_and_invokes_a_real_node_api_addon`, with `Cannot redefine property:
prototype`. A complete clean Rust suite is not claimed.

## Reproduction

```sh
cargo build --release --no-default-features --bin napi-vm-test262
python3 tools/test262/run.py .napi-vm/test262 \
  --engine target/release/napi-vm-test262 \
  --revision 5992dc3b60faf62a48fd6be8a40ae9d9a8c84d81 \
  --jobs 4 --timeout 5 \
  --path built-ins/Temporal/PlainTime --path built-ins/Temporal/Duration \
  --output artifacts/test262/temporal-time-duration-release-focused.json
cargo test --no-default-features --test temporal --test json_reviver \
  --test builtin_quick_wins \
  --config profile.dev.package.napi-vm-core.codegen-units=1
```

Most remaining failures in these two focused groups depend on absent PlainDate,
PlainDateTime, or ZonedDateTime constructors/conversions. Those types, Instant,
calendar types, and Temporal.Now remain substantial work toward the full goal.
