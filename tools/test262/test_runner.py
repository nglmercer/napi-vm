import unittest
from run import metadata, outcome, variants, selected_files
from pathlib import Path
from tempfile import TemporaryDirectory

class RunnerTests(unittest.TestCase):
    def test_yaml_metadata_and_variants(self):
        data = metadata('/*---\nflags: [onlyStrict]\nnegative:\n  phase: parse\n  type: SyntaxError\n---*/\nvar x;')
        self.assertEqual(variants(data), [("strict", True)])
        self.assertEqual(variants({}), [("script", False), ("strict", True)])
        self.assertEqual(variants({"flags": ["module"]}), [("module", False)])
        self.assertEqual(variants({"flags": ["raw"]}), [("script", False)])
        self.assertEqual(variants({"flags": ["noStrict"]}), [("script", False)])
    def test_negative_requires_phase_and_type(self):
        data = {"negative": {"phase": "parse", "type": "SyntaxError"}}
        self.assertEqual(outcome({"status": "error", "phase": "parse", "error_type": "SyntaxError"}, data), "pass")
        self.assertEqual(outcome({"status": "error", "phase": "runtime", "error_type": "SyntaxError"}, data), "fail")
        self.assertEqual(outcome({"status": "error", "phase": "parse", "error_type": "TypeError"}, data), "fail")
        self.assertEqual(outcome({"status": "ok"}, data), "fail")
        self.assertEqual(outcome({"status": "error", "phase": "harness", "error_type": "SyntaxError"}, data), "harness_error")
    def test_focused_groups_union_and_validate_selections(self):
        with TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            directory = root / "test" / "built-ins" / "Object"
            directory.mkdir(parents=True)
            realm = directory / "realm.js"
            ordinary = directory / "ordinary.js"
            realm.write_text("")
            ordinary.write_text("")
            groups = {"realms": {"paths": ["."], "pattern": "realm"}}
            self.assertEqual(selected_files(root, [], ["realms"], groups), {realm})
            self.assertEqual(selected_files(root, ["built-ins/Object"], ["realms"], groups), {realm, ordinary})
            with self.assertRaises(ValueError): selected_files(root, [], ["unknown"], groups)
            with self.assertRaises(ValueError): selected_files(root, ["../escape"], [], groups)
            with self.assertRaises(ValueError): selected_files(root, ["missing"], [], groups)

    def test_missing_metadata_fails(self):
        with self.assertRaises(ValueError): metadata("var x;")

if __name__ == "__main__": unittest.main()
