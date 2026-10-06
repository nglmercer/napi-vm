import unittest
from run import metadata, outcome, variants

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
    def test_missing_metadata_fails(self):
        with self.assertRaises(ValueError): metadata("var x;")

if __name__ == "__main__": unittest.main()
