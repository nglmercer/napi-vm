#!/usr/bin/env python3
"""Build immutable Rust engine-comparison artifacts and record finite runs."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "bench/rquickjs/Cargo.toml"
TARGET = ROOT / "target/rquickjs-comparison"


def call(args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, text=True, **kwargs)


def output(args):
    return call(args, stdout=subprocess.PIPE).stdout.strip()


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inputs():
    files = [p for folder in [ROOT / "crates/napi-vm-core", ROOT / "bench/rquickjs"]
             for p in folder.rglob("*") if p.is_file() and "target" not in p.parts
             and p.suffix != ".md"]
    files.extend([ROOT / "bench/rquickjs.py", ROOT / ".cargo/config.toml"])
    hashes = {str(p.relative_to(ROOT)): sha(p) for p in sorted(files)}
    return {"sha256": hashlib.sha256(json.dumps(hashes, sort_keys=True).encode()).hexdigest(),
            "files": hashes}


def build(directory):
    directory.mkdir(parents=True, exist_ok=False)
    start = inputs()
    cmd = ["cargo", "build", "--locked", "--release", "--jobs", "2", "--manifest-path",
           str(MANIFEST), "--target-dir", str(TARGET)]
    with (directory / "build.log").open("w") as log:
        call(cmd, stdout=log, stderr=subprocess.STDOUT)
    if inputs() != start:
        raise RuntimeError("comparison build inputs changed during compilation")
    executable = directory / "comparison"
    shutil.copy2(TARGET / "release/napi-vm-rquickjs-bench", executable)
    shutil.copy2(TARGET / "release/memory", directory / "memory")
    workloads = output([str(executable), '--emit-workloads'])
    exported = json.loads(workloads)
    if exported.get('kind') != 'guest_workloads' or not exported.get('workloads'):
        raise RuntimeError('comparison executable did not export workloads')
    (directory / 'workloads.json').write_text(workloads+'\n')
    metadata = {"workloads_sha256": sha(directory / 'workloads.json'), "memory_executable_sha256": sha(directory / "memory"), "utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "git_head": output(["git", "rev-parse", "HEAD"]),
                "git_status": output(["git", "status", "--porcelain=v1"]),
                "inputs": start, "executable_sha256": sha(executable),
                "rustc": output(["rustc", "--version", "--verbose"]),
                "cargo": output(["cargo", "--version"]), "command": cmd,
                "comparison_dependencies": [{k: package[k] for k in ("name", "version", "checksum") if k in package}
                                            for package in tomllib.loads((MANIFEST.parent / "Cargo.lock").read_text())["package"]
                                            if package["name"].startswith("rquickjs")],
                "profile": "release: opt-level=3, thin LTO, codegen-units=1",
                "build_environment": {k: v for k, v in os.environ.items()
                                      if k in ("RUSTFLAGS", "RUSTC_WRAPPER", "RUSTUP_TOOLCHAIN", "CARGO_BUILD_TARGET",
                                               "CARGO_ENCODED_RUSTFLAGS", "CC", "AR", "CFLAGS", "CXXFLAGS")
                                      or k.startswith(("CARGO_PROFILE_", "CARGO_TARGET_"))}}
    (directory / "build.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(directory)


def run(directory, destination, samples, iterations, filter_text):
    metadata = json.loads((directory / "build.json").read_text())
    executable = directory / "comparison"
    if sha(executable) != metadata["executable_sha256"]:
        raise RuntimeError("comparison executable does not match its build metadata")
    if destination.exists():
        raise RuntimeError("output must be new")
    command = [str(executable), "--samples", str(samples), "--iterations", str(iterations),
               "--filter", filter_text]
    cpuinfo = Path("/proc/cpuinfo")
    start_load = os.getloadavg() if hasattr(os, "getloadavg") else None
    report = json.loads(output(command))
    report["metadata"] = {"build": metadata, "command": command,
                          "utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                          "platform": platform.platform(), "machine": platform.machine(),
                          "cpu": next((line.split(":", 1)[1].strip() for line in cpuinfo.read_text().splitlines()
                                       if line.startswith("model name")), "unknown") if cpuinfo.exists() else "unknown",
                          "cpu_count": os.cpu_count(), "load_average_start": start_load,
                          "load_average_end": os.getloadavg() if hasattr(os, "getloadavg") else None,
                          "limitations": ["Representative synchronous guest workloads; no Node, WASM, I/O, async or host bridge timing.",
                                          "No host isolation or enforced CPU affinity; repeat and interleave before accepting small gains.",
                                          "Fresh source includes VM construction, unique-comment source allocation, evaluation, destruction and napi-vm cycle collection.",
                                          "Warm source uses each public source-evaluation API; napi-vm caches parsed/prepared source, rquickjs evaluates source afresh.",
                                          "Warm call retains each guest function and excludes source compilation, lookup and VM construction.",
                                          "Both engines explicitly collect cycles between warm batches, outside the timed interval; fresh collection stays inside the interval.",
                                          "Synchronous bodies have no jobs; napi-vm host-call includes its empty job checkpoint, rquickjs Function::call does not drain jobs.",
                                          "Each sample is a batch average; medians/MAD are descriptive, not a significance test."]}
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(json.dumps(report, indent=2) + "\n")
    print(destination)


parser = argparse.ArgumentParser(description=__doc__)
commands = parser.add_subparsers(dest="command", required=True)
b = commands.add_parser("build")
b.add_argument("--output", type=Path, required=True)
r = commands.add_parser("run")
r.add_argument("--build", type=Path, required=True)
r.add_argument("--output", type=Path, required=True)
r.add_argument("--samples", type=int, default=15)
r.add_argument("--iterations", type=int, default=10)
r.add_argument("--filter", default="")
m = commands.add_parser("memory")
m.add_argument("--build", type=Path, required=True)
m.add_argument("--output", type=Path, required=True)
m.add_argument("--samples", type=int, default=3)
m.add_argument("--iterations", type=int, default=8)
m.add_argument("--objects", type=int, default=2000)
args = parser.parse_args()
if args.command == "build":
    build(args.output.resolve())
elif args.command == "memory":
    if args.samples <= 0 or args.samples % 2 != 1 or args.iterations <= 0 or args.objects <= 0:
        parser.error("samples must be positive and odd; iterations and objects must be positive")
    from memory_report import run_memory
    run_memory(args.build.resolve(), args.output.resolve(), args.samples, args.iterations, args.objects)
else:
    run(args.build.resolve(), args.output.resolve(), args.samples, args.iterations, args.filter)
