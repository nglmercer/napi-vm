import copy
import gzip
import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from compare import compare, read_report


def report(statuses):
    return {'revision': 'pinned', 'worker_jobs': 4, 'timeout_seconds': 5,
            'engine_sha256': 'digest', 'total': len(statuses), 'counts': {},
            'results': [{'test': f'{index}.js', 'variant': 'script', 'status': status}
                        for index, status in enumerate(statuses)]}


class ComparisonTests(unittest.TestCase):
    def test_compressed_full_evidence_preserves_every_outcome(self):
        expected = report(['pass', 'timeout', 'crash', 'harness_error'])
        with TemporaryDirectory() as temporary:
            path = Path(temporary) / 'report.json.gz'
            path.write_bytes(gzip.compress(json.dumps(expected).encode(), mtime=0))
            self.assertEqual(read_report(path), expected)

    def test_retains_failures_harness_errors_crashes_timeouts_and_regressions(self):
        result = compare(report(['pass', 'fail', 'crash', 'timeout', 'harness_error']),
                         report(['timeout', 'pass', 'fail', 'harness_error', 'pass']))
        self.assertEqual(result['new_passes'], 2)
        self.assertEqual(result['lost_passes'], 1)
        self.assertEqual(len(result['all_status_transitions']), 5)
        self.assertEqual(result['formerly_passing'][0]['after']['status'], 'timeout')

    def test_rejects_incompatible_or_incomplete_measurements(self):
        baseline = report(['pass', 'fail'])
        for field, value in [('revision', 'different'), ('worker_jobs', 1), ('timeout_seconds', 1)]:
            current = copy.deepcopy(baseline)
            current[field] = value
            with self.assertRaises(ValueError): compare(baseline, current)
        with self.assertRaises(ValueError): compare(baseline, report(['pass']))
        duplicate = report(['pass', 'pass'])
        duplicate['results'][1] = duplicate['results'][0]
        with self.assertRaises(ValueError): compare(baseline, duplicate)
