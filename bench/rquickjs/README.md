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

## Understand the modes

| Mode | Timed work |
| --- | --- |
| `fresh_source` | Unique-comment source, new VM/context and builtins, evaluation, destruction, and napi-vm cycle collection |
| `warm_source` | Public source evaluation on reused VMs; napi-vm caches source/preparation, rquickjs reparses the invocation |
| `warm_call` | Retained guest-function call without setup/lookup/parsing; napi-vm includes an empty job checkpoint |

The ten workloads use the same sloppy synchronous scripts and expected results.
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
