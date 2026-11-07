#!/usr/bin/env bash
# Compare the reviewed revision with the current checkout using separate builds.
set -euo pipefail
repo_dir="$(git rev-parse --show-toplevel)"
reviewed_revision=cf9d240a99747e29d94b970c30483d14ad831e9f
baseline_dir="${SCHEDULER_BASELINE_DIR:-${TMPDIR:-/tmp}/napi-vm-benchmark-$reviewed_revision}"
output_dir="${SCHEDULER_OUTPUT_DIR:-$repo_dir/artifacts/scheduler}"
mkdir -p "$output_dir" "$baseline_dir"
if [[ ! -f "$baseline_dir/Cargo.toml" ]]; then
  git -C "$repo_dir" archive "$reviewed_revision" | tar -x -C "$baseline_dir"
fi
cp "$repo_dir/examples/scheduler_bench.rs" "$baseline_dir/examples/scheduler_bench.rs"
cp "$repo_dir/bench/scheduler-async.cjs" "$baseline_dir/bench/scheduler-async.cjs"
# Compile everything before timing. Never benchmark competing Cargo builds.
(cd "$baseline_dir" && cargo build --locked --release --lib --example scheduler_bench) > "$output_dir/baseline-build.log" 2>&1
(cd "$repo_dir" && cargo build --locked --release --features runtime --lib --example scheduler_bench --example scheduler_metrics) > "$output_dir/modified-build.log" 2>&1
cp "$baseline_dir/target/release/libnapi_vm.so" "$baseline_dir/napi-vm.linux-x64-gnu.node"
cp "$repo_dir/target/release/libnapi_vm.so" "$repo_dir/napi-vm.linux-x64-gnu.node"
"$baseline_dir/target/release/examples/scheduler_bench" > "$output_dir/baseline-rust.jsonl"
"$repo_dir/target/release/examples/scheduler_bench" > "$output_dir/modified-rust.jsonl"
(cd "$baseline_dir" && node bench/scheduler-async.cjs) > "$output_dir/baseline-node.jsonl"
(cd "$repo_dir" && node bench/scheduler-async.cjs) > "$output_dir/modified-node-legacy.jsonl"
(cd "$repo_dir" && node bench/scheduler-async.cjs persistent) > "$output_dir/modified-node-persistent.jsonl"
"$repo_dir/target/release/examples/scheduler_metrics" > "$output_dir/extended-metrics.jsonl"
{
  git -C "$repo_dir" rev-parse HEAD
  node --version
  rustc --version
  uname -sm
  nproc
} > "$output_dir/machine.txt"
printf 'Benchmark JSONL and build logs: %s\n' "$output_dir"
