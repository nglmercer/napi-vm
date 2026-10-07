#!/usr/bin/env python3
"""Test262 corpus runner: isolated process per variant, explicit corpus revision."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import hashlib
import os
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
               "can_block": "CanBlockIsFalse" not in flags,
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


def selected_files(root, paths, groups, catalog):
    """Union explicit focused selections; errors never become silent omissions."""
    tests = set()
    selections = [(path, None) for path in paths]
    for group in groups:
        if group not in catalog:
            raise ValueError(f"unknown focused group: {group}")
        entry = catalog[group]
        pattern = re.compile(entry["pattern"]) if "pattern" in entry else None
        selections.extend((path, pattern) for path in entry["paths"])
    if not selections:
        selections = [(".", None)]
    for selection, pattern in selections:
        selected = (root / "test" / selection).resolve()
        if not selected.is_relative_to(root / "test"):
            raise ValueError("selection escapes test directory")
        if not selected.exists():
            raise ValueError(f"selection does not exist: {selection}")
        files = [selected] if selected.is_file() else selected.rglob("*.js")
        for test in files:
            if pattern is None or pattern.search(test.relative_to(root / "test").as_posix()):
                tests.add(test)
    return tests


def resume_checkpoint(path, configuration, names):
    """Restore a verified ordered prefix; an interrupted final write is rerun."""
    contents = path.read_bytes()
    end = contents.rfind(b'\n') + 1
    lines = contents[:end].splitlines()
    if not lines or json.loads(lines[0]) != configuration:
        raise ValueError('checkpoint configuration or worker digest mismatch')
    results = []
    for index, line in enumerate(lines[1:]):
        record = json.loads(line)
        if index >= len(names) or record['test'] != names[index]:
            raise ValueError('checkpoint is not an ordered selection prefix')
        rows = record['rows']
        if not rows or len({row['variant'] for row in rows}) != len(rows):
            raise ValueError('checkpoint contains empty or duplicate variants')
        for row in rows:
            if row['test'] != names[index] or row['status'] not in (
                    'pass', 'fail', 'skip', 'timeout', 'crash', 'harness_error'):
                raise ValueError('invalid checkpoint outcome')
        results.extend(rows)
    if end != len(contents):
        with path.open('r+b') as checkpoint:
            checkpoint.truncate(end)
    return len(lines) - 1, results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("--engine", required=True, type=Path)
    parser.add_argument("--revision", required=True, help="exact checked-out Test262 commit SHA")
    parser.add_argument("--path", action="append", default=[], help="test-relative subset path (repeatable)")
    parser.add_argument("--group", action="append", default=[], help="named focused group from groups.json (repeatable)")
    parser.add_argument("--skip-feature", action="append", default=[])
    parser.add_argument("--jobs", type=int, default=1, help="isolated worker processes in parallel")
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument("--output", type=Path, default=Path("test262-results.json"))
    parser.add_argument("--checkpoint", type=Path, help="durable per-file outcome journal")
    parser.add_argument("--resume", action="store_true", help="resume a matching checkpoint")
    args = parser.parse_args()
    if args.jobs <= 0:
        parser.error("jobs must be positive")
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    if args.resume and not args.checkpoint:
        parser.error("--resume requires --checkpoint")
    root = args.corpus.resolve()
    revision = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    if revision != args.revision:
        parser.error(f"corpus revision mismatch: {revision}")
    dirty = subprocess.check_output(["git", "-C", str(root), "status", "--porcelain", "--untracked-files=all"], text=True)
    if dirty:
        parser.error("corpus has modified tracked files")
    catalog = json.loads(Path(__file__).with_name("groups.json").read_text())
    try:
        tests = selected_files(root, args.path, args.group, catalog)
    except (ValueError, re.error) as error:
        parser.error(str(error))
    selections = args.path or (["."] if not args.group else [])
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
        names = [test.relative_to(root / 'test').as_posix() for test in selected_tests]
        configuration = {'revision': revision, 'engine_sha256': engine_sha256,
                         'worker_jobs': args.jobs, 'timeout_seconds': args.timeout,
                         'skip_features': args.skip_feature, 'selection': selections,
                         'groups': args.group,
                         'selection_sha256': hashlib.sha256(json.dumps(names).encode()).hexdigest()}
        resumed_files = 0
        if args.resume:
            try:
                resumed_files, results = resume_checkpoint(args.checkpoint, configuration, names)
            except (OSError, ValueError, KeyError, TypeError) as error:
                parser.error(str(error))
            print(f'Test262: restored {resumed_files}/{len(selected_tests)} files', flush=True)
        checkpoint = None
        if args.checkpoint:
            args.checkpoint.parent.mkdir(parents=True, exist_ok=True)
            checkpoint = args.checkpoint.open('a' if args.resume else 'w', encoding='utf-8')
            if not args.resume:
                checkpoint.write(json.dumps(configuration) + '\n')
                checkpoint.flush()
                os.fsync(checkpoint.fileno())
        try:
            with ThreadPoolExecutor(max_workers=args.jobs) as pool:
                for index, rows in enumerate(pool.map(run_test, selected_tests[resumed_files:]), resumed_files + 1):
                    results.extend(rows)
                    if checkpoint:
                        checkpoint.write(json.dumps({'test': names[index - 1], 'rows': rows}) + '\n')
                        checkpoint.flush()
                    if index % 1000 == 0:
                        if checkpoint:
                            os.fsync(checkpoint.fileno())
                        print(f"Test262: {index}/{len(selected_tests)} files", flush=True)
        finally:
            if checkpoint:
                checkpoint.flush()
                os.fsync(checkpoint.fileno())
                checkpoint.close()
    counts = {status: sum(r["status"] == status for r in results)
              for status in ("pass", "fail", "skip", "timeout", "crash", "harness_error")}
    report = {"domain": "ECMAScript", "suite": "Test262", "revision": revision,
              "selection": selections, "groups": args.group, "engine_sha256": engine_sha256, "skip_features": args.skip_feature,
              "denominator": "all selected variants, including skips and errors",
              "worker_jobs": args.jobs, "timeout_seconds": args.timeout,
              "resumed_files": resumed_files,
              "total": len(results), "counts": counts,
              "pass_percentage": 100 * counts["pass"] / len(results) if results else None,
              "limitations": ["Host capabilities depend on the selected worker; GC requests run at quiescent host boundaries"],
              "results": results}
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: value for key, value in report.items() if key != "results"}, indent=2))
    return 0 if results and counts["pass"] == len(results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
