#!/usr/bin/env node
import { materializeManifests } from './template-manifests.mjs';
import {readFile,writeFile,mkdir,stat,lstat,realpath,readdir,cp,rm,mkdtemp,symlink} from 'node:fs/promises';
import {watch} from 'node:fs';
import {resolve,join,dirname,basename,delimiter,sep,relative} from 'node:path';
import {tmpdir} from 'node:os';
import {pathToFileURL,fileURLToPath} from 'node:url';
import {spawn,spawnSync} from 'node:child_process';
import {createInterface} from 'node:readline';
import {pack,readJson,contained,relativePath,currentTarget,prepareDevelopmentPackage} from './pack.mjs';
export {pack} from './pack.mjs';
import {generate,generateAll,canonical,generatePluginMetadata} from '@napi-vm/plugin-codegen';
const help=`napi-vm-plugin <command> [directory] [options]
create <directory> --language ts|js|rust
codegen <descriptor-or-config> [--out directory] [--check]
validate|doctor|inspect <directory>
build <directory> --format js|executable
pack <directory> --out directory
invoke <directory> --interface id --method name --input JSON [--runtime node|bun]
test <directory> --artifact [--runtime node|bun]
dev <directory> [--runtime node|bun] [--setup module]
Native invoke/test/dev additionally require --host-executable /path/to/trusted-host-rust.
Build and development are explicit compiler operations. Loading never installs or builds.
Results are JSON on stdout; logs and compiler diagnostics go to stderr.`;
const children=new Set();
const compilers=new Set();
function parse(args){const positional=[];const options={};const booleans=new Set(['help','check','artifact']);for(let i=0;i<args.length;i++){if(!args[i].startsWith('--'))positional.push(args[i]);else{const key=args[i].slice(2);if(booleans.has(key))options[key]=true;else{if(!args[i+1]||args[i+1].startsWith('--'))throw new Error('--'+key+' requires a value');options[key]=args[++i];}}}return {positional,options};}
async function run(command,args,cwd){return new Promise((res,rej)=>{const child=spawn(command,args,{cwd,shell:false,stdio:['ignore','pipe','pipe']});children.add(child);compilers.add(child);child.stdout.on('data',x=>process.stderr.write(x));child.stderr.on('data',x=>process.stderr.write(x));child.once('error',rej);child.once('close',(code,signal)=>{children.delete(child);compilers.delete(child);code===0?res():rej(new Error(`${command} exited ${code??signal}`));});});}
async function contracts(directory,manifest){return Promise.all((manifest.contracts??[]).map(async p=>readJson(await contained(directory,p))));}
async function copyRuntimeFiles(directory,destination,manifest,{dependencies=true}={}){
 await mkdir(destination,{recursive:true});
 for(const path of [...new Set([...(manifest.contracts??[]),...(manifest.assets??[])])]){const p=relativePath(path);await mkdir(dirname(join(destination,p)),{recursive:true});await cp(await contained(directory,p),join(destination,p),{recursive:true});}
 try{await cp(join(directory,'package.json'),join(destination,'package.json'));}catch(e){if(e.code!=='ENOENT')throw e;}
 if(dependencies)try{await stat(join(directory,'node_modules'));await symlink(join(directory,'node_modules'),join(destination,'node_modules'),process.platform==='win32'?'junction':'dir');}catch(e){if(e.code!=='ENOENT'&&e.code!=='EEXIST')throw e;}
}
export async function build(directory,format,{stage}={}){
 const config=await readJson(join(directory,'plugin.build.json'));const entry=await contained(directory,config.entry);const output=relativePath(config.output);const destination=stage??directory;
 if(!['js','executable'].includes(format))throw new Error('--format must be js or executable');
 if(!['ts','js','rust'].includes(config.language))throw new Error('Unknown build language');
 let manifest=await readJson(join(directory,'plugin.json'));
 if(stage)await copyRuntimeFiles(directory,stage,manifest);
 if(config.language==='rust'){
  if(format!=='executable')throw new Error('Rust requires --format executable');
  if(typeof config.binary!=='string'||!/^[A-Za-z0-9_-]+$/.test(config.binary))throw new Error('Rust build config needs a binary name');
  await run('cargo',['build','--release','--manifest-path',join(directory,'Cargo.toml'),'--bin',config.binary],directory);
  const metadata=spawnSync('cargo',['metadata','--no-deps','--format-version','1','--manifest-path',join(directory,'Cargo.toml')],{encoding:'utf8',shell:false,cwd:directory});
  if(metadata.error||metadata.status!==0)throw new Error('cargo metadata failed: '+(metadata.error?.message??metadata.stderr));
  const built=join(JSON.parse(metadata.stdout).target_directory,'release',config.binary+(process.platform==='win32'?'.exe':''));
  const finalOutput=output+(process.platform==='win32'&&!output.endsWith('.exe')?'.exe':'');
  await mkdir(dirname(join(destination,finalOutput)),{recursive:true});await cp(built,join(destination,finalOutput));
  manifest={...manifest,profile:'native-executable',launch:{kind:'executable',entry:'./'+finalOutput,args:manifest.launch.args??[],target:currentTarget()}};
 }else if(format==='executable'){
  const binary='bin/plugin'+(process.platform==='win32'?'.exe':'');await mkdir(dirname(join(destination,binary)),{recursive:true});
  await run('bun',['build',entry,'--compile','--outfile',join(destination,binary)],directory);
  manifest={...manifest,profile:'native-executable',launch:{kind:'executable',entry:'./'+binary,args:manifest.launch.args??[],target:currentTarget()}};
  // Standalone artifacts do not need JS source or dependency trees.
  if(manifest.extensions?.['napi-vm.pack'])manifest.extensions={'napi-vm.pack':{include:[]}};
 }else{
  const compiler=fileURLToPath(import.meta.resolve('typescript/bin/tsc'));
  await run(process.execPath,[compiler,'-p',join(directory,'tsconfig.json'),...(stage?['--outDir',join(stage,'dist')]:[])],directory);
  manifest={...manifest,profile:manifest.profile==='native-executable'?'external-runtime':manifest.profile??'external-runtime',launch:{kind:'javascript',entry:'./'+output,args:manifest.launch.args??[],...(manifest.launch.kind==='javascript'&&manifest.launch.versions?{versions:manifest.launch.versions}:{}),runtimes:config.runtimes??manifest.launch.runtimes??['bun'],preferredRuntime:config.preferredRuntime??manifest.launch.preferredRuntime??'bun'}};
 }
 if(stage)delete manifest.development;
 await writeFile(join(destination,'plugin.json'),JSON.stringify(manifest,null,2)+'\n');
 // A rebuild invalidates the old inventory. Packaging creates a new one explicitly.
 await rm(join(destination,'plugin.lock.json'),{force:true});
 return {built:true,format,directory:destination,output:manifest.launch.entry,target:manifest.launch.target??null};
}
async function rustHost(directory,options){
 if(typeof options['host-executable']!=='string')throw new Error('Native executable plugins require a Rust host. Supply --host-executable /path/to/trusted-host-rust; the JavaScript host does not load native executables.');
 const executable=resolve(options['host-executable']);const child=spawn(executable,['serve',join(directory,'plugin.json'),...(typeof options.runtime==='string'?[options.runtime]:[])],{cwd:directory,shell:false,stdio:['pipe','pipe','pipe']});children.add(child);
 child.stderr.on('data',x=>process.stderr.write(x));const lines=createInterface({input:child.stdout});const queue=[];let readyResolve,readyReject,ended=false;
 const ready=new Promise((res,rej)=>{readyResolve=res;readyReject=rej;});
 const fail=error=>{readyReject(error);while(queue.length)queue.shift().reject(error);};
 lines.on('line',line=>{let value;try{value=JSON.parse(line);}catch{fail(new Error('Rust host emitted non-JSON stdout'));return;}if(value.ready){readyResolve(value);return;}const pending=queue.shift();if(pending)value.ok===false?pending.reject(new Error(JSON.stringify(value.error))):pending.resolve(value.result);});
 child.once('error',fail);child.once('close',(code,signal)=>{ended=true;children.delete(child);fail(new Error('Rust host exited '+(code??signal)));});
 const timer=setTimeout(()=>{fail(new Error('Rust host readiness timeout'));child.kill();},15_000);
 try{await ready;}catch(e){child.kill();throw e;}finally{clearTimeout(timer);}
 function request(value){if(ended)return Promise.reject(new Error('Rust host is closed'));return new Promise((resolve,reject)=>{queue.push({resolve,reject});child.stdin.write(JSON.stringify(value)+'\n',error=>{if(error)fail(error);});});}
 return {invoke:(contract,method,input)=>request({command:'invoke',interface:contract.descriptor.id,method,input}),reload:path=>request({command:'reload',manifest:path}),shutdown:async()=>{if(ended)return;const killed=setTimeout(()=>child.kill('SIGKILL'),6000);try{await request({command:'shutdown'});}catch(e){if(!ended)throw e;}finally{child.stdin.end();if(!ended)await new Promise(res=>child.once('close',res));clearTimeout(killed);lines.close();}}};
}
async function hostFor(directory,options){
 const manifest=await readJson(join(directory,'plugin.json'));const cs=await contracts(directory,manifest);
 if(manifest.launch.kind==='executable'){if(options.setup)throw new Error('--setup JavaScript modules are supported by the JavaScript development host; configure services in the selected Rust host executable');const host=await rustHost(directory,options);return {host,handle:host,manifest,contracts:cs,native:true};}
 const {TrustedPluginHost}=await import('@napi-vm/plugin-host');const host=new TrustedPluginHost({contracts:cs});
 try{
  if(options.setup){const setup=await import(pathToFileURL(resolve(options.setup)).href);await setup.default(host);}
  else for(const c of cs)if(c.descriptor.id==='app.configuration')host.registerService(c,{get:async()=>({value:'Hola'})});
  const handle=await host.load(join(directory,'plugin.json'),{...(typeof options.runtime==='string'?{runtime:options.runtime}:{}),development:options.development===true});return {host,handle,manifest,contracts:cs};
 }catch(error){await host.shutdown();throw error;}
}
async function invoke(directory,options){
 if(typeof options.interface!=='string'||typeof options.method!=='string')throw new Error('--interface and --method are required');
 const input=JSON.parse(String(options.input??'{}'));const state=await hostFor(directory,options);
 try{const c=state.contracts.find(c=>c.descriptor.id===options.interface);if(!c)throw new Error('Unknown interface');return await state.handle.invoke(c,options.method,input);}finally{await state.host.shutdown();}
}
async function artifactTest(directory,options){
 const fixture=await readJson(join(directory,'plugin.test.json'));if(!Array.isArray(fixture.scenarios)||!fixture.scenarios.length)throw new Error('plugin.test.json must contain nonempty scenarios');
 const results=[];let temporary;let artifactDirectory=directory;try{await stat(join(directory,'plugin.lock.json'));}catch(e){if(e.code!=='ENOENT')throw e;temporary=await mkdtemp(join(tmpdir(),'napi-vm-artifact-'));artifactDirectory=join(temporary,'package');try{await pack(directory,artifactDirectory);}catch(error){await rm(temporary,{recursive:true,force:true});throw error;}}
 let state;try{state=await hostFor(artifactDirectory,options);for(const scenario of fixture.scenarios){const c=state.contracts.find(c=>c.descriptor.id===scenario.interface);if(!c)throw new Error('Unknown scenario interface');const result=await state.handle.invoke(c,scenario.method,scenario.input);if(canonical(result)!==canonical(scenario.expected))throw new Error('Scenario mismatch: '+scenario.method);results.push({method:scenario.method,result});}return {passed:results.length,results};}finally{if(state)await state.host.shutdown();if(temporary)await rm(temporary,{recursive:true,force:true});}
}
async function dev(directory,options){
 const config=await readJson(join(directory,'plugin.build.json'));const format=config.language==='rust'?'executable':'js';const stages=[];const watchers=[];let state,running=false,pending=false,closed=false,timer,operation=Promise.resolve();let resolveDone;const stopped=new Promise(res=>resolveDone=res);
 async function stageBuild(){const stage=await mkdtemp(join(tmpdir(),'napi-vm-dev-'));stages.push(stage);await build(directory,format,{stage});const launchDirectory=await prepareDevelopmentPackage(stage,format);if(launchDirectory!==stage)stages.push(launchDirectory);return launchDirectory;}
 async function reload(){if(closed)return;if(running){pending=true;return;}running=true;try{do{pending=false;try{const stage=await stageBuild();if(closed)break;if(state.native)await state.host.reload(join(stage,'plugin.json'));else await state.host.reload(state.handle.instanceId??state.handle.id,join(stage,'plugin.json'));process.stderr.write('Reloaded successfully\n');}catch(e){if(!closed)process.stderr.write('Build/reload failed: '+e.message+'\n');}}while(pending&&!closed);}finally{running=false;}}
 async function stop(){if(closed)return;closed=true;clearTimeout(timer);for(const watcher of watchers)watcher.close();for(const child of compilers)child.kill('SIGTERM');resolveDone();}
 process.once('SIGINT',stop);process.once('SIGTERM',stop);
 try{
  const stage=await stageBuild();if(closed)return {stopped:true};state=await hostFor(stage,{...options,development:true});
  const watchPaths=['src','contracts',...(await readJson(join(directory,'plugin.json'))).assets??[],'plugin.json','plugin.build.json','tsconfig.json','Cargo.toml'];
  for(const sub of watchPaths)try{const p=join(directory,sub);const info=await stat(p);watchers.push(watch(p,{recursive:info.isDirectory()},()=>{clearTimeout(timer);timer=setTimeout(()=>{if(running)pending=true;else operation=reload();},100);}));}catch(e){if(e.code!=='ENOENT')throw e;}
  process.stderr.write('Development host ready. Watching sources; Ctrl-C stops all owned processes.\n');await stopped;await operation;return {stopped:true};
 }finally{closed=true;clearTimeout(timer);for(const w of watchers)w.close();process.removeListener('SIGINT',stop);process.removeListener('SIGTERM',stop);if(state)await state.host.shutdown();for(const path of stages)await rm(path,{recursive:true,force:true});}
}
async function doctor(directory){
 const {readManifest,validateManifest}=await import('@napi-vm/plugin-host');let manifest;try{manifest=validateManifest(await readJson(join(directory,'plugin.json')));}catch(e){return {validation:{valid:false,error:e.message},target:null,hostTarget:currentTarget(),runtimes:[]};}let validation;
 try{await readManifest(join(directory,'plugin.json'));validation={valid:true};}catch(e){validation={valid:false,error:e.message};}
 const runtimes=[];
 for(const runtime of manifest.launch.runtimes??[]){let path=null;for(const dir of (process.env.PATH??'').split(delimiter))try{const p=join(dir,runtime+(process.platform==='win32'?'.exe':''));await stat(p);path=p;break;}catch{}const r=spawnSync(path??runtime,['--version'],{encoding:'utf8',timeout:3000,shell:false});runtimes.push({runtime,path,available:!r.error&&r.status===0,version:r.stdout?.trim()??null,error:r.error?.message??null});}
 return {validation,target:manifest.launch.target??null,hostTarget:currentTarget(),runtimes};
}
async function vendorLocalPackages(directory) {
 const names=['plugin-protocol','plugin-sdk','plugin-host','plugin-codegen','plugin-cli'];
 const sourceRoots=new Map();
 for(const name of names){let source=dirname(fileURLToPath(import.meta.resolve('@napi-vm/'+name)));for(;;){try{const metadata=await readJson(join(source,'package.json'));if(metadata.name==='@napi-vm/'+name){sourceRoots.set(name,{source,metadata});break;}}catch(error){if(error.code!=='ENOENT')throw error;}const parent=dirname(source);if(parent===source)throw new Error('Cannot locate installed package '+name);source=parent;}}
 for(const [name,{source,metadata}]of sourceRoots){
  const target=join(directory,'vendor',name);await mkdir(target,{recursive:true});
  const files=[...(metadata.files??[]),'README.md','LICENSE','LICENSE.md'];
  for(const file of files){const path=relativePath(file);if(path.includes('*'))throw new Error('Unsupported package payload glob: '+path);const from=join(source,path);try{await cp(from,join(target,path),{recursive:true,filter:async input=>{const parts=relative(source,input).split(sep);if(input===directory||input.startsWith(directory+sep)||parts.some(part=>['node_modules','target','.git','.cache','.npm','__pycache__'].includes(part)))return false;if((await lstat(input)).isSymbolicLink())throw new Error('Refusing symlink in vendored package payload: '+relative(source,input));return true;}});}catch(error){if(error.code==='ENOENT'&&(path.startsWith('LICENSE')||path==='README.md'))continue;if(error.code==='ENOENT')throw new Error('Installed '+metadata.name+' is missing its '+path+' payload; build or install that local package before creating a project');throw error;}}
  const rewritten=structuredClone(metadata);
  for(const group of ['dependencies','devDependencies','optionalDependencies'])for(const dep of Object.keys(rewritten[group]??{}))if(dep.startsWith('@napi-vm/')){const local=dep.slice('@napi-vm/'.length);if(!names.includes(local))throw new Error('Unvendored local dependency: '+dep);rewritten[group][dep]='file:../'+local;}
  await writeFile(join(target,'package.json'),JSON.stringify(rewritten,null,2)+'\n');
 }
 const packagePath=join(directory,'package.json');const metadata=await readJson(packagePath);
 for(const group of ['dependencies','devDependencies'])for(const dep of Object.keys(metadata[group]??{}))if(dep.startsWith('@napi-vm/'))metadata[group][dep]='file:vendor/'+dep.slice('@napi-vm/'.length);
 await writeFile(packagePath,JSON.stringify(metadata,null,2)+'\n');
}
async function create(directory,language){
 if(!['ts','js','rust'].includes(language))throw new Error('language must be ts, js or rust');
 try{await lstat(directory);throw new Error('Refusing to overwrite existing directory');}catch(e){if(e.code!=='ENOENT')throw e;}
 await mkdir(directory,{recursive:false});try{const template=fileURLToPath(new URL('../templates/'+language+'/',import.meta.url));for(const entry of await readdir(template))await cp(join(template,entry),join(directory,entry),{recursive:true,errorOnExist:true,force:false});if(language!=='rust')await vendorLocalPackages(directory);if(language==='rust'){await materializeManifests(directory);const manifest=await readJson(join(directory,'plugin.json'));manifest.launch.target=currentTarget();if(process.platform==='win32')manifest.launch.entry+='.exe';await writeFile(join(directory,'plugin.json'),JSON.stringify(manifest,null,2)+'\n');}return {created:directory,language};}catch(e){await rm(directory,{recursive:true,force:true});throw e;}
}
async function codegen(file,options){
 const config=await readJson(file);if(config.descriptorVersion)return generate(file,resolve(String(options.out??join(dirname(file),'generated'))),{check:options.check===true});
 if(!Array.isArray(config.contracts)||!config.contracts.length)throw new Error('Codegen config requires contracts: [{input, out}]');const results=[];
 for(const contract of config.contracts)results.push(await generate(resolve(dirname(file),contract.input),resolve(dirname(file),contract.out),{check:options.check===true}));if(config.plugin){const required=[];for(const input of config.plugin.requiresHost??[])required.push(await readJson(resolve(dirname(file),input)));results.push({metadata:await generatePluginMetadata({...config.plugin,requiresHost:required},resolve(dirname(file),config.metadataOut??'generated'),{check:options.check===true})});}return results;
}
export async function main(args=process.argv.slice(2)){
 const {positional,options}=parse(args);const [command,path]=positional;if(!command||options.help||command==='help')return {help};if(positional.length>2)throw new Error('Unexpected positional arguments; use named options shown by help');const allowed={create:['language'],codegen:['out','check'],pack:['out'],build:['format'],invoke:['interface','method','input','runtime','host-executable','setup'],test:['artifact','runtime','host-executable','setup'],dev:['runtime','host-executable','setup'],inspect:[],validate:[],doctor:[]}[command];if(allowed)for(const key of Object.keys(options))if(!allowed.includes(key))throw new Error('Unknown option --'+key+' for '+command);const directory=resolve(path??'.');
 switch(command){case 'codegen':return path?codegen(directory,options):generateAll({check:options.check===true});case 'pack':if(typeof options.out!=='string')throw new Error('--out is required');return pack(directory,options.out);case 'build':return build(directory,String(options.format??'js'));case 'invoke':return invoke(directory,options);case 'test':if(!options.artifact)throw new Error('--artifact required');return artifactTest(directory,options);case 'dev':return dev(directory,options);case 'inspect':{const manifest=await readJson(join(directory,'plugin.json'));return {manifest,contracts:await contracts(directory,manifest)};}case 'validate':{const {readManifest}=await import('@napi-vm/plugin-host');const result=await readManifest(join(directory,'plugin.json'));return {valid:true,manifest:result.manifest,contracts:result.contracts};}case 'doctor':return doctor(directory);case 'create':return create(directory,String(options.language??'ts'));default:throw new Error('Unknown command: '+command);}
}
if(process.argv[1]&&await realpath(process.argv[1]).catch(()=>null)===await realpath(fileURLToPath(import.meta.url)))main().then(result=>console.log(JSON.stringify(result,null,2))).catch(e=>{console.error(JSON.stringify({error:e.message,code:e.code??'CLI_ERROR'}));process.exitCode=1;});
