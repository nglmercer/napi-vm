#!/usr/bin/env python3
"""Test262 corpus runner: isolated process per variant, explicit corpus revision."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import hashlib
import shutil
from tempfile import TemporaryDirectory
import re
import subprocess
from pathlib import Path
import yaml


def metadata(source):
    match = re.search(r"/\*---(.*?)---\*/", source, re.S)
    if not match:
        raise ValueError("missing Test262 metadata")
    data = yaml.safe_load(match.group(1)) or {}
    if not isinstance(data, dict):
        raise ValueError("metadata must be a mapping")
    return data


def variants(data):
    flags = data.get("flags", [])
    if "module" in flags:
        return [("module", False)]
    if "raw" in flags or "noStrict" in flags:
        return [("script", False)]
    if "onlyStrict" in flags:
        return [("strict", True)]
    return [("script", False), ("strict", True)]


def outcome(report, data):
    if report.get("phase") in ("driver", "harness"):
        return "harness_error"
    negative = data.get("negative")
    if negative:
        if report.get("status") != "error":
            return "fail"
        return "pass" if (report.get("phase") == negative["phase"] and
                          report.get("error_type") == negative["type"]) else "fail"
    return "pass" if report.get("status") == "ok" else "fail"


def safe_harness(root, name):
    path = (root / "harness" / name).resolve()
    if not path.is_relative_to((root / "harness").resolve()):
        raise ValueError("harness include escapes corpus")
    return path.read_text(encoding="utf-8")


def run_variant(engine, root, test, source, data, mode, strict, timeout):
    flags = data.get("flags", [])
    harness = [] if "raw" in flags else ["sta.js", "assert.js"]
    for include in data.get("includes", []):
        if include not in harness:
            harness.append(include)
    # The engine provides $DONE directly; doneprintHandle.js relies on print.
    harness = [name for name in harness if name != "doneprintHandle.js"]
    modules = {}
    request = {"source": ('"use strict";\n' if strict else "") + source,
               "harness": "\n".join(safe_harness(root, name) for name in harness),
               "module": mode == "module", "asynchronous": "async" in flags,
               "modules": modules, "id": test.relative_to(root / "test").as_posix(),
               "corpus_root": str(root / "test")}
    try:
        process = subprocess.run([str(engine)], input=json.dumps(request), text=True,
                                 capture_output=True, timeout=timeout, check=False)
    except subprocess.TimeoutExpired:
        return {"status": "timeout"}
    if process.returncode:
        return {"status": "crash", "exit_code": process.returncode,
                "message": process.stderr[-4000:]}
    try:
        report = json.loads(process.stdout)
    except json.JSONDecodeError:
        return {"status": "harness_error", "message": "worker returned invalid JSON"}
    return {"status": outcome(report, data), "engine": report}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("--engine", required=True, type=Path)
    parser.add_argument("--revision", required=True, help="exact checked-out Test262 commit SHA")
    parser.add_argument("--path", action="append", default=[], help="test-relative subset path (repeatable)")
    parser.add_argument("--skip-feature", action="append", default=[])
    parser.add_argument("--jobs", type=int, default=1, help="isolated worker processes in parallel")
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument("--output", type=Path, default=Path("test262-results.json"))
    args = parser.parse_args()
    if args.jobs <= 0:
        parser.error("jobs must be positive")
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    root = args.corpus.resolve()
    revision = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    if revision != args.revision:
        parser.error(f"corpus revision mismatch: {revision}")
    dirty = subprocess.check_output(["git", "-C", str(root), "status", "--porcelain", "--untracked-files=all"], text=True)
    if dirty:
        parser.error("corpus has modified tracked files")
    selections = args.path or ["."]
    tests = set()
    for selection in selections:
        selected = (root / "test" / selection).resolve()
        if not selected.is_relative_to(root / "test"):
            parser.error("selection escapes test directory")
        tests.update([selected] if selected.is_file() else selected.rglob("*.js"))
    def run_test(test):
        rows = []
        name = test.relative_to(root / "test").as_posix()
        try:
            source = test.read_text(encoding="utf-8")
            data = metadata(source)
            for mode, strict in variants(data):
                skipped = sorted(set(data.get("features", [])) & set(args.skip_feature))
                result = {"status": "skip", "features": skipped} if skipped else run_variant(
                    engine, root, test, source, data, mode, strict, args.timeout)
                rows.append({"test": name, "variant": mode, **result})
        except (ValueError, OSError, yaml.YAMLError, KeyError) as error:
            rows.append({"test": name, "variant": "metadata", "status": "harness_error", "message": str(error)})
        return rows

    results = []
    selected_tests = [test for test in sorted(tests) if not test.name.endswith("_FIXTURE.js")]
    # A concurrent cargo build must not change the worker halfway through a run.
    with TemporaryDirectory(prefix="napi-vm-test262-") as temporary:
        engine = Path(temporary) / "worker"
        shutil.copy2(args.engine.resolve(), engine)
        engine_sha256 = hashlib.sha256(engine.read_bytes()).hexdigest()
        with ThreadPoolExecutor(max_workers=args.jobs) as pool:
            for index, rows in enumerate(pool.map(run_test, selected_tests), 1):
                results.extend(rows)
                if index % 1000 == 0:
                    print(f"Test262: {index}/{len(selected_tests)} files", flush=True)
    counts = {status: sum(r["status"] == status for r in results)
              for status in ("pass", "fail", "skip", "timeout", "crash", "harness_error")}
    report = {"domain": "ECMAScript", "suite": "Test262", "revision": revision,
              "selection": selections, "engine_sha256": engine_sha256, "skip_features": args.skip_feature,
              "denominator": "all selected variants, including skips and errors",
              "worker_jobs": args.jobs, "timeout_seconds": args.timeout,
              "total": len(results), "counts": counts,
              "pass_percentage": 100 * counts["pass"] / len(results) if results else None,
              "limitations": ["Agents are not implemented; GC requests run at quiescent host boundaries"],
              "results": results}
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: value for key, value in report.items() if key != "results"}, indent=2))
    return 0 if results and counts["pass"] == len(results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
