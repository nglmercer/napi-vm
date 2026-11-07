"""Protect raw owner-memory phases and before/after compatibility."""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC=importlib.util.spec_from_file_location('collection_memory',Path(__file__).resolve().parents[1]/'collection_memory.py')
MEMORY=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(MEMORY)

class OwnerMemoryTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.report={'schema':1,'kind':'node_collection_memory','samples':1,
            'metadata':dict(addon_sha256='a'*64,script_sha256='b'*64,helper_sha256='c'*64,node='node',platform='host',machine='x64',cpu='cpu'),
            'limitations':['fixture'],'results':[]}
        for name,(kind,count,entries) in MEMORY.CASES.items():
            snapshots=[dict(phase=p,round=n,rss_bytes=100+n,high_water_bytes=200-n,
                engine_stats=None if p in ('baseline','disposed') else dict(tracked=10,collectedTotal=n))
                for p,n in MEMORY.PHASES]
            run=dict(schema=1,kind=kind,collections=count,entries=entries,rounds=3,source_sha256='d'*64,
                     addon_sha256='a'*64,script_sha256='b'*64,snapshots=snapshots)
            self.report['results'].append(dict(name=name,runs=[run],summary=MEMORY.summarize([run])))
        self.before=self.save(self.report,'before.json')

    def save(self,report,name='after.json'):
        p=Path(self.temp.name)/name;p.write_text(json.dumps(report));return p

    def test_complete_capture_and_nonmonotonic_kernel_high_water_are_accepted(self):
        self.assertEqual(len(MEMORY.compare(self.before,self.before)['results']),4)

    def test_incomplete_corrupt_or_changed_capture_is_rejected(self):
        mutations=[lambda r:r['results'].pop(),lambda r:r['results'].append(r['results'][0]),
            lambda r:r['results'][0]['runs'][0]['snapshots'].pop(),
            lambda r:r['results'][0]['runs'][0]['snapshots'][2].update(rss_bytes=True),
            lambda r:r['results'][0]['runs'][0].update(addon_sha256='e'*64),
            lambda r:r['results'][0]['runs'][0].update(source_sha256='e'*64),
            lambda r:r['results'][0]['summary']['ready_rss_bytes'].update(median=0),
            lambda r:r['metadata'].update(helper_sha256='e'*64),
            lambda r:r['metadata'].update(cpu='other')]
        for i,mutate in enumerate(mutations):
            with self.subTest(i=i):
                report=copy.deepcopy(self.report);mutate(report)
                with self.assertRaises(ValueError):MEMORY.compare(self.before,self.save(report))

if __name__=='__main__':unittest.main()
