"""Reject incompatible or corrupt HTTP benchmark evidence."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('http_bench', Path(__file__).resolve().parents[1] / 'http.py')
HTTP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HTTP)

class HttpTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        required = ['Cargo.lock', 'Cargo.toml', '.cargo/config.toml', 'crates/napi-vm-runtime/Cargo.toml', 'crates/napi-vm-runtime/examples/http_bench.rs', 'crates/napi-vm-runtime/examples/http_bench/native.rs', 'crates/napi-vm-runtime/src/runtime/network.rs']
        build = dict(profile='ci-optimized', features='runtime-net', rustc='fixed', build_environment={}, inputs=dict.fromkeys(required, 'hash'))
        self.report = dict(schema=1, samples=3, iterations=1, metadata=dict(platform='host', machine='machine', cpu='cpu', cpu_count=4, build=build), results=[dict(name=name, unit='ns/request', samples=[1,2,3], summary=dict(median=2,mad=1), backend='boundary') for name in ['get_empty','get_64k','post_4m','post_4m_redirect_3']])
        self.before = self.save(self.report, 'before.json')

    def save(self, value, name='after.json'):
        path = Path(self.temp.name) / name
        path.write_text(json.dumps(value))
        return path

    def test_production_changes_allowed_and_fixed_inputs_rejected(self):
        r = copy.deepcopy(self.report)
        r['metadata']['build']['inputs']['crates/napi-vm-runtime/src/runtime/network.rs'] = 'changed'
        self.assertEqual(len(HTTP.compare(self.before,self.save(r))['results']),4)
        for field in ['Cargo.lock','crates/napi-vm-runtime/Cargo.toml','crates/napi-vm-runtime/examples/http_bench.rs']:
            with self.subTest(field=field):
                r = copy.deepcopy(self.report)
                r['metadata']['build']['inputs'][field] = 'changed'
                with self.assertRaises(ValueError): HTTP.compare(self.before,self.save(r))

    def test_incomplete_corrupt_or_mismatched_reports_rejected(self):
        mutations = [lambda r:r['results'].pop(), lambda r:r['results'].append(r['results'][0]), lambda r:r['results'][0]['samples'].pop(), lambda r:r['results'][0]['samples'].__setitem__(0,float('nan')), lambda r:r['results'][0]['samples'].__setitem__(0,True), lambda r:r['results'][0]['summary'].update(mad=99), lambda r:r['metadata']['build'].update(rustc='other'), lambda r:r['metadata'].update(cpu='other'), lambda r:r.update(iterations=2)]
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index):
                r = copy.deepcopy(self.report); mutate(r)
                with self.assertRaises(ValueError): HTTP.compare(self.before,self.save(r))

if __name__ == '__main__': unittest.main()
