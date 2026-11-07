"""Preserve behavioral differences and reject incompatible collection evidence."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('collection_report', Path(__file__).resolve().parents[1]/'collection_report.py')
REPORT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REPORT)


class CollectionReportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.report = {'schema': 1, 'metadata': dict(addonSha256='a'*64, scriptSha256='b'*64,
                       node='v1', platform='linux', arch='x64', cpu='cpu', samples=3, iterations=2),
                       'results': [dict(name=name, unit='ns/call', references=refs, expectedEntries=256,
                           source=name, sharedIdentityPreserved=True, samples=[1,2,3],
                           summary=dict(median=2,mad=1)) for name, refs in REPORT.CASES.items()]}
        self.before = self.save(self.report, 'before.json')

    def save(self, report, name='after.json'):
        path = Path(self.temp.name)/name
        path.write_text(json.dumps(report))
        return path

    def test_identity_changes_are_visible_without_hiding_before_defect(self):
        before = copy.deepcopy(self.report)
        before['results'][1]['sharedIdentityPreserved'] = False
        after = copy.deepcopy(self.report)
        after['metadata']['addonSha256'] = 'c'*64
        comparison = REPORT.compare(self.save(before,'old.json'), self.save(after))
        self.assertFalse(comparison['results'][1]['identity_before'])
        self.assertTrue(comparison['results'][1]['identity_after'])

    def test_corrupt_matrix_and_incompatible_settings_rejected(self):
        mutations = [lambda r:r['results'].pop(), lambda r:r['results'].append(r['results'][0]),
            lambda r:r['results'][0]['samples'].pop(), lambda r:r['results'][0]['samples'].__setitem__(0,True),
            lambda r:r['results'][0]['samples'].__setitem__(0,float('inf')),
            lambda r:r['results'][0]['summary'].update(mad=7),
            lambda r:r['results'][0].update(source='changed'),
            lambda r:r['results'][0].update(sharedIdentityPreserved=False),
            lambda r:r['metadata'].update(cpu='changed'), lambda r:r['metadata'].update(iterations=3),
            lambda r:r['metadata'].update(scriptSha256='c'*64)]
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index):
                report = copy.deepcopy(self.report); mutate(report)
                with self.assertRaises(ValueError): REPORT.compare(self.before, self.save(report))


if __name__ == '__main__': unittest.main()
