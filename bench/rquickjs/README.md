# Rust engine comparison

This standalone workspace pins `rquickjs = 0.14.0` and its lockfile, without
adding it to the engine's dependency graph. It compares `napi-vm-core` without
NAPI/runtime features against rquickjs's default `std` build with `Context::full`.
The pinned `rquickjs-sys` bundles QuickJS-NG 0.16.2; see
[rquickjs](https://github.com/DelSkayn/rquickjs) and its locked source checksum.
The runner requires Python 3.11+, Cargo and a native C build toolchain.

```sh
python3 bench/rquickjs.py build --output artifacts/performance/comparison-before
python3 bench/rquickjs.py run --build artifacts/performance/comparison-before \
  --output artifacts/performance/comparison-before.json
python3 bench/rquickjs-compare.py engines artifacts/performance/comparison-before.json
```

Build directories and report paths must be new. The build command saves an
immutable executable, hashes the input files before and after compilation,
and records the toolchain, lockfile dependency checksums, build settings and
Git state. The run command verifies that executable's hash and records host
details, source hashes, observed outer execution tiers, all timing samples,
median and median absolute deviation. Nested functions can independently
fall back to the AST evaluator; outer tier labels do not assert their tiers.
Every batch checks its accumulated numeric result against the expected value.

The ten workloads cover small startup-dominated source, arithmetic loops,
recursion, array callbacks, string operations, closures, JSON, property reads,
property updates and classes. Modes measure different boundaries:

| Mode | Timed work |
| --- | --- |
| `fresh_source` | Allocate unique-comment source, create VM/builtins, evaluate source, destroy VM and collect napi-vm cycles. Unique comments bypass the process-wide parse cache. |
| `warm_source` | Evaluate `bench();` on reused VMs through each public source-evaluation API. napi-vm uses its source caches; rquickjs reparses the invocation. |
| `warm_call` | Invoke the retained guest function without parsing, source setup, function lookup or VM construction. napi-vm includes an empty job checkpoint. |

Both engines use sloppy scripts. Workloads are synchronous and create no
pending jobs. Each warm mode gets five correctness-checked warmup calls;
between warm samples both engines collect cycles outside the timer. Samples
alternate engine order. Each sample is an average of ten calls by default,
with fifteen repeated batches per mode. Fresh builtin/template caches can
warm up, even though the source parser cache is bypassed.
Their default builtin sets differ; fresh startup includes each engine's full
default builtin population, including napi-vm's Intl and Temporal
builtins. It does not isolate parser or execution speed.

After editing the engine, build into another new directory and alternate
runs of the retained before/after executables. Compare matching workloads,
mode, source hash, features and settings. For targeted repetitions:

```sh
python3 bench/rquickjs.py run --build artifacts/performance/comparison-before \
  --output artifacts/performance/properties-before-1.json --filter property
python3 bench/rquickjs-compare.py changes artifacts/performance/comparison-before.json \
  artifacts/performance/comparison-after.json
```

Use a quiet host without competing builds or tests. These finite, synchronous
Rust workloads do not measure Node bindings, WASM, I/O, async scheduling,
security, language completeness, or every engine path. A ratio is specific
to its workload and timing boundary; medians/MAD alone are not a statistical
significance test.
