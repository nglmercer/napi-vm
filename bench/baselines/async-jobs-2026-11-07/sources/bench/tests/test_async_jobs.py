"""Async phase results must prove completion and retain a complete matrix."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC=importlib.util.spec_from_file_location('async_jobs',Path(__file__).resolve().parents[1]/'async_jobs.py')
ASYNC=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(ASYNC)


class AsyncTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        setup='function bench(){return Promise.resolve(42);}'
        source=setup+'\nstart();'
        import hashlib
        self.w=dict(name='fixture',setup=setup,invocation='start();',source=source,expected=42,source_sha256=hashlib.sha256(source.encode()).hexdigest())
        self.fixture=dict(schema=1,kind='async_workloads',workloads=[self.w])
        rows=[dict(engine=e,mode=m,workload='fixture',unit='ns/operation',source_sha256=self.w['source_sha256'],samples=[1,2,3],median=2,mad=1) for e in ASYNC.ENGINES for m in ASYNC.MODES]
        child=dict(schema=1,kind='async_job_timings',samples=3,iterations=2,warmup_calls=5,workloads=[self.w],results=rows,job_counts=[dict(workload='fixture',napi_jobs=1,rquickjs_jobs=1)],limitations=['test'])
        build=dict(schema=1,kind='async_build',driver_sha256='a'*64,worker_sha256='b'*64,fixtures_sha256='c'*64,fixture_matrix_sha256=ASYNC.fixture_digest(self.fixture),rustc='rustc',profile='release',environment={},inputs={'bench/async_jobs.py':'h','bench/rquickjs/src/bin/async_jobs.rs':'h','bench/rquickjs/Cargo.lock':'h','crates/napi-vm-core/src/interpreter/promise.rs':'old'})
        self.r=dict(schema=1,kind='async_jobs',samples=3,iterations=2,processes=1,fixtures=self.fixture,runs=[child],results=ASYNC.aggregate([child],{'fixture':self.w}),metadata=dict(limitations=['test'],cpu='cpu',platform='platform',machine='x64',driver_sha256='a'*64,build=build))
        self.before=self.save(self.r,'before.json')

    def save(self,data,name='after.json'):
        p=Path(self.temp.name)/name;p.write_text(json.dumps(data));return p

    def test_complete_matrix_and_core_change(self):
        self.assertEqual(len(ASYNC.validate(self.r)),1)
        after=copy.deepcopy(self.r);after['metadata']['build']['inputs']['crates/napi-vm-core/src/interpreter/promise.rs']='new'
        comparison=ASYNC.compare(self.before,self.save(after))
        self.assertEqual(comparison['changed_inputs'],['crates/napi-vm-core/src/interpreter/promise.rs'])
        self.assertEqual(len(comparison['results']),8)
        self.assertEqual(len(ASYNC.ratios(self.before)['results']),4)

    def test_corrupt_samples_completion_diagnostics_and_fixtures_rejected(self):
        mutations=[lambda r:r['runs'][0]['results'].pop(),
            lambda r:r['runs'][0]['results'].append(r['runs'][0]['results'][0]),
            lambda r:r['runs'][0]['results'][0]['samples'].__setitem__(0,True),
            lambda r:r['runs'][0]['results'][0].update(median=8),
            lambda r:r['runs'][0]['results'][0].update(mode='only_enqueued'),
            lambda r:r['runs'][0]['job_counts'][0].update(napi_jobs=0),
            lambda r:r['runs'][0]['job_counts'][0].update(rquickjs_jobs=True),
            lambda r:r['runs'][0]['job_counts'].clear(),
            lambda r:r['fixtures']['workloads'][0].update(source='different'),
            lambda r:r['results'][0].update(mad=7),
            lambda r:r['metadata']['build'].update(worker_sha256='invalid'),
            lambda r:r['metadata']['build'].update(fixture_matrix_sha256='a'*64)]
        for i,mutate in enumerate(mutations):
            with self.subTest(i=i):
                r=copy.deepcopy(self.r);mutate(r)
                with self.assertRaises(ValueError):ASYNC.validate(r)

    def test_changed_harness_dependency_and_host_rejected(self):
        mutations=[lambda r:r['metadata'].update(cpu='other'),
            lambda r:r['metadata']['build']['inputs'].update({'bench/rquickjs/src/bin/async_jobs.rs':'changed'}),
            lambda r:r['metadata']['build']['inputs'].update({'bench/rquickjs/Cargo.lock':'changed'}),
            lambda r:r['metadata']['build'].update(environment={'RUSTFLAGS':'-C opt-level=0'})]
        for mutate in mutations:
            r=copy.deepcopy(self.r);mutate(r)
            with self.assertRaises(ValueError):ASYNC.compare(self.before,self.save(r))


if __name__=='__main__':unittest.main()
