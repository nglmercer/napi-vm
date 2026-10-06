import unittest
from triage import summarize


class TriageTests(unittest.TestCase):
    def test_every_outcome_remains_in_its_group_and_denominator(self):
        report = {'domain': 'ECMAScript', 'suite': 'Test262', 'revision': 'pinned',
                  'engine_sha256': 'digest', 'selection': ['.'], 'total': 5,
                  'counts': {'pass': 1, 'fail': 1, 'skip': 1, 'crash': 1, 'timeout': 1},
                  'pass_percentage': 20,
                  'results': [
                      {'test': 'built-ins/WeakMap/a.js', 'status': 'pass'},
                      {'test': 'built-ins/WeakMap/b.js', 'status': 'fail', 'engine': {'phase': 'runtime'}},
                      {'test': 'language/expressions/addition/a.js', 'status': 'crash'},
                      {'test': 'language/expressions/addition/b.js', 'status': 'timeout'},
                      {'test': 'language/expressions/addition/c.js', 'status': 'skip'},
                  ]}
        summary = summarize(report)
        self.assertEqual(summary['total'], 5)
        self.assertEqual(sum(group['total'] for group in summary['groups']), 5)
        self.assertEqual(sum(group['non_passing'] for group in summary['groups']), 4)
        self.assertEqual(summary['groups'][0]['path'], 'language/expressions/addition')
        self.assertEqual(summary['non_passing_phases']['runtime'], 1)
        self.assertEqual(summary['counts'], report['counts'])


if __name__ == '__main__':
    unittest.main()
