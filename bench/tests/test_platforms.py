"""Reject corrupt public-platform matrices and changed timing boundaries."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import contextlib
import io

SPEC=importlib.util.spec_from_file_location('platforms',Path(__file__).resolve().parents[1]/'platforms.py')
PLATFORM=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(PLATFORM)

class PlatformTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        setup='function bench(){return 1;}';source=setup+'\nbench();'
        w=dict(name='fixture',setup=setup,source=source,invocation='bench();',expected=1,
               source_sha256=hashlib.sha256(source.encode()).hexdigest())
        self.fixtures=dict(schema=1,kind='guest_workloads',workloads=[w])
        self.host=dict(node='v1',v8='v8',platform='linux',arch='x64',cpu='cpu')
        hashes=dict(addon='a'*64,wasm='b'*64,glue='c'*64,workloads='d'*64,script='e'*64)
        rows=[dict(engine=e,mode=m,workload=n,source_sha256=s,unit=u,samples=[1,2,3],median=2,mad=1)
              for (e,m,n),(s,u) in PLATFORM.matrix({'fixture':w}).items()]
        run=dict(schema=1,kind='public_platform_timing',samples=3,iterations=2,warmup_calls=5,
                 workloads=[w],metadata=dict(**self.host,hashes=hashes,host_gc=True),
                 startup=dict.fromkeys(PLATFORM.STARTUP,10),results=rows)
        build=dict(schema=1,kind='public_platform_build',
                   artifacts={'napi-vm.node':hashes['addon'],'wasm/napi_vm_bg.wasm':hashes['wasm'],'wasm/napi_vm.js':hashes['glue'],'workloads.json':hashes['workloads']},
                   helpers={'bench/platforms.py':'f'*64,'bench/performance/platforms.cjs':hashes['script']},profiles={'p':'profile'},rustc='rustc',wasm_pack='pack',build_environment={},
                   inputs=dict.fromkeys(('Cargo.toml','Cargo.lock','.cargo/config.toml','build.rs',
                            'crates/napi-vm-core/Cargo.toml','crates/napi-vm-runtime/Cargo.toml'),'hash'))
        hashes['package']=PLATFORM.package_digest(build['artifacts'])
        self.report=dict(schema=1,kind='public_platforms',samples=3,iterations=2,processes=1,fixtures=self.fixtures,
            runs=[run],results=PLATFORM.aggregate([run],{'fixture':w}),
            metadata=dict(**self.host,hashes=hashes,helper_sha256='f'*64,build=build,limitations=['fixture']))
        self.before=self.save(self.report,'before.json')

    def save(self,data,name='after.json'):
        p=Path(self.temp.name)/name;p.write_text(json.dumps(data));return p

    def test_full_matrix_and_aggregate_validate(self):
        self.assertEqual(len(PLATFORM.validate(self.report)),1)
        self.assertEqual(len(PLATFORM.compare(self.before,self.before)['results']),14)

    def test_corrupt_and_incompatible_evidence_rejected(self):
        mutations=[lambda r:r['runs'][0]['results'].pop(),
            lambda r:r['runs'][0]['results'].append(r['runs'][0]['results'][0]),
            lambda r:r['runs'][0]['results'][0]['samples'].__setitem__(0,True),
            lambda r:r['runs'][0]['results'][0].update(mad=8),
            lambda r:r['runs'][0]['results'][0].update(median=True),
            lambda r:r['runs'][0]['startup'].pop('wasm_compile_ns'),
            lambda r:r['runs'][0]['metadata'].update(host_gc=False),
            lambda r:r['fixtures']['workloads'][0].update(source='changed'),
            lambda r:r['results'][0].update(median=8),
            lambda r:r['metadata'].update(cpu='other'),
            lambda r:r['metadata']['build']['helpers'].update({'bench/platforms.py':'changed'}),
            lambda r:r['metadata']['hashes'].update(wasm='changed'),
            lambda r:r['metadata']['build']['artifacts'].update({'wasm/snippets/inline.js':'e'*64}),
            lambda r:r['metadata']['build']['inputs'].update({'Cargo.lock':'changed'})]
        for i,mutate in enumerate(mutations):
            with self.subTest(i=i):
                report=copy.deepcopy(self.report);mutate(report)
                with self.assertRaises(ValueError):PLATFORM.compare(self.before,self.save(report))

    def test_staging_preserves_runtime_bytes_and_rejects_modified_helpers(self):
        source=Path(self.temp.name)/'source';source.mkdir()
        (source/'wasm/snippets').mkdir(parents=True)
        (source/'napi-vm.node').write_bytes(b'native')
        (source/'workloads.json').write_text(json.dumps(self.fixtures))
        (source/'wasm/napi_vm_bg.wasm').write_bytes(b'wasm')
        (source/'wasm/napi_vm.js').write_text("import './snippets/helper.js';")
        (source/'wasm/snippets/helper.js').write_text('export function helper(){}')
        (source/'wasm/package.json').write_text(json.dumps(dict(name='fixture',files=['napi_vm.js','napi_vm_bg.wasm'])))
        original=dict(schema=1,kind='public_platform_build',helpers={'bench/performance/platforms.cjs':PLATFORM.sha(PLATFORM.SCRIPT)},
                      artifacts=PLATFORM.artifact_hashes(source),inputs={'source':'hash'})
        (source/'build.json').write_text(json.dumps(original))
        target=Path(self.temp.name)/'staged'
        with contextlib.redirect_stdout(io.StringIO()):PLATFORM.stage(source,target)
        staged=json.loads((target/'build.json').read_text())
        self.assertEqual(staged['artifact_staging']['original_build_record'],original)
        self.assertEqual(staged['artifacts']['wasm/snippets/helper.js'],original['artifacts']['wasm/snippets/helper.js'])
        self.assertIn('snippets',json.loads((target/'wasm/package.json').read_text())['files'])
        (source/'wasm/snippets/helper.js').write_text('modified')
        with self.assertRaises(ValueError):PLATFORM.stage(source,Path(self.temp.name)/'bad')

if __name__=='__main__':unittest.main()
