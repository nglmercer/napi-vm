'use strict';
// Explicit binding path avoids the generated loader's fallback to a stale addon.
const assert = require('node:assert/strict');
const bindingPath = require('node:path').resolve(process.env.NAPI_BENCH_BINDING);
const vm = require(bindingPath);
assert.equal(require.cache[bindingPath]?.filename, bindingPath);
const cases = [
  ['arithmetic', 'let s=0; for(let i=0;i<10000;i++) s+=i*2-1; s;', '99980000'],
  ['fibonacci', 'function f(n){return n<2?n:f(n-1)+f(n-2)} f(10);', '55'],
  ['array', 'let a=[]; for(let i=0;i<1000;i++)a.push(i); a.map(x=>x*2).filter(x=>x%3===0).reduce((s,x)=>s+x,0);', '333666'],
  ['json', "const o={a:1,b:[1,2,3],c:{d:'x',e:[true,null]}}; let r; for(let i=0;i<200;i++)r=JSON.parse(JSON.stringify(o)); r.c.e.length+r.b.length;", '5'],
];
const output = [];
for (const [name, source, expected] of cases) {
  const invoke = () => assert.equal(vm.runCode(source), expected);
  for(let i=0;i<5;i++) invoke();
  const samples=[];
  for(let i=0;i<20;i++){const t=process.hrtime.bigint();invoke();samples.push(Number(process.hrtime.bigint()-t));}
  output.push({name:'node_binding/'+name,unit:'ns/call',samples,backend:'runCode: fresh VM with warmed source-template cache via explicitly loaded NAPI addon'});
}
const service = new vm.LanguageService();
const uri = 'file:///performance.js';
const source = Array.from({length: 200}, (_, i) => `const item${i} = ${i};`).join('\n') + '\nitem19';
service.open(uri, source);
const languageCases = [
  ['completion', () => assert(service.complete(uri, source.length).some(x => x.label === 'item199'))],
  ['diagnostics_update', () => { assert(service.update(uri, 'const value = (1;\n')); assert(service.diagnostics(uri).length > 0); }],
];
for (const [name, invoke] of languageCases) {
  for (let i=0;i<5;i++) invoke();
  const samples=[];
  for(let i=0;i<20;i++){const t=process.hrtime.bigint();invoke();samples.push(Number(process.hrtime.bigint()-t));}
  output.push({name:'language_service/'+name,unit:'ns/call',samples,backend:'NAPI LanguageService; '+(name === 'completion' ? 'cached document completion' : 'update plus diagnostics')});
}
console.log(JSON.stringify({bindingPath, results:output}));
