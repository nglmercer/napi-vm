// Build the web package and retain wasm-bindgen's linked JS helpers in npm packs.
import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {spawnSync} from 'node:child_process';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');

export function prepareWasmPackage(directory) {
  const manifest=path.join(directory,'package.json');
  const data=JSON.parse(fs.readFileSync(manifest,'utf8'));
  if(!fs.existsSync(path.join(directory,'snippets')))return false;
  // Without a whitelist npm already includes the helper directory.
  if(data.files===undefined)return false;
  if(!Array.isArray(data.files))throw new Error('WASM package files must be an array');
  if(data.files.includes('snippets')||data.files.includes('snippets/'))return false;
  data.files.push('snippets');
  fs.writeFileSync(manifest,JSON.stringify(data,null,2)+'\n');
  return true;
}

if(process.argv[1]&&import.meta.url===pathToFileURL(path.resolve(process.argv[1])).href) {
  const [command,...args]=process.argv.slice(2);
  if(command==='prepare') {
    if(args.length!==1)throw new Error('prepare requires a package directory');
    prepareWasmPackage(path.resolve(args[0]));
  } else if(command==='build') {
    if(args.length!==0&&(args.length!==2||args[0]!=='--out-dir'))throw new Error('build accepts --out-dir PATH');
    const out=path.resolve(args.length?args[1]:path.join(root,'playground/pkg'));
    const result=spawnSync('wasm-pack',['build','--target','web','--out-dir',out,'--release','--',
      '--locked','--no-default-features','--features','wasm,runtime'],{cwd:root,stdio:'inherit'});
    if(result.error)throw result.error;
    if(result.status!==0)process.exit(result.status??1);
    prepareWasmPackage(out);
  } else throw new Error('expected build or prepare');
}
