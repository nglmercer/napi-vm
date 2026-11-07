'use strict';
const fs = require('node:fs');
const path = require('node:path');
const {createRequire} = require('node:module');
const {pathToFileURL} = require('node:url');
const assert = require('node:assert/strict');
const [root, sampleArg, iterationArg] = process.argv.slice(2);
const samples = Number(sampleArg), iterations = Number(iterationArg);
assert(samples > 0 && samples % 2 === 1 && iterations > 0);
const results = [];
function measure(backend, name, checked_result, operation) {
  for (let n = 0; n < 5; n++) assert.equal(operation(), checked_result);
  const values = [];
  for (let s = 0; s < samples; s++) {
    const start = process.hrtime.bigint();
    for (let i = 0; i < iterations; i++) assert.equal(operation(), checked_result);
    values.push(Number(process.hrtime.bigint() - start) / iterations);
  }
  results.push({backend, name, checked_result, samples: values, unit:'ns/op', requested_bytes_per_op:[]});
}
for (const [name, relative, size] of [
  ['read_bytes_empty','empty',0],['read_bytes_4k','4k',4096],
  ['read_bytes_64k','64k',65536],['read_bytes_4m','4m',4<<20],
  ['read_bytes_nested_4k','a/b/c/d/4k',4096],
]) {
  const filename = path.join(root,relative);
  assert.deepEqual(fs.readFileSync(filename), Buffer.alloc(size, 'x'));
  measure('node_fs',name,size,()=>fs.readFileSync(filename).length);
}
const filename = path.join(root,'64k');
assert.equal(fs.readFileSync(filename,'utf8'),'x'.repeat(65536));
measure('node_fs','read_text_64k',65536,()=>fs.readFileSync(filename,'utf8').length);
const payload = Buffer.alloc(4096,'w'), target = path.join(root,'write-node_fs');
measure('node_fs','write_bytes_4k',4096,()=>{fs.writeFileSync(target,payload);return payload.length;});
assert.deepEqual(fs.readFileSync(target),payload);
measure('node_fs','mkdir_existing_nested',1,()=>{fs.mkdirSync(path.join(root,'a/b/c/d'),{recursive:true});return 1;});
const baseURL = pathToFileURL(root+'/'), id = new URL('./module.mjs',baseURL).href;
measure('node_file','resolve',id.length,()=>new URL('./module.mjs',baseURL).href.length);
const expected = fs.readFileSync(path.join(root,'module.mjs'),'utf8');
assert.equal(fs.readFileSync(new URL(id),'utf8'),expected);
measure('node_file','load_64k',65536,()=>fs.readFileSync(new URL(id),'utf8').length);
const requireLocal = createRequire(path.join(root,'driver.cjs'));
assert.equal(fs.readFileSync(requireLocal.resolve('perf-local'),'utf8'),fs.readFileSync(path.join(root,'node_modules/perf-local/cjs.cjs'),'utf8'));
measure('node_cjs','cjs_resolve_read_64k',65536,()=>fs.readFileSync(requireLocal.resolve('perf-local'),'utf8').length);
console.log(JSON.stringify({schema:1,kind:'latency',samples,iterations,results}));
