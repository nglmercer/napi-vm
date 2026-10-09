#!/usr/bin/env python3
"""Audit source acceptance; these outcomes are not Test262 execution passes."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

from run import metadata, selected_files, variants


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus", type=Path)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--engine", type=Path, help="existing compile-only companion")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.corpus = args.corpus.resolve()
    repository = Path(__file__).resolve().parents[2]
    revision = subprocess.check_output(
        ["git", "-C", str(args.corpus), "rev-parse", "HEAD"], text=True).strip()
    if revision != args.revision:
        parser.error(f"corpus revision mismatch: {revision}")
    dirty = subprocess.check_output(["git", "-C", str(args.corpus), "status", "--porcelain", "--untracked-files=all"], text=True)
    if dirty:
        parser.error("corpus has modified files")
    source = subprocess.check_output(
        ["git", "-C", str(repository), "rev-parse", "HEAD"], text=True).strip()
    if args.engine:
        engine = args.engine.resolve()
    else:
        build = subprocess.check_output([
            "cargo", "build", "-p", "napi-vm-core", "--lib", "--message-format=json"
        ], cwd=repository, text=True)
        libraries = {}
        for line in build.splitlines():
            artifact = json.loads(line)
            if artifact.get("reason") == "compiler-artifact":
                for filename in artifact["filenames"]:
                    if filename.endswith(".rlib"):
                        libraries[artifact["target"]["name"]] = filename
        engine = repository / "artifacts/test262/engines" / f"syntax-audit-{source[:7]}"
        engine.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run([
            "rustc", "--edition=2021", str(Path(__file__).with_suffix(".rs")),
            "--extern", f"napi_vm_core={libraries['napi_vm_core']}",
            "--extern", f"serde_json={libraries['serde_json']}",
            # Cargo can emit the top-level core rlib in debug/ while its
            # dependencies live in debug/deps/. The dependency artifact's
            # parent works for both top-level and hashed core artifacts.
            "-L", f"dependency={Path(libraries['serde_json']).parent}", "-o", str(engine)
        ], check=True)
    expectations = {}
    with tempfile.TemporaryFile(mode="w+", encoding="utf-8") as requests, \
            tempfile.TemporaryFile(mode="w+", encoding="utf-8") as responses:
        for path in sorted(selected_files(args.corpus.resolve(), [], [], {})):
            if path.name.endswith("_FIXTURE.js"):
                continue  # Imported fixture files have no independent Test262 variants.
            text = path.read_text(encoding="utf-8")
            data = metadata(text)
            name = path.relative_to(args.corpus / "test").as_posix()
            for mode, strict in variants(data):
                variant = "strict" if strict else mode
                expectations[(name, variant)] = data.get("negative", {}).get("phase") != "parse"
                requests.write(json.dumps({"test": name, "variant": variant,
                    "module": mode == "module", "source": ('"use strict";\n' if strict else "") + text}) + "\n")
        requests.seek(0)
        subprocess.run([str(engine)], stdin=requests, stdout=responses, check=True)
        responses.seek(0)
        rows = []
        for line in responses:
            row = json.loads(line)
            row["expected_acceptance"] = expectations.pop((row["test"], row["variant"]))
            rows.append(row)
        if expectations:
            raise ValueError("compile-only companion omitted source variants")
    counts = {
        "parse_negative": sum(not row["expected_acceptance"] for row in rows),
        "accepted_invalid": sum(row["accepted"] and not row["expected_acceptance"] for row in rows),
        "requires_acceptance": sum(row["expected_acceptance"] for row in rows),
        "rejected_valid": sum(not row["accepted"] and row["expected_acceptance"] for row in rows),
    }
    report = {"domain": "source syntax only", "revision": revision,
        "source_commit": source, "engine_sha256": hashlib.sha256(engine.read_bytes()).hexdigest(),
        "total": len(rows), "counts": counts,
        "limitations": ["No harness or guest execution; acceptance is not a Test262 execution pass."],
        "results": rows}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: value for key, value in report.items() if key != "results"}, indent=2))
    return int(bool(counts["accepted_invalid"] or counts["rejected_valid"]))


if __name__ == "__main__":
    raise SystemExit(main())
