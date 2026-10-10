import json
import unittest

from audit_contextual_bindings import outcomes, requests


class ContextualAuditTests(unittest.TestCase):
    def test_missing_or_duplicate_rows_cannot_report_a_successful_audit(self):
        rows = requests()[:2]
        row = {**rows[0], 'accepted': True}
        for output in [json.dumps(row), '\n'.join([json.dumps(row)] * 2)]:
            with self.assertRaises(ValueError):
                outcomes(output, rows)

    def test_invalid_acceptance_and_reordered_rows_are_rejected(self):
        rows = requests()[:2]
        output = [{**row, 'accepted': True} for row in rows]
        with self.assertRaises(ValueError):
            outcomes('\n'.join(map(json.dumps, reversed(output))), rows)
        output[0]['accepted'] = 'true'
        with self.assertRaises(ValueError):
            outcomes('\n'.join(map(json.dumps, output)), rows)


if __name__ == '__main__':
    unittest.main()
