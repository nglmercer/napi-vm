"""The I/O layer must retain a complete, mutually consistent raw matrix."""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest

SPEC=importlib.util.spec_from_file_location('io_bench',Path(__file__).resolve().parents[1]/'io.py')
BENCH=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(BENCH)

def fixture():
    report={'schema':1,'samples':3,'iterations':2,'processes':1,'results':[],'raw_processes':[],
            'metadata':{'build':{n:{} for n in ['harness','fixtures','command','profile','rustc','node','node_executable_sha256','build_environment','inputs']}}}
    for executable,kind,keys in [('io-bench','latency',BENCH.NATIVE),('io.cjs','latency',BENCH.NODE),('io-allocations','allocations',BENCH.NATIVE)]:
        rows=[]
        for backend,name in sorted(keys):
            values=[1,2,3];requested=[4,5,6] if kind=='allocations' else []
            row={'backend':backend,'name':name,'kind':kind,'unit':'ns/op' if kind=='latency' else 'allocation requests/op',
                 'checked_result':42,'process_samples':[values],'requested_bytes_process_samples':[requested],'summary':BENCH.summary(values)}
            if requested:row['requested_bytes_summary']=BENCH.summary(requested)
            report['results'].append(row)
            rows.append({'backend':backend,'name':name,'unit':row['unit'],'checked_result':42,'samples':values,'requested_bytes_per_op':requested})
        report['raw_processes'].append({'executable':executable,'process':0,'report':{'schema':1,'kind':kind,'samples':3,'iterations':2,'results':rows}})
    return report

class IoTests(unittest.TestCase):
    def path(self,report,name='report.json'):
        if not hasattr(self,'directory'):
            temp=tempfile.TemporaryDirectory();self.addCleanup(temp.cleanup);self.directory=Path(temp.name)
        path=self.directory/name;path.write_text(json.dumps(report));return path

    def test_complete_report_and_self_comparison(self):
        path=self.path(fixture());data,rows=BENCH.read_report(path)
        self.assertEqual(len(rows),57)
        with contextlib.redirect_stdout(io.StringIO()) as output:BENCH.compare(path,path)
        self.assertEqual(json.loads(output.getvalue())['changed_inputs'],[])

    def test_missing_duplicate_and_foreign_rows_rejected(self):
        for mutate in [lambda r:r['results'].pop(),lambda r:r['results'].append(copy.deepcopy(r['results'][0])),lambda r:r['results'][0].update(backend='unexpected')]:
            report=fixture();mutate(report)
            with self.assertRaises(ValueError):BENCH.read_report(self.path(report))

    def test_bad_samples_statistics_and_units_rejected(self):
        for mutate in [lambda r:r['results'][0]['process_samples'][0].pop(),lambda r:r['results'][0]['summary'].update(median=99),lambda r:r['results'][0]['process_samples'][0].__setitem__(0,float('nan')),lambda r:r['results'][0].update(unit='bytes'),lambda r:r.update(processes=2)]:
            report=fixture();mutate(report)
            with self.assertRaises(ValueError):BENCH.read_report(self.path(report))

    def test_raw_divergence_or_missing_process_rejected(self):
        for mutate in [lambda r:r['raw_processes'].pop(),lambda r:r['raw_processes'][0]['report']['results'][0].update(checked_result=99),lambda r:r['raw_processes'][0]['report'].update(iterations=99)]:
            report=fixture();mutate(report)
            with self.assertRaises(ValueError):BENCH.read_report(self.path(report))

    def test_comparison_rejects_changed_harness_fixtures_or_compiler(self):
        before=fixture();left=self.path(before,'before.json')
        for key in ['harness','fixtures','rustc','node','build_environment']:
            after=copy.deepcopy(before);after['metadata']['build'][key]='changed'
            with self.assertRaises(ValueError):BENCH.compare(left,self.path(after,'after.json'))

    def test_fixture_content_sizes_and_mutable_files(self):
        directory=self.path({}).parent/'fixtures';manifest=BENCH.fixtures(directory)
        for relative,size in [('module.mjs',65536),('node_modules/perf-local/cjs.cjs',65536),('node_modules/perf-local/esm.mjs',4<<20)]:
            self.assertEqual((directory/relative).stat().st_size,size)
            self.assertEqual(manifest[relative],BENCH.sha(directory/relative))
        self.assertNotIn('write-permissions',manifest)

if __name__=='__main__':unittest.main()
