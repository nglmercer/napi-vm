"""Regression checks for accepting only compatible, complete timing reports."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


BENCH = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("rquickjs_compare", BENCH / "rquickjs-compare.py")
COMPARE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(COMPARE)
EVIDENCE = BENCH / "baselines/performance-review-2026-11-06"


class ComparisonTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.before = EVIDENCE / "rquickjs-before.json"
        self.after = EVIDENCE / "rquickjs-after.json"
        self.report = json.loads(self.after.read_text())

    def save(self, report):
        path = Path(self.directory.name) / "report.json"
        path.write_text(json.dumps(report))
        return path

    def test_retained_reports_and_ratios(self):
        for path in (self.before, self.after):
            self.assertEqual(len(COMPARE.read(path)[1]), 60)
            self.assertEqual(len(COMPARE.engines(path)["results"]), 30)
        actual = COMPARE.changes(self.before, self.after)
        retained = json.loads((EVIDENCE / "rquickjs-changes.json").read_text())
        self.assertEqual(len(actual.pop("provenance_warnings")), 2)
        self.assertEqual(actual, retained)

    def test_malformed_rows_fail_instead_of_producing_partial_comparisons(self):
        mutations = {
            "missing engine": lambda r: r["results"].pop(),
            "missing mode": lambda r: r.update(results=[x for x in r["results"] if x["mode"] != "warm_call"]),
            "unknown engine": lambda r: r["results"][0].update(engine="unknown"),
            "duplicate row": lambda r: r["results"].append(r["results"][0]),
            "wrong sample count": lambda r: r["results"][0]["samples"].pop(),
            "wrong MAD": lambda r: r["results"][0].update(mad=123456),
            "nonfinite median": lambda r: r["results"][0].update(median=float("nan")),
            "nonfinite MAD": lambda r: r["results"][0].update(mad=float("inf")),
            "boolean median": lambda r: r["results"][0].update(median=True),
            "boolean timing": lambda r: r["results"][0]["samples"].__setitem__(0, True),
            "source mismatch": lambda r: r["results"][0].update(source_sha256="different"),
            "duplicate workload": lambda r: r["workloads"].append(r["workloads"][0]),
            "missing workloads": lambda r: r.pop("workloads"),
            "zero iterations": lambda r: r.update(iterations_per_sample=0),
            "even sample count": lambda r: r.update(samples=14),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name):
                report = copy.deepcopy(self.report)
                mutate(report)
                with self.assertRaises(ValueError):
                    COMPARE.read(self.save(report))

    def test_changed_harness_and_dependencies_fail(self):
        for field in ("bench/rquickjs/src/main.rs", "bench/rquickjs/Cargo.toml",
                      "bench/rquickjs/Cargo.lock", ".cargo/config.toml"):
            with self.subTest(field=field):
                report = copy.deepcopy(self.report)
                report["metadata"]["build"]["inputs"]["files"][field] = "changed"
                with self.assertRaises(ValueError):
                    COMPARE.changes(self.after, self.save(report))
        for change in ("new harness module", "missing harness", "dependency checksum"):
            with self.subTest(change=change):
                report = copy.deepcopy(self.report)
                build = report["metadata"]["build"]
                if change == "new harness module":
                    build["inputs"]["files"]["bench/rquickjs/src/new.rs"] = "new"
                elif change == "missing harness":
                    build["inputs"]["files"].pop("bench/rquickjs/src/main.rs")
                else:
                    build["comparison_dependencies"][0]["checksum"] = "changed"
                with self.assertRaises(ValueError):
                    COMPARE.changes(self.after, self.save(report))

    def test_engine_and_metadata_wrapper_changes_remain_comparable(self):
        report = copy.deepcopy(self.report)
        files = report["metadata"]["build"]["inputs"]["files"]
        files["crates/napi-vm-core/src/interpreter/eval.rs"] = "optimized"
        files["bench/rquickjs.py"] = "new metadata"
        env = report["metadata"]["build"]["build_environment"]
        env["CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER"] = "test-runner"
        result = COMPARE.changes(self.after, self.save(report))
        self.assertTrue(all(row["after_over_before"] == 1 for row in result["results"]))
        self.assertIn("CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER", result["build_environment_differences"])

    def test_filtered_workload_reports_keep_added_removed_information(self):
        report = copy.deepcopy(self.report)
        name = report["workloads"][0]["name"]
        report["workloads"] = report["workloads"][:1]
        report["results"] = [row for row in report["results"] if row["workload"] == name]
        result = COMPARE.changes(self.after, self.save(report))
        self.assertEqual(len(result["results"]), 6)
        self.assertEqual(len(result["removed"]), 54)
        self.assertEqual(result["added"], [])


if __name__ == "__main__":
    unittest.main()
