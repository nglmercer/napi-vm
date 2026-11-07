// Diagnostic import-call counts from a derived glue copy, not latency samples.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const original=path.resolve(process.argv[2]);
const out=path.resolve(process.argv[3]);
assert(!fs.existsSync(out),'output must be new');
fs.mkdirSync(out,{recursive:true});
const pkg=path.join(out,'wasm');
fs.cpSync(original,pkg,{recursive:true});
const originalGlue=fs.readFileSync(path.join(pkg,'napi_vm.js'),'utf8');
assert(originalGlue.includes('function __wbg_get_imports()'),'unsupported generated glue');
const injection=[
  'const __profile_counts_data=Object.create(null);',
  'const __profile_sources_data=Object.create(null);',
  'const __profile_original_get_imports=__wbg_get_imports;',
  '__wbg_get_imports=function(){',
  '  const imports=__profile_original_get_imports();',
  '  for(const [namespace,fields] of Object.entries(imports)){',
  '    for(const [name,original] of Object.entries(fields)){',
  '      if(typeof original!=="function")continue;',
  '      const key=namespace+"."+name;',
  '      __profile_sources_data[key]=original.toString();',
  '      fields[name]=function(...args){',
  '        __profile_counts_data[key]=(__profile_counts_data[key]??0)+1;',
  '        return Reflect.apply(original,this,args);',
  '      };',
  '    }',
  '  }',
  '  return imports;',
  '};',
  'export function __profile_reset(){for(const key of Object.keys(__profile_counts_data))delete __profile_counts_data[key];}',
  'export function __profile_counts(){return {...__profile_counts_data};}',
  'export function __profile_sources(){return {...__profile_sources_data};}',
].join('\n');
const glue=path.join(pkg,'napi_vm_profile.mjs');
fs.writeFileSync(glue,originalGlue+'\n'+injection+'\n');
const bytes=fs.readFileSync(path.join(pkg,'napi_vm_bg.wasm'));
const module=await import(pathToFileURL(glue).href);
module.initSync({module:await WebAssembly.compile(bytes)});
const cases=[
  ['constant','1;',r=>assert.equal(r.value,'1')],
  ['call','bench();',r=>assert.equal(r.value,'1')],
  ['logs',"console.log('x');42;",r=>{assert.equal(r.value,'42');assert.equal(r.logs.length,1);}],
  ['error',"throw Error('x');",r=>{assert.equal(r.ok,false);assert(r.error.includes('x'));}],
  ['unicode',"'😀';",r=>assert.equal(r.value,'😀')],
];
const vm=new module.WasmVm();
assert(vm.run('function bench(){return 1;}').ok);
const results=[];
try {
  for(const [name,source,check] of cases) {
    for(let i=0;i<5;i++)check(vm.run(source));
    module.__profile_reset();
    for(let i=0;i<100;i++)check(vm.run(source));
    const counts=module.__profile_counts();
    results.push({name,source,repetitions:100,counts,total_imports_per_call:Object.values(counts).reduce((a,b)=>a+b,0)/100});
  }
} finally {vm.free();}
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const report={schema:1,wasm_sha256:sha(bytes),original_glue_sha256:sha(originalGlue),
  profiled_glue_sha256:sha(fs.readFileSync(glue)),node:process.version,
  limitations:['Import wrappers change call overhead; these are operation counts, not latency measurements.',
    'Only a derived package copy is instrumented. The compiled WASM bytes remain unchanged.'],
  results,import_sources:module.__profile_sources()};
fs.writeFileSync(path.join(out,'profile.json'),JSON.stringify(report,null,2)+'\n');
console.log(path.join(out,'profile.json'));
