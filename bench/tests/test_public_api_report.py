"""Reject incomplete or incompatible public API captures."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('public_api_report', Path(__file__).resolve().parents[1]/'public_api_report.py')
REPORT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REPORT)


class PublicApiReportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.report = {'schema': 1, 'metadata': dict(addonSha256='a'*64, scriptSha256='b'*64,
                       node='v1', platform='linux', arch='x64', cpu='cpu', samples=3, iterations=2),
                       'results': [dict(name=name, unit='ns/call', backend='boundary '+name,
                           samples=[1,2,3], summary=dict(median=2,mad=1)) for name in REPORT.CASES]}
        self.before = self.save(self.report, 'before.json')

    def save(self, report, name='after.json'):
        path = Path(self.temp.name)/name
        path.write_text(json.dumps(report))
        return path

    def test_same_boundary_allows_different_addon(self):
        report = copy.deepcopy(self.report)
        report['metadata']['addonSha256'] = 'c'*64
        self.assertEqual(len(REPORT.compare(self.before,self.save(report))['results']),26)

    def test_corrupt_or_incompatible_capture_rejected(self):
        mutations = [lambda r:r['results'].pop(), lambda r:r['results'].append(r['results'][0]),
            lambda r:r['results'][0]['samples'].pop(), lambda r:r['results'][0]['samples'].__setitem__(0,True),
            lambda r:r['results'][0]['samples'].__setitem__(0,float('nan')),
            lambda r:r['results'][0]['summary'].update(mad=8), lambda r:r['results'][0].update(backend='other'),
            lambda r:r['metadata'].update(iterations=1), lambda r:r['metadata'].update(cpu='other'),
            lambda r:r['metadata'].update(scriptSha256='c'*64)]
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index):
                report = copy.deepcopy(self.report); mutate(report)
                with self.assertRaises(ValueError): REPORT.compare(self.before,self.save(report))


if __name__ == '__main__': unittest.main()
