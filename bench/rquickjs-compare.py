#!/usr/bin/env python3
"""Validate comparison reports and print engine ratios or before/after changes."""
import argparse
import json
import math
from pathlib import Path
import statistics


MODES = ("fresh_source", "warm_source", "warm_call")
ENGINES = ("napi-vm", "rquickjs")


def finite_number(value):
    return (not isinstance(value, bool) and isinstance(value, (int, float))
            and math.isfinite(value))


def read(path):
    report = json.loads(path.read_text())
    if report.get("schema") != 1 or not report.get("results"):
        raise ValueError(f"{path}: missing comparison results")
    for field, minimum in (("samples", 1), ("iterations_per_sample", 1), ("warmup_calls", 0)):
        value = report.get(field)
        if type(value) is not int or value < minimum or (field == "samples" and value % 2 != 1):
            raise ValueError(f"{path}: invalid setting {field}")
    workloads = {}
    for workload in report.get("workloads", []):
        name = workload["name"]
        if name in workloads or not workload.get("source_sha256"):
            raise ValueError(f"{path}: duplicate or invalid workload {name}")
        workloads[name] = workload["source_sha256"]
    if not workloads:
        raise ValueError(f"{path}: missing workload definitions")
    rows = {}
    for row in report["results"]:
        key = tuple(row[k] for k in ("workload", "mode", "engine"))
        samples = row["samples"]
        if key in rows or row["unit"] != "ns/call" or len(samples) != report["samples"]:
            raise ValueError(f"{path}: duplicate or invalid row {key}")
        if any(not finite_number(v) or v <= 0 for v in samples):
            raise ValueError(f"{path}: invalid timing {key}")
        median = statistics.median(samples)
        if not finite_number(row["median"]) or not math.isclose(median, row["median"], rel_tol=1e-12):
            raise ValueError(f"{path}: median disagrees with samples {key}")
        mad = statistics.median(abs(sample - median) for sample in samples)
        if not finite_number(row["mad"]) or not math.isclose(mad, row["mad"], rel_tol=1e-12):
            raise ValueError(f"{path}: MAD disagrees with samples {key}")
        if row["source_sha256"] != workloads.get(key[0]):
            raise ValueError(f"{path}: row source disagrees with workload {key}")
        rows[key] = row
    expected = {(name, mode, engine) for name in workloads for mode in MODES for engine in ENGINES}
    if rows.keys() != expected:
        raise ValueError(f"{path}: incomplete or unknown workload/mode/engine rows")
    return report, rows


def engines(path):
    report, rows = read(path)
    result = []
    for (name, mode, engine), napi in sorted(rows.items()):
        if engine != "napi-vm":
            continue
        quick = rows[(name, mode, "rquickjs")]
        if napi["source_sha256"] != quick["source_sha256"]:
            raise ValueError(f"mismatched source: {name}/{mode}")
        result.append({"workload": name, "mode": mode, "unit": "ns/call",
                       "napi_vm_median": napi["median"], "rquickjs_median": quick["median"],
                       "napi_vm_over_rquickjs": napi["median"] / quick["median"]})
    return {"schema": 1, "rquickjs_version": report["rquickjs_version"], "results": result}


def changes(before, after):
    old, left = read(before)
    new, right = read(after)
    for key in ("features", "rquickjs_version", "samples", "iterations_per_sample", "warmup_calls"):
        if old[key] != new[key]:
            raise ValueError(f"incompatible settings: {key}")
    for key in ("platform", "cpu", "machine"):
        if old["metadata"][key] != new["metadata"][key]:
            raise ValueError(f"incompatible host: {key}")
    for key in ("rustc", "profile"):
        if old["metadata"]["build"][key] != new["metadata"]["build"][key]:
            raise ValueError(f"incompatible build: {key}")
    builds = [report["metadata"]["build"] for report in (old, new)]
    provenance_warnings = []
    dependencies = [build.get("comparison_dependencies") for build in builds]
    if all(dependencies):
        if dependencies[0] != dependencies[1]:
            raise ValueError("incompatible build: comparison_dependencies")
    else:
        provenance_warnings.append("Legacy dependency metadata is incomplete; dependency identity uses the matching Cargo.lock digest.")
    # Workload hashes alone cannot detect changes to the timed region, GC or
    # engine settings. Keep the standalone harness, its dependency lock and Cargo
    # configuration fixed. The Python wrapper may change metadata collection.
    harness_inputs = [{path: digest for path, digest in build["inputs"]["files"].items()
                       if path.startswith("bench/rquickjs/")}
                      for build in builds]
    required_inputs = {"bench/rquickjs/Cargo.toml", "bench/rquickjs/Cargo.lock",
                       "bench/rquickjs/src/main.rs"}
    if not required_inputs <= harness_inputs[0].keys() or harness_inputs[0] != harness_inputs[1]:
        raise ValueError("incompatible build: benchmark harness inputs")
    configs = [build["inputs"]["files"].get(".cargo/config.toml") for build in builds]
    if all(configs):
        if configs[0] != configs[1]:
            raise ValueError("incompatible build: Cargo configuration")
    else:
        provenance_warnings.append("Legacy Cargo configuration fingerprints are incomplete; configuration identity is unverified.")
    environments = [report["metadata"]["build"]["build_environment"] for report in (old, new)]
    # Cargo test runners do not affect code generation or the direct executable
    # invocation used here. Preserve their metadata difference, without treating
    # a newly recorded test-runner setting as a compiler configuration change.
    if ({k: v for k, v in environments[0].items() if not k.endswith("_RUNNER")} !=
            {k: v for k, v in environments[1].items() if not k.endswith("_RUNNER")}):
        raise ValueError("incompatible build: build_environment")
    result = []
    for key in sorted(left.keys() & right.keys()):
        a, b = left[key], right[key]
        if a["source_sha256"] != b["source_sha256"]:
            raise ValueError(f"changed workload source: {key}")
        result.append({"workload": key[0], "mode": key[1], "engine": key[2], "unit": "ns/call",
                       "before_median": a["median"], "after_median": b["median"],
                       "after_over_before": b["median"] / a["median"]})
    return {"schema": 1, "removed": sorted(left.keys() - right.keys()),
            "added": sorted(right.keys() - left.keys()), "results": result,
            "provenance_warnings": provenance_warnings,
            "build_environment_differences": {k: {"before": environments[0].get(k), "after": environments[1].get(k)}
                                              for k in environments[0].keys() | environments[1].keys()
                                              if environments[0].get(k) != environments[1].get(k)},
            "warning": "Ratios are descriptive. Inspect raw samples/MAD and repeat interleaved measurements."}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="command", required=True)
    e = subcommands.add_parser("engines")
    e.add_argument("report", type=Path)
    c = subcommands.add_parser("changes")
    c.add_argument("before", type=Path)
    c.add_argument("after", type=Path)
    args = parser.parse_args()
    print(json.dumps(engines(args.report) if args.command == "engines" else changes(args.before, args.after), indent=2))


if __name__ == "__main__":
    main()
