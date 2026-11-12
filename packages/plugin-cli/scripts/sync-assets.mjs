// Deterministic, licensed scaffolding payload. Never part of production plugin loading.
import {access,mkdir,readFile,writeFile,rm,readdir} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
import {join,resolve,dirname,relative} from 'node:path';
const cli=fileURLToPath(new URL('../',import.meta.url));
const root=resolve(cli,'../..');
const check=process.argv.includes('--check');
try {await access(join(root,'crates/napi-vm-plugin-sdk/Cargo.toml'));}
catch {
 await access(join(cli,'templates/rust/vendor/napi-vm-plugin-sdk/src/lib.rs'));
 process.stderr.write('Using packaged template assets\n');
 process.exit(0);
}
const expected=new Map();
const lf=bytes=>Buffer.from(bytes.toString('utf8').replace(/\r\n?/g,'\n'));
async function add(source,destination) {expected.set(destination,lf(await readFile(source)));}
async function tree(source,destination) {
 for(const item of (await readdir(source,{withFileTypes:true})).sort((a,b)=>a.name<b.name?-1:a.name>b.name?1:0)) {
  const from=join(source,item.name),to=join(destination,item.name);
  if(item.isDirectory())await tree(from,to);
  else if(item.isFile())await add(from,to);
  else throw new Error('Unsupported template source entry: '+from);
 }
}
let license;
for(const name of ['LICENSE','LICENSE.md'])try{await access(join(root,name));license=name;break;}catch(error){if(error.code!=='ENOENT')throw error;}
if(!license)throw new Error('Rust template vendoring requires the repository license');
const vendorRoots=[];
for(const name of ['napi-vm-plugin-sdk','napi-vm-plugin-protocol']) {
 const source=join(root,'crates',name),destination=join(cli,'templates/rust/vendor',name);vendorRoots.push(destination);
 await tree(join(source,'src'),join(destination,'src'));
 for(const name of ['Cargo.toml','README.md'])await add(join(source,name),join(destination,name==='Cargo.toml'?'Cargo.toml.template':name));
 await add(join(root,license),join(destination,license));
}
for(const language of ['ts','js','rust']) {
 for(const name of ['greeter.interface.json','greeter.types.schema.json'])await add(join(root,'contracts/trusted-plugins/examples',name),join(cli,'templates',language,'contracts',name));
 await add(join(root,'contracts/trusted-plugins/generated/greeter.contract.json'),join(cli,'templates',language,'contracts/greeter.contract.json'));
}
await add(join(root,'contracts/trusted-plugins/generated/greeter.ts'),join(cli,'templates/ts/src/generated/greeter.ts'));
await add(join(root,'contracts/trusted-plugins/generated/greeter.rs'),join(cli,'templates/rust/src/generated.rs'));
const contract=JSON.parse(await readFile(join(root,'contracts/trusted-plugins/generated/greeter.contract.json'),'utf8'));
const js=join(cli,'templates/js/src/generated/greeter.js');
const text=(await readFile(js,'utf8')).replace(/\r\n?/g,'\n').replace(/export const CONTRACT = JSON.parse\(.*\);/,`export const CONTRACT = JSON.parse(${JSON.stringify(JSON.stringify(contract))});`);
expected.set(js,Buffer.from(text));
if(check) {
 const stale=[];
 for(const [path,bytes] of expected)try{if(!(await readFile(path)).equals(bytes))stale.push(relative(cli,path));}catch(error){if(error.code!=='ENOENT')throw error;stale.push(relative(cli,path)+' (missing)');}
 async function checkExtras(directory) {
  let entries;try{entries=await readdir(directory,{withFileTypes:true});}catch(error){if(error.code==='ENOENT')return;throw error;}
  for(const item of entries){const path=join(directory,item.name);if(item.isDirectory())await checkExtras(path);else if(!expected.has(path))stale.push(relative(cli,path)+' (unexpected)');}
 }
 for(const directory of vendorRoots)await checkExtras(directory);
 if(stale.length)throw new Error('Stale CLI template assets; run npm run prepare:templates -w @napi-vm/plugin-cli:\n'+stale.sort().join('\n'));
 process.stderr.write('CLI template assets are current\n');
} else {
 for(const directory of vendorRoots)await rm(directory,{recursive:true,force:true});
 for(const [path,bytes] of expected){await mkdir(dirname(path),{recursive:true});await writeFile(path,bytes);}
 process.stderr.write('Prepared deterministic contract bindings and independent licensed Rust SDK assets\n');
}
