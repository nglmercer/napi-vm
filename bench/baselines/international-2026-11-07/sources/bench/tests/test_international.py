"""Capability absence and semantic mismatch must never become fast timing rows."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

BENCH=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(BENCH))
SPEC=importlib.util.spec_from_file_location('international',BENCH/'international.py')
INTL=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(INTL)


class InternationalTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        source='function bench(){return 42;}\nbench();'
        self.w=dict(name='fixture',setup=source.split('\n')[0],source=source,invocation='bench();',requires=['Intl.NumberFormat'],expected=42,source_sha256=hashlib.sha256(source.encode()).hexdigest())
        f=dict(schema=1,kind='international_workloads',probe_source='JSON.stringify({});',workloads=[self.w])
        self.host=dict(node='node',v8='v8',icu='icu',platform='linux',arch='x64',cpu='cpu')
        rows=[]
        for engine in INTL.ENGINES:
            for mode in INTL.MODES:
                r=dict(engine=engine,mode=mode,workload='fixture',source_sha256=self.w['source_sha256'],unit='ns/call',missing=[])
                if engine=='napi-vm':r.update(status='incompatible_result',diagnostic='different localized output')
                elif engine=='rquickjs':r.update(status='unsupported',missing=['Intl.NumberFormat'])
                else:r.update(status='measured',samples=[1,2,3],median=2,mad=1)
                rows.append(r)
        common=dict(schema=1,kind='international_child',samples=3,iterations=2,warmup_calls=5)
        rust=dict(**common,capabilities={'napi-vm':{'Intl.NumberFormat':True},'rquickjs':{'Intl.NumberFormat':False}},results=[x for x in rows if x['engine']!='node-v8-context'])
        node=dict(**common,capabilities={'node-v8-context':{'Intl.NumberFormat':True}},results=[x for x in rows if x['engine']=='node-v8-context'],metadata=dict(**self.host,fixtures_sha256='c'*64,script_sha256='b'*64,host_gc=True))
        build=dict(kind='international_build',driver_sha256='d'*64,fixture_matrix_sha256=INTL.fixture_digest(f),artifacts={'worker':'a'*64,'node.cjs':'b'*64,'workloads.json':'c'*64},rustc='rustc',profile='release',environment={},inputs={'bench/international.py':'h','bench/rquickjs/src/bin/international.rs':'h','bench/international-workloads.json':'h','bench/rquickjs/Cargo.lock':'h','crates/napi-vm-core/src/builtins/intl/displaynames.rs':'old'})
        runs=[dict(rust=rust,node=node)]
        self.r=dict(schema=1,kind='international',samples=3,iterations=2,processes=1,fixtures=f,runs=runs,results=INTL.aggregate(runs,{'fixture':self.w}),metadata=dict(**self.host,build=build,driver_sha256='d'*64,affinity=[0],run_environment={},limitations=['test']))
        self.before=self.save(self.r,'before.json')

    def save(self,r,name='after.json'):
        p=Path(self.temp.name)/name;p.write_text(json.dumps(r));return p

    def test_complete_matrix_includes_all_three_statuses(self):
        self.assertEqual(len(INTL.validate(self.r)),1)
        self.assertEqual(len(INTL.compare(self.before,self.before)['results']),9)
        ratios=INTL.ratios(self.before)
        self.assertTrue(all('napi_vm_over_reference' not in row for row in ratios['results']))

    def test_fake_timings_missing_capabilities_and_bad_statistics_rejected(self):
        mutations=[lambda r:r['runs'][0]['rust']['results'].pop(),
            lambda r:r['runs'][0]['rust']['results'].append(r['runs'][0]['rust']['results'][0]),
            lambda r:r['runs'][0]['rust']['results'][0].update(samples=[1,2,3],median=2,mad=1),
            lambda r:r['runs'][0]['rust']['results'][0].pop('diagnostic'),
            lambda r:r['runs'][0]['rust']['results'][3].update(status='measured',samples=[1,2,3],median=2,mad=1),
            lambda r:r['runs'][0]['rust']['capabilities']['rquickjs'].clear(),
            lambda r:r['runs'][0]['node']['results'][0]['samples'].__setitem__(0,True),
            lambda r:r['runs'][0]['node']['results'][0].update(median=99),
            lambda r:r['runs'][0]['node']['metadata'].update(host_gc=False),
            lambda r:r['fixtures']['workloads'][0].update(source='changed'),
            lambda r:r['results'][0].update(status='measured'),
            lambda r:r['metadata']['build']['artifacts'].update(worker='bad')]
        for i,mutate in enumerate(mutations):
            with self.subTest(i=i):
                r=copy.deepcopy(self.r);mutate(r)
                with self.assertRaises(ValueError):INTL.validate(r)

    def test_changed_harness_dependency_and_affinity_rejected(self):
        mutations=[lambda r:r['metadata']['build']['inputs'].update({'bench/international-workloads.json':'new'}),
            lambda r:r['metadata']['build']['inputs'].update({'bench/rquickjs/Cargo.lock':'new'}),
            lambda r:r['metadata'].update(affinity=[1]),
            lambda r:r['metadata'].update(icu='different')]
        for mutate in mutations:
            r=copy.deepcopy(self.r);mutate(r)
            with self.assertRaises(ValueError):INTL.compare(self.before,self.save(r))

    def test_semantic_fix_reports_status_change_without_inventing_speedup(self):
        after=copy.deepcopy(self.r)
        after['metadata']['build']['inputs']['crates/napi-vm-core/src/builtins/intl/displaynames.rs']='new'
        for r in after['runs'][0]['rust']['results']:
            if r['engine']=='napi-vm':r.pop('diagnostic');r.update(status='measured',samples=[1,2,3],median=2,mad=1)
        after['results']=INTL.aggregate(after['runs'],{'fixture':self.w})
        comparison=INTL.compare(self.before,self.save(after))
        native=[r for r in comparison['results'] if r['engine']=='napi-vm']
        self.assertEqual(len(native),3)
        self.assertTrue(all(r['before_status']=='incompatible_result' and r['after_status']=='measured' and 'after_over_before' not in r for r in native))


if __name__=='__main__':unittest.main()
