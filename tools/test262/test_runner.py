import unittest
from run import metadata, outcome, variants, selected_files, resume_checkpoint, validate_engine
import json
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory

class RunnerTests(unittest.TestCase):
    def test_invalid_worker_is_a_driver_error_before_corpus_selection(self):
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, 'not a file'):
                validate_engine(root / 'missing')
            with self.assertRaisesRegex(ValueError, 'not a file'):
                validate_engine(root)
            engine = root / 'worker'
            engine.write_text('#!/bin/sh\nexit 0\n')
            engine.chmod(0o644)
            with self.assertRaisesRegex(ValueError, 'not executable'):
                validate_engine(engine)
            report = root / 'outcomes.json'
            result = subprocess.run([
                sys.executable, str(Path(__file__).with_name('run.py')),
                str(root / 'missing-corpus'), '--engine', str(engine),
                '--revision', 'unused', '--output', str(report),
            ], capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn('engine is not executable', result.stderr)
            self.assertFalse(report.exists())
            engine.chmod(0o755)
            validate_engine(engine)

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

    def test_checkpoint_preserves_outcomes_and_rejects_drift(self):
        with TemporaryDirectory() as temporary:
            path = Path(temporary) / 'journal.jsonl'
            config = {'engine_sha256': 'original', 'worker_jobs': 4}
            row = {'test': 'a.js', 'variant': 'strict', 'status': 'timeout'}
            complete = json.dumps(config) + '\n' + json.dumps({'test': 'a.js', 'rows': [row]}) + '\n'
            path.write_text(complete + '{"test": "b.js"')
            self.assertEqual(resume_checkpoint(path, config, ['a.js', 'b.js']), (1, [row]))
            self.assertEqual(path.read_text(), complete)
            with self.assertRaises(ValueError):
                resume_checkpoint(path, {**config, 'engine_sha256': 'changed'}, ['a.js', 'b.js'])
            with self.assertRaises(ValueError):
                resume_checkpoint(path, config, ['b.js', 'a.js'])
            path.write_text(json.dumps(config) + '\n' + json.dumps({'test': 'a.js', 'rows': [row, row]}) + '\n')
            with self.assertRaises(ValueError):
                resume_checkpoint(path, config, ['a.js'])

if __name__ == "__main__": unittest.main()
