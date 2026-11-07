# Rust engine comparison

This standalone workspace compares `napi-vm-core` without NAPI/runtime features
with pinned rquickjs 0.14.0, its default `std` build, and `Context::full`.
Its lockfile fixes dependencies without adding QuickJS to the engine's release
dependency graph. The locked rquickjs-sys bundles QuickJS-NG 0.16.2.

## Build and run

Requires Python 3.11+, Cargo, and a native C build toolchain. From the repository
root, choose new artifact paths:

```sh
python3 bench/rquickjs.py build --output artifacts/performance/before-build
python3 bench/rquickjs.py run --build artifacts/performance/before-build \
  --output artifacts/performance/before.json --samples 15 --iterations 10
python3 bench/rquickjs-compare.py engines artifacts/performance/before.json
```

Build records hash inputs before/after compilation, retain a copied executable,
and record flags, environment, toolchains, lock/dependency identity, and Git
state. Runs verify the executable hash and retain host/load metadata, source
hashes, tiers, all batch samples, medians, and MAD. Every batch checks the numeric
outcome. Samples must be positive and odd; iterations must be positive.

Builds also retain `workloads.json`, emitted by `comparison --emit-workloads`.
This canonical export includes setup, invocation, expected results and source
hashes. `bench/platforms.py` uses it for native/WASM/Node public API comparisons;
the exported source is not independently duplicated in those adapters.

## Understand the modes

| Mode | Timed work |
| --- | --- |
| `fresh_source` | Unique-comment source, new VM/context and builtins, evaluation, destruction, and napi-vm cycle collection |
| `warm_source` | Public source evaluation on reused VMs; napi-vm caches source/preparation, rquickjs reparses the invocation |
| `warm_call` | Retained guest-function call without setup/lookup/parsing; napi-vm includes an empty job checkpoint |

The 65 workloads use the same sloppy synchronous scripts and expected results.
The original 25 cases are joined by 24 collection construction/lookup cases at
16, 64, 256 and 1,024 entries. Numeric, string and object keys are distinct
fixtures. Population precedes warm timers and remains inside fresh-source
timing. Sixteen additional cases exercise generators/iterators, proxy/Reflect,
Date, BigInt division, string replacement/normalization, weak collections,
DataView, bulk typed-array copies, buffer slices and sparse arrays.
Each function returns a checked numeric result.
Both engines collect cycles outside warm batches; fresh collection remains
inside the timer. Default builtin sets differ, and outer tier labels do not
assert nested-callee tiers. These boundaries are not one universal engine score.

## Compare an optimization

Build a separate after executable and alternate retained before/after runs:

```sh
python3 bench/rquickjs.py run --build artifacts/performance/before-build \
  --output artifacts/performance/property-before-1.json --filter property
python3 bench/rquickjs-compare.py changes BEFORE.json AFTER.json
python3 -B -m unittest discover -s bench/tests -v
```

Use actual report paths for `BEFORE.json` and `AFTER.json`. The comparator rejects
incomplete rows, inconsistent statistics, and mismatched timed harness/settings
or mutually recorded dependency/config fingerprints. Legacy missing fingerprints
remain explicit warnings; they must not be filled by assumption.

Use a quiet host and repeated/interleaved comparisons. No enforced CPU affinity
or isolation is provided. Samples are batch averages; medians/MAD are descriptive,
not formal significance tests. Node/WASM bridges, I/O, async scheduling, language
completeness, and security are outside this finite synchronous comparison.
See [performance](../../docs/performance.md) for retained results and other layers.

The operation matrix now includes bitwise operations, array indexing and sorting,
Map, Set, typed arrays, regular expressions, Unicode strings, prototype getters,
key churn/deletion, exceptions, spread/destructuring, Math, BigInt, and cyclic
allocations alongside the original ten cases. A full run produces 390 timing
rows. Cyclic allocation counts created objects; warm cycle collection occurs
outside the timer, so this case is not a GC throughput benchmark.

## Memory and collection probes (Linux)

New builds also retain a `memory` executable. Each engine/workload runs in a
separate process; three independent repetitions are the default:

```sh
python3 bench/rquickjs.py memory --build artifacts/performance/before-build \
  --output artifacts/performance/memory.json --samples 3 --iterations 8 --objects 2000
python3 bench/memory_report.py BEFORE.json AFTER.json
```

Use a build made after adding the memory probe. Four workloads cover ordinary
objects, cyclic garbage, unique-key/read churn, and guest-retained cycles. Raw
phases distinguish engine setup, allocations, explicit GC, guest-root release,
and destruction. RSS includes allocator caches and mapped code/data; it is not
an exact live-object count. Kernel VmHWM can fluctuate, so reports preserve raw
readings and summarize their observed maximum. Engine-specific counters use
different definitions. See the measurement guide for captures and limitations.


## Intl and Temporal

`bench/international.py` builds the separate `international` worker using the
same pinned dependencies and profile. It exports shared fixtures for Node's
context adapter and checks both API presence and numeric outcomes. The complete
27-case matrix retains unsupported and incompatible-result rows without timing
samples. In particular, the configured rquickjs build lacks these Intl/Temporal
APIs; this is not an infinite speed ratio. See the measurement guide for the
DisplayNames fallback mismatch, timing boundaries and current comparisons.
