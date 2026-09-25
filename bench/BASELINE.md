# Performance Baseline

Captured at commit `2a6a610` (clean tree), Linux x64, Node v26.4.0, Rust release profile (default, no LTO).
Test suite: **535 pass / 0 fail**.

## End-to-end through NAPI (`npm run bench`)

Each workload measured ≥ 250 ms after 20 warmup iterations.

| workload        | vm/op     | vm ops/s | native/op  | ratio |
|-----------------|-----------|----------|------------|-------|
| arithmetic_loop | 3.69 ms   | 271      | 5.02 µs    | 736x  |
| recursion_fib   | 20.06 ms  | 50       | 102.19 µs  | 196x  |
| array_chain     | 2.11 ms   | 473      | 16.24 µs   | 130x  |
| string_ops      | 1.13 ms   | 888      | 69.38 µs   | 16x   |
| class_methods   | 2.48 ms   | 403      | 40.17 µs   | 62x   |
| closures        | 5.65 ms   | 177      | 32.63 µs   | 173x  |
| json_roundtrip  | 1.58 ms   | 635      | 164.31 µs  | 10x   |

ratio = vm time / native time (higher = slower than the host engine).

## Criterion microbenchmarks (`npm run bench:rust`)

Full pipeline (lex → parse → eval) unless noted; 100 samples each.

| benchmark                 | time (estimate)          |
|---------------------------|--------------------------|
| run/arithmetic_loop       | 3.78 ms (3.75–3.81)      |
| run/recursion_fib         | 18.38 ms (18.19–18.61)   |
| run/array_chain           | 2.02 ms (2.00–2.04)      |
| run/string_ops            | 1.11 ms (1.10–1.13)      |
| run/class_methods         | 2.55 ms (2.52–2.60)      |
| run/closures              | 5.77 ms (5.74–5.82)      |
| run/json_roundtrip        | 1.67 ms (1.65–1.70)      |
| frontend/lex_big_source   | 2.66 ms (2.60–2.74)      |
| frontend/parse_big_source | 5.33 ms (5.27–5.39)      |

Criterion also persists these as the saved baseline in `target/criterion/`,
so subsequent `cargo bench` runs report % change automatically.

## Observations

- Call-heavy workloads (fib, closures, class_methods) carry the largest
  overhead: every call allocates an environment (`Rc<RefCell<Environment>>` +
  `HashMap`) and eagerly builds an `arguments` object.
- Parsing is a significant share of end-to-end time: `parse_big_source`
  (5.3 ms) exceeds `lex_big_source` (2.7 ms) ~2:1, and `bench.js` re-parses on
  every `runCode` iteration.
- Operators are stored as `String`s in the AST and string-matched on every
  evaluation; control-flow signals (`break`/`continue`) allocate `String`s.

## Results after optimization (commit `7f69614`)

Same machine and methodology, after four optimization phases (each a
revertable commit: `760c65e` release profile + lazy `arguments`, `6d66c3a`
typed control flow + operator enums, `6eddcdf` dedicated JSON parser +
cheaper string concat, `7f69614` hybrid small-vec/hash-map env frames).
Test suite: **535 pass / 0 fail**.

### End-to-end through NAPI (`npm run bench`)

| workload        | before   | after      | delta | ratio before | ratio after |
|-----------------|----------|------------|-------|--------------|-------------|
| arithmetic_loop | 3.69 ms  | 2.86 ms    | −22%  | 736x         | 474x        |
| recursion_fib   | 20.06 ms | 8.81 ms    | −56%  | 196x         | 69x         |
| array_chain     | 2.11 ms  | 1.04 ms    | −51%  | 130x         | 49x         |
| string_ops      | 1.13 ms  | 1.06 ms    | −6%   | 16x          | 13x         |
| class_methods   | 2.48 ms  | 1.63 ms    | −34%  | 62x          | 31x         |
| closures        | 5.65 ms  | 3.09 ms    | −45%  | 173x         | 83x         |
| json_roundtrip  | 1.58 ms  | 671.14 µs  | −58%  | 10x          | 3x          |

### Criterion microbenchmarks (`npm run bench:rust`)

| benchmark                 | before                | after                 | delta |
|---------------------------|-----------------------|-----------------------|-------|
| run/arithmetic_loop       | 3.78 ms               | 3.95 ms               | +4% (noise; lex canary +1%) |
| run/recursion_fib         | 18.38 ms              | 8.56 ms               | −53%  |
| run/array_chain           | 2.02 ms               | 1.08 ms               | −47%  |
| run/string_ops            | 1.11 ms               | 1.04 ms               | −6%   |
| run/class_methods         | 2.55 ms               | 1.53 ms               | −40%  |
| run/closures              | 5.77 ms               | 3.72 ms               | −36%  |
| run/json_roundtrip        | 1.67 ms               | 592.74 µs             | −65%  |
| frontend/lex_big_source   | 2.66 ms               | 2.69 ms               | +1%   |
| frontend/parse_big_source | 5.33 ms               | 5.19 ms               | −3%   |

Run-to-run drift on this machine is ±6–8% on the JS bench (Criterion is
tighter); `frontend/lex_big_source` exercises untouched code and serves as a
noise canary. Levers evaluated and deferred as not worth their invasiveness
for this workload profile: O(1) property maps for objects (benchmark objects
have ≤6 props; the linear scan is cache-friendly and a per-object HashMap
would tax creation-heavy code), string-literal interning (`Value::String`
to `Rc<str>`), and RefCell-traffic reduction.

## Results after borrowed-key lookups + hoist skip (uncommitted worktree)

Same machine and methodology (`npm run bench`, Node v26.8.2, Linux x64).
Changes, all behavior-preserving: `&str`-keyed property reads/writes end to
end (`prop_str`, `get_prop_value_str`, `assign_member_str`, static-member fast
paths in `eval_expr`, internal `"prototype"`/`"length"`/`"name"`/`"next"` call
sites), per-call hoist skip via a `needs_hoisting` flag computed once at
function creation, allocation-free `array_index`, ASCII fast paths for string
length/char access, and removal of double-clones on property reads plus
upfront receiver clones in prototype-chain walks. Test suites:
`cargo test --release` **298 pass / 0 fail** (incl. 3 new `array_index`/string
unit tests), `bun test` **1497 pass / 1 environmental fail** (rdev
`t.skip()` unsupported under Bun, pre-existing), `node --test` **17/17**.

### End-to-end through NAPI (`npm run bench`)

| workload        | before   | after    | delta |
|-----------------|----------|----------|-------|
| arithmetic_loop | 3.64 ms  | 3.66 ms  | +1% (control path untouched; drift) |
| recursion_fib   | 10.93 ms | 10.65 ms | −3%   |
| array_chain     | 1.73 ms  | 1.60 ms  | −8%   |
| string_ops      | 1.57 ms  | 1.44 ms  | −8%   |
| class_methods   | 3.53 ms  | 3.09 ms  | −12%  |
| closures        | 4.86 ms  | 4.58 ms  | −6%   |
| json_roundtrip  | 1.25 ms  | 1.17 ms  | −6%   |

Cross-checked with an interleaved old/new probe (eval-only, setup excluded,
2×2 runs against HEAD): array −9%, string −6%, class −8%, json −3%, fib −2%,
closures −1%, prop-read micro −21%, method-call micro −13% (untouched
arithmetic/parse controls read +0.4%/±1% in the well-balanced round,
confirming the gains exceed drift). Micro win
breakdown: static member key alloc elimination (class/method workloads),
hoist-walk skip (call-heavy workloads), receiver/deref clone removal
(member-heavy workloads).

Deferred again as not worth it: sharing/caching the builtins realm across
`runCode` calls (breaks fresh-realm isolation — guest `Array.prototype`
mutation would leak), `Rc<str>` string interning, prototype/inline caches
(invalidation surface), and frontend work (parse is 4–12 µs of millisecond
workloads).

## Results after runtime-upgrade merge + lazy shapes (commit `de472ce`)

Same machine and methodology (Node v26.8.2, Linux x64, Rust release
profile). Before = `f6a1f57` (phase H tip: bytecode VM + heap/GC, without
the remote hot-path opts); after = `e8d025a` (merge of phases A–J +
cross-cutting with remote `006132b` borrowed-key lookups + hoist skip)
plus `de472ce` (two-strike lazy shape assignment, keeping object creation
cheap). The delta therefore measures phases I (shapes/slots/inline caches),
J (tier-up seam), cross-cutting (RuntimeBuilder/observability), and the
remote hot-path opts combined. Phases A–H are roughly perf-neutral on these
workloads: the before-column below matches the previous section's
before-column within run-to-run drift.
Test suites: `cargo test --lib` **325 pass / 0 fail**, `--features
node-api-host` **358 pass / 0 fail**, `node --test` **17/17**, `bun test`
1446 pass / 15 pre-existing environmental fails (browser-build artifacts,
rdev-node setup, timing-sensitive promise ordering — verified identical
with and without `de472ce` via stash).

### End-to-end through NAPI (`npm run bench`)

| workload        | before   | after    | delta |
|-----------------|----------|----------|-------|
| arithmetic_loop | 3.97 ms  | 3.83 ms  | −4%   |
| recursion_fib   | 10.96 ms | 10.60 ms | −3%   |
| array_chain     | 1.73 ms  | 1.63 ms  | −6%   |
| string_ops      | 1.57 ms  | 1.53 ms  | −3%   |
| class_methods   | 3.53 ms  | 3.18 ms  | −10%  |
| closures        | 4.98 ms  | 4.67 ms  | −6%   |
| json_roundtrip  | 1.26 ms  | 1.26 ms  | flat  |

### Criterion microbenchmarks (paired back-to-back runs)

Full-suite runs on this box showed load/order artifacts (late-running
benches inflated when the machine was busy), so each bench below was run
filtered and back-to-back on both trees, alternating before/after; noisy
cases were re-run until confidence intervals stabilized.
`frontend/lex_big_source` exercises untouched code and serves as the noise
canary (+3% ≈ drift).

| benchmark                   | before   | after    | delta                    |
|-----------------------------|----------|----------|--------------------------|
| run/arithmetic_loop         | 3.91 ms  | 3.91 ms  | flat                     |
| run/recursion_fib           | —        | —        | noisy, no claim (320–655 µs run-to-run on the same tree; bench.js fib −3% is the stable read) |
| run/array_chain             | 1.80 ms  | 1.61 ms  | −11%                     |
| run/string_ops              | 1.46 ms  | 1.42 ms  | −3% (~flat)              |
| run/closures                | 5.69 ms  | 4.84 ms  | −15%                     |
| run/json_roundtrip          | 1.21 ms  | 1.24 ms  | +2% (flat)               |
| frontend/lex_big_source     | 3.71 ms  | 3.81 ms  | +3% (canary ≈ drift)     |
| frontend/parse_big_source   | 6.77 ms  | 6.73 ms  | flat                     |
| warm/class_methods_reused_vm| 3.03 ms  | 2.73 ms  | −10%                     |
| plugin_call/legacy_wrapper_eval | 164 µs | 160 µs  | flat                     |
| plugin_call/direct_call_json| 57 µs   | 56 µs    | flat                     |

### New `tiers` steady-state suite (after only, first baselines)

| benchmark          | time (estimate) |
|--------------------|-----------------|
| tiers/prop_mono_hot| 1.12 ms         |
| tiers/prop_mega_hot| 2.03 ms         |
| tiers/call_tiny_hot| 2.07 ms         |
| tiers/shape_churn  | 1.08 ms         |
| tiers/gc_collect   | 877 µs          |

Headline: property/call-heavy paths −10..−15% (shapes + inline caches +
borrowed-key lookups); everything else flat; no regressions. Gains from the
remote opts vs phase I were not bisected further — the two landed together
in the merge and both target the same member-access paths.
