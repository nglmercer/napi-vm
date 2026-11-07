const {test}=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const os=require('node:os');
const path=require('node:path');

test('WASM packaging includes generated helpers without changing existing entries',async()=>{
  const {prepareWasmPackage}=await import('../../tools/ci/wasm-package.mjs');
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'napi-vm-wasm-package-'));
  try {
    fs.mkdirSync(path.join(dir,'snippets'));
    fs.writeFileSync(path.join(dir,'package.json'),JSON.stringify({files:['main.js','types.d.ts']}));
    assert.equal(prepareWasmPackage(dir),true);
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(dir,'package.json'))).files,['main.js','types.d.ts','snippets']);
    const first=fs.readFileSync(path.join(dir,'package.json'),'utf8');
    assert.equal(prepareWasmPackage(dir),false);
    assert.equal(fs.readFileSync(path.join(dir,'package.json'),'utf8'),first);
  } finally {fs.rmSync(dir,{recursive:true,force:true});}
});

test('WASM packaging preserves unrestricted manifests and packages without snippets',async()=>{
  const {prepareWasmPackage}=await import('../../tools/ci/wasm-package.mjs');
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'napi-vm-wasm-package-'));
  try {
    fs.writeFileSync(path.join(dir,'package.json'),'{"name":"fixture"}\n');
    assert.equal(prepareWasmPackage(dir),false);
    fs.mkdirSync(path.join(dir,'snippets'));
    assert.equal(prepareWasmPackage(dir),false);
    assert.equal(fs.readFileSync(path.join(dir,'package.json'),'utf8'),'{"name":"fixture"}\n');
  } finally {fs.rmSync(dir,{recursive:true,force:true});}
});
