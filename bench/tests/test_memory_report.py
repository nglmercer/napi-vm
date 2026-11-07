"""Validate the memory matrix, lifecycle phases, provenance, and statistics."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('memory_report', Path(__file__).resolve().parents[1]/'memory_report.py')
MEMORY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MEMORY)

class MemoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        rows = []
        source = 'function bench(){return 1;}'
        for engine in MEMORY.ENGINES:
            for workload in MEMORY.WORKLOADS:
                snapshots = []
                for phase,batch,rss in [('baseline',0,5),('ready',0,10),('allocated',1,20),('collected',1,15),('released',1,11),('dropped',1,9)]:
                    snap = dict(phase=phase,batch=batch,rss_bytes=rss,process_high_water_bytes=20,gc_ns=100 if phase=='collected' else None)
                    if phase not in ['baseline','dropped']:
                        names = ['heap_tracked','heap_collected_total','shapes_created'] if engine=='napi-vm' else ['malloc_size','memory_used_size','obj_count','shape_count','atom_count']
                        snap['engine_stats'] = dict.fromkeys(names,1)
                    snapshots.append(snap)
                run = dict(engine=engine,workload=workload,iterations=1,objects_per_call=1,source=source,source_sha256=hashlib.sha256(source.encode()).hexdigest(),snapshots=snapshots)
                runs = [copy.deepcopy(run) for _ in range(3)]
                rows.append(dict(engine=engine,workload=workload,runs=runs,summary=MEMORY.summary(runs)))
        inputs = dict.fromkeys(['bench/rquickjs/src/bin/memory.rs','bench/rquickjs/Cargo.toml','bench/rquickjs/Cargo.lock','.cargo/config.toml','crates/napi-vm-core/src/shape.rs'],'hash')
        build = dict(profile='release',rustc='fixed',build_environment={},comparison_dependencies=[],inputs=dict(files=inputs))
        self.report = dict(schema=1,kind='process_memory',samples=3,iterations=1,objects_per_call=1,results=rows,metadata=dict(platform='host',machine='machine',cpu='cpu',cpu_count=4,run_helper_sha256='helper',build=build))
        self.before = self.save(self.report,'before.json')

    def save(self,r,name='after.json'):
        path=Path(self.temp.name)/name;path.write_text(json.dumps(r));return path

    def test_complete_report_and_signed_growth(self):
        rows=MEMORY.validate(self.report)
        self.assertEqual(len(rows),8)
        run=copy.deepcopy(self.report['results'][0]['runs'][0]);run['snapshots'][-1]['rss_bytes']=4
        self.assertEqual(MEMORY.metrics(run)['retained_after_drop_bytes'],-1)
        self.assertEqual(len(MEMORY.compare(self.before,self.before)['results']),8)
        # Kernel VmHWM/RSS reports are approximate; a lower later reading is valid.
        r=copy.deepcopy(self.report)
        for row in r['results']:
            for run in row['runs']:run['snapshots'][-1]['process_high_water_bytes']=19
            row['summary']=MEMORY.summary(row['runs'])
        MEMORY.validate(r)

    def test_corrupt_or_incomplete_reports_rejected(self):
        mutations=[lambda r:r['results'].pop(),lambda r:r['results'].append(r['results'][0]),lambda r:r['results'][0]['runs'].pop(),lambda r:r['results'][0]['runs'][0]['snapshots'].pop(),lambda r:r['results'][0]['runs'][0]['snapshots'][2].update(batch=9),lambda r:r['results'][0]['runs'][0]['snapshots'][2].update(rss_bytes=True),lambda r:r['results'][0]['runs'][0]['snapshots'][2].update(process_high_water_bytes=1),lambda r:r['results'][0]['runs'][0]['snapshots'][3].update(gc_ns=-1),lambda r:r['results'][0]['runs'][0].update(source='different'),lambda r:r['results'][0]['summary']['peak_rss_bytes'].update(mad=99),lambda r:r['results'][0]['runs'][0]['snapshots'][2]['engine_stats'].update(heap_tracked=-1)]
        for i,mutate in enumerate(mutations):
            with self.subTest(i=i):
                r=copy.deepcopy(self.report);mutate(r)
                with self.assertRaises(ValueError):MEMORY.validate(r)

    def test_core_change_allowed_fixed_harness_and_helper_rejected(self):
        r=copy.deepcopy(self.report);r['metadata']['build']['inputs']['files']['crates/napi-vm-core/src/shape.rs']='new'
        MEMORY.compare(self.before,self.save(r))
        for field in ['bench/rquickjs/src/bin/memory.rs','.cargo/config.toml','bench/rquickjs/Cargo.lock']:
            r=copy.deepcopy(self.report);r['metadata']['build']['inputs']['files'][field]='new'
            with self.assertRaises(ValueError):MEMORY.compare(self.before,self.save(r))
        r=copy.deepcopy(self.report);r['metadata']['run_helper_sha256']='different'
        with self.assertRaises(ValueError):MEMORY.compare(self.before,self.save(r))

if __name__=='__main__':unittest.main()
