import test from 'node:test';
import assert from 'node:assert/strict';
import {mkdtemp,mkdir,readFile,writeFile,rm,cp} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {capture} from '../../../tools/plugins/capture.mjs';
test('asset checks reject missing, stale and extra payloads without writing; sync uses LF and licenses',async()=>{
 const root=await mkdtemp(join(tmpdir(),'plugin-asset-check-'));
 const cli=join(root,'packages/plugin-cli');
 const write=async(path,text)=>{await mkdir(join(path,'..'),{recursive:true});await writeFile(path,text);};
 try {
  const script=join(cli,'scripts/sync-assets.mjs');await mkdir(join(cli,'scripts'),{recursive:true});await cp(new URL('../scripts/sync-assets.mjs',import.meta.url),script);
  await write(join(root,'LICENSE'),'MIT\r\n');
  for(const name of ['napi-vm-plugin-sdk','napi-vm-plugin-protocol']){
   await write(join(root,'crates',name,'Cargo.toml'),'[package]\r\nname = "'+name+'"\r\n');
   await write(join(root,'crates',name,'README.md'),'SDK\r\n');
   await write(join(root,'crates',name,'src/lib.rs'),'// fixture\r\n');
  }
  for(const name of ['greeter.interface.json','greeter.types.schema.json'])await write(join(root,'contracts/trusted-plugins/examples',name),'{}\r\n');
  for(const name of ['greeter.contract.json','greeter.ts','greeter.rs'])await write(join(root,'contracts/trusted-plugins/generated',name),name.endsWith('.json')?'{}\r\n':'// generated\r\n');
  await write(join(cli,'templates/js/src/generated/greeter.js'),'export const CONTRACT = JSON.parse("{}");\r\n');
  await capture(process.execPath,[script],{cwd:root});await capture(process.execPath,[script,'--check'],{cwd:root});
  const vendor=join(cli,'templates/rust/vendor/napi-vm-plugin-sdk');
  await assert.rejects(readFile(join(vendor,'Cargo.toml')), {code:'ENOENT'});
  assert.match(await readFile(join(vendor,'Cargo.toml.template'),'utf8'), /name = "napi-vm-plugin-sdk"/);
  assert.equal(await readFile(join(vendor,'README.md'),'utf8'),'SDK\n');assert.equal(await readFile(join(vendor,'LICENSE'),'utf8'),'MIT\n');
  const file=join(vendor,'src/lib.rs');await writeFile(file,'stale\n');
  await assert.rejects(capture(process.execPath,[script,'--check'],{cwd:root}),/Stale CLI template assets/);
  assert.equal(await readFile(file,'utf8'),'stale\n');
  await capture(process.execPath,[script],{cwd:root});
  await writeFile(join(vendor,'extra.log'),'unexpected');
  await assert.rejects(capture(process.execPath,[script,'--check'],{cwd:root}),/extra\.log \(unexpected\)/);
  assert.equal(await readFile(join(vendor,'extra.log'),'utf8'),'unexpected');
  await rm(join(vendor,'extra.log'));await rm(join(vendor,'README.md'));
  await assert.rejects(capture(process.execPath,[script,'--check'],{cwd:root}),/README\.md \(missing\)/);
 } finally {await rm(root,{recursive:true,force:true});}
});
