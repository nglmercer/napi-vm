#!/usr/bin/env node
import {readFile,writeFile,mkdir,readdir,realpath,mkdtemp,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {dirname,resolve,relative,basename,join,sep,isAbsolute} from 'node:path';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
const IDENT=/^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)*$/;
const VERSION=/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
const allowed=new Set(['$schema','$defs','$ref','title','description','type','properties','required','additionalProperties','items','minItems','maxItems','minLength','maxLength','minimum','maximum','enum','const','oneOf','x-wire-type']);
export function canonical(value){if(Array.isArray(value))return '['+value.map(canonical).join(',')+']';if(value&&typeof value==='object')return '{'+Object.keys(value).sort().map(k=>JSON.stringify(k)+':'+canonical(value[k])).join(',')+'}';return JSON.stringify(value);}
const fail=(where,msg)=>{throw new Error(`${where}: ${msg}`)};
const plain=x=>x!==null&&typeof x==='object'&&!Array.isArray(x);
const pointer=s=>s.replaceAll('~','~0').replaceAll('/','~1');
function safeName(s){const r=s.replace(/[^a-zA-Z0-9_]/g,'_');const n=/^[A-Za-z_]/.test(r)?r:'_'+r;return ['_','abstract','as','asserts','async','await','break','case','catch','class','continue','debugger','declare','default','delete','do','else','export','extends','false','finally','for','from','function','get','if','implements','import','in','infer','instanceof','interface','is','keyof','let','module','namespace','new','of','package','private','protected','public','readonly','require','return','set','static','switch','symbol','this','throw','true','try','typeof','var','void','while','with','yield','fn','dyn','impl','trait','mod','move','mut','ref','pub','unsafe','use','where','loop','match','Self','self','super','crate','type','const','enum','struct','String','Option','Vec','CONTRACT','Contract','CallOptions','Emitter','std','serde','serde_json','napi_vm_plugin_protocol','napi_vm_plugin_sdk','CallContext','LifecycleHooks','RuntimeHandler','PluginError','JsonValue','Client','Handler','Handlers','Invoker','Promise','Array','unknown','any','never','number','string','boolean'].includes(n)?n+'_Type':n;}
function rustName(s){let n=s.replace(/[^a-zA-Z0-9_]/g,'_').replace(/([a-z0-9])([A-Z])/g,'$1_$2').toLowerCase();if(!/^[A-Za-z_]/.test(n))n='_'+n;return ['self','super','crate','_'].includes(n)?n+'_field':'r#'+n;}
const invalidScalar=/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u;
function parseSource(text, file) {
 const value=JSON.parse(text);
 function inspect(item, pointerPath, depth) {
  if(depth>128)fail(file+pointerPath,'source nesting exceeds128');
  if(typeof item==='string'&&invalidScalar.test(item))fail(file+pointerPath,'invalid Unicode scalar sequence');
  if(item&&typeof item==='object')for(const key of Object.keys(item)){if(invalidScalar.test(key))fail(file+pointerPath,'invalid Unicode scalar property name');inspect(item[key],pointerPath+'/'+pointer(key),depth+1);}
 }
 inspect(value,'#',0);return value;
}
function validWireLiteral(value, kind) {
 try { switch(kind) {
  case 'i64-decimal': return /^(0|-[1-9][0-9]*|[1-9][0-9]*)$/.test(value)&&BigInt(value)>=-(1n<<63n)&&BigInt(value)<(1n<<63n);
  case 'u64-decimal': return /^(0|[1-9][0-9]*)$/.test(value)&&BigInt(value)<(1n<<64n);
  case 'bytes-base64': return Buffer.from(value,'base64').toString('base64')===value;
  case 'utc-datetime': return /^(?!0000)\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(value)&&new Date(value).toISOString()===value;
  default: return false;
 }} catch { return false; }
}
export async function normalize(file){
 file=await realpath(file);const base=dirname(file);const descriptor=parseSource(await readFile(file,'utf8'),file);if(!plain(descriptor))fail(file,'descriptor must be an object');
 for(const k of Object.keys(descriptor))if(!['descriptorVersion','id','version','types','methods','errors','events','state','title','description'].includes(k))fail(file+'/'+k,'unknown descriptor field');
 if(descriptor.descriptorVersion!==1||typeof descriptor.id!=='string'||descriptor.id.length>128||!IDENT.test(descriptor.id)||typeof descriptor.version!=='string'||descriptor.version.length>128||!VERSION.test(descriptor.version))fail(file,'invalid descriptor version/id/version');
 if(typeof descriptor.types!=='string'||!descriptor.types.startsWith('./'))fail(file,'types must be a local relative file');
 const sources=new Map();const defs=Object.create(null);const names=new Map();let count=0;
 async function load(path){path=await realpath(path);const rel=relative(base,path);if(!rel||rel==='..'||rel.startsWith('..'+sep)||isAbsolute(rel))fail(path,'reference escapes contract directory');if(!sources.has(path)){if(sources.size>=64)fail(path,'too many referenced files');const doc=parseSource(await readFile(path,'utf8'),path);if(!plain(doc))fail(path,'schema document must be an object');for(const k of Object.keys(doc))if(!['$schema','$defs','title','description'].includes(k))fail(path+'/'+pointer(k),'unsupported document keyword');for(const k of ['title','description'])if(doc[k]!==undefined&&typeof doc[k]!=='string')fail(path+'/'+k,'metadata must be a string');if(doc.$schema!==undefined&&doc.$schema!=='https://json-schema.org/draft/2020-12/schema')fail(path+'/$schema','draft 2020-12 required');sources.set(path,doc);}return sources.get(path);}
 const rootFile=resolve(base,descriptor.types);const root=await load(rootFile);
 function get(document,fragment,where){if(!fragment.startsWith('/')||/~(?![01])/u.test(fragment))fail(where,'reference must have a JSON pointer');let x=document;for(const t of fragment.slice(1).split('/')){const k=t.replaceAll('~1','/').replaceAll('~0','~');if(!plain(x)||!Object.hasOwn(x,k))fail(where,'missing reference '+fragment);x=x[k];}return x;}
 async function ref(ref,current,stack){if(typeof ref!=='string'||/^[a-z]+:/i.test(ref)||ref.includes('\\'))fail(current,'remote/invalid reference');const [name,frag,...extra]=ref.split('#');if(extra.length||!frag)fail(current,'reference must include a JSON pointer');if(name&&(name.startsWith('/')||name.includes('?')))fail(current,'reference must be a relative local file');const path=await realpath(name?resolve(dirname(current),name):current);const key=path+'#'+frag;if(stack.includes(key))fail(key,'recursive reference');if(stack.length>=32)fail(key,'reference depth exceeds32');if(!names.has(key)){if(names.size>=1024)fail(key,'too many generated definitions');const suffix=safeName(frag.split('/').at(-1));const n=path===rootFile&&frag.startsWith('/$defs/')?suffix:'Ref'+(++count)+'_'+suffix;if(Object.hasOwn(defs,n))fail(key,'generated type name collision');names.set(key,n);defs[n]=null;defs[n]=await visit(get(await load(path),frag,key),path,[...stack,key],key);}return '#/$defs/'+names.get(key);}
 async function visit(s,current,stack,where){if(stack.length+(where.split('#').at(-1)?.split('/').length??0)>64)fail(where,'schema nesting exceeds64');if(!plain(s))fail(where,'schema must be an object');for(const k of ['$schema','$defs'])if(Object.hasOwn(s,k))fail(where+'/'+k,'keyword only allowed at schema document root');for(const k of ['title','description'])if(s[k]!==undefined&&typeof s[k]!=='string')fail(where+'/'+k,'metadata must be a string');for(const k of Object.keys(s))if(!allowed.has(k))fail(where+'/'+pointer(k),'unsupported keyword');
 if(s.$ref!==undefined){if(Object.keys(s).some(k=>!['$ref','description','title'].includes(k)))fail(where,'$ref siblings not supported');return {$ref:await ref(s.$ref,current,stack)};}
 if(s.oneOf!==undefined){if(Object.keys(s).some(k=>!['oneOf','title','description'].includes(k)))fail(where,'oneOf siblings not supported');if(!Array.isArray(s.oneOf)||s.oneOf.length<2||s.oneOf.length>32)fail(where,'oneOf requires2–32 tagged object variants');const variants=[];for(let i=0;i<s.oneOf.length;i++)variants.push(await visit(s.oneOf[i],current,stack,where+'/oneOf/'+i));const tag=Object.keys(variants[0]?.properties??{}).find(k=>variants.every(v=>v.type==='object'&&v.required?.includes(k)&&typeof v.properties?.[k]?.const==='string')&&new Set(variants.map(v=>v.properties[k].const)).size===variants.length);if(!tag)fail(where,'oneOf variants require a shared required unique string const discriminator');return {oneOf:variants};}
 const types=Array.isArray(s.type)?s.type:[s.type];if(Array.isArray(s.type)&&(types.length!==2||!types.includes('null')||new Set(types).size!==2))fail(where,'nullable syntax is type:[nonNullType,"null"]');
 if(types.some(t=>!['object','array','string','boolean','null','integer','number'].includes(t)))fail(where,'unsupported or missing type');
 if(Array.isArray(s.type)&&(s.enum!==undefined||s.const!==undefined))fail(where,'nullable enum/const combination is not supported');const t=types.find(t=>t!=='null')??'null';const out={...s};delete out.$schema;delete out.$defs;
 const allowedForType={object:['properties','required','additionalProperties'],array:['items','minItems','maxItems'],string:['minLength','maxLength','enum','const','x-wire-type'],integer:['minimum','maximum','const'],number:['minimum','maximum','const'],boolean:['const'],null:['const']};
 for(const k of Object.keys(out))if(!['type','title','description',...allowedForType[t]].includes(k))fail(where+'/'+k,'keyword not valid for type '+t);
 if(t==='object'){if(!plain(s.properties)||s.additionalProperties!==false||!Array.isArray(s.required)||new Set(s.required).size!==s.required.length||s.required.some(k=>typeof k!=='string'||!Object.hasOwn(s.properties,k)))fail(where,'objects require properties, unique required and additionalProperties:false');out.properties=Object.create(null);for(const k of Object.keys(s.properties).sort())out.properties[k]=await visit(s.properties[k],current,stack,where+'/properties/'+pointer(k));}
 if(t==='array')out.items=await visit(s.items,current,stack,where+'/items');
 for(const [min,max] of [['minLength','maxLength'],['minItems','maxItems'],['minimum','maximum']]){for(const k of [min,max])if(s[k]!==undefined&&(!Number.isFinite(s[k])||(!['minimum','maximum'].includes(k)&&(!Number.isSafeInteger(s[k])||s[k]<0))))fail(where+'/'+k,'invalid bound');if(s[min]!==undefined&&s[max]!==undefined&&s[min]>s[max])fail(where,'conflicting bounds');}
 if(s.enum!==undefined&&(!Array.isArray(s.enum)||s.enum.length===0||s.enum.some(v=>typeof v!=='string')||new Set(s.enum).size!==s.enum.length))fail(where,'enum requires unique strings');
 if(s.enum&&s.const!==undefined&&!s.enum.includes(s.const))fail(where,'const conflicts with enum');if(t==='string')for(const v of [...(s.enum??[]),...(s.const===undefined?[]:[s.const])]){if(typeof v!=='string'||/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u.test(v))fail(where,'invalid Unicode string literal');if(s['x-wire-type']&&!validWireLiteral(v,s['x-wire-type']))fail(where,'literal conflicts with wire annotation');if([...v].length<(s.minLength??0)||[...v].length>(s.maxLength??Infinity))fail(where,'literal conflicts with length bounds');}
 if(s.const!==undefined){const expected=t==='integer'?'number':t;if((t==='null'?s.const!==null:typeof s.const!==expected)||(typeof s.const==='number'&&(!Number.isFinite(s.const)||t==='integer'&&!Number.isSafeInteger(s.const))))fail(where,'const must match schema type');if(typeof s.const==='number'&&(s.const<(s.minimum??-Infinity)||s.const>(s.maximum??Infinity)))fail(where,'const conflicts with numeric bounds');}
 if(s['x-wire-type']!==undefined&&!['i64-decimal','u64-decimal','bytes-base64','utc-datetime'].includes(s['x-wire-type']))fail(where,'unsupported wire type');
 return out;}
 if(!plain(root.$defs))fail(rootFile,'$defs required');if(Object.keys(root.$defs).length>1024)fail(rootFile,'too many definitions');for(const key of Object.keys(root.$defs).sort())await ref('#/$defs/'+pointer(key),rootFile,[]);
 const d=JSON.parse(canonical(descriptor));delete d.types;for(const k of ['title','description'])if(d[k]!==undefined&&typeof d[k]!=='string')fail(file+'/'+k,'metadata must be a string');
 for(const category of ['methods','errors','events']){if(!plain(d[category]))fail(file+'/'+category,'object required');if(Object.keys(d[category]).length>256)fail(file+'/'+category,'too many members');for(const [name,item] of Object.entries(d[category])){if(!name||name.length>200||!plain(item))fail(file+'/'+category,'invalid member');const keys=category==='methods'?['input','output','errors','idempotent','description']:category==='errors'?['data','description']:['payload','description'];for(const k of Object.keys(item))if(!keys.includes(k))fail(file+'/'+category+'/'+name+'/'+k,'unknown field');for(const k of category==='methods'?['input','output']:category==='errors'?['data']:['payload'])item[k]=await ref(item[k],rootFile,[]);if(category==='methods'){if(!Array.isArray(item.errors)||new Set(item.errors).size!==item.errors.length||item.errors.some(e=>!Object.hasOwn(d.errors,e)))fail(file+'/'+name,'undeclared domain errors');if(item.idempotent!==undefined&&typeof item.idempotent!=='boolean')fail(file+'/'+name,'invalid idempotent');}}}
 if(d.state){if(!plain(d.state)||Object.keys(d.state).some(k=>!['contract','version','schema'].includes(k))||!IDENT.test(d.state.contract)||!Number.isSafeInteger(d.state.version)||d.state.version<1)fail(file+'/state','invalid state identity');d.state.schema=await ref(d.state.schema,rootFile,[]);}
 const body={descriptor:d,schemas:{$defs:defs}};const bytes=canonical(body)+'\n';return JSON.parse(canonical({...body,digest:createHash('sha256').update(bytes).digest('hex')}));
}
function tsType(s){if(s.$ref)return safeName(s.$ref.split('/').at(-1));if(s.oneOf)return s.oneOf.map(tsType).map(t=>'('+t+')').join(' | ');if(Array.isArray(s.type))return tsType({...s,type:s.type.find(t=>t!=='null')})+' | null';if(s.const!==undefined)return JSON.stringify(s.const);if(s.enum)return s.enum.map(x=>JSON.stringify(x)).join(' | ');switch(s.type){case 'object':return '{ '+Object.entries(s.properties).map(([k,v])=>JSON.stringify(k)+(s.required.includes(k)?'':'?')+': '+tsType(v)).join('; ')+' }';case 'array':return 'Array<'+tsType(s.items)+'>';case 'integer':case 'number':return 'number';default:return s.type;}}
function tsOutput(c) {
 let out='// Generated. Do not edit.\nimport { validate, PluginError, type Contract, type CallOptions } from "@napi-vm/plugin-protocol";\nimport { definePlugin, type CallContext, type LifecycleHooks, type Handler as RuntimeHandler } from "@napi-vm/plugin-sdk";\n';
 for(const [name,s] of Object.entries(c.schemas.$defs)) out+='export type '+safeName(name)+' = '+tsType(s)+';\n';
 // JSON.parse preserves prototype-sensitive keys such as __proto__ as ordinary data.
 out+='export const CONTRACT: Contract = JSON.parse('+JSON.stringify(JSON.stringify(c))+') as Contract;\n';
 for(const name of Object.keys(c.schemas.$defs)) out+='export function validate'+safeName(name)+'(value: unknown): '+safeName(name)+' { return validate(CONTRACT, '+JSON.stringify('#/$defs/'+pointer(name))+', value) as '+safeName(name)+'; }\n';
 out+='export interface Invoker { invoke(contract: Contract, method: string, input: unknown, options?: CallOptions): Promise<unknown> }\n';
 out+='export function client(target: Invoker) { return {\n';
 for(const [name,m]of Object.entries(c.descriptor.methods)) out+='['+JSON.stringify(name)+']: async (input: '+tsType({$ref:m.input})+', options?: CallOptions): Promise<'+tsType({$ref:m.output})+'> => { const result = await target.invoke(CONTRACT, '+JSON.stringify(name)+', validate(CONTRACT, '+JSON.stringify(m.input)+', input), options); return validate(CONTRACT, '+JSON.stringify(m.output)+', result, "INVALID_RESULT") as '+tsType({$ref:m.output})+'; },\n';
 out+='}; }\nexport interface Handlers {\n';
 for(const [name,m]of Object.entries(c.descriptor.methods))out+='['+JSON.stringify(name)+'](input: '+tsType({$ref:m.input})+', context: CallContext): Promise<'+tsType({$ref:m.output})+'> | '+tsType({$ref:m.output})+';\n';
 out+='}\nexport function register(handlers: Handlers, hooks?: LifecycleHooks) { const adapters: Record<string, RuntimeHandler> = Object.create(null);\n';
 for(const [name,m]of Object.entries(c.descriptor.methods))out+='adapters['+JSON.stringify(name)+'] = async (input, context) => validate(CONTRACT, '+JSON.stringify(m.output)+', await handlers['+JSON.stringify(name)+'](validate(CONTRACT, '+JSON.stringify(m.input)+', input) as '+tsType({$ref:m.input})+', context), "INVALID_RESULT");\n';
 out+='return definePlugin(CONTRACT, adapters, hooks); }\n';out+='export const errors = {\n';for(const [name,e]of Object.entries(c.descriptor.errors))out+='['+JSON.stringify(name)+']: (data: '+tsType({$ref:e.data})+', message = '+JSON.stringify(name)+') => PluginError.domain('+JSON.stringify(name)+', validate(CONTRACT, '+JSON.stringify(e.data)+', data), message),\n';out+='};\n';out+='export interface Emitter { emit(contract: Contract, event: string, payload: unknown): Promise<void> }\nexport function events(target: Emitter) { return {\n';for(const [name,e]of Object.entries(c.descriptor.events))out+='['+JSON.stringify(name)+']: (payload: '+tsType({$ref:e.payload})+') => target.emit(CONTRACT, '+JSON.stringify(name)+', validate(CONTRACT, '+JSON.stringify(e.payload)+', payload)),\n';out+='}; }\n';return out;
}
function rustLiteral(value) {
 return '"'+[...value].map(char=>{
  if(char==='"')return '\\"';if(char==='\\')return '\\\\';
  if(char==='\n')return '\\n';if(char==='\r')return '\\r';if(char==='\t')return '\\t';
  const point=char.codePointAt(0);return point<32||point===127?'\\u{'+point.toString(16)+'}':char;
 }).join('')+'"';
}

function rustOutput(c) {
 const definitions=[];const used=new Set(Object.keys(c.schemas.$defs).map(safeName));const emitted=new Set();
 function unique(name){let candidate=name;let i=2;while(used.has(candidate))candidate=name+(i++);used.add(candidate);return candidate;}
 function ty(s,name){
  if(s.$ref)return safeName(s.$ref.split('/').at(-1));
  if(Array.isArray(s.type))return 'Option<'+ty({...s,type:s.type.find(t=>t!=='null')},name+'Value')+'>';
  if(s.oneOf||s.type==='object'||s.type==='string'&&(s.enum||s.const!==undefined)){const n=unique(name);emit(n,s);return n;}
  switch(s.type){case 'array':return 'Vec<'+ty(s.items,name+'Item')+'>';case 'string':return 'String';case 'integer':return 'i64';case 'number':return 'napi_vm_plugin_protocol::FiniteF64';case 'boolean':return 'bool';case 'null':return '()';default:throw new Error('unsupported type');}
 }
 function emit(name,s){
  if(emitted.has(name))return;emitted.add(name);
  if(s.type==='object'){
   const fieldNames=new Set();const fields=Object.entries(s.properties).map(([k,v])=>{let n=rustName(k),i=2;while(fieldNames.has(n.replace(/^r#/,'')))n=rustName(k)+'_'+(i++);fieldNames.add(n.replace(/^r#/,''));let t=ty(v,name+safeName(k[0]?.toUpperCase()+k.slice(1)));const optional=!s.required.includes(k);if(optional)t='napi_vm_plugin_protocol::Field<'+t+'>';return '#[serde(rename = '+rustLiteral(k)+(optional?', default, skip_serializing_if = "napi_vm_plugin_protocol::Field::is_missing"':'')+')]\npub '+n+': '+t+',';});
   definitions.push('#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]\n#[serde(deny_unknown_fields)]\npub struct '+name+' {\n'+fields.join('\n')+'\n}');
  } else if(s.oneOf){const vars=s.oneOf.map((v,i)=>'V'+i+'('+ty(v,name+'Variant'+i)+')');definitions.push('#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]\n#[serde(untagged)]\npub enum '+name+' { '+vars.join(', ')+' }');}
  else if(s.type==='string'&&(s.enum||s.const!==undefined))definitions.push('#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]\npub enum '+name+' { '+(s.const!==undefined?[s.const]:s.enum).map((v,i)=>'#[serde(rename = '+rustLiteral(v)+')] V'+i).join(', ')+' }');
  else definitions.push('pub type '+name+' = '+ty(s,name+'Value')+';');
 }
 for(const [n,s]of Object.entries(c.schemas.$defs))emit(safeName(n),s);
 let adapters='\npub fn contract() -> napi_vm_plugin_protocol::PluginResult<napi_vm_plugin_protocol::Contract> { napi_vm_plugin_protocol::Contract::from_value(serde_json::from_str(CONTRACT_JSON)?) }\n';
 const validatorNames=new Set();for(const n of Object.keys(c.schemas.$defs)){const base='validate_'+rustName(n).replace('r#','');let name=base,i=2;while(validatorNames.has(name))name=base+'_'+(i++);validatorNames.add(name);adapters+='pub fn '+name+'(value: serde_json::Value) -> napi_vm_plugin_protocol::PluginResult<'+safeName(n)+'> { contract()?.validate_ref('+rustLiteral('#/$defs/'+pointer(n))+', &value)?; napi_vm_plugin_protocol::from_wire_value(value) }\n';}
 adapters+='#[derive(Clone)]\npub struct Client { pub context: napi_vm_plugin_sdk::CallContext }\nimpl Client {\n';
 const methodNames=new Set();const methods=Object.entries(c.descriptor.methods).map(([name,m])=>{let n=rustName(name),i=2;while(methodNames.has(n.replace(/^r#/,'')))n=rustName(name)+'_'+(i++);methodNames.add(n.replace(/^r#/,''));return [name,m,n];});
 for(const [name,m,n] of methods)adapters+='pub async fn '+n+'(&self, input: '+safeName(m.input.split('/').at(-1))+') -> napi_vm_plugin_protocol::PluginResult<'+safeName(m.output.split('/').at(-1))+'> { let c=contract()?; let input=serde_json::to_value(input)?; c.validate_ref('+rustLiteral(m.input)+', &input)?; let value = self.context.invoke(&c, '+rustLiteral(name)+', input).await?; c.output('+rustLiteral(name)+', &value)?; napi_vm_plugin_protocol::from_wire_value(value) }\n';
 adapters+='}\npub trait Handler: Send + Sync + \'static {\n';
 for(const [,m,n] of methods)adapters+='fn '+n+'(&self, input: '+safeName(m.input.split('/').at(-1))+', context: napi_vm_plugin_sdk::CallContext) -> napi_vm_plugin_sdk::PluginFuture<\'_, '+safeName(m.output.split('/').at(-1))+'>;\n';
 adapters+='}\npub fn register(registry: &napi_vm_plugin_sdk::Registry, handler: std::sync::Arc<dyn Handler>) -> napi_vm_plugin_protocol::PluginResult<()> {\n';
 for(const [name,m,n] of methods)adapters+='let h=handler.clone(); registry.register(contract()?, '+rustLiteral(name)+', move |value, context| { let h=h.clone(); Box::pin(async move { contract()?.validate_ref('+rustLiteral(m.input)+', &value)?; let output=serde_json::to_value(h.'+n+'(napi_vm_plugin_protocol::from_wire_value(value)?, context).await?)?; contract()?.output('+rustLiteral(name)+', &output)?; Ok(output) }) })?;\n';
 adapters+='Ok(()) }\n';const errorNames=new Set();for(const [name,error]of Object.entries(c.descriptor.errors)){const base='error_'+rustName(name).replace('r#','');let n=base,i=2;while(errorNames.has(n))n=base+'_'+(i++);errorNames.add(n);adapters+='pub fn '+n+'(data: '+safeName(error.data.split('/').at(-1))+', message: impl Into<String>) -> napi_vm_plugin_protocol::PluginResult<napi_vm_plugin_protocol::RpcError> { let data=serde_json::to_value(data)?; contract()?.validate_ref('+rustLiteral(error.data)+', &data)?; Ok(napi_vm_plugin_protocol::RpcError::domain('+rustLiteral(name)+', message, data)) }\n';}
 return '// Generated. Do not edit.\n'+definitions.join('\n')+'\npub const INTERFACE_ID: &str = '+rustLiteral(c.descriptor.id)+';\npub const INTERFACE_VERSION: &str = '+rustLiteral(c.descriptor.version)+';\npub const CONTRACT_DIGEST: &str = '+rustLiteral(c.digest)+';\npub const CONTRACT_JSON: &str = '+rustLiteral(JSON.stringify(c))+';\n'+adapters;
}
function apiDocs(c) {
 const d=c.descriptor;
 const lines=['# '+d.id,'','Generated contract reference. Regenerate this page with its TypeScript, Rust, and JSON artifacts; do not edit it by hand.','',
  '| Identity | Value |','| --- | --- |','| Interface version | `'+d.version+'` |','| Contract digest | `'+c.digest+'` |','| Descriptor version | '+d.descriptorVersion+' |','',
  d.description??'An asynchronous interface exchanging validated data-only messages.','',
  'Host and plugin must agree on the exact interface version and digest. A package version or matching method name alone is insufficient.','','## Methods',''];
 if(!Object.keys(d.methods).length)lines.push('No callable methods are declared.','');
 for(const [name,m]of Object.entries(d.methods)) {
  lines.push('### `'+name+'`','');
  if(m.description)lines.push(m.description,'');
  lines.push('| Boundary | Type |','| --- | --- |','| Input | `'+tsType({$ref:m.input})+'` |','| Resolved output | `'+tsType({$ref:m.output})+'` |','',
   'The JavaScript client returns a promise; the Rust client is asynchronous. Declared domain errors: '+(m.errors.length?m.errors.map(e=>'`'+e+'`').join(', '):'none')+'.','',
   'Idempotent metadata: `'+String(m.idempotent??false)+'`. Peers do not automatically retry calls.','');
 }
 lines.push('## Data types','');
 for(const [name,schema]of Object.entries(c.schemas.$defs)) {
  lines.push('### `'+safeName(name)+'`','');
  if(schema.description)lines.push(schema.description,'');
  lines.push('```typescript','type '+safeName(name)+' = '+tsType(schema)+';','```','');
 }
 lines.push('## Domain errors','');
 if(!Object.keys(d.errors).length)lines.push('No domain errors are declared.','');
 for(const [name,e]of Object.entries(d.errors))lines.push('### `'+name+'`','',e.description??'A declared application error.', '', 'Data type: `'+tsType({$ref:e.data})+'`.','');
 lines.push('## Events','');
 if(!Object.keys(d.events).length)lines.push('No events are declared.','');
 for(const [name,e]of Object.entries(d.events))lines.push('### `'+name+'`','',e.description??'A contract-validated event.', '', 'Payload type: `'+tsType({$ref:e.payload})+'`.','');
 if(d.state)lines.push('## State transfer','', '| Field | Value |','| --- | --- |','| Snapshot contract | `'+d.state.contract+'` |','| State version | '+d.state.version+' |','| Data type | `'+tsType({$ref:d.state.schema})+'` |','');
 lines.push('## Runtime validation','',
  'Generated validators enforce schema bounds and wire encodings in addition to static types. Optional fields preserve absence; null is accepted only where declared. Wire data rejects cycles, functions, non-finite numbers, and unpaired Unicode surrogates.', '',
  'Use the generated client/registration helpers with the matching SDK. Cancellation and deadlines are cooperative and do not reverse completed application effects.','');
 return lines.join('\n');
}
export async function generate(file,outDir,{check=false}={}){const c=await normalize(file);outDir=resolve(outDir);const stem=basename(file).replace(/\.interface\.json$/,'');const outputs={[stem+'.contract.json']:canonical(c)+'\n',[stem+'.ts']:tsOutput(c),[stem+'.rs']:rustOutput(c),[stem+'.md']:apiDocs(c)};let temporary;if(check)temporary=await mkdtemp(join(tmpdir(),'napi-vm-codegen-'));else await mkdir(outDir,{recursive:true});try{for(const [name,bytes]of Object.entries(outputs)){const path=join(outDir,name);if(check){await writeFile(join(temporary,name),bytes);let old;try{old=await readFile(path,'utf8')}catch{fail(path,'generated output missing')}if(old!==bytes)fail(path,'generated output stale');}else await writeFile(path,bytes);}}finally{if(temporary)await rm(temporary,{recursive:true,force:true});}return {id:c.descriptor.id,digest:c.digest,files:Object.keys(outputs).map(n=>join(outDir,n))};}
export async function generatePluginMetadata(plugin, outDir, {check=false}={}) {
 if(!plain(plugin)||typeof plugin.id!=='string'||plugin.id.length>128||!IDENT.test(plugin.id)||typeof plugin.version!=='string'||plugin.version.length>128||!VERSION.test(plugin.version)||!Array.isArray(plugin.requiresHost??[]))fail('plugin','metadata requires id/version and optional requiresHost contracts');
 const value={id:plugin.id,version:plugin.version,requiresHost:plugin.requiresHost??[]};outDir=resolve(outDir);
 const outputs={'plugin-metadata.json':canonical(value)+'\n','plugin-metadata.ts':'// Generated. Do not edit.\nimport type { Contract } from "@napi-vm/plugin-protocol";\nexport const PLUGIN_METADATA: {id:string;version:string;requiresHost:Contract[]} = JSON.parse('+JSON.stringify(JSON.stringify(value))+');\n','plugin-metadata.rs':'// Generated. Do not edit.\npub fn plugin_metadata() -> napi_vm_plugin_protocol::PluginResult<napi_vm_plugin_sdk::PluginMetadata> { Ok(serde_json::from_str('+rustLiteral(JSON.stringify(value))+')?) }\n'};
 if(!check)await mkdir(outDir,{recursive:true});for(const [name,bytes] of Object.entries(outputs)){const path=join(outDir,name);if(check){let old;try{old=await readFile(path,'utf8')}catch{fail(path,'generated metadata missing')}if(old!==bytes)fail(path,'generated metadata stale');}else await writeFile(path,bytes);}return Object.keys(outputs).map(n=>join(outDir,n));
}
export async function generateAll({check=false,sourceDir='contracts/trusted-plugins/examples',outDir}={}){const dir=resolve(sourceDir);const output=outDir===undefined?resolve(dir,'../generated'):resolve(outDir);const results=[];for(const f of (await readdir(dir)).filter(x=>x.endsWith('.interface.json')).sort())results.push(await generate(join(dir,f),output,{check}));if(check){const expected=new Set(results.flatMap(r=>r.files.map(p=>basename(p))));for(const name of await readdir(output))if(/\.(?:ts|rs|md|contract\.json)$/.test(name)&&!expected.has(name))fail(join(output,name),'untracked generated output has no descriptor');}return results;}
if(process.argv[1]&&await realpath(process.argv[1]).catch(()=>null)===fileURLToPath(import.meta.url)){try{const args=process.argv.slice(2),positions=args.filter(x=>!x.startsWith('--'));console.log(JSON.stringify(positions.length?await generate(positions[0],positions[1]??'generated',{check:args.includes('--check')}):await generateAll({check:args.includes('--check')})));}catch(e){console.error(e.message);process.exitCode=1;}}
