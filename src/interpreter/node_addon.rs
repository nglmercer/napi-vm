use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value as JsonValue, json};

use crate::error::VmErr;
use crate::host::{HostBridge, HostCallback, HostCallbackKind, HostEvent};
use crate::interpreter::NativeAddonLoader;
use crate::value::{
    MAX_ARRAY_LEN, MAX_OBJECT_PROPS, MAX_STRING_LEN, PromiseInner, PromiseState, PropAttrs,
    SymbolData, TypedArrayData, TypedKind, Value,
};

const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAX_WIRE_DEPTH: usize = 128;
const MAX_NATIVE_HANDLES: usize = 262_144;

static NEXT_GUEST_GRAPH_NODE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
struct LocalObjectTrap {
    object_id: u64,
    operation: ObjectOperation,
}

#[derive(Clone, Copy)]
enum ObjectOperation {
    Get,
    Set,
    Has,
    Delete,
    OwnKeys,
}

impl ObjectOperation {
    fn guest_name(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Set => "set",
            Self::Has => "has",
            Self::Delete => "deleteProperty",
            Self::OwnKeys => "ownKeys",
        }
    }
}

const NODE_BRIDGE: &str = r#"
'use strict';
const net = require('node:net');
const { Worker } = require('node:worker_threads');
const socket = net.connect({host:'127.0.0.1',port:Number(process.env.NAPI_VM_BRIDGE_PORT)});
let input = Buffer.alloc(0);
let connected = false;
let workerReady = false;
const pendingSyncCallbacks = new Map();
function send(message) {
  const body = Buffer.from(JSON.stringify(message));
  const header = Buffer.allocUnsafe(4);
  header.writeUInt32BE(body.length, 0);
  socket.write(Buffer.concat([header, body]));
}
function maybeSendHello() {
  if (connected && workerReady) send({hello:process.env.NAPI_VM_BRIDGE_TOKEN,nodeVersion:process.versions.node,napiVersion:process.versions.napi});
}
function finishSyncCallback(message) {
  const pending = pendingSyncCallbacks.get(message.callId);
  if (!pending) {
    socket.destroy(new Error('unknown synchronous guest callback response'));
    return;
  }
  pendingSyncCallbacks.delete(message.callId);
  let response = {ok:message.ok===true,value:message.value,snapshots:message.snapshots||[]};
  let bytes = Buffer.from(JSON.stringify(response));
  if (bytes.length > pending.shared.byteLength - 8) {
    response = {ok:false,value:{t:'error',name:'RangeError',message:'synchronous guest callback result exceeds the bridge limit'}};
    bytes = Buffer.from(JSON.stringify(response));
  }
  new Uint8Array(pending.shared,8,bytes.length).set(bytes);
  const words = new Int32Array(pending.shared,0,2);
  Atomics.store(words,1,bytes.length);
  Atomics.store(words,0,response.ok?1:2);
  Atomics.notify(words,0);
}
function consume() {
  while(input.length>=4) {
    const n=input.readUInt32BE(0);if(n>16777216){socket.destroy(new Error('frame too large'));return;}
    if(input.length<n+4)return;
    const body=input.subarray(4,n+4);input=input.subarray(n+4);
    let message;try{message=JSON.parse(body.toString('utf8'));}catch(e){socket.destroy(e);return;}
    if(message.event==='syncGuestCallbackResult') finishSyncCallback(message);
    else if(message.requestId!==undefined){
      if(!workerReady){
        send({requestId:message.requestId,ok:false,error:{name:'Error',message:'native addon worker is not ready'}});
      } else worker.postMessage({kind:'request',request:message});
    } else {socket.destroy(new Error('invalid napi-vm bridge frame'));return;}
  }
}
function addonWorkerMain() {
  'use strict';
  const { parentPort, receiveMessageOnPort } = require('node:worker_threads');
  const { types } = require('node:util');
  const MAX_SYNC_CALLBACK_RESULT_BYTES = 1024 * 1024;
  let nextHandle=1, nextCallbackCall=1, nextNativeSymbolId=1, dispatchDepth=0;
  const refs=new Map(), objectIds=new WeakMap(), functionIds=new WeakMap(), promiseIds=new WeakMap(), guestCallbackIds=new WeakMap(), guestGraphNodes=new Map();
  const symbols=new Map(), symbolIds=new Map();
  const wellKnownSymbols=[undefined,Symbol.iterator,Symbol.asyncIterator,Symbol.toStringTag,Symbol.hasInstance,Symbol.toPrimitive,Symbol.species,Symbol.unscopables,Symbol.isConcatSpreadable,Symbol.match,Symbol.matchAll,Symbol.replace,Symbol.search,Symbol.split];
  function symbolId(value){let id=symbolIds.get(value);if(id===undefined){if(symbols.size>=262144)throw new RangeError('symbol handle limit exceeded');id='n:'+nextNativeSymbolId++;symbols.set(id,value);symbolIds.set(value,id);}return id;}
  function newGraph(){return {seen:new Map(),nextId:1};}
  function setGraphNode(graph,id,value){if(id!==undefined){if(graph.has(id))throw new TypeError('duplicate guest graph node id');if(graph.size>=262144)throw new RangeError('guest graph node limit exceeded');if(!guestGraphNodes.has(id)&&guestGraphNodes.size>=262144)throw new RangeError('persistent guest graph node limit exceeded');graph.set(id,value);guestGraphNodes.set(id,value);if(graph.guestRefs)graph.guestRefs.set(value,id);}}
  function newDecodeGraph(){const graph=new Map();graph.guestRefs=new WeakMap();graph.guestCallbacks=new WeakMap();return graph;}
  // Atomics.wait blocks the Node worker while Rust runs the guest callback.
  // Pump nested addon requests here so that callback can call the addon again.
  function waitForSyncGuestCallback(words){
    const deadline=Date.now()+60000;
    while(Atomics.load(words,0)===0){
      const pending=receiveMessageOnPort(parentPort);
      if(pending){
        const message=pending.message;
        if(message.kind!=='request')throw new TypeError('unexpected Node bridge message while waiting for a guest callback');
        parentPort.postMessage({kind:'response',message:dispatch(message.request)});
        continue;
      }
      const remaining=deadline-Date.now();
      if(remaining<=0)throw new Error('timed out waiting for synchronous guest callback');
      Atomics.wait(words,0,0,Math.min(remaining,10));
    }
  }
  function hold(value,receiver){
    const kind=typeof value,ids=kind==='function'?functionIds:objectIds,existing=ids.get(value);
    if(existing!==undefined)return existing;
    if(refs.size>=262144)throw new RangeError('native handle limit exceeded');
    const id=nextHandle++;refs.set(id,{value,receiver});ids.set(value,id);return id;
  }
  function event(message){
    if(message.event==='syncGuestCallback')parentPort.postMessage({kind:'syncGuestCallback',callId:message.callId,shared:message.shared,message});
    else parentPort.postMessage({kind:'event',message});
  }
  function encode(value,receiver,depth,graph,guestRefs,guestCallbacks,copyPlainObjects=false){
    if(depth>128)throw new RangeError('bridge depth exceeded');
    if(value&&(guestRefs&&guestRefs.has(value)))return {t:'guestRef',v:guestRefs.get(value)};
    if(value&&(guestCallbacks&&guestCallbacks.has(value)))return {t:'guestCallbackRef',v:guestCallbacks.get(value)};
    if(value===undefined)return {t:'undefined'};if(value===null)return {t:'null'};
    if(typeof value==='boolean')return {t:'boolean',v:value};
    if(typeof value==='number')return {t:'number',v:Object.is(value,-0)?'-0':String(value)};
    if(typeof value==='string')return {t:'string',v:value};
    if(typeof value==='bigint')return {t:'bigint',v:value.toString()};
    if(typeof value==='symbol')return {t:'symbol',v:symbolId(value),description:value.description};
    if(typeof value==='function')return {t:'function',v:hold(value,receiver),n:value.name};
    if(value&&typeof value.then==='function'){
      let id=promiseIds.get(value);
      if(id===undefined){id=hold(value,undefined);promiseIds.set(value,id);Promise.resolve(value).then(
        result=>event({event:'hostPromiseSettled',promiseId:id,state:'fulfilled',value:encode(result,undefined,0,newGraph())}),
        reason=>event({event:'hostPromiseSettled',promiseId:id,state:'rejected',value:encode(reason,undefined,0,newGraph())})
      ).catch(error=>event({event:'hostPromiseSettled',promiseId:id,state:'rejected',value:{t:'error',name:'TypeError',message:'native promise result could not be marshalled: '+String(error)}}));}
      return {t:'hostPromise',v:id};
    }
    if(value instanceof Error)return {t:'error',name:value.name,message:value.message,code:typeof value.code==='string'?value.code:undefined};
    if(value instanceof Date)return {t:'date',v:Number.isNaN(value.getTime())?'NaN':String(value.getTime())};
    if(value instanceof RegExp)return {t:'regexp',source:value.source,flags:value.flags,lastIndex:String(value.lastIndex)};
    if(Buffer.isBuffer(value))return {t:'hostObject',v:hold(value,undefined)};
    if(value instanceof DataView)return {t:'dataView',length:value.byteLength,bytes:Array.from(new Uint8Array(value.buffer,value.byteOffset,value.byteLength))};
    if(ArrayBuffer.isView(value))return {t:'typedArray',kind:value.constructor.name,length:value.length,bytes:Array.from(new Uint8Array(value.buffer,value.byteOffset,value.byteLength))};
    if(value instanceof ArrayBuffer)return {t:'arrayBuffer',v:Array.from(new Uint8Array(value))};
    if(Array.isArray(value)){
      const existing=graph.seen.get(value);if(existing!==undefined)return {t:'ref',v:existing};
      if(graph.seen.size>=262144)throw new RangeError('native graph node limit exceeded');
      if(value.length>262144)throw new RangeError('native array exceeds the VM limit');
      const id='n:'+graph.nextId++;graph.seen.set(value,id);
      const items=Array.from({length:value.length},(_,i)=>Object.hasOwn(value,String(i))?encode(value[i],value,depth+1,graph,guestRefs,guestCallbacks,copyPlainObjects):{t:'hole'});
      const named=[];
      for(const key of Object.keys(value))if(!/^(0|[1-9][0-9]*)$/.test(key)||Number(key)>=value.length)named.push([key,encode(value[key],value,depth+1,graph,guestRefs,guestCallbacks,copyPlainObjects)]);
      return {t:'array',id,v:items,named};
    }
    if(copyPlainObjects&&(Object.getPrototypeOf(value)===Object.prototype||Object.getPrototypeOf(value)===null)){
      const existing=graph.seen.get(value);if(existing!==undefined)return {t:'ref',v:existing};
      if(graph.seen.size>=262144)throw new RangeError('native graph node limit exceeded');
      const id='n:'+graph.nextId++;graph.seen.set(value,id);
      const entries=[];
      for(const key of Reflect.ownKeys(value)){
        const descriptor=Object.getOwnPropertyDescriptor(value,key);
        if(!descriptor)throw new TypeError('property descriptor cannot cross the Node addon bridge');
        const wireKey=typeof key==='symbol'?encode(key,value,depth+1,graph,guestRefs,guestCallbacks,copyPlainObjects):key;
        if(Object.hasOwn(descriptor,'value'))entries.push([wireKey,encode(descriptor.value,value,depth+1,graph,guestRefs,guestCallbacks,copyPlainObjects),descriptor.writable,descriptor.enumerable,descriptor.configurable]);
        else entries.push([wireKey,{t:'undefined'},true,descriptor.enumerable,descriptor.configurable,descriptor.get?encode(descriptor.get,value,depth+1,graph,guestRefs,guestCallbacks,copyPlainObjects):null,descriptor.set?encode(descriptor.set,value,depth+1,graph,guestRefs,guestCallbacks,copyPlainObjects):null]);
      }
      const extensible=Object.isExtensible(value);
      const objectPrototype=Object.getPrototypeOf(value);
      const prototype=objectPrototype===Object.prototype?{t:'defaultPrototype'}:{t:'null'};
      return {t:'object',id,v:entries,prototype,extensible};
    }
    return {t:'hostObject',v:hold(value,undefined)};
  }
  function encodeGuestValue(value,receiver,depth,graph,guestRefs,guestCallbacks){
    if(value&&(guestRefs&&guestRefs.has(value)))return {t:'guestRef',v:guestRefs.get(value)};
    if(value&&(guestCallbacks&&guestCallbacks.has(value)))return {t:'guestCallbackRef',v:guestCallbacks.get(value)};
    return encode(value,receiver,depth,graph,guestRefs,guestCallbacks,true);
  }
  function encodeGuestPrototype(value,receiver,depth,graph,guestRefs,guestCallbacks){
    if(value===Object.prototype)return {t:'defaultPrototype'};
    if(value===null)return {t:'null'};
    return encodeGuestValue(value,receiver,depth,graph,guestRefs,guestCallbacks);
  }
  function makeGuestCallback(value,graph,isClass=false){
    const callbackGraph=graph,callbackId=isClass?value.callbackId:value.v;
    const callback=function(...args){
      const construct=new.target!==undefined;
      if(isClass&&!construct)throw new TypeError('Class constructor cannot be invoked without new');
      if(!isClass&&construct&&value.constructable!==true)throw new TypeError('guest callback is not a constructor');
      if(construct&&dispatchDepth===0)throw new TypeError('guest constructors must be invoked during a native addon call');
      const kind=construct?'construct':'call',graph=newGraph();
      const guestMutations=callbackGraph.mutationBefore?collectGuestMutations(callbackGraph,callbackGraph.mutationBefore,callbackGraph.guestRefs,callbackGraph.guestCallbacks,graph):[];
      const payload={callbackId,kind,thisValue:construct?{t:'undefined'}:encodeGuestValue(this,undefined,0,graph,callbackGraph.guestRefs,callbackGraph.guestCallbacks),args:args.map(v=>encodeGuestValue(v,undefined,0,graph,callbackGraph.guestRefs,callbackGraph.guestCallbacks)),guestMutations};
      if(dispatchDepth===0){event({event:'guestCallback',...payload});return undefined;}
      const callId=nextCallbackCall++,shared=new SharedArrayBuffer(MAX_SYNC_CALLBACK_RESULT_BYTES+8),words=new Int32Array(shared,0,2);
      event({event:'syncGuestCallback',callId,shared,...payload});
      waitForSyncGuestCallback(words);
      const length=Atomics.load(words,1);if(length<0||length>MAX_SYNC_CALLBACK_RESULT_BYTES)throw new RangeError('invalid synchronous guest callback response size');
      const response=JSON.parse(Buffer.from(new Uint8Array(shared,8,length)).toString('utf8'));
      applyGuestSnapshots(response.snapshots||[],callbackGraph);
      if(!response.ok)throw decode(response.value,0,callbackGraph);return decode(response.value,0,callbackGraph);
    };
    guestCallbackIds.set(callback,callbackId);
    if(callbackGraph.guestCallbacks)callbackGraph.guestCallbacks.set(callback,callbackId);
    if(isClass){
      setGraphNode(callbackGraph,value.id,callback);
      Object.defineProperty(callback,'name',{value:String(value.name||''),configurable:true});
      const prototype=decode(value.prototype,0,callbackGraph);
      if(!prototype||typeof prototype!=='object')throw new TypeError('guest class prototype is invalid');
      Object.defineProperty(callback,'prototype',{value:prototype,writable:false,enumerable:false,configurable:false});
      const constructorDescriptor=Object.getOwnPropertyDescriptor(prototype,'constructor');
      if(constructorDescriptor&&constructorDescriptor.configurable)Object.defineProperty(prototype,'constructor',{value:callback,writable:true,enumerable:false,configurable:true});
      for(const [key,item]of(value.statics||[]))if(!['name','length','prototype','arguments','caller'].includes(key))Object.defineProperty(callback,key,{value:decode(item,0,callbackGraph),enumerable:true,writable:true,configurable:true});
    }
    return callback;
  }
  function decode(value,depth,graph){
    if(depth>128)throw new RangeError('guest argument depth exceeded');
    switch(value.t){
      case 'ref':{let result;if(graph.has(value.v)){result=graph.get(value.v);}else{result=guestGraphNodes.get(value.v);if(result!==undefined){if(graph.size>=262144)throw new RangeError('guest graph node limit exceeded');graph.set(value.v,result);if(graph.guestRefs)graph.guestRefs.set(result,value.v);}}if(result===undefined)throw new TypeError('invalid guest object reference');return result;}
      case 'undefined':return undefined;case 'null':return null;case 'boolean':return value.v;
      case 'number':if(value.v==='-0')return -0;if(value.v==='NaN')return NaN;if(value.v==='Infinity')return Infinity;if(value.v==='-Infinity')return -Infinity;return Number(value.v);
      case 'string':return value.v;case 'bigint':return BigInt(value.v);
      case 'symbol':{let symbol=symbols.get(value.v);if(symbol===undefined){const guestId=value.v.startsWith('g:')?Number(value.v.slice(2)):0;symbol=wellKnownSymbols[guestId];if(symbol===undefined){if(symbols.size>=262144)throw new RangeError('symbol handle limit exceeded');symbol=Symbol(value.description);}symbols.set(value.v,symbol);symbolIds.set(symbol,value.v);}return symbol;}
      case 'date':return new Date(value.v==='NaN'?NaN:Number(value.v));
      case 'regexp':{const regex=new RegExp(value.source,value.flags);regex.lastIndex=Number(value.lastIndex||0);return regex;}
      case 'arrayBuffer':return Uint8Array.from(value.v).buffer;
      case 'typedArray':{const Constructor=globalThis[value.kind];if(typeof Constructor!=='function')throw new TypeError('unsupported guest typed array kind');const bytes=Uint8Array.from(value.bytes);return new Constructor(bytes.buffer,0,value.length);}
      case 'dataView':{const bytes=Uint8Array.from(value.bytes);return new DataView(bytes.buffer,0,value.length);}
      case 'bytes':return Buffer.from(value.v);
      case 'error':{const error=new Error(value.message||'guest callback threw');error.name=value.name||'Error';if(value.code)error.code=value.code;return error;}
      case 'hostObject':{const entry=refs.get(value.v);if(!entry||!entry.value||typeof entry.value!=='object')throw new TypeError('native object handle is invalid');return entry.value;}
      case 'guestCallback':return makeGuestCallback(value,graph);
      case 'guestClass':return makeGuestCallback(value,graph,true);
      case 'function':{const entry=refs.get(value.v);if(!entry||typeof entry.value!=='function')throw new TypeError('native function handle is invalid');return entry.value;}
      case 'array':{const a=new Array(value.v.length);setGraphNode(graph,value.id,a);value.v.forEach((item,index)=>{if(item.t!=='hole')a[index]=decode(item,depth+1,graph);});for(const [k,v]of(value.named||[]))Object.defineProperty(a,k,{value:decode(v,depth+1,graph),enumerable:true,writable:true,configurable:true});if(graph.mutationBefore&&value.id!==undefined&&!graph.mutationBefore.has(value.id))graph.mutationBefore.set(value.id,descriptorState(a));return a;}
      case 'object':{const o={};setGraphNode(graph,value.id,o);for(const item of value.v){const [key,v,writable=true,enumerable=true,configurable=true]=item;const k=typeof key==='string'?key:decode(key,depth+1,graph);if(typeof k!=='string'&&typeof k!=='symbol')throw new TypeError('invalid guest property key');const descriptor={enumerable,configurable};if(item.length>=7){if(item[5]!==null)descriptor.get=decode(item[5],depth+1,graph);if(item[6]!==null)descriptor.set=decode(item[6],depth+1,graph);}else{descriptor.value=decode(v,depth+1,graph);descriptor.writable=writable;}Object.defineProperty(o,k,descriptor);}if(value.prototype&&value.prototype.t!=='defaultPrototype'){const prototype=value.prototype.t==='null'?null:decode(value.prototype,depth+1,graph);if(prototype!==null&&typeof prototype!=='object'&&typeof prototype!=='function')throw new TypeError('invalid guest object prototype');Object.setPrototypeOf(o,prototype);}if(value.extensible===false)Object.preventExtensions(o);if(graph.mutationBefore&&value.id!==undefined&&!graph.mutationBefore.has(value.id))graph.mutationBefore.set(value.id,descriptorState(o));return o;}
      case 'proxy':{const target=decode(value.target,depth+1,graph);const handler=decode(value.handler,depth+1,graph);if((!target||typeof target!=='object')&&typeof target!=='function')throw new TypeError('invalid guest Proxy target');if(!handler||typeof handler!=='object')throw new TypeError('invalid guest Proxy handler');const supported=new Set(['get','set','has','deleteProperty','ownKeys','apply','construct']);const filteredHandler=new Proxy(Object.create(null),{get(_target,key){return supported.has(key)?Reflect.get(handler,key,handler):undefined;}});const proxy=new Proxy(target,filteredHandler);setGraphNode(graph,value.id,proxy);return proxy;}
      default:throw new TypeError('unsupported napi-vm argument');
    }
  }
  function descriptorState(object){
    const entries=new Map();
    for(const key of Reflect.ownKeys(object)){
      if(Array.isArray(object)&&key==='length')continue;
      const descriptor=Object.getOwnPropertyDescriptor(object,key);
      if(!descriptor)throw new TypeError('guest property descriptor could not be read');
      entries.set(key,descriptor);
    }
    return {entries,extensible:Object.isExtensible(object),prototype:Object.getPrototypeOf(object),length:Array.isArray(object)?object.length:undefined};
  }
  function applyGuestSnapshots(snapshots,graph){
    for(const snapshot of snapshots){
      const object=graph.get(snapshot.id);if(!object||types.isProxy(object))continue;
      if(snapshot.t==='array'){
        for(const key of Reflect.ownKeys(object))if(key!=='length')Reflect.deleteProperty(object,key);
        object.length=0;
        object.length=snapshot.v.length;
        for(let index=0;index<snapshot.v.length;index++)if(snapshot.v[index].t!=='hole')Object.defineProperty(object,String(index),{value:decode(snapshot.v[index],0,graph),enumerable:true,writable:true,configurable:true});
        const desired=new Set(['length']);
        for(let index=0;index<snapshot.v.length;index++)if(snapshot.v[index].t!=='hole')desired.add(String(index));
        for(const [key,value] of(snapshot.named||[])){desired.add(key);Object.defineProperty(object,key,{value:decode(value,0,graph),enumerable:true,writable:true,configurable:true});}
        for(const key of Reflect.ownKeys(object))if(!desired.has(key))Reflect.deleteProperty(object,key);
      }else if(snapshot.t==='object'){
        const desired=new Set();
        for(const item of snapshot.v){
          const [wireKey,value,writable=true,enumerable=true,configurable=true]=item;
          const key=typeof wireKey==='string'?wireKey:decode(wireKey,0,graph);desired.add(key);
          const descriptor={enumerable,configurable};
          if(item.length>=7){if(item[5]!==null)descriptor.get=decode(item[5],0,graph);if(item[6]!==null)descriptor.set=decode(item[6],0,graph);}
          else{descriptor.value=decode(value,0,graph);descriptor.writable=writable;}
          Object.defineProperty(object,key,descriptor);
        }
        for(const key of Reflect.ownKeys(object))if(!desired.has(key))Reflect.deleteProperty(object,key);
        if(snapshot.prototype){
          const prototype=snapshot.prototype.t==='defaultPrototype'?Object.prototype:snapshot.prototype.t==='null'?null:decode(snapshot.prototype,0,graph);
          if(prototype!==null&&typeof prototype!=='object'&&typeof prototype!=='function')throw new TypeError('invalid guest object prototype snapshot');
          Object.setPrototypeOf(object,prototype);
        }
        if(snapshot.extensible===false)Object.preventExtensions(object);
      }else if(snapshot.t==='guestClass'&&typeof object==='function'){
        const desired=new Set(['arguments','caller','length','name','prototype']);
        for(const [key,value] of(snapshot.statics||[])){
          if(key==='name'||key==='length'||key==='prototype'||key==='arguments'||key==='caller')continue;
          desired.add(key);
          Object.defineProperty(object,key,{value:decode(value,0,graph),enumerable:true,writable:true,configurable:true});
        }
        for(const key of Reflect.ownKeys(object))if(typeof key==='string'&&!desired.has(key)){
          const descriptor=Object.getOwnPropertyDescriptor(object,key);
          if(descriptor&&descriptor.configurable)Reflect.deleteProperty(object,key);
        }
      }
      if(graph.mutationBefore)graph.mutationBefore.set(snapshot.id,descriptorState(object));
    }
  }
  function captureMutableState(nodes){
    const state=new Map(),visited=new WeakSet();
    function visit(value){
      if(!value||typeof value!=='object'||visited.has(value)||types.isProxy(value))return;visited.add(value);
      if(value instanceof Date)state.set(value,'date:'+String(value.getTime()));
      else if(value instanceof RegExp)state.set(value,'regexp:'+value.source+'/'+value.flags+':'+value.lastIndex);
      else if(ArrayBuffer.isView(value))state.set(value,value.constructor.name+':'+Buffer.from(value.buffer,value.byteOffset,value.byteLength).toString('hex'));
      else if(value instanceof ArrayBuffer)state.set(value,'buffer:'+Buffer.from(value).toString('hex'));
      if(Array.isArray(value)||Object.getPrototypeOf(value)===Object.prototype||Object.getPrototypeOf(value)===null)
        for(const key of Reflect.ownKeys(value)){const descriptor=Object.getOwnPropertyDescriptor(value,key);if(descriptor&&Object.hasOwn(descriptor,'value'))visit(descriptor.value);}
    }
    for(const value of nodes.values())visit(value);
    return state;
  }
  function assertMutableStateUnchanged(state){
    for(const [value,baseline] of state){
      let current;
      if(value instanceof Date)current='date:'+String(value.getTime());
      else if(value instanceof RegExp)current='regexp:'+value.source+'/'+value.flags+':'+value.lastIndex;
      else if(ArrayBuffer.isView(value))current=value.constructor.name+':'+Buffer.from(value.buffer,value.byteOffset,value.byteLength).toString('hex');
      else current='buffer:'+Buffer.from(value).toString('hex');
      if(current!==baseline)throw new TypeError('in-place mutation of guest Date, RegExp, or binary values is not supported by the Node addon bridge');
    }
  }
  function collectGuestMutations(decodeGraph,before,guestRefs,guestCallbackIds,graph){
    const mutations=[];
    for(const [id,value] of decodeGraph){
      if(typeof id!=='string'||!id.startsWith('g:'))continue;
      const old=before.get(id);if(!old)continue;const now=descriptorState(value);
      const prototypeChanged=old.prototype!==now.prototype;
      const changedKeys=[];
      for(const [key,descriptor] of now.entries){
        const prior=old.entries.get(key);
        const priorIsData=prior&&Object.hasOwn(prior,'value'),currentIsData=Object.hasOwn(descriptor,'value');
        const valueChanged=!prior||priorIsData!==currentIsData||(currentIsData?!Object.is(prior.value,descriptor.value):!Object.is(prior.get,descriptor.get)||!Object.is(prior.set,descriptor.set));
        if(valueChanged||prior.writable!==descriptor.writable||prior.enumerable!==descriptor.enumerable||prior.configurable!==descriptor.configurable)changedKeys.push(key);
      }
      const deleted=[...old.entries.keys()].filter(key=>!now.entries.has(key));
      const changed=old.extensible!==now.extensible||old.length!==now.length||prototypeChanged||changedKeys.length>0||deleted.length>0;
      if(!changed)continue;
      const entries=[];
      for(const key of changedKeys){
        const descriptor=now.entries.get(key);
        const wireKey=typeof key==='symbol'?encodeGuestValue(key,value,1,graph,guestRefs,guestCallbackIds):key;
        if(Object.hasOwn(descriptor,'value'))entries.push([wireKey,encodeGuestValue(descriptor.value,value,1,graph,guestRefs,guestCallbackIds),descriptor.writable,descriptor.enumerable,descriptor.configurable]);
        else entries.push([wireKey,{t:'undefined'},true,descriptor.enumerable,descriptor.configurable,descriptor.get?encodeGuestValue(descriptor.get,value,1,graph,guestRefs,guestCallbackIds):null,descriptor.set?encodeGuestValue(descriptor.set,value,1,graph,guestRefs,guestCallbackIds):null]);
      }
      if(Array.isArray(value)){
        if(prototypeChanged)throw new TypeError('changing the prototype of a guest array through a Node addon is not supported');
        if(!now.extensible)throw new TypeError('preventExtensions on guest arrays is not supported by the Node addon bridge');
        const lengthDescriptor=Object.getOwnPropertyDescriptor(value,'length');
        if(!lengthDescriptor||lengthDescriptor.writable!==true)throw new TypeError('array length descriptor changes are not supported by the Node addon bridge');
        mutations.push({id,kind:'array',...(old.length!==now.length?{length:value.length}:{}),entries,deleted:deleted.map(key=>typeof key==='symbol'?encodeGuestValue(key,value,1,graph,guestRefs,guestCallbackIds):key)});
      }else mutations.push({id,kind:'object',...(old.extensible!==now.extensible?{extensible:now.extensible}:{}),...(prototypeChanged?{prototype:encodeGuestPrototype(now.prototype,value,1,graph,guestRefs,guestCallbackIds)}:{}),entries,deleted:deleted.map(key=>typeof key==='symbol'?encodeGuestValue(key,value,1,graph,guestRefs,guestCallbackIds):key)});
    }
    return mutations;
  }
  function dispatch(r){
    dispatchDepth++;
    try{
      let result,receiver;const decodeGraph=newDecodeGraph();
      if(r.op==='load')result=require(r.filename);
      else if(['get','set','has','delete','ownKeys'].includes(r.op)){
        const entry=refs.get(r.id);if(!entry||!entry.value||typeof entry.value!=='object')throw new Error('native object handle is invalid');
        const object=entry.value;
        if(r.op==='get'){result=Reflect.get(object,r.key,object);receiver=object;}
        else if(r.op==='set')result=Reflect.set(object,r.key,decode(r.value,0,decodeGraph),object);
        else if(r.op==='has')result=Reflect.has(object,r.key);
        else if(r.op==='delete')result=Reflect.deleteProperty(object,r.key);
        else result=Object.keys(object);
      }else{
        const entry=refs.get(r.id);if(!entry||typeof entry.value!=='function')throw new Error('native function handle is invalid');
        const args=r.args.map(v=>decode(v,0,decodeGraph));
        const guestReceiver=Object.hasOwn(r,'receiver')?decode(r.receiver,0,decodeGraph):entry.receiver;
        receiver=guestReceiver;
        const before=new Map();for(const [graphId,node] of decodeGraph)if(typeof graphId==='string'&&graphId.startsWith('g:')&&!types.isProxy(node))before.set(graphId,descriptorState(node));
        decodeGraph.mutationBefore=before;
        const mutableState=captureMutableState(decodeGraph);
        const guestRefs=decodeGraph.guestRefs;
        let didThrow=false,thrownValue;
        try{
          if(r.op==='construct')result=Reflect.construct(entry.value,args);
          else result=Reflect.apply(entry.value,guestReceiver,args);
        }catch(error){didThrow=true;thrownValue=error;}
        assertMutableStateUnchanged(mutableState);
        const graph=newGraph();
        const encodedResult=didThrow?{t:'undefined'}:encode(result,receiver,0,graph,guestRefs,guestCallbackIds,true);
        const encodedThrow=didThrow?encode(thrownValue,receiver,0,graph,guestRefs,guestCallbackIds,true):undefined;
        const mutations=collectGuestMutations(decodeGraph,before,guestRefs,guestCallbackIds,graph);
        const value={t:'guestCallResult',result:encodedResult,mutations};
        if(didThrow)value.thrown=encodedThrow;
        return {requestId:r.requestId,ok:true,value};
      }
      return {requestId:r.requestId,ok:true,value:encode(result,receiver,0,newGraph())};
    }catch(e){return {requestId:r.requestId,ok:false,error:{name:typeof e?.name==='string'?e.name:'Error',message:typeof e?.message==='string'?e.message:String(e),code:typeof e?.code==='string'?e.code:undefined}};}
    finally{dispatchDepth--;}
  }
  parentPort.on('message',message=>{
    if(message.kind==='request'){
      try{parentPort.postMessage({kind:'response',message:dispatch(message.request)});}
      catch(error){parentPort.postMessage({kind:'response',message:{requestId:message.request.requestId,ok:false,error:{name:'Error',message:String(error)}}});}
    }
  });
  parentPort.postMessage({kind:'ready'});
}
const worker = new Worker('('+addonWorkerMain.toString()+')()', {eval:true});
worker.on('message',message=>{
  if(message.kind==='ready'){workerReady=true;maybeSendHello();}
  else if(message.kind==='response')send(message.message);
  else if(message.kind==='event')send(message.message);
  else if(message.kind==='syncGuestCallback'){
    pendingSyncCallbacks.set(message.callId,message);send(message.message);
  }
});
worker.on('error',error=>socket.destroy(error));
worker.on('exit',code=>{if(code!==0)socket.destroy(new Error('Node addon worker exited with code '+code));});
socket.on('connect',()=>{connected=true;maybeSendHello();});
socket.on('data',c=>{input=Buffer.concat([input,c]);consume();});
socket.on('error',e=>process.stderr.write('napi-vm sidecar: '+e.message+'\n'));
"#;

struct State {
    child: Child,
    stream: TcpStream,
    reader: Option<JoinHandle<()>>,
    response_rx: Receiver<JsonValue>,
    event_rx: Receiver<JsonValue>,
    pending_events: VecDeque<JsonValue>,
    request_id: u64,
    failed: bool,
    next_local_handle: usize,
    local_handles: HashMap<usize, LocalObjectTrap>,
    object_proxies: HashMap<u64, Value>,
    proxy_ids: HashMap<usize, u64>,
    guest_callbacks: HashMap<u64, Value>,
    guest_callback_keys: HashMap<(usize, usize), u64>,
    guest_graph_nodes: HashMap<u64, Value>,
    guest_callback_graphs: HashMap<u64, HashSet<u64>>,
    host_symbols: HashMap<String, Value>,
    symbol_remote_ids: HashMap<u64, String>,
    next_guest_callback_id: u64,
    native_promises: HashMap<u64, Rc<RefCell<PromiseInner>>>,
}

#[derive(Default)]
struct WireEncodeContext {
    seen: HashMap<usize, u64>,
    nodes: HashMap<u64, Value>,
    callbacks: HashMap<u64, Value>,
    active_proxies: HashSet<usize>,
}

impl WireEncodeContext {
    fn register(&mut self, identity: usize, value: Value) -> Result<u64, VmErr> {
        if self.seen.len() >= MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg("guest graph node limit exceeded".into()));
        }
        let id = NEXT_GUEST_GRAPH_NODE_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| VmErr::Msg("native graph id exhausted".into()))?;
        self.seen.insert(identity, id);
        self.nodes.insert(id, value);
        Ok(id)
    }

    fn from_callback_state(state: &State, callback_id: u64) -> Self {
        let mut context = Self {
            callbacks: state.guest_callbacks.clone(),
            ..Self::default()
        };
        if let Some(ids) = state.guest_callback_graphs.get(&callback_id) {
            for id in ids {
                if let Some(value) = state.guest_graph_nodes.get(id) {
                    context.nodes.insert(*id, value.clone());
                    if let Some(identity) = guest_graph_identity(value) {
                        context.seen.insert(identity, *id);
                    }
                }
            }
        }
        context
    }
}

#[derive(Default)]
struct WireDecodeContext {
    nodes: HashMap<String, Value>,
    callbacks: HashMap<u64, Value>,
}

impl WireDecodeContext {
    fn from_encode_context(encoded: &WireEncodeContext) -> Self {
        Self {
            nodes: encoded
                .nodes
                .iter()
                .map(|(id, value)| (format!("g:{id}"), value.clone()))
                .collect(),
            callbacks: encoded.callbacks.clone(),
        }
    }

    fn from_callback_state(state: &State, callback_id: u64) -> Self {
        let nodes = state
            .guest_callback_graphs
            .get(&callback_id)
            .into_iter()
            .flatten()
            .filter_map(|id| {
                state
                    .guest_graph_nodes
                    .get(id)
                    .map(|value| (format!("g:{id}"), value.clone()))
            })
            .collect();
        Self {
            nodes,
            callbacks: state.guest_callbacks.clone(),
        }
    }

    fn register(&mut self, id: String, value: Value) -> Result<(), VmErr> {
        if self.nodes.contains_key(&id) {
            return Err(VmErr::Msg("duplicate Node graph node id".into()));
        }
        if self.nodes.len() >= MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg("Node graph node limit exceeded".into()));
        }
        self.nodes.insert(id, value);
        Ok(())
    }
}

fn guest_graph_identity(value: &Value) -> Option<usize> {
    match value {
        Value::Array(array) => Some(Rc::as_ptr(array) as usize),
        Value::Object { props } => Some(Rc::as_ptr(props) as usize),
        Value::Proxy(proxy) => Some(Rc::as_ptr(proxy) as usize),
        Value::Class(class) => Some(Rc::as_ptr(&class.statics) as usize),
        _ => None,
    }
}

fn guest_callback_identity(value: &Value) -> Option<(usize, usize)> {
    match value {
        Value::Function(function) => Some((
            Rc::as_ptr(&function.body) as usize,
            function
                .closure
                .as_ref()
                .map_or(0, |closure| Rc::as_ptr(closure) as usize),
        )),
        Value::NativeFunction { name, callable } => {
            Some((*callable as *const () as usize, name.as_ptr() as usize))
        }
        Value::Class(class) => Some((
            Rc::as_ptr(&class.statics) as usize,
            Rc::as_ptr(&class.prototype) as usize,
        )),
        _ => None,
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

struct StartupChild(Option<Child>);

impl StartupChild {
    fn as_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("startup child is present")
    }

    fn into_inner(mut self) -> Child {
        self.0.take().expect("startup child is present")
    }
}

impl Drop for StartupChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Loads allowlisted `.node` addons by running a Node.js sidecar process.
///
/// Addon functions and constructors invoke synchronously over a bounded
/// bridge. Native object instances use identity-preserving proxy values whose
/// property operations are forwarded to Node. Addon code runs with host
/// privileges and is not contained by the guest sandbox.
#[derive(Clone)]
pub struct NodeAddonSidecar {
    state: Rc<RefCell<State>>,
    runtime_info: NodeAddonRuntimeInfo,
}

/// Runtime versions reported by the Node process hosting native addons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeAddonRuntimeInfo {
    /// The `process.versions.node` value reported by the sidecar.
    pub node_version: String,
    /// The Node-API ABI version reported by `process.versions.napi`.
    pub napi_version: u32,
}

/// Host configuration for enabling CommonJS modules and trusted `.node`
/// addons in a Rust-embedded interpreter.
///
/// JavaScript modules still execute inside napi-vm. Each native addon must be
/// explicitly listed with [`Self::allow_native_addon`] or
/// [`Self::allow_native_addon_with_sha256`]. Its SHA-256 is pinned when the
/// runtime is configured and checked again when it is loaded. A trusted digest
/// can also be supplied by the host to validate the binary before Node starts.
/// The configured Node executable hosts the Node-API environment in a child
/// process, so it must be compatible with the addon's Node-API requirements.
#[derive(Clone, Debug)]
pub struct NodeAddonOptions {
    pub(crate) node_executable: OsString,
    pub(crate) roots: Vec<PathBuf>,
    pub(crate) allowed_addons: Vec<(PathBuf, Option<[u8; 32]>)>,
    pub(crate) entry: Option<PathBuf>,
    pub(crate) minimum_napi_version: Option<u32>,
}

impl NodeAddonOptions {
    /// Configure the Node executable and filesystem roots visible to
    /// `require()`. Native addon loading stays disabled until at least one
    /// path is added with [`Self::allow_native_addon`] or
    /// [`Self::allow_native_addon_with_sha256`].
    pub fn new<I, P>(node_executable: impl AsRef<OsStr>, roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        Self {
            node_executable: node_executable.as_ref().to_owned(),
            roots: roots.into_iter().map(Into::into).collect(),
            allowed_addons: Vec::new(),
            entry: None,
            minimum_napi_version: None,
        }
    }

    /// Trust one specific native addon binary. Its bytes are pinned when the
    /// interpreter is configured. The path must be inside one of `roots`.
    /// Use [`Self::allow_native_addon_with_sha256`] when the host has an
    /// expected digest from a trusted build manifest.
    pub fn allow_native_addon(mut self, path: impl Into<PathBuf>) -> Self {
        self.allowed_addons.push((path.into(), None));
        self
    }

    /// Allow a native addon only when its bytes match `expected_sha256` from
    /// trusted host metadata. The loader checks the digest during setup and
    /// again immediately before loading the addon.
    pub fn allow_native_addon_with_sha256(
        mut self,
        path: impl Into<PathBuf>,
        expected_sha256: [u8; 32],
    ) -> Self {
        self.allowed_addons
            .push((path.into(), Some(expected_sha256)));
        self
    }

    /// Require the configured Node runtime to provide at least this Node-API
    /// version. The check runs during
    /// [`Interpreter::enable_node_addons`](crate::interpreter::Interpreter::enable_node_addons)
    /// and fails before any guest module or addon is loaded.
    pub fn minimum_napi_version(mut self, version: u32) -> Self {
        self.minimum_napi_version = Some(version);
        self
    }

    /// Set the application entry path used to resolve top-level `require()`.
    /// The path must exist and be inside one of `roots`.
    pub fn entry(mut self, path: impl Into<PathBuf>) -> Self {
        self.entry = Some(path.into());
        self
    }
}

impl std::fmt::Debug for NodeAddonSidecar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeAddonSidecar").finish_non_exhaustive()
    }
}

impl NodeAddonSidecar {
    pub fn new(node_executable: impl AsRef<OsStr>) -> Result<Self, VmErr> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|e| VmErr::Msg(format!("cannot bind Node bridge: {e}")))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| VmErr::Msg(format!("cannot configure Node bridge: {e}")))?;
        let address = listener
            .local_addr()
            .map_err(|e| VmErr::Msg(format!("cannot read Node bridge address: {e}")))?;
        let token = token();
        let mut child = StartupChild(Some(
            Command::new(node_executable)
                .arg("--no-warnings")
                .arg("-e")
                .arg(NODE_BRIDGE)
                .env("NAPI_VM_BRIDGE_PORT", address.port().to_string())
                .env("NAPI_VM_BRIDGE_TOKEN", &token)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|e| VmErr::Msg(format!("cannot start Node sidecar: {e}")))?,
        ));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Some(status) = child
                        .as_mut()
                        .try_wait()
                        .map_err(|e| VmErr::Msg(format!("cannot inspect Node sidecar: {e}")))?
                    {
                        return Err(VmErr::Msg(format!(
                            "Node sidecar exited during startup ({status})"
                        )));
                    }
                    if Instant::now() >= deadline {
                        return Err(VmErr::Msg("timed out starting Node sidecar".into()));
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => {
                    return Err(VmErr::Msg(format!("cannot accept Node bridge: {e}")));
                }
            }
        };
        stream
            .set_read_timeout(None)
            .map_err(|e| VmErr::Msg(format!("cannot configure Node bridge: {e}")))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| VmErr::Msg(format!("cannot set Node bridge timeout: {e}")))?;
        let hello = read_frame(&mut stream)?;
        if hello.get("hello").and_then(JsonValue::as_str) != Some(&token) {
            return Err(VmErr::Msg("Node sidecar authentication failed".into()));
        }
        let runtime_info = NodeAddonRuntimeInfo {
            node_version: hello
                .get("nodeVersion")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("Node sidecar did not report its Node version".into()))?
                .to_string(),
            napi_version: hello
                .get("napiVersion")
                .and_then(JsonValue::as_str)
                .and_then(|version| version.parse().ok())
                .ok_or_else(|| {
                    VmErr::Msg("Node sidecar does not expose a Node-API version".into())
                })?,
        };
        let mut read_stream = stream
            .try_clone()
            .map_err(|e| VmErr::Msg(format!("cannot clone Node bridge stream: {e}")))?;
        let (response_tx, response_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let reader = std::thread::Builder::new()
            .name("napi-vm-node-events".into())
            .spawn(move || {
                loop {
                    let Ok(frame) = read_frame(&mut read_stream) else {
                        break;
                    };
                    let sender = if frame.get("event").is_some() {
                        &event_tx
                    } else {
                        &response_tx
                    };
                    if sender.send(frame).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| VmErr::Msg(format!("cannot start Node bridge reader: {e}")))?;
        Ok(Self {
            state: Rc::new(RefCell::new(State {
                child: child.into_inner(),
                stream,
                reader: Some(reader),
                response_rx,
                event_rx,
                pending_events: VecDeque::new(),
                request_id: 1,
                failed: false,
                next_local_handle: 0,
                local_handles: HashMap::new(),
                object_proxies: HashMap::new(),
                proxy_ids: HashMap::new(),
                guest_callbacks: HashMap::new(),
                guest_callback_keys: HashMap::new(),
                guest_graph_nodes: HashMap::new(),
                guest_callback_graphs: HashMap::new(),
                host_symbols: HashMap::new(),
                symbol_remote_ids: HashMap::new(),
                next_guest_callback_id: 1,
                native_promises: HashMap::new(),
            })),
            runtime_info,
        })
    }

    /// Return the Node and Node-API versions supplied by the hosting process.
    pub fn runtime_info(&self) -> &NodeAddonRuntimeInfo {
        &self.runtime_info
    }

    fn request(&self, message: JsonValue) -> Result<JsonValue, VmErr> {
        self.request_with_callback_handler(
            message,
            &mut |_| {
                Err(VmErr::Msg(
                    "synchronous guest callback was requested outside a VM host call".into(),
                ))
            },
            &WireEncodeContext::default(),
        )
    }

    fn persist_guest_graph(
        &self,
        graph: &WireEncodeContext,
        callback_scope: Option<u64>,
    ) -> Result<(), VmErr> {
        if graph.callbacks.is_empty() && callback_scope.is_none() {
            return Ok(());
        }
        let mut state = self.state.borrow_mut();
        let additional = graph
            .nodes
            .keys()
            .filter(|id| !state.guest_graph_nodes.contains_key(id))
            .count();
        if state.guest_graph_nodes.len().saturating_add(additional) > MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg(
                "persistent guest graph node limit exceeded".into(),
            ));
        }
        for (id, value) in &graph.nodes {
            state
                .guest_graph_nodes
                .entry(*id)
                .or_insert_with(|| value.clone());
        }
        if let Some(callback_id) = callback_scope {
            state
                .guest_callback_graphs
                .entry(callback_id)
                .or_default()
                .extend(graph.nodes.keys().copied());
        }
        let callback_ids = callback_scope
            .map(|callback_id| vec![callback_id])
            .unwrap_or_else(|| graph.callbacks.keys().copied().collect());
        for callback_id in callback_ids {
            state
                .guest_callback_graphs
                .entry(callback_id)
                .or_default()
                .extend(graph.nodes.keys().copied());
        }
        Ok(())
    }

    fn encode_context_for_callback(&self, callback_id: u64) -> WireEncodeContext {
        WireEncodeContext::from_callback_state(&self.state.borrow(), callback_id)
    }

    fn guest_graph_snapshots(
        &self,
        callback_id: u64,
    ) -> Result<(Vec<JsonValue>, WireEncodeContext), VmErr> {
        let (mut graph, nodes) = {
            let state = self.state.borrow();
            let mut graph = WireEncodeContext::from_callback_state(&state, callback_id);
            graph.callbacks.clear();
            let nodes = state
                .guest_callback_graphs
                .get(&callback_id)
                .into_iter()
                .flatten()
                .filter_map(|id| {
                    state
                        .guest_graph_nodes
                        .get(id)
                        .map(|value| (*id, value.clone()))
                })
                .collect::<Vec<_>>();
            (graph, nodes)
        };
        let mut snapshots = Vec::new();
        for (id, value) in nodes {
            if let Some(snapshot) = guest_graph_node_snapshot(self, id, &value, &mut graph)? {
                snapshots.push(snapshot);
            }
        }
        Ok((snapshots, graph))
    }

    fn persist_snapshot_graph(
        &self,
        graph: &WireEncodeContext,
        callback_id: u64,
    ) -> Result<(), VmErr> {
        self.persist_guest_graph(graph, Some(callback_id))?;
        for new_callback_id in graph.callbacks.keys().copied() {
            if new_callback_id != callback_id {
                self.persist_guest_graph(graph, Some(new_callback_id))?;
            }
        }
        Ok(())
    }

    fn request_with_callback_handler(
        &self,
        mut message: JsonValue,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
        guest_graph: &WireEncodeContext,
    ) -> Result<JsonValue, VmErr> {
        self.persist_guest_graph(guest_graph, None)?;
        let id = {
            let mut state = self.state.borrow_mut();
            if state.failed {
                return Err(VmErr::Msg(
                    "Node addon bridge is unavailable after a transport failure".into(),
                ));
            }
            let id = state.request_id;
            state.request_id = state
                .request_id
                .checked_add(1)
                .ok_or_else(|| VmErr::Msg("Node request id exhausted".into()))?;
            message
                .as_object_mut()
                .ok_or_else(|| VmErr::Msg("invalid internal Node request".into()))?
                .insert("requestId".into(), json!(id));
            if let Err(error) = write_frame(&mut state.stream, &message) {
                fail_state(&mut state);
                return Err(error);
            }
            id
        };

        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let event = {
                let mut state = self.state.borrow_mut();
                state
                    .pending_events
                    .pop_front()
                    .or_else(|| state.event_rx.try_recv().ok())
            };
            if let Some(event) = event {
                if event.get("event").and_then(JsonValue::as_str) == Some("syncGuestCallback") {
                    self.answer_sync_guest_callback(&event, callback_handler)?;
                } else {
                    self.state.borrow_mut().pending_events.push_back(event);
                }
            }

            let response = {
                let state = self.state.borrow();
                match state.response_rx.try_recv() {
                    Ok(response) => Some(response),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err(VmErr::Msg("Node sidecar disconnected".into()));
                    }
                    Err(mpsc::TryRecvError::Empty) => None,
                }
            };
            if let Some(response) = response {
                if response.get("requestId").and_then(JsonValue::as_u64) != Some(id) {
                    return Err(VmErr::Msg("Node response id mismatch".into()));
                }
                if response.get("ok").and_then(JsonValue::as_bool) == Some(true) {
                    return response
                        .get("value")
                        .cloned()
                        .ok_or_else(|| VmErr::Msg("Node response has no value".into()));
                }
                let error = response.get("error").unwrap_or(&JsonValue::Null);
                let name = error
                    .get("name")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("Error");
                let message = error
                    .get("message")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("native addon failed");
                let code = error.get("code").and_then(JsonValue::as_str);
                let error = Value::Error(match code {
                    Some(code) => crate::value::ErrorData::with_code(name, message, code),
                    None => crate::value::ErrorData::new(name, message),
                });
                return Err(VmErr::Throw(error));
            }

            let now = Instant::now();
            if now >= deadline {
                let mut state = self.state.borrow_mut();
                fail_state(&mut state);
                return Err(VmErr::Msg(
                    "Node sidecar did not respond before timeout".into(),
                ));
            }
            let wait = (deadline - now).min(Duration::from_millis(2));
            let receive = {
                let state = self.state.borrow();
                state.response_rx.recv_timeout(wait)
            };
            match receive {
                Ok(response) => {
                    if response.get("requestId").and_then(JsonValue::as_u64) != Some(id) {
                        return Err(VmErr::Msg("Node response id mismatch".into()));
                    }
                    if response.get("ok").and_then(JsonValue::as_bool) == Some(true) {
                        return response
                            .get("value")
                            .cloned()
                            .ok_or_else(|| VmErr::Msg("Node response has no value".into()));
                    }
                    let error = response.get("error").unwrap_or(&JsonValue::Null);
                    let name = error
                        .get("name")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("Error");
                    let message = error
                        .get("message")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("native addon failed");
                    let code = error.get("code").and_then(JsonValue::as_str);
                    let error = Value::Error(match code {
                        Some(code) => crate::value::ErrorData::with_code(name, message, code),
                        None => crate::value::ErrorData::new(name, message),
                    });
                    return Err(VmErr::Throw(error));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(VmErr::Msg("Node sidecar disconnected".into()));
                }
            }
        }
    }

    fn answer_sync_guest_callback(
        &self,
        event: &JsonValue,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<(), VmErr> {
        let call_id = event
            .get("callId")
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| VmErr::Msg("Node synchronous callback id is invalid".into()))?;
        let callback_id = event
            .get("callbackId")
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| VmErr::Msg("Node callback event has an invalid id".into()))?;
        let callback = self.guest_callback_from_event(event)?;
        let result = callback_handler(callback);
        let mut graph = self.encode_context_for_callback(callback_id);
        let (mut ok, mut value) = match result {
            Ok(value) => match self.guest_to_wire_with_context(&value, 0, &mut graph) {
                Ok(value) => (true, value),
                Err(error) => (
                    false,
                    json!({"t":"error","name":"TypeError","message":error.to_string()}),
                ),
            },
            Err(VmErr::Throw(reason)) => {
                match self.guest_to_wire_with_context(&reason, 0, &mut graph) {
                    Ok(value) => (false, value),
                    Err(error) => (
                        false,
                        json!({"t":"error","name":"TypeError","message":error.to_string()}),
                    ),
                }
            }
            Err(error) => (
                false,
                json!({"t":"error","name":"Error","message":error.to_string()}),
            ),
        };
        self.persist_guest_graph(&graph, Some(callback_id))?;
        let snapshots = match self.guest_graph_snapshots(callback_id) {
            Ok((snapshots, snapshot_graph)) => {
                self.persist_snapshot_graph(&snapshot_graph, callback_id)?;
                snapshots
            }
            Err(error) => {
                ok = false;
                value = json!({"t":"error","name":"TypeError","message":error.to_string()});
                Vec::new()
            }
        };
        let response = json!({
            "event":"syncGuestCallbackResult",
            "callId":call_id,
            "ok":ok,
            "value":value,
            "snapshots":snapshots,
        });
        let mut state = self.state.borrow_mut();
        write_frame(&mut state.stream, &response)
    }

    fn guest_callback_from_event(&self, event: &JsonValue) -> Result<HostCallback, VmErr> {
        let callback_id = event
            .get("callbackId")
            .and_then(JsonValue::as_u64)
            .ok_or_else(|| VmErr::Msg("Node callback event has an invalid id".into()))?;
        let (callback, mut graph) = {
            let state = self.state.borrow();
            let callback = state
                .guest_callbacks
                .get(&callback_id)
                .cloned()
                .ok_or_else(|| VmErr::Msg("Node callback handle is invalid".into()))?;
            (
                callback,
                WireDecodeContext::from_callback_state(&state, callback_id),
            )
        };
        if let Some(mutations) = event.get("guestMutations").and_then(JsonValue::as_array) {
            for mutation in mutations {
                apply_guest_mutation(self, mutation, &mut graph)?;
            }
        }
        let this_wire = event
            .get("thisValue")
            .cloned()
            .unwrap_or_else(|| json!({"t":"undefined"}));
        let this_value = wire_to_guest_with_context(self, &this_wire, 0, &mut graph)?;
        let args = event
            .get("args")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| VmErr::Msg("Node callback event has invalid arguments".into()))?
            .iter()
            .map(|arg| wire_to_guest_with_context(self, arg, 0, &mut graph))
            .collect::<Result<Vec<_>, _>>()?;
        let kind = match event.get("kind").and_then(JsonValue::as_str) {
            None | Some("call") => HostCallbackKind::Call,
            Some("construct") => HostCallbackKind::Construct,
            Some(_) => return Err(VmErr::Msg("Node callback event has an invalid kind".into())),
        };
        Ok(HostCallback {
            callback,
            this_value,
            args,
            kind,
        })
    }

    fn wire_to_guest(&self, value: &JsonValue, depth: usize) -> Result<Value, VmErr> {
        wire_to_guest(self, value, depth)
    }

    fn guest_to_wire_with_context(
        &self,
        value: &Value,
        depth: usize,
        graph: &mut WireEncodeContext,
    ) -> Result<JsonValue, VmErr> {
        let proxy_ids = self.state.borrow().proxy_ids.clone();
        guest_to_wire(self, value, depth, graph, &proxy_ids)
    }

    fn register_guest_callback(&self, callback: Value) -> Result<u64, VmErr> {
        let mut state = self.state.borrow_mut();
        let identity = guest_callback_identity(&callback);
        if let Some(id) = identity.and_then(|identity| state.guest_callback_keys.get(&identity)) {
            return Ok(*id);
        }
        if state.guest_callbacks.len() >= MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg("guest callback handle limit exceeded".into()));
        }
        let id = state.next_guest_callback_id;
        state.next_guest_callback_id = state
            .next_guest_callback_id
            .checked_add(1)
            .ok_or_else(|| VmErr::Msg("guest callback handle id exhausted".into()))?;
        state.guest_callbacks.insert(id, callback);
        if let Some(identity) = identity {
            state.guest_callback_keys.insert(identity, id);
        }
        Ok(id)
    }

    fn host_object(&self, object_id: u64) -> Result<Value, VmErr> {
        let mut state = self.state.borrow_mut();
        if let Some(proxy) = state.object_proxies.get(&object_id) {
            return Ok(proxy.clone());
        }
        if state.object_proxies.len() >= MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg("native object handle limit exceeded".into()));
        }

        let target = Value::object(vec![]);
        let mut traps = Vec::with_capacity(5);
        for operation in [
            ObjectOperation::Get,
            ObjectOperation::Set,
            ObjectOperation::Has,
            ObjectOperation::Delete,
            ObjectOperation::OwnKeys,
        ] {
            let local_id = usize::MAX
                .checked_sub(state.next_local_handle)
                .ok_or_else(|| VmErr::Msg("local host handle id exhausted".into()))?;
            state.next_local_handle = state
                .next_local_handle
                .checked_add(1)
                .ok_or_else(|| VmErr::Msg("local host handle id exhausted".into()))?;
            state.local_handles.insert(
                local_id,
                LocalObjectTrap {
                    object_id,
                    operation,
                },
            );
            traps.push((
                operation.guest_name().to_string(),
                Value::HostFunction {
                    name: operation.guest_name().into(),
                    id: local_id,
                },
            ));
        }
        let handler = Value::object(traps);
        let proxy_data = Rc::new(crate::value::ProxyData { target, handler });
        let proxy_id = Rc::as_ptr(&proxy_data) as usize;
        let proxy = Value::Proxy(proxy_data);
        state.proxy_ids.insert(proxy_id, object_id);
        state.object_proxies.insert(object_id, proxy.clone());
        Ok(proxy)
    }

    fn host_promise(&self, promise_id: u64) -> Result<Value, VmErr> {
        let mut state = self.state.borrow_mut();
        if let Some(promise) = state.native_promises.get(&promise_id) {
            return Ok(Value::Promise(promise.clone()));
        }
        if state.native_promises.len() >= MAX_NATIVE_HANDLES {
            return Err(VmErr::Msg("native promise handle limit exceeded".into()));
        }
        let promise = Value::pending_promise();
        promise.borrow_mut().external_pending = true;
        state.native_promises.insert(promise_id, promise.clone());
        Ok(Value::Promise(promise))
    }

    fn dispatch_object_trap_with_callback_handler(
        &self,
        trap: LocalObjectTrap,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        let mut graph = WireEncodeContext::default();
        let proxy_ids = self.state.borrow().proxy_ids.clone();
        let request = match trap.operation {
            ObjectOperation::Get => json!({
                "op": "get",
                "id": trap.object_id,
                "key": required_string_arg(&args, 1)?,
            }),
            ObjectOperation::Set => json!({
                "op": "set",
                "id": trap.object_id,
                "key": required_string_arg(&args, 1)?,
                "value": guest_to_wire(
                    self,
                    args.get(2).unwrap_or(&Value::Undefined),
                    0,
                    &mut graph,
                    &proxy_ids,
                )?,
            }),
            ObjectOperation::Has => json!({
                "op": "has",
                "id": trap.object_id,
                "key": required_string_arg(&args, 1)?,
            }),
            ObjectOperation::Delete => json!({
                "op": "delete",
                "id": trap.object_id,
                "key": required_string_arg(&args, 1)?,
            }),
            ObjectOperation::OwnKeys => json!({
                "op": "ownKeys",
                "id": trap.object_id,
            }),
        };
        let result = self.request_with_callback_handler(request, callback_handler, &graph)?;
        self.wire_to_guest(&result, 0)
    }
}

fn fail_state(state: &mut State) {
    state.failed = true;
    let _ = state.stream.shutdown(Shutdown::Both);
    let _ = state.child.kill();
    let _ = state.child.wait();
}

impl NativeAddonLoader for NodeAddonSidecar {
    fn load(&self, filename: &Path) -> Result<Value, VmErr> {
        let filename = filename
            .to_str()
            .ok_or_else(|| VmErr::Msg("native addon path is not UTF-8".into()))?;
        self.wire_to_guest(&self.request(json!({"op":"load","filename":filename}))?, 0)
    }
}

impl HostBridge for NodeAddonSidecar {
    fn poll_host_events(&self, timeout: Duration) -> Result<Vec<HostEvent>, VmErr> {
        let events = {
            let mut state = self.state.borrow_mut();
            let mut events = Vec::new();
            events.extend(state.pending_events.drain(..));
            if events.is_empty() {
                let first = if timeout.is_zero() {
                    state.event_rx.try_recv().ok()
                } else {
                    state.event_rx.recv_timeout(timeout).ok()
                };
                if let Some(event) = first {
                    events.push(event);
                    events.extend(state.event_rx.try_iter());
                }
            }
            events
        };

        let mut host_events = Vec::with_capacity(events.len());
        for event in events {
            match event.get("event").and_then(JsonValue::as_str) {
                Some("guestCallbackError") => {
                    let error = event.get("error").unwrap_or(&JsonValue::Null);
                    let name = error
                        .get("name")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("Error");
                    let message = error
                        .get("message")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("guest callback arguments could not be marshalled");
                    return Err(VmErr::Throw(Value::Error(crate::value::ErrorData::new(
                        name, message,
                    ))));
                }
                Some("guestCallback") => {}
                Some("syncGuestCallback") => {
                    return Err(VmErr::Msg(
                        "synchronous guest callback arrived outside a host call".into(),
                    ));
                }
                Some("hostPromiseSettled") => {
                    let promise_id = event
                        .get("promiseId")
                        .and_then(JsonValue::as_u64)
                        .ok_or_else(|| VmErr::Msg("Node promise event has an invalid id".into()))?;
                    let promise = self
                        .state
                        .borrow()
                        .native_promises
                        .get(&promise_id)
                        .cloned()
                        .ok_or_else(|| VmErr::Msg("Node promise handle is invalid".into()))?;
                    let state = match event.get("state").and_then(JsonValue::as_str) {
                        Some("fulfilled") => PromiseState::Fulfilled,
                        Some("rejected") => PromiseState::Rejected,
                        _ => {
                            return Err(VmErr::Msg(
                                "Node promise event has an invalid state".into(),
                            ));
                        }
                    };
                    let wire = event
                        .get("value")
                        .ok_or_else(|| VmErr::Msg("Node promise event has no value".into()))?;
                    host_events.push(HostEvent::PromiseSettled {
                        promise,
                        state,
                        value: self.wire_to_guest(wire, 0)?,
                    });
                    continue;
                }
                other => {
                    return Err(VmErr::Msg(format!(
                        "unknown Node sidecar event {:?}",
                        other
                    )));
                }
            }

            host_events.push(HostEvent::Callback(self.guest_callback_from_event(&event)?));
        }
        Ok(host_events)
    }

    fn has_pending_host_work(&self, promise: &Rc<RefCell<PromiseInner>>) -> bool {
        promise.borrow().external_pending
    }

    fn call_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        self.call_host_with_this(id, Value::Undefined, args)
    }

    fn call_host_with_this(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
    ) -> Result<Value, VmErr> {
        self.call_host_with_callback_handler(id, this_value, args, &mut |_| {
            Err(VmErr::Msg(
                "synchronous guest callback was requested outside a VM host call".into(),
            ))
        })
    }

    fn call_host_with_callback_handler(
        &self,
        id: usize,
        this_value: Value,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        let trap = self.state.borrow().local_handles.get(&id).copied();
        if let Some(trap) = trap {
            return self.dispatch_object_trap_with_callback_handler(trap, args, callback_handler);
        }
        let proxy_ids = self.state.borrow().proxy_ids.clone();
        let mut graph = WireEncodeContext::default();
        let args = args
            .iter()
            .map(|v| guest_to_wire(self, v, 0, &mut graph, &proxy_ids))
            .collect::<Result<Vec<_>, _>>()?;
        let receiver = guest_to_wire(self, &this_value, 0, &mut graph, &proxy_ids)?;
        let response = self.request_with_callback_handler(
            json!({"op":"call","id":id,"args":args,"receiver":receiver}),
            callback_handler,
            &graph,
        )?;
        guest_call_result_to_value(self, &response, &graph)
    }
    fn construct_host(&self, id: usize, args: Vec<Value>) -> Result<Value, VmErr> {
        self.construct_host_with_callback_handler(id, args, &mut |_| {
            Err(VmErr::Msg(
                "synchronous guest callback was requested outside a VM host call".into(),
            ))
        })
    }
    fn construct_host_with_callback_handler(
        &self,
        id: usize,
        args: Vec<Value>,
        callback_handler: &mut dyn FnMut(HostCallback) -> Result<Value, VmErr>,
    ) -> Result<Value, VmErr> {
        let proxy_ids = self.state.borrow().proxy_ids.clone();
        let mut graph = WireEncodeContext::default();
        let args = args
            .iter()
            .map(|v| guest_to_wire(self, v, 0, &mut graph, &proxy_ids))
            .collect::<Result<Vec<_>, _>>()?;
        let response = self.request_with_callback_handler(
            json!({"op":"construct","id":id,"args":args}),
            callback_handler,
            &graph,
        )?;
        guest_call_result_to_value(self, &response, &graph)
    }
}

fn token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}-{time:x}-{:x}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}
fn write_frame(stream: &mut TcpStream, value: &JsonValue) -> Result<(), VmErr> {
    let body = serde_json::to_vec(value)
        .map_err(|e| VmErr::Msg(format!("cannot encode Node frame: {e}")))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(VmErr::Msg("Node frame exceeds size limit".into()));
    }
    let size = u32::try_from(body.len()).map_err(|_| VmErr::Msg("Node frame too large".into()))?;
    stream
        .write_all(&size.to_be_bytes())
        .and_then(|()| stream.write_all(&body))
        .map_err(|e| VmErr::Msg(format!("Node sidecar write failed: {e}")))
}
fn read_frame(stream: &mut TcpStream) -> Result<JsonValue, VmErr> {
    let mut header = [0; 4];
    stream
        .read_exact(&mut header)
        .map_err(|e| VmErr::Msg(format!("Node sidecar disconnected: {e}")))?;
    let size = u32::from_be_bytes(header) as usize;
    if size > MAX_FRAME_BYTES {
        return Err(VmErr::Msg("Node frame exceeds size limit".into()));
    }
    let mut body = vec![0; size];
    stream
        .read_exact(&mut body)
        .map_err(|e| VmErr::Msg(format!("Node sidecar disconnected: {e}")))?;
    serde_json::from_slice(&body).map_err(|e| VmErr::Msg(format!("invalid Node frame: {e}")))
}

fn required_string_arg(args: &[Value], index: usize) -> Result<String, VmErr> {
    match args.get(index) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => Err(VmErr::Msg("native object property key is invalid".into())),
    }
}

fn typed_kind(name: &str) -> Option<TypedKind> {
    Some(match name {
        "Int8Array" => TypedKind::Int8,
        "Uint8Array" => TypedKind::Uint8,
        "Uint8ClampedArray" => TypedKind::Uint8Clamped,
        "Int16Array" => TypedKind::Int16,
        "Uint16Array" => TypedKind::Uint16,
        "Int32Array" => TypedKind::Int32,
        "Uint32Array" => TypedKind::Uint32,
        "Float32Array" => TypedKind::Float32,
        "Float64Array" => TypedKind::Float64,
        "BigInt64Array" => TypedKind::BigInt64,
        "BigUint64Array" => TypedKind::BigUint64,
        _ => return None,
    })
}

fn wire_bytes(value: &JsonValue) -> Result<Vec<u8>, VmErr> {
    value
        .as_array()
        .ok_or_else(|| VmErr::Msg("invalid Node bytes".into()))?
        .iter()
        .map(|byte| {
            byte.as_u64()
                .filter(|value| *value <= 255)
                .map(|value| value as u8)
                .ok_or_else(|| VmErr::Msg("invalid Node byte".into()))
        })
        .collect()
}

fn guest_accessor_kind(key: &str, value: &Value) -> Option<&'static str> {
    let name = match value {
        Value::Function(function) => function.name.as_deref(),
        Value::NativeFunction { name, .. } | Value::HostFunction { name, .. } => {
            Some(name.as_ref())
        }
        _ => None,
    }?;
    if name == format!("get {key}") {
        Some("get")
    } else if name == format!("set {key}") {
        Some("set")
    } else {
        None
    }
}

fn guest_symbol_key_wire(sidecar: &NodeAddonSidecar, symbol: &SymbolData) -> JsonValue {
    let remote_id = sidecar
        .state
        .borrow()
        .symbol_remote_ids
        .get(&symbol.id)
        .cloned()
        .unwrap_or_else(|| format!("g:{}", symbol.id));
    json!({
        "t": "symbol",
        "v": remote_id,
        "description": symbol.description,
    })
}

fn guest_to_wire(
    sidecar: &NodeAddonSidecar,
    v: &Value,
    depth: usize,
    graph: &mut WireEncodeContext,
    proxy_ids: &HashMap<usize, u64>,
) -> Result<JsonValue, VmErr> {
    if depth > MAX_WIRE_DEPTH {
        return Err(VmErr::Msg("guest value exceeds bridge depth limit".into()));
    }
    Ok(match v {
        Value::Undefined => json!({"t":"undefined"}),
        Value::Null => json!({"t":"null"}),
        Value::Bool(x) => json!({"t":"boolean","v":x}),
        Value::Number(x) => {
            json!({"t":"number","v":if x.is_nan(){"NaN".into()}else if *x==f64::INFINITY{"Infinity".into()}else if *x==f64::NEG_INFINITY{"-Infinity".into()}else if *x==0.0&&x.is_sign_negative(){"-0".into()}else{x.to_string()}})
        }
        Value::String(x) => {
            if x.len() > MAX_STRING_LEN {
                return Err(VmErr::Msg("guest string exceeds bridge limit".into()));
            }
            json!({"t":"string","v":x})
        }
        Value::Array(a) => {
            let id = Rc::as_ptr(a) as usize;
            if let Some(node_id) = graph.seen.get(&id) {
                return Ok(json!({"t":"ref","v":format!("g:{node_id}")}));
            }
            let items = a.borrow().clone();
            let presence = a.presence_snapshot();
            if items.len() > MAX_ARRAY_LEN {
                return Err(VmErr::Msg("guest array exceeds limit".into()));
            }
            let node_id = graph.register(id, v.clone())?;
            let wire = items
                .iter()
                .enumerate()
                .map(|(index, x)| {
                    if presence.get(index).copied().unwrap_or(true) {
                        guest_to_wire(sidecar, x, depth + 1, graph, proxy_ids)
                    } else {
                        Ok(json!({"t":"hole"}))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            let named = a
                .named
                .borrow()
                .iter()
                .filter(|(key, _)| !crate::interpreter::is_internal_key(key))
                .map(|(key, value)| {
                    Ok(json!([
                        key,
                        guest_to_wire(sidecar, value, depth + 1, graph, proxy_ids)?
                    ]))
                })
                .collect::<Result<Vec<_>, VmErr>>()?;
            json!({"t":"array","id":format!("g:{node_id}"),"v":wire,"named":named})
        }
        Value::Object { props } => {
            let id = Rc::as_ptr(props) as usize;
            if let Some(node_id) = graph.seen.get(&id) {
                return Ok(json!({"t":"ref","v":format!("g:{node_id}")}));
            }
            let entries = props.borrow().clone();
            if entries.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg("guest object exceeds limit".into()));
            }
            let meta = props.meta.borrow();
            let extensible = !meta.non_extensible;
            let node_id = graph.register(id, v.clone())?;
            let mut wire = Vec::with_capacity(entries.len());
            for (key, value) in &entries {
                let wire_key = if let Some(symbol) = meta.symbol_key(key) {
                    guest_symbol_key_wire(sidecar, &symbol)
                } else if crate::interpreter::symbol_id_from_slot(key).is_some() {
                    return Err(VmErr::Msg(
                        "guest symbol property has no symbol metadata".into(),
                    ));
                } else if crate::interpreter::is_internal_key(key) {
                    continue;
                } else {
                    json!(key)
                };
                let attrs = meta.attrs_of(key);
                let kind = guest_accessor_kind(key, value);
                let getter = (kind == Some("get")).then_some(value);
                let setter = if kind == Some("set") {
                    Some(value)
                } else if getter.is_some() {
                    entries
                        .iter()
                        .find(|(slot, _)| slot == &format!("__setter:{key}__"))
                        .and_then(|(_, candidate)| {
                            (guest_accessor_kind(key, candidate) == Some("set"))
                                .then_some(candidate)
                        })
                } else {
                    None
                };
                if getter.is_some() || setter.is_some() {
                    let getter = getter
                        .map(|getter| guest_to_wire(sidecar, getter, depth + 1, graph, proxy_ids))
                        .transpose()?
                        .unwrap_or(JsonValue::Null);
                    let setter = setter
                        .map(|setter| guest_to_wire(sidecar, setter, depth + 1, graph, proxy_ids))
                        .transpose()?
                        .unwrap_or(JsonValue::Null);
                    wire.push(json!([
                        wire_key,
                        {"t":"undefined"},
                        true,
                        attrs.enumerable,
                        attrs.configurable,
                        getter,
                        setter
                    ]));
                } else {
                    wire.push(json!([
                        wire_key,
                        guest_to_wire(sidecar, value, depth + 1, graph, proxy_ids)?,
                        attrs.writable,
                        attrs.enumerable,
                        attrs.configurable
                    ]));
                }
            }
            let prototype = if meta.uses_default_prototype {
                json!({"t":"defaultPrototype"})
            } else {
                match meta.proto.as_deref() {
                    Some(prototype) => {
                        guest_to_wire(sidecar, prototype, depth + 1, graph, proxy_ids)?
                    }
                    None => json!({"t":"null"}),
                }
            };
            json!({"t":"object","id":format!("g:{node_id}"),"v":wire,"prototype":prototype,"extensible":extensible})
        }
        Value::ArrayBuffer(bytes) => json!({"t":"arrayBuffer","v":bytes.borrow().as_slice()}),
        Value::TypedArray(view) => {
            let start = view.byte_offset;
            let byte_len = view
                .length
                .checked_mul(view.kind.size())
                .ok_or_else(|| VmErr::Msg("guest typed array exceeds the bridge limit".into()))?;
            let end = start
                .checked_add(byte_len)
                .ok_or_else(|| VmErr::Msg("guest typed array exceeds the bridge limit".into()))?;
            let bytes = view.buffer.borrow();
            let slice = bytes
                .get(start..end)
                .ok_or_else(|| VmErr::Msg("guest typed array has an invalid byte range".into()))?;
            json!({"t":"typedArray","kind":view.kind.name(),"length":view.length,"bytes":slice})
        }
        Value::DataView(view) => {
            let start = view.byte_offset;
            let end = start
                .checked_add(view.length)
                .ok_or_else(|| VmErr::Msg("guest DataView exceeds the bridge limit".into()))?;
            let bytes = view.buffer.borrow();
            let slice = bytes
                .get(start..end)
                .ok_or_else(|| VmErr::Msg("guest DataView has an invalid byte range".into()))?;
            json!({"t":"dataView","length":view.length,"bytes":slice})
        }
        Value::BigInt(x) => json!({"t":"bigint","v":x.to_string()}),
        Value::Date(milliseconds) => {
            let value = milliseconds.get();
            let wire = if value.is_nan() {
                "NaN".to_string()
            } else if value == f64::INFINITY {
                "Infinity".to_string()
            } else if value == f64::NEG_INFINITY {
                "-Infinity".to_string()
            } else {
                value.to_string()
            };
            json!({"t":"date","v":wire})
        }
        Value::RegExp(data) => json!({
            "t":"regexp",
            "source":data.regex.source,
            "flags":data.regex.flags,
            "lastIndex":data.last_index.get().to_string(),
        }),
        Value::Symbol(symbol) => {
            let remote_id = sidecar
                .state
                .borrow()
                .symbol_remote_ids
                .get(&symbol.id)
                .cloned()
                .unwrap_or_else(|| format!("g:{}", symbol.id));
            json!({
                "t":"symbol",
                "v":remote_id,
                "description":symbol.description,
            })
        }
        Value::Error(error) => json!({
            "t":"error",
            "name":error.name,
            "message":error.message,
            "code":error.code,
        }),
        Value::Proxy(proxy) => {
            let proxy_id = Rc::as_ptr(proxy) as usize;
            match proxy_ids.get(&proxy_id) {
                Some(object_id) => json!({"t":"hostObject","v":object_id}),
                None => {
                    if let Some(node_id) = graph.seen.get(&proxy_id) {
                        if graph.active_proxies.contains(&proxy_id) {
                            return Err(VmErr::Msg(
                                "cyclic guest Proxy graphs cannot cross the Node addon bridge yet"
                                    .into(),
                            ));
                        }
                        json!({"t":"ref","v":format!("g:{node_id}")})
                    } else {
                        let node_id = graph.register(proxy_id, v.clone())?;
                        graph.active_proxies.insert(proxy_id);
                        let target =
                            guest_to_wire(sidecar, &proxy.target, depth + 1, graph, proxy_ids)?;
                        let handler =
                            guest_to_wire(sidecar, &proxy.handler, depth + 1, graph, proxy_ids)?;
                        graph.active_proxies.remove(&proxy_id);
                        json!({
                            "t":"proxy",
                            "id":format!("g:{node_id}"),
                            "target":target,
                            "handler":handler,
                        })
                    }
                }
            }
        }
        Value::Class(class) => {
            let identity = Rc::as_ptr(&class.statics) as usize;
            if let Some(node_id) = graph.seen.get(&identity) {
                return Ok(json!({"t":"ref","v":format!("g:{node_id}")}));
            }
            if class.statics.borrow().len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg(
                    "guest class exceeds bridge property limit".into(),
                ));
            }
            let node_id = graph.register(identity, v.clone())?;
            let callback_id = sidecar.register_guest_callback(v.clone())?;
            graph.callbacks.insert(callback_id, v.clone());
            let prototype = guest_to_wire(
                sidecar,
                class.prototype.as_ref(),
                depth + 1,
                graph,
                proxy_ids,
            )?;
            let statics = class
                .statics
                .borrow()
                .iter()
                .filter(|(key, _)| {
                    !matches!(key.as_str(), "name" | "length" | "prototype")
                        && !crate::interpreter::is_internal_key(key)
                })
                .map(|(key, value)| {
                    Ok(json!([
                        key,
                        guest_to_wire(sidecar, value, depth + 1, graph, proxy_ids)?
                    ]))
                })
                .collect::<Result<Vec<_>, VmErr>>()?;
            json!({
                "t":"guestClass",
                "id":format!("g:{node_id}"),
                "callbackId":callback_id,
                "name":class.name,
                "prototype":prototype,
                "statics":statics,
            })
        }
        Value::Function(function) => {
            let callback_id = sidecar.register_guest_callback(v.clone())?;
            graph.callbacks.insert(callback_id, v.clone());
            json!({
                "t":"guestCallback",
                "v":callback_id,
                "constructable":!function.is_arrow,
            })
        }
        Value::NativeFunction { .. } => {
            let callback_id = sidecar.register_guest_callback(v.clone())?;
            graph.callbacks.insert(callback_id, v.clone());
            json!({"t":"guestCallback","v":callback_id,"constructable":false})
        }
        Value::HostFunction { id, .. } => {
            if sidecar.state.borrow().local_handles.contains_key(id) {
                return Err(VmErr::Msg(
                    "native object proxy traps cannot be passed as callbacks".into(),
                ));
            }
            json!({"t":"function","v":id})
        }
        _ => {
            return Err(VmErr::Msg(
                "this guest value cannot cross the Node addon bridge yet".into(),
            ));
        }
    })
}

fn guest_graph_node_snapshot(
    sidecar: &NodeAddonSidecar,
    node_id: u64,
    value: &Value,
    graph: &mut WireEncodeContext,
) -> Result<Option<JsonValue>, VmErr> {
    let snapshot = match value {
        Value::Array(array) => {
            let items = array.borrow().clone();
            if items.len() > MAX_ARRAY_LEN {
                return Err(VmErr::Msg(
                    "guest array exceeds limit during callback sync".into(),
                ));
            }
            let values = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    if array.has_index(index) {
                        sidecar.guest_to_wire_with_context(item, 0, graph)
                    } else {
                        Ok(json!({"t":"hole"}))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            let named = array
                .named
                .borrow()
                .iter()
                .filter(|(key, _)| !crate::interpreter::is_internal_key(key))
                .map(|(key, item)| {
                    Ok(json!([
                        key,
                        sidecar.guest_to_wire_with_context(item, 0, graph)?
                    ]))
                })
                .collect::<Result<Vec<_>, VmErr>>()?;
            json!({"t":"array","id":format!("g:{node_id}"),"v":values,"named":named})
        }
        Value::Object { props } => {
            let entries = props.borrow().clone();
            if entries.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg(
                    "guest object exceeds limit during callback sync".into(),
                ));
            }
            let meta = props.meta.borrow();
            let prototype = if meta.uses_default_prototype {
                json!({"t":"defaultPrototype"})
            } else {
                match meta.proto.as_deref() {
                    Some(prototype) => sidecar.guest_to_wire_with_context(prototype, 0, graph)?,
                    None => json!({"t":"null"}),
                }
            };
            let mut wire = Vec::with_capacity(entries.len());
            for (key, item) in &entries {
                let wire_key = if let Some(symbol) = meta.symbol_key(key) {
                    guest_symbol_key_wire(sidecar, &symbol)
                } else if crate::interpreter::symbol_id_from_slot(key).is_some() {
                    return Err(VmErr::Msg(
                        "guest symbol property has no symbol metadata".into(),
                    ));
                } else if crate::interpreter::is_internal_key(key) {
                    continue;
                } else {
                    json!(key)
                };
                let attrs = meta.attrs_of(key);
                let kind = guest_accessor_kind(key, item);
                let getter = (kind == Some("get")).then_some(item);
                let setter = if kind == Some("set") {
                    Some(item)
                } else if getter.is_some() {
                    entries
                        .iter()
                        .find(|(slot, _)| slot == &format!("__setter:{key}__"))
                        .and_then(|(_, candidate)| {
                            (guest_accessor_kind(key, candidate) == Some("set"))
                                .then_some(candidate)
                        })
                } else {
                    None
                };
                if getter.is_some() || setter.is_some() {
                    let getter = getter
                        .map(|getter| sidecar.guest_to_wire_with_context(getter, 0, graph))
                        .transpose()?
                        .unwrap_or(JsonValue::Null);
                    let setter = setter
                        .map(|setter| sidecar.guest_to_wire_with_context(setter, 0, graph))
                        .transpose()?
                        .unwrap_or(JsonValue::Null);
                    wire.push(json!([
                        wire_key,
                        {"t":"undefined"},
                        true,
                        attrs.enumerable,
                        attrs.configurable,
                        getter,
                        setter
                    ]));
                } else {
                    wire.push(json!([
                        wire_key,
                        sidecar.guest_to_wire_with_context(item, 0, graph)?,
                        attrs.writable,
                        attrs.enumerable,
                        attrs.configurable
                    ]));
                }
            }
            json!({
                "t":"object",
                "id":format!("g:{node_id}"),
                "v":wire,
                "prototype":prototype,
                "extensible":!meta.non_extensible,
            })
        }
        Value::Class(class) => {
            let statics = class.statics.borrow();
            if statics.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg(
                    "guest class exceeds limit during callback sync".into(),
                ));
            }
            let statics = statics
                .iter()
                .filter(|(key, _)| {
                    !matches!(key.as_str(), "name" | "length" | "prototype")
                        && !crate::interpreter::is_internal_key(key)
                })
                .map(|(key, item)| {
                    Ok(json!([
                        key,
                        sidecar.guest_to_wire_with_context(item, 0, graph)?
                    ]))
                })
                .collect::<Result<Vec<_>, VmErr>>()?;
            json!({"t":"guestClass","id":format!("g:{node_id}"),"statics":statics})
        }
        _ => return Ok(None),
    };
    Ok(Some(snapshot))
}

fn wire_to_guest(sidecar: &NodeAddonSidecar, v: &JsonValue, depth: usize) -> Result<Value, VmErr> {
    wire_to_guest_with_context(sidecar, v, depth, &mut WireDecodeContext::default())
}

fn wire_graph_id(value: Option<&JsonValue>) -> Result<String, VmErr> {
    let value = value.ok_or_else(|| VmErr::Msg("Node graph value has no id".into()))?;
    if let Some(id) = value.as_str() {
        if id.len() > 64 || !id.contains(':') {
            return Err(VmErr::Msg("invalid Node graph id".into()));
        }
        return Ok(id.to_string());
    }
    value
        .as_u64()
        .map(|id| format!("n:{id}"))
        .ok_or_else(|| VmErr::Msg("invalid Node graph id".into()))
}

fn guest_call_result_to_value(
    sidecar: &NodeAddonSidecar,
    envelope: &JsonValue,
    encoded: &WireEncodeContext,
) -> Result<Value, VmErr> {
    if envelope.get("t").and_then(JsonValue::as_str) != Some("guestCallResult") {
        return Err(VmErr::Msg(
            "Node call response has no guest graph result".into(),
        ));
    }
    let mut graph = WireDecodeContext::from_encode_context(encoded);
    {
        let state = sidecar.state.borrow();
        graph.nodes.extend(
            state
                .guest_graph_nodes
                .iter()
                .map(|(id, value)| (format!("g:{id}"), value.clone())),
        );
        graph.callbacks.extend(state.guest_callbacks.clone());
    }
    let result = envelope
        .get("result")
        .ok_or_else(|| VmErr::Msg("Node call response has no result".into()))?;
    let result = wire_to_guest_with_context(sidecar, result, 0, &mut graph)?;
    let thrown = envelope
        .get("thrown")
        .map(|thrown| wire_to_guest_with_context(sidecar, thrown, 0, &mut graph))
        .transpose()?;
    let mutations = envelope
        .get("mutations")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| VmErr::Msg("Node call response has invalid guest mutations".into()))?;
    for mutation in mutations {
        apply_guest_mutation(sidecar, mutation, &mut graph)?;
    }
    if let Some(reason) = thrown {
        Err(VmErr::Throw(reason))
    } else {
        Ok(result)
    }
}

fn mutation_property(
    sidecar: &NodeAddonSidecar,
    item: &JsonValue,
    graph: &mut WireDecodeContext,
) -> Result<GuestPropertyMutation, VmErr> {
    let pair = item
        .as_array()
        .filter(|pair| pair.len() == 5 || pair.len() == 7)
        .ok_or_else(|| VmErr::Msg("invalid Node guest mutation property".into()))?;
    let (key, symbol) = wire_property_slot(sidecar, &pair[0], 0, graph)?;
    let attrs = PropAttrs {
        writable: pair[2]
            .as_bool()
            .ok_or_else(|| VmErr::Msg("invalid Node property writable flag".into()))?,
        enumerable: pair[3]
            .as_bool()
            .ok_or_else(|| VmErr::Msg("invalid Node property enumerable flag".into()))?,
        configurable: pair[4]
            .as_bool()
            .ok_or_else(|| VmErr::Msg("invalid Node property configurable flag".into()))?,
    };
    let value = wire_to_guest_with_context(sidecar, &pair[1], 0, graph)?;
    let getter = pair
        .get(5)
        .filter(|value| !value.is_null())
        .map(|value| wire_to_guest_with_context(sidecar, value, 0, graph))
        .transpose()?;
    let setter = pair
        .get(6)
        .filter(|value| !value.is_null())
        .map(|value| wire_to_guest_with_context(sidecar, value, 0, graph))
        .transpose()?;
    Ok(GuestPropertyMutation {
        key,
        symbol,
        value,
        attrs,
        getter,
        setter,
    })
}

fn wire_property_slot(
    sidecar: &NodeAddonSidecar,
    key: &JsonValue,
    depth: usize,
    graph: &mut WireDecodeContext,
) -> Result<(String, Option<Rc<SymbolData>>), VmErr> {
    if let Some(key) = key.as_str() {
        return Ok((key.to_string(), None));
    }
    let value = wire_to_guest_with_context(sidecar, key, depth, graph)?;
    match &value {
        Value::Symbol(symbol) => Ok((
            crate::interpreter::symbol_slot_key(symbol),
            Some(symbol.clone()),
        )),
        _ => Err(VmErr::Msg(
            "invalid Node guest mutation property key".into(),
        )),
    }
}

fn is_reserved_guest_property_key(key: &str, symbol: bool) -> bool {
    !symbol && crate::interpreter::is_internal_key(key)
}

struct GuestPropertyMutation {
    key: String,
    symbol: Option<Rc<SymbolData>>,
    value: Value,
    attrs: PropAttrs,
    getter: Option<Value>,
    setter: Option<Value>,
}

fn callable_value(value: &Value) -> bool {
    matches!(
        value,
        Value::Function(_) | Value::NativeFunction { .. } | Value::HostFunction { .. }
    )
}

fn named_accessor(value: Value, name: String) -> Result<Value, VmErr> {
    Ok(match &value {
        Value::Function(function) => {
            let mut function = function.clone();
            function.name = Some(name.into());
            Value::Function(function)
        }
        Value::NativeFunction { callable, .. } => Value::NativeFunction {
            name: name.into(),
            callable: *callable,
        },
        Value::HostFunction { id, .. } => Value::HostFunction {
            name: name.into(),
            id: *id,
        },
        _ => {
            return Err(VmErr::Msg("Node accessor value is not callable".into()));
        }
    })
}

#[derive(Clone)]
enum GuestPrototypeState {
    Default,
    Explicit(Option<Rc<Value>>),
}

fn decode_guest_prototype(
    sidecar: &NodeAddonSidecar,
    wire: &JsonValue,
    depth: usize,
    graph: &mut WireDecodeContext,
) -> Result<GuestPrototypeState, VmErr> {
    if wire.get("t").and_then(JsonValue::as_str) == Some("defaultPrototype") {
        return Ok(GuestPrototypeState::Default);
    }
    let value = wire_to_guest_with_context(sidecar, wire, depth, graph)?;
    match &value {
        Value::Null => Ok(GuestPrototypeState::Explicit(None)),
        Value::Object { .. } => Ok(GuestPrototypeState::Explicit(Some(Rc::new(value.clone())))),
        Value::Class(class) => Ok(GuestPrototypeState::Explicit(Some(class.prototype.clone()))),
        _ => Err(VmErr::Msg(
            "Node object prototype must be an object or null".into(),
        )),
    }
}

fn ensure_no_guest_prototype_cycle(
    target: &Rc<crate::value::ObjectCell>,
    prototype: &Value,
) -> Result<(), VmErr> {
    let mut current = Some(Rc::new(prototype.clone()));
    let mut visited = HashSet::new();
    while let Some(value) = current {
        let Value::Object { props } = value.as_ref() else {
            break;
        };
        if Rc::ptr_eq(target, props) {
            return Err(VmErr::Msg(
                "Node addon prototype mutation would create a cycle".into(),
            ));
        }
        if !visited.insert(Rc::as_ptr(props) as usize) {
            break;
        }
        current = props.proto();
    }
    Ok(())
}

fn apply_guest_mutation(
    sidecar: &NodeAddonSidecar,
    mutation: &JsonValue,
    graph: &mut WireDecodeContext,
) -> Result<(), VmErr> {
    let id = mutation
        .get("id")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| VmErr::Msg("Node mutation has an invalid guest id".into()))?;
    if !id.starts_with("g:") {
        return Err(VmErr::Msg(
            "Node mutation targets a non-guest graph node".into(),
        ));
    }
    let target = graph
        .nodes
        .get(id)
        .cloned()
        .ok_or_else(|| VmErr::Msg("Node mutation references an unknown guest node".into()))?;
    let entries = mutation
        .get("entries")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| VmErr::Msg("Node mutation has invalid properties".into()))?;
    let kind = mutation
        .get("kind")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| VmErr::Msg("Node mutation has no kind".into()))?;
    let prototype = mutation
        .get("prototype")
        .map(|wire| decode_guest_prototype(sidecar, wire, 0, graph))
        .transpose()?;
    match (kind, &target) {
        ("object", Value::Object { props }) => {
            if let Some(GuestPrototypeState::Explicit(Some(prototype))) = &prototype {
                ensure_no_guest_prototype_cycle(props, prototype)?;
            }
            if entries.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg("Node object mutation exceeds VM limit".into()));
            }
            let mut updates = Vec::with_capacity(entries.len());
            let mut attributes = Vec::with_capacity(entries.len());
            let mut keys = HashSet::with_capacity(entries.len());
            for item in entries {
                let property = mutation_property(sidecar, item, graph)?;
                if is_reserved_guest_property_key(&property.key, property.symbol.is_some()) {
                    return Err(VmErr::Msg(
                        "Node addon mutation uses a reserved VM property name".into(),
                    ));
                }
                if !keys.insert(property.key.clone()) {
                    return Err(VmErr::Msg(
                        "Node object mutation has duplicate properties".into(),
                    ));
                }
                if property.getter.is_some() || property.setter.is_some() {
                    if property
                        .getter
                        .as_ref()
                        .is_some_and(|getter| !callable_value(getter))
                        || property
                            .setter
                            .as_ref()
                            .is_some_and(|setter| !callable_value(setter))
                    {
                        return Err(VmErr::Msg(
                            "Node accessor properties require callable getter/setter values".into(),
                        ));
                    }
                    if property.getter.is_none() && property.setter.is_none() {
                        return Err(VmErr::Msg(
                            "empty Node accessor properties cannot be represented by the VM".into(),
                        ));
                    }
                }
                attributes.push((
                    property.key.clone(),
                    property.attrs,
                    property.symbol.clone(),
                ));
                updates.push(property);
            }
            let has_accessor_updates = updates
                .iter()
                .any(|property| property.getter.is_some() || property.setter.is_some());
            let deleted_entries = mutation
                .get("deleted")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("Node object mutation has invalid deletions".into()))?;
            let mut deleted = Vec::with_capacity(deleted_entries.len());
            for key in deleted_entries {
                let (key, symbol) = wire_property_slot(sidecar, key, 0, graph)?;
                if is_reserved_guest_property_key(&key, symbol.is_some()) {
                    return Err(VmErr::Msg(
                        "Node addon mutation deletes a reserved VM property name".into(),
                    ));
                }
                deleted.push(key);
            }
            let mut slots = props.borrow_mut();
            slots.retain(|(key, _)| {
                !deleted
                    .iter()
                    .any(|deleted| deleted == key || key == &format!("__setter:{deleted}__"))
            });
            for property in updates {
                let key = property.key;
                let companion = format!("__setter:{key}__");
                slots.retain(|(slot, _)| slot != &companion);
                let (primary, setter) = match (property.getter, property.setter) {
                    (Some(getter), Some(setter)) => (
                        named_accessor(getter, format!("get {key}"))?,
                        Some(named_accessor(setter, format!("set {key}"))?),
                    ),
                    (Some(getter), None) => (named_accessor(getter, format!("get {key}"))?, None),
                    (None, Some(setter)) => (named_accessor(setter, format!("set {key}"))?, None),
                    (None, None) => (property.value, None),
                };
                if let Some((_, slot)) = slots.iter_mut().find(|(name, _)| name == &key) {
                    *slot = primary;
                } else {
                    if slots.len() >= MAX_OBJECT_PROPS {
                        return Err(VmErr::Msg("Node object mutation exceeds VM limit".into()));
                    }
                    slots.push((key, primary));
                }
                if let Some(setter) = setter {
                    if slots.len() >= MAX_OBJECT_PROPS {
                        return Err(VmErr::Msg("Node object mutation exceeds VM limit".into()));
                    }
                    slots.push((companion, setter));
                }
            }
            drop(slots);
            let mut meta = props.meta.borrow_mut();
            for key in &deleted {
                meta.forget(key);
            }
            for (key, attrs, symbol) in attributes {
                meta.forget(&key);
                meta.set_attrs(&key, attrs);
                if let Some(symbol) = symbol {
                    meta.set_symbol_key(&key, symbol);
                }
            }
            meta.has_accessors |= has_accessor_updates;
            if let Some(extensible) = mutation.get("extensible").and_then(JsonValue::as_bool) {
                meta.non_extensible = !extensible;
            }
            match prototype {
                Some(GuestPrototypeState::Default) => {
                    meta.proto = None;
                    meta.uses_default_prototype = true;
                }
                Some(GuestPrototypeState::Explicit(prototype)) => {
                    meta.proto = prototype;
                    meta.uses_default_prototype = false;
                }
                None => {}
            }
        }
        ("object", Value::Class(class)) => {
            if entries.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg("Node class mutation exceeds VM limit".into()));
            }
            let mut updates = Vec::with_capacity(entries.len());
            let mut keys = HashSet::with_capacity(entries.len());
            for item in entries {
                let property = mutation_property(sidecar, item, graph)?;
                if property.symbol.is_some()
                    || property.getter.is_some()
                    || property.setter.is_some()
                    || property.attrs != PropAttrs::default()
                {
                    return Err(VmErr::Msg(
                        "Node class accessor, symbol, or descriptor mutation is not supported"
                            .into(),
                    ));
                }
                if property.key == "name"
                    || property.key == "prototype"
                    || property.key == "length"
                    || property.key == "arguments"
                    || property.key == "caller"
                    || is_reserved_guest_property_key(&property.key, false)
                {
                    return Err(VmErr::Msg(
                        "Node class mutation targets a reserved constructor property".into(),
                    ));
                }
                if !keys.insert(property.key.clone()) {
                    return Err(VmErr::Msg(
                        "Node class mutation has duplicate properties".into(),
                    ));
                }
                updates.push((property.key, property.value));
            }
            let deleted_entries = mutation
                .get("deleted")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("Node class mutation has invalid deletions".into()))?;
            let mut deleted = Vec::with_capacity(deleted_entries.len());
            for key in deleted_entries {
                let (key, symbol) = wire_property_slot(sidecar, key, 0, graph)?;
                if symbol.is_some()
                    || key == "name"
                    || key == "prototype"
                    || key == "length"
                    || key == "arguments"
                    || key == "caller"
                    || is_reserved_guest_property_key(&key, false)
                {
                    return Err(VmErr::Msg(
                        "Node class deletion targets a reserved constructor property".into(),
                    ));
                }
                deleted.push(key);
            }
            let mut statics = class.statics.borrow_mut();
            statics.retain(|(key, _)| !deleted.iter().any(|deleted| deleted == key));
            for (key, value) in updates {
                if let Some((_, existing)) = statics.iter_mut().find(|(name, _)| name == &key) {
                    *existing = value;
                } else {
                    if statics.len() >= MAX_OBJECT_PROPS {
                        return Err(VmErr::Msg("Node class mutation exceeds VM limit".into()));
                    }
                    statics.push((key, value));
                }
            }
        }
        ("array", Value::Array(array)) => {
            let requested_length = mutation
                .get("length")
                .map(|length| {
                    length
                        .as_u64()
                        .and_then(|length| usize::try_from(length).ok())
                        .filter(|length| *length <= MAX_ARRAY_LEN)
                        .ok_or_else(|| {
                            VmErr::Msg("Node array mutation has an invalid length".into())
                        })
                })
                .transpose()?;
            let mut items = array.borrow().clone();
            let mut presence = array.presence_snapshot();
            if let Some(length) = requested_length {
                items.truncate(length);
                items.resize(length, Value::Undefined);
                presence.resize(length, false);
                presence.truncate(length);
            }
            let mut named = array.named.borrow().clone();
            let mut keys = HashSet::with_capacity(entries.len());
            for item in entries {
                let property = mutation_property(sidecar, item, graph)?;
                if property.symbol.is_some() {
                    return Err(VmErr::Msg(
                        "symbol-keyed array mutation is not supported by the VM".into(),
                    ));
                }
                if !keys.insert(property.key.clone()) {
                    return Err(VmErr::Msg(
                        "Node array mutation has duplicate properties".into(),
                    ));
                }
                if property.getter.is_some() || property.setter.is_some() {
                    return Err(VmErr::Msg(
                        "array accessor mutations cannot be written back to the VM".into(),
                    ));
                }
                if property.attrs != PropAttrs::default() {
                    return Err(VmErr::Msg(
                        "non-default array property attributes cannot be written back to the VM"
                            .into(),
                    ));
                }
                if let Ok(index) = property.key.parse::<usize>()
                    && property.key == index.to_string()
                    && index < items.len()
                {
                    items[index] = property.value;
                    presence[index] = true;
                    continue;
                }
                if crate::interpreter::is_internal_key(&property.key) {
                    return Err(VmErr::Msg(
                        "Node addon mutation uses a reserved VM property name".into(),
                    ));
                }
                if let Some((_, slot)) = named.iter_mut().find(|(name, _)| name == &property.key) {
                    *slot = property.value;
                } else {
                    named.push((property.key, property.value));
                }
            }
            let deleted = mutation
                .get("deleted")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("Node array mutation has invalid deletions".into()))?;
            for key in deleted {
                let (key, symbol) = wire_property_slot(sidecar, key, 0, graph)?;
                if symbol.is_some() {
                    return Err(VmErr::Msg(
                        "symbol-keyed array mutation is not supported by the VM".into(),
                    ));
                }
                if let Ok(index) = key.parse::<usize>()
                    && key == index.to_string()
                    && index < items.len()
                {
                    presence[index] = false;
                    items[index] = Value::Undefined;
                    continue;
                }
                named.retain(|(name, _)| name != &key);
            }
            *array.borrow_mut() = items;
            array.replace_presence(presence);
            *array.named.borrow_mut() = named;
        }
        _ => {
            return Err(VmErr::Msg(
                "Node mutation kind does not match guest value".into(),
            ));
        }
    }
    Ok(())
}

fn wire_to_guest_with_context(
    sidecar: &NodeAddonSidecar,
    v: &JsonValue,
    depth: usize,
    graph: &mut WireDecodeContext,
) -> Result<Value, VmErr> {
    if depth > MAX_WIRE_DEPTH {
        return Err(VmErr::Msg(
            "native result exceeds bridge depth limit".into(),
        ));
    }
    let t = v
        .get("t")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| VmErr::Msg("invalid Node value tag".into()))?;
    match t {
        "ref" | "guestRef" => {
            let id = wire_graph_id(v.get("v"))?;
            graph
                .nodes
                .get(&id)
                .cloned()
                .ok_or_else(|| VmErr::Msg("Node value references an unknown graph node".into()))
        }
        "guestCallbackRef" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| VmErr::Msg("invalid guest callback reference id".into()))?;
            graph
                .callbacks
                .get(&id)
                .cloned()
                .ok_or_else(|| VmErr::Msg("Node value references an unknown guest callback".into()))
        }
        "undefined" => Ok(Value::Undefined),
        "hole" => Err(VmErr::Msg("array hole appeared outside an array".into())),
        "null" => Ok(Value::Null),
        "boolean" => v
            .get("v")
            .and_then(JsonValue::as_bool)
            .map(Value::Bool)
            .ok_or_else(|| VmErr::Msg("invalid Node boolean".into())),
        "number" => {
            let s = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node number".into()))?;
            let n = match s {
                "NaN" => f64::NAN,
                "Infinity" => f64::INFINITY,
                "-Infinity" => f64::NEG_INFINITY,
                "-0" => -0.0,
                _ => s
                    .parse()
                    .map_err(|e| VmErr::Msg(format!("invalid Node number: {e}")))?,
            };
            Ok(Value::Number(n))
        }
        "string" => v
            .get("v")
            .and_then(JsonValue::as_str)
            .map(|s| Value::String(s.to_string()))
            .ok_or_else(|| VmErr::Msg("invalid Node string".into())),
        "bigint" => {
            let s = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node BigInt".into()))?;
            let n = crate::bigint::BigInt::parse(s).map_err(VmErr::Msg)?;
            Ok(Value::BigInt(Rc::new(n)))
        }
        "date" => {
            let value = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node date".into()))?;
            let milliseconds = parse_wire_number(value)?;
            Ok(Value::Date(Rc::new(std::cell::Cell::new(milliseconds))))
        }
        "regexp" => {
            let source = v
                .get("source")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node regular expression source".into()))?;
            let flags = v
                .get("flags")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node regular expression flags".into()))?;
            if source.len() > MAX_STRING_LEN || flags.len() > MAX_STRING_LEN {
                return Err(VmErr::Msg(
                    "Node regular expression exceeds the VM string limit".into(),
                ));
            }
            let last_index = v
                .get("lastIndex")
                .and_then(JsonValue::as_str)
                .unwrap_or("0")
                .parse::<f64>()
                .unwrap_or(0.0);
            let regex = crate::builtins::compile_regex(source, flags)?;
            let Value::RegExp(data) = &regex else {
                unreachable!("regex compiler returns a RegExp")
            };
            if last_index.is_finite() && last_index >= 0.0 {
                data.last_index.set(last_index as usize);
            }
            Ok(regex)
        }
        "symbol" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node symbol id".into()))?;
            let description = v
                .get("description")
                .and_then(JsonValue::as_str)
                .map(str::to_string);
            if description
                .as_ref()
                .is_some_and(|description| description.len() > MAX_STRING_LEN)
            {
                return Err(VmErr::Msg(
                    "Node symbol description exceeds VM limit".into(),
                ));
            }
            if let Some(guest_id) = id.strip_prefix("g:") {
                let guest_id = guest_id
                    .parse::<u64>()
                    .map_err(|_| VmErr::Msg("invalid guest symbol id".into()))?;
                return Ok(Value::Symbol(Rc::new(SymbolData {
                    id: guest_id,
                    description,
                })));
            }
            if let Some(symbol) = sidecar.state.borrow().host_symbols.get(id).cloned() {
                return Ok(symbol);
            }
            if !id.starts_with("n:") {
                return Err(VmErr::Msg("invalid native symbol id".into()));
            }
            if sidecar.state.borrow().host_symbols.len() >= MAX_NATIVE_HANDLES {
                return Err(VmErr::Msg("native symbol handle limit exceeded".into()));
            }
            let symbol = crate::builtins::new_symbol(description);
            let Value::Symbol(data) = &symbol else {
                unreachable!("new_symbol returns a Symbol")
            };
            let mut state = sidecar.state.borrow_mut();
            state.symbol_remote_ids.insert(data.id, id.to_string());
            state.host_symbols.insert(id.to_string(), symbol.clone());
            Ok(symbol)
        }
        "hostObject" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| VmErr::Msg("invalid Node object id".into()))?;
            sidecar.host_object(id)
        }
        "hostPromise" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_u64)
                .ok_or_else(|| VmErr::Msg("invalid Node promise id".into()))?;
            sidecar.host_promise(id)
        }
        "error" => {
            let name = v.get("name").and_then(JsonValue::as_str).unwrap_or("Error");
            let message = v
                .get("message")
                .and_then(JsonValue::as_str)
                .unwrap_or("native promise result could not be marshalled");
            let error = match v.get("code").and_then(JsonValue::as_str) {
                Some(code) => crate::value::ErrorData::with_code(name, message, code),
                None => crate::value::ErrorData::new(name, message),
            };
            Ok(Value::Error(error))
        }
        "arrayBuffer" | "bytes" => {
            let bytes = wire_bytes(
                v.get("v")
                    .ok_or_else(|| VmErr::Msg("invalid Node bytes".into()))?,
            )?;
            Ok(Value::ArrayBuffer(Rc::new(RefCell::new(bytes))))
        }
        "typedArray" => {
            let kind_name = v
                .get("kind")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| VmErr::Msg("invalid Node typed array kind".into()))?;
            let kind = typed_kind(kind_name)
                .ok_or_else(|| VmErr::Msg("unsupported Node typed array kind".into()))?;
            let length = v
                .get("length")
                .and_then(JsonValue::as_u64)
                .and_then(|length| usize::try_from(length).ok())
                .ok_or_else(|| VmErr::Msg("invalid Node typed array length".into()))?;
            let bytes = wire_bytes(
                v.get("bytes")
                    .ok_or_else(|| VmErr::Msg("invalid Node typed array bytes".into()))?,
            )?;
            if length.checked_mul(kind.size()) != Some(bytes.len()) {
                return Err(VmErr::Msg(
                    "Node typed array has an invalid byte length".into(),
                ));
            }
            Ok(Value::TypedArray(Rc::new(TypedArrayData {
                kind,
                buffer: Rc::new(RefCell::new(bytes)),
                byte_offset: 0,
                length,
            })))
        }
        "dataView" => {
            let length = v
                .get("length")
                .and_then(JsonValue::as_u64)
                .and_then(|length| usize::try_from(length).ok())
                .ok_or_else(|| VmErr::Msg("invalid Node DataView length".into()))?;
            let bytes = wire_bytes(
                v.get("bytes")
                    .ok_or_else(|| VmErr::Msg("invalid Node DataView bytes".into()))?,
            )?;
            if length != bytes.len() {
                return Err(VmErr::Msg(
                    "Node DataView has an invalid byte length".into(),
                ));
            }
            Ok(Value::DataView(Rc::new(TypedArrayData {
                kind: TypedKind::Uint8,
                buffer: Rc::new(RefCell::new(bytes)),
                byte_offset: 0,
                length,
            })))
        }
        "function" => {
            let id = v
                .get("v")
                .and_then(JsonValue::as_u64)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| VmErr::Msg("invalid Node function id".into()))?;
            Ok(Value::HostFunction {
                name: v
                    .get("n")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("nodeAddon")
                    .into(),
                id,
            })
        }
        "array" => {
            let a = v
                .get("v")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("invalid Node array".into()))?;
            if a.len() > MAX_ARRAY_LEN {
                return Err(VmErr::Msg("Node array exceeds VM limit".into()));
            }
            let array = Value::checked_array(Vec::new())?;
            if let Some(id) = v.get("id") {
                graph.register(wire_graph_id(Some(id))?, array.clone())?;
            }
            let mut presence = Vec::with_capacity(a.len());
            let items = a
                .iter()
                .map(|x| {
                    let is_hole = x.get("t").and_then(JsonValue::as_str) == Some("hole");
                    presence.push(!is_hole);
                    if is_hole {
                        Ok(Value::Undefined)
                    } else {
                        wire_to_guest_with_context(sidecar, x, depth + 1, graph)
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let Value::Array(cell) = &array {
                *cell.borrow_mut() = items;
                cell.replace_presence(presence);
            }
            if let Some(named) = v.get("named").and_then(JsonValue::as_array) {
                for property in named {
                    let pair = property
                        .as_array()
                        .filter(|pair| pair.len() == 2)
                        .ok_or_else(|| VmErr::Msg("invalid Node array property".into()))?;
                    let key = pair[0]
                        .as_str()
                        .ok_or_else(|| VmErr::Msg("invalid Node array property key".into()))?;
                    let value = wire_to_guest_with_context(sidecar, &pair[1], depth + 1, graph)?;
                    if let Value::Array(cell) = &array {
                        cell.set_named(key.to_string(), value);
                    }
                }
            }
            Ok(array)
        }
        "object" => {
            let a = v
                .get("v")
                .and_then(JsonValue::as_array)
                .ok_or_else(|| VmErr::Msg("invalid Node object".into()))?;
            if a.len() > MAX_OBJECT_PROPS {
                return Err(VmErr::Msg("Node object exceeds VM limit".into()));
            }
            let object = Value::checked_object(Vec::new())?;
            if let Some(id) = v.get("id") {
                graph.register(wire_graph_id(Some(id))?, object.clone())?;
            }
            let mut slots = Vec::with_capacity(a.len());
            let mut attrs = Vec::with_capacity(a.len());
            let mut has_accessors = false;
            for item in a {
                let pair = item
                    .as_array()
                    .filter(|p| p.len() == 2 || p.len() == 5 || p.len() == 7)
                    .ok_or_else(|| VmErr::Msg("invalid Node property".into()))?;
                let (key, symbol) = wire_property_slot(sidecar, &pair[0], depth + 1, graph)?;
                if pair.len() == 7 {
                    let getter = (!pair[5].is_null())
                        .then(|| wire_to_guest_with_context(sidecar, &pair[5], depth + 1, graph))
                        .transpose()?;
                    let setter = (!pair[6].is_null())
                        .then(|| wire_to_guest_with_context(sidecar, &pair[6], depth + 1, graph))
                        .transpose()?;
                    if getter.is_none() && setter.is_none() {
                        return Err(VmErr::Msg(
                            "empty Node accessor properties cannot be represented by the VM".into(),
                        ));
                    }
                    if let Some(getter) = getter {
                        if !callable_value(&getter) {
                            return Err(VmErr::Msg("Node accessor getter is not callable".into()));
                        }
                        slots.push((key.clone(), named_accessor(getter, format!("get {key}"))?));
                    }
                    if let Some(setter) = setter {
                        if !callable_value(&setter) {
                            return Err(VmErr::Msg("Node accessor setter is not callable".into()));
                        }
                        let slot = if slots.last().is_some_and(|(slot, _)| slot == &key) {
                            format!("__setter:{key}__")
                        } else {
                            key.to_string()
                        };
                        slots.push((slot, named_accessor(setter, format!("set {key}"))?));
                    }
                    has_accessors = true;
                } else {
                    slots.push((
                        key.clone(),
                        wire_to_guest_with_context(sidecar, &pair[1], depth + 1, graph)?,
                    ));
                }
                attrs.push((
                    key.clone(),
                    PropAttrs {
                        writable: pair.get(2).and_then(JsonValue::as_bool).unwrap_or(true),
                        enumerable: pair.get(3).and_then(JsonValue::as_bool).unwrap_or(true),
                        configurable: pair.get(4).and_then(JsonValue::as_bool).unwrap_or(true),
                    },
                    symbol,
                ));
            }
            let prototype = v
                .get("prototype")
                .map(|wire| decode_guest_prototype(sidecar, wire, depth + 1, graph))
                .transpose()?
                .unwrap_or(GuestPrototypeState::Default);
            if let Value::Object { props } = &object {
                *props.borrow_mut() = slots;
                let mut meta = props.meta.borrow_mut();
                for (key, value, symbol) in attrs {
                    meta.set_attrs(&key, value);
                    if let Some(symbol) = symbol {
                        meta.set_symbol_key(&key, symbol);
                    }
                }
                meta.has_accessors = has_accessors;
                match prototype {
                    GuestPrototypeState::Default => {
                        meta.proto = None;
                        meta.uses_default_prototype = true;
                    }
                    GuestPrototypeState::Explicit(prototype) => {
                        meta.proto = prototype;
                        meta.uses_default_prototype = false;
                    }
                }
                if v.get("extensible").and_then(JsonValue::as_bool) == Some(false) {
                    meta.non_extensible = true;
                }
            }
            Ok(object)
        }
        _ => Err(VmErr::Msg(format!("unknown Node value tag '{t}'"))),
    }
}

fn parse_wire_number(value: &str) -> Result<f64, VmErr> {
    match value {
        "NaN" => Ok(f64::NAN),
        "Infinity" => Ok(f64::INFINITY),
        "-Infinity" => Ok(f64::NEG_INFINITY),
        "-0" => Ok(-0.0),
        _ => value
            .parse()
            .map_err(|error| VmErr::Msg(format!("invalid Node number: {error}"))),
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::interpreter::{Interpreter, NodeAddonOptions};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command as ProcessCommand;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn node_addon_configuration_rejects_entry_outside_roots_before_startup() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "napi-vm-node-addon-config-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let root = base.join("root");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let entry = outside.join("main.cjs");
        fs::write(&entry, "").unwrap();

        let mut interpreter = Interpreter::with_builtins();
        let error = interpreter
            .enable_node_addons(
                NodeAddonOptions::new("node-executable-must-not-start", [root.clone()])
                    .entry(entry),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("CommonJS entry escapes configured roots")
        );
        assert!(
            interpreter
                .require_commonjs("./main.cjs", None)
                .unwrap_err()
                .to_string()
                .contains("configure a host CommonJS module loader")
        );

        let addon = root.join("fixture.node");
        let valid_entry = root.join("main.cjs");
        fs::write(&addon, "untrusted addon bytes").unwrap();
        fs::write(&valid_entry, "").unwrap();
        let error = interpreter
            .enable_node_addons(
                NodeAddonOptions::new("node-executable-must-not-start", [root.clone()])
                    .allow_native_addon_with_sha256(addon, [0; 32])
                    .entry(valid_entry),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("integrity check failed while configuring")
        );
        assert!(
            interpreter
                .require_commonjs("./main.cjs", None)
                .unwrap_err()
                .to_string()
                .contains("configure a host CommonJS module loader")
        );

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn loads_and_invokes_a_real_node_api_addon() {
        static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "napi-vm-node-addon-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();

        let node = ProcessCommand::new("node").arg("--version").output();
        let cc = ProcessCommand::new("cc").arg("--version").output();
        let mut include_dirs = Vec::new();
        if let Some(include) = std::env::var_os("NODE_INCLUDE_DIR") {
            include_dirs.push(PathBuf::from(include));
        }
        include_dirs.push(PathBuf::from("/usr/include/node"));
        include_dirs.push(PathBuf::from("/usr/local/include/node"));
        let include = include_dirs
            .into_iter()
            .find(|path| path.join("node_api.h").is_file());
        let (Ok(node), Ok(cc), Some(include)) = (node, cc, include) else {
            eprintln!(
                "skipping real Node-API addon test: Node, cc, or Node headers are unavailable"
            );
            let _ = fs::remove_dir_all(&root);
            return;
        };
        assert!(node.status.success(), "node --version failed");
        assert!(cc.status.success(), "cc --version failed");

        fs::write(root.join("main.cjs"), "").unwrap();
        fs::write(
            root.join("package.json"),
            r##"{"name":"fixture-runtime","imports":{"#native":"fixture"}}"##,
        )
        .unwrap();
        let source = root.join("fixture.c");
        let package_root = root.join("node_modules/fixture");
        let package_build = package_root.join("build/Release");
        fs::create_dir_all(&package_build).unwrap();
        fs::write(
            package_root.join("package.json"),
            r#"{"exports":{".":{"node-addons":"./build/Release/fixture.node","require":"./build/Release/fixture.node","default":"./build/Release/fixture.node"}}}"#,
        )
        .unwrap();
        let wrapper_root = root.join("node_modules/fixture-wrapper");
        fs::create_dir_all(&wrapper_root).unwrap();
        fs::write(
            wrapper_root.join("package.json"),
            r#"{"main":"./index.cjs"}"#,
        )
        .unwrap();
        fs::write(
            wrapper_root.join("index.cjs"),
            "module.exports = require('fixture');",
        )
        .unwrap();
        let addon = package_build.join("fixture.node");
        let direct_addon = root.join("fixture.node");
        std::os::unix::fs::symlink(&addon, &direct_addon).unwrap();
        fs::write(
            &source,
            r#"
#include <node_api.h>
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static napi_value add(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2];
  double left = 0, right = 0;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2) return NULL;
  if (napi_get_value_double(env, argv[0], &left) != napi_ok) return NULL;
  if (napi_get_value_double(env, argv[1], &right) != napi_ok) return NULL;
  napi_value result;
  if (napi_create_double(env, left + right, &result) != napi_ok) return NULL;
  return result;
}

static napi_value echo(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1) return NULL;
  return argv[0];
}

static napi_value big(napi_env env, napi_callback_info info) {
  napi_value result;
  if (napi_create_bigint_int64(env, 9007199254740993LL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value date_value(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  double milliseconds;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_date_value(env, argv[0], &milliseconds) != napi_ok ||
      napi_create_double(env, milliseconds, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_date(napi_env env, napi_callback_info info) {
  napi_value result;
  if (napi_create_date(env, 123456.5, &result) != napi_ok) return NULL;
  return result;
}

static napi_value regex_source(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "source", &result) != napi_ok) return NULL;
  return result;
}

static napi_value regex_flags(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "flags", &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_regex(napi_env env, napi_callback_info info) {
  napi_value global, constructor, args[2], result;
  if (napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "RegExp", &constructor) != napi_ok ||
      napi_create_string_utf8(env, "a+", NAPI_AUTO_LENGTH, &args[0]) != napi_ok ||
      napi_create_string_utf8(env, "gi", NAPI_AUTO_LENGTH, &args[1]) != napi_ok ||
      napi_new_instance(env, constructor, 2, args, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_symbol(napi_env env, napi_callback_info info) {
  napi_value description, result;
  if (napi_create_string_utf8(env, "native", NAPI_AUTO_LENGTH, &description) != napi_ok ||
      napi_create_symbol(env, description, &result) != napi_ok) return NULL;
  return result;
}

static napi_value identity(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1) return NULL;
  return argv[0];
}

static napi_value is_symbol(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  napi_valuetype type;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_typeof(env, argv[0], &type) != napi_ok ||
      napi_create_int32(env, type == napi_symbol ? 1 : 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value is_node_iterator(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], global, symbol_constructor, iterator, result;
  bool equal = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Symbol", &symbol_constructor) != napi_ok ||
      napi_get_named_property(env, symbol_constructor, "iterator", &iterator) != napi_ok ||
      napi_strict_equals(env, argv[0], iterator, &equal) != napi_ok ||
      napi_create_int32(env, equal ? 1 : 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value same_object(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  bool equal = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_strict_equals(env, argv[0], argv[1], &equal) != napi_ok ||
      napi_create_int32(env, equal ? 1 : 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value set_prototype(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], global, object, setter, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Object", &object) != napi_ok ||
      napi_get_named_property(env, object, "setPrototypeOf", &setter) != napi_ok ||
      napi_call_function(env, object, setter, argc, argv, &result) != napi_ok) return NULL;
  return result;
}

static napi_value set_default_prototype(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], global, object, prototype, setter, args[2], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Object", &object) != napi_ok ||
      napi_get_named_property(env, object, "prototype", &prototype) != napi_ok ||
      napi_get_named_property(env, object, "setPrototypeOf", &setter) != napi_ok) return NULL;
  args[0] = argv[0];
  args[1] = prototype;
  if (napi_call_function(env, object, setter, 2, args, &result) != napi_ok) return NULL;
  return result;
}

static napi_value prototype_matches(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], prototype, result;
  bool equal = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_prototype(env, argv[0], &prototype) != napi_ok ||
      napi_strict_equals(env, prototype, argv[1], &equal) != napi_ok ||
      napi_get_boolean(env, equal, &result) != napi_ok) return NULL;
  return result;
}

static napi_value prototype_is_null(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], prototype, result;
  napi_valuetype type;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_prototype(env, argv[0], &prototype) != napi_ok ||
      napi_typeof(env, prototype, &type) != napi_ok ||
      napi_get_boolean(env, type == napi_null, &result) != napi_ok) return NULL;
  return result;
}

static napi_value prototype_is_default(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], global, object, expected, prototype, result;
  bool equal = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Object", &object) != napi_ok ||
      napi_get_named_property(env, object, "prototype", &expected) != napi_ok ||
      napi_get_prototype(env, argv[0], &prototype) != napi_ok ||
      napi_strict_equals(env, prototype, expected, &equal) != napi_ok ||
      napi_get_boolean(env, equal, &result) != napi_ok) return NULL;
  return result;
}

static napi_value read_property(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "value", &result) != napi_ok) return NULL;
  return result;
}

static napi_value call_inherited(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], method, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "read", &method) != napi_ok ||
      napi_call_function(env, argv[0], method, 0, NULL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value call_to_string(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], method, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_named_property(env, argv[0], "toString", &method) != napi_ok ||
      napi_call_function(env, argv[0], method, 0, NULL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value construct_and_read(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], instance, method, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_new_instance(env, argv[0], 1, &argv[1], &instance) != napi_ok ||
      napi_get_named_property(env, instance, "read", &method) != napi_ok ||
      napi_call_function(env, instance, method, 0, NULL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value write_property(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_set_named_property(env, argv[0], "value", argv[1]) != napi_ok) return NULL;
  return argv[1];
}

static napi_value write_then_read_property(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_set_named_property(env, argv[0], "value", argv[1]) != napi_ok ||
      napi_get_named_property(env, argv[0], "value", &result) != napi_ok) return NULL;
  return result;
}

static napi_value read_symbol_property(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_property(env, argv[0], argv[1], &result) != napi_ok) return NULL;
  return result;
}

static napi_value write_symbol_property(napi_env env, napi_callback_info info) {
  size_t argc = 3;
  napi_value argv[3];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 3 ||
      napi_set_property(env, argv[0], argv[1], argv[2]) != napi_ok) return NULL;
  return argv[2];
}

static napi_value delete_symbol_property(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  bool deleted = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_delete_property(env, argv[0], argv[1], &deleted) != napi_ok ||
      napi_get_boolean(env, deleted, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_symbol_object(napi_env env, napi_callback_info info) {
  napi_value object, description, key, value;
  if (napi_create_object(env, &object) != napi_ok ||
      napi_create_string_utf8(env, "native-key", NAPI_AUTO_LENGTH, &description) != napi_ok ||
      napi_create_symbol(env, description, &key) != napi_ok ||
      napi_create_int32(env, 89, &value) != napi_ok ||
      napi_set_property(env, object, key, value) != napi_ok ||
      napi_set_named_property(env, object, "key", key) != napi_ok) return NULL;
  return object;
}

static napi_value mutate_object(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], value, child, key;
  bool deleted = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_create_int32(env, 73, &value) != napi_ok ||
      napi_set_named_property(env, argv[0], "changed", value) != napi_ok ||
      napi_get_named_property(env, argv[0], "child", &child) != napi_ok ||
      napi_create_int32(env, 91, &value) != napi_ok ||
      napi_set_named_property(env, child, "value", value) != napi_ok ||
      napi_create_string_utf8(env, "removeMe", NAPI_AUTO_LENGTH, &key) != napi_ok ||
      napi_delete_property(env, argv[0], key, &deleted) != napi_ok || !deleted) return NULL;
  return argv[0];
}

static napi_value mutate_array(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], value;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_create_int32(env, 42, &value) != napi_ok ||
      napi_set_element(env, argv[0], 1, value) != napi_ok ||
      napi_create_int32(env, 84, &value) != napi_ok ||
      napi_set_element(env, argv[0], 2, value) != napi_ok ||
      napi_create_string_utf8(env, "native", NAPI_AUTO_LENGTH, &value) != napi_ok ||
      napi_set_named_property(env, argv[0], "tag", value) != napi_ok) return NULL;
  return argv[0];
}

static napi_value make_sparse_array(napi_env env, napi_callback_info info) {
  napi_value array, value;
  if (napi_create_array_with_length(env, 4, &array) != napi_ok ||
      napi_create_int32(env, 17, &value) != napi_ok ||
      napi_set_element(env, array, 1, value) != napi_ok ||
      napi_get_undefined(env, &value) != napi_ok ||
      napi_set_element(env, array, 3, value) != napi_ok) return NULL;
  return array;
}

static napi_value array_has_element(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  uint32_t index;
  bool has = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_value_uint32(env, argv[1], &index) != napi_ok ||
      napi_has_element(env, argv[0], index, &has) != napi_ok ||
      napi_get_boolean(env, has, &result) != napi_ok) return NULL;
  return result;
}

static napi_value delete_array_element(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  uint32_t index;
  bool deleted = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_value_uint32(env, argv[1], &index) != napi_ok ||
      napi_delete_element(env, argv[0], index, &deleted) != napi_ok ||
      napi_get_boolean(env, deleted, &result) != napi_ok) return NULL;
  return result;
}

static napi_value mutate_after_callback(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], receiver, value;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_global(env, &receiver) != napi_ok ||
      napi_call_function(env, receiver, argv[1], 0, NULL, NULL) != napi_ok ||
      napi_create_int32(env, 27, &value) != napi_ok ||
      napi_set_named_property(env, argv[0], "native", value) != napi_ok) return NULL;
  return argv[0];
}

static napi_value assign_and_return(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result, value;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, 15, &value) != napi_ok ||
      napi_set_named_property(env, result, "value", value) != napi_ok ||
      napi_set_named_property(env, argv[0], "created", result) != napi_ok) return NULL;
  return result;
}

static napi_value make_shared_array(napi_env env, napi_callback_info info) {
  napi_value outer, inner, value;
  if (napi_create_array_with_length(env, 2, &outer) != napi_ok ||
      napi_create_array_with_length(env, 1, &inner) != napi_ok ||
      napi_create_int32(env, 7, &value) != napi_ok ||
      napi_set_element(env, inner, 0, value) != napi_ok ||
      napi_set_element(env, outer, 0, inner) != napi_ok ||
      napi_set_element(env, outer, 1, inner) != napi_ok) return NULL;
  return outer;
}

static napi_value make_cycle(napi_env env, napi_callback_info info) {
  napi_value object, child, value;
  if (napi_create_object(env, &object) != napi_ok ||
      napi_create_object(env, &child) != napi_ok ||
      napi_create_int32(env, 1, &value) != napi_ok ||
      napi_set_named_property(env, child, "value", value) != napi_ok ||
      napi_set_named_property(env, object, "self", object) != napi_ok ||
      napi_set_named_property(env, object, "child", child) != napi_ok) return NULL;
  return object;
}

static napi_value make_cyclic_array(napi_env env, napi_callback_info info) {
  napi_value array;
  if (napi_create_array_with_length(env, 1, &array) != napi_ok ||
      napi_set_element(env, array, 0, array) != napi_ok) return NULL;
  return array;
}

static napi_value make_buffer(napi_env env, napi_callback_info info) {
  const char bytes[] = {'a', 'b', 'c'};
  napi_value result;
  if (napi_create_buffer_copy(env, sizeof(bytes), bytes, NULL, &result) != napi_ok) return NULL;
  return result;
}

static napi_value is_buffer(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  bool is_buffer = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_is_buffer(env, argv[0], &is_buffer) != napi_ok ||
      napi_create_int32(env, is_buffer ? 1 : 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value typed_array_length(napi_env env, napi_callback_info info) {
  size_t argc = 1, length = 0;
  napi_value argv[1], result;
  napi_typedarray_type kind;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_typedarray_info(env, argv[0], &kind, &length, NULL, NULL, NULL) != napi_ok ||
      napi_create_int32(env, kind == napi_uint16_array ? (int32_t)length : -1, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_typed_array(napi_env env, napi_callback_info info) {
  napi_value buffer, result;
  void *data = NULL;
  if (napi_create_arraybuffer(env, 4, &data, &buffer) != napi_ok) return NULL;
  uint16_t *items = (uint16_t *)data;
  items[0] = 300;
  items[1] = 400;
  if (napi_create_typedarray(env, napi_uint16_array, 2, buffer, 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_array_buffer(napi_env env, napi_callback_info info) {
  napi_value result;
  void *data = NULL;
  if (napi_create_arraybuffer(env, 3, &data, &result) != napi_ok) return NULL;
  memcpy(data, "xyz", 3);
  return result;
}

static napi_value array_buffer_length(napi_env env, napi_callback_info info) {
  size_t argc = 1, length = 0;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_arraybuffer_info(env, argv[0], NULL, &length) != napi_ok ||
      napi_create_int32(env, (int32_t)length, &result) != napi_ok) return NULL;
  return result;
}

static napi_value make_data_view(napi_env env, napi_callback_info info) {
  napi_value buffer, result;
  void *data = NULL;
  if (napi_create_arraybuffer(env, 2, &data, &buffer) != napi_ok) return NULL;
  ((uint8_t *)data)[0] = 17;
  ((uint8_t *)data)[1] = 29;
  if (napi_create_dataview(env, 2, buffer, 0, &result) != napi_ok) return NULL;
  return result;
}

static napi_value data_view_byte(napi_env env, napi_callback_info info) {
  size_t argc = 1, length = 0, offset = 0;
  napi_value argv[1], result;
  void *data = NULL;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_dataview_info(env, argv[0], &length, &data, NULL, &offset) != napi_ok || length < 2 ||
      napi_create_int32(env, ((uint8_t *)data)[1], &result) != napi_ok) return NULL;
  return result;
}

static napi_value fail(napi_env env, napi_callback_info info) {
  napi_throw_type_error(env, "E_FIXTURE", "fixture failure");
  return NULL;
}

static napi_value mutate_then_throw(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], value;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_create_int32(env, 88, &value) != napi_ok ||
      napi_set_named_property(env, argv[0], "afterThrow", value) != napi_ok ||
      napi_throw_type_error(env, "E_AFTER_MUTATION", "mutation happened") != napi_ok) return NULL;
  return NULL;
}

static napi_value promise_result(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, result;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok) return NULL;
  if (napi_create_string_utf8(env, "native-promise-value", NAPI_AUTO_LENGTH, &result) != napi_ok) return NULL;
  if (napi_resolve_deferred(env, deferred, result) != napi_ok) return NULL;
  return promise;
}

static napi_value promise_reject(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, reason;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok) return NULL;
  if (napi_create_string_utf8(env, "native-rejection", NAPI_AUTO_LENGTH, &reason) != napi_ok) return NULL;
  if (napi_reject_deferred(env, deferred, reason) != napi_ok) return NULL;
  return promise;
}

static napi_value promise_reject_error(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, message, reason, code;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok ||
      napi_create_string_utf8(env, "native error rejection", NAPI_AUTO_LENGTH, &message) != napi_ok ||
      napi_create_error(env, NULL, message, &reason) != napi_ok ||
      napi_create_string_utf8(env, "E_NATIVE_REJECTION", NAPI_AUTO_LENGTH, &code) != napi_ok ||
      napi_set_named_property(env, reason, "code", code) != napi_ok ||
      napi_reject_deferred(env, deferred, reason) != napi_ok) return NULL;
  return promise;
}

static napi_value promise_pending(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok) return NULL;
  return promise;
}

static napi_value counter_constructor(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], self;
  if (napi_get_cb_info(env, info, &argc, argv, &self, NULL) != napi_ok) return NULL;
  napi_value initial;
  if (argc > 0) initial = argv[0];
  else if (napi_create_double(env, 0, &initial) != napi_ok) return NULL;
  if (napi_set_named_property(env, self, "count", initial) != napi_ok) return NULL;
  return self;
}

static napi_value counter_increment(napi_env env, napi_callback_info info) {
  napi_value self, value, next;
  if (napi_get_cb_info(env, info, NULL, NULL, &self, NULL) != napi_ok) return NULL;
  double count = 0;
  if (napi_get_named_property(env, self, "count", &value) != napi_ok) return NULL;
  if (napi_get_value_double(env, value, &count) != napi_ok) return NULL;
  if (napi_create_double(env, count + 1, &next) != napi_ok) return NULL;
  if (napi_set_named_property(env, self, "count", next) != napi_ok) return NULL;
  return next;
}

static napi_value counter_self(napi_env env, napi_callback_info info) {
  napi_value self;
  if (napi_get_cb_info(env, info, NULL, NULL, &self, NULL) != napi_ok) return NULL;
  return self;
}

typedef struct {
  napi_async_work work;
  napi_ref callback;
  napi_ref value;
} callback_work;

static void execute_callback_work(napi_env env, void *data) {
  usleep(50000);
}

static void complete_callback_work(napi_env env, napi_status status, void *data) {
  callback_work *work = (callback_work *)data;
  napi_value callback, receiver, value;
  napi_status value_status = work->value != NULL
      ? napi_get_reference_value(env, work->value, &value)
      : napi_create_string_utf8(env, "async-value", NAPI_AUTO_LENGTH, &value);
  if (napi_get_reference_value(env, work->callback, &callback) == napi_ok &&
      napi_get_global(env, &receiver) == napi_ok && value_status == napi_ok) {
    napi_call_function(env, receiver, callback, 1, &value, NULL);
  }
  napi_delete_reference(env, work->callback);
  if (work->value != NULL) napi_delete_reference(env, work->value);
  napi_delete_async_work(env, work->work);
  free(work);
}

static napi_value on_later(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], resource_name;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc < 1) return NULL;
  callback_work *work = (callback_work *)calloc(1, sizeof(callback_work));
  if (!work) return NULL;
  if (napi_create_reference(env, argv[0], 1, &work->callback) != napi_ok) { free(work); return NULL; }
  if (argc == 2 && napi_create_reference(env, argv[1], 1, &work->value) != napi_ok) {
    napi_delete_reference(env, work->callback);
    free(work);
    return NULL;
  }
  if (napi_create_string_utf8(env, "fixture callback", NAPI_AUTO_LENGTH, &resource_name) != napi_ok ||
      napi_create_async_work(env, NULL, resource_name, execute_callback_work,
                             complete_callback_work, work, &work->work) != napi_ok ||
      napi_queue_async_work(env, work->work) != napi_ok) {
    napi_delete_reference(env, work->callback);
    if (work->value != NULL) napi_delete_reference(env, work->value);
    free(work);
    return NULL;
  }
  napi_value result;
  napi_get_undefined(env, &result);
  return result;
}

typedef struct {
  napi_threadsafe_function function;
} threadsafe_work;

static void call_threadsafe_js(napi_env env, napi_value callback,
                               void *context, void *data) {
  (void)context;
  char *message = (char *)data;
  if (env != NULL && callback != NULL && message != NULL) {
    napi_value receiver, argument;
    if (napi_get_global(env, &receiver) == napi_ok &&
        napi_create_string_utf8(env, message, NAPI_AUTO_LENGTH, &argument) == napi_ok) {
      napi_call_function(env, receiver, callback, 1, &argument, NULL);
    }
  }
  free(message);
}

static void *threadsafe_thread_main(void *data) {
  threadsafe_work *work = (threadsafe_work *)data;
  char *message = strdup("threadsafe-value");
  if (message == NULL ||
      napi_call_threadsafe_function(work->function, message, napi_tsfn_nonblocking) != napi_ok) {
    free(message);
  }
  napi_release_threadsafe_function(work->function, napi_tsfn_release);
  free(work);
  return NULL;
}

static napi_value on_threadsafe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], resource_name, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_create_string_utf8(env, "fixture threadsafe callback", NAPI_AUTO_LENGTH,
                              &resource_name) != napi_ok ||
      napi_get_undefined(env, &result) != napi_ok) return NULL;
  threadsafe_work *work = (threadsafe_work *)calloc(1, sizeof(threadsafe_work));
  if (work == NULL) return NULL;
  if (napi_create_threadsafe_function(env, argv[0], NULL, resource_name,
                                      1, 1, NULL, NULL, NULL,
                                      call_threadsafe_js, &work->function) != napi_ok) {
    free(work);
    return NULL;
  }
  pthread_t thread;
  if (pthread_create(&thread, NULL, threadsafe_thread_main, work) != 0) {
    napi_release_threadsafe_function(work->function, napi_tsfn_abort);
    free(work);
    return NULL;
  }
  pthread_detach(thread);
  return result;
}

static napi_value on_sync(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], receiver, value, result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1) return NULL;
  if (napi_get_global(env, &receiver) != napi_ok ||
      napi_create_string_utf8(env, "sync-value", NAPI_AUTO_LENGTH, &value) != napi_ok) return NULL;
  if (napi_call_function(env, receiver, argv[0], 1, &value, &result) != napi_ok) return NULL;
  return result;
}

static napi_value init(napi_env env, napi_value exports) {
  napi_value fn;
  if (napi_create_function(env, "add", NAPI_AUTO_LENGTH, add, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "add", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "echo", NAPI_AUTO_LENGTH, echo, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "echo", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "big", NAPI_AUTO_LENGTH, big, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "big", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "dateValue", NAPI_AUTO_LENGTH, date_value, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "dateValue", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeDate", NAPI_AUTO_LENGTH, make_date, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeDate", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "regexSource", NAPI_AUTO_LENGTH, regex_source, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "regexSource", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "regexFlags", NAPI_AUTO_LENGTH, regex_flags, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "regexFlags", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeRegex", NAPI_AUTO_LENGTH, make_regex, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeRegex", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeSymbol", NAPI_AUTO_LENGTH, make_symbol, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeSymbol", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "identity", NAPI_AUTO_LENGTH, identity, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "identity", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "isSymbol", NAPI_AUTO_LENGTH, is_symbol, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "isSymbol", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "isNodeIterator", NAPI_AUTO_LENGTH, is_node_iterator, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "isNodeIterator", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "sameObject", NAPI_AUTO_LENGTH, same_object, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "sameObject", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "setPrototype", NAPI_AUTO_LENGTH, set_prototype, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "setPrototype", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "setDefaultPrototype", NAPI_AUTO_LENGTH, set_default_prototype, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "setDefaultPrototype", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "prototypeMatches", NAPI_AUTO_LENGTH, prototype_matches, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "prototypeMatches", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "prototypeIsNull", NAPI_AUTO_LENGTH, prototype_is_null, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "prototypeIsNull", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "prototypeIsDefault", NAPI_AUTO_LENGTH, prototype_is_default, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "prototypeIsDefault", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "readProperty", NAPI_AUTO_LENGTH, read_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "readProperty", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "callInherited", NAPI_AUTO_LENGTH, call_inherited, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "callInherited", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "callToString", NAPI_AUTO_LENGTH, call_to_string, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "callToString", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "constructAndRead", NAPI_AUTO_LENGTH, construct_and_read, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "constructAndRead", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "writeProperty", NAPI_AUTO_LENGTH, write_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "writeProperty", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "writeThenRead", NAPI_AUTO_LENGTH, write_then_read_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "writeThenRead", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "readSymbolProperty", NAPI_AUTO_LENGTH, read_symbol_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "readSymbolProperty", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "writeSymbolProperty", NAPI_AUTO_LENGTH, write_symbol_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "writeSymbolProperty", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "deleteSymbolProperty", NAPI_AUTO_LENGTH, delete_symbol_property, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "deleteSymbolProperty", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeSymbolObject", NAPI_AUTO_LENGTH, make_symbol_object, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeSymbolObject", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "mutateObject", NAPI_AUTO_LENGTH, mutate_object, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "mutateObject", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "mutateArray", NAPI_AUTO_LENGTH, mutate_array, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "mutateArray", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeSparseArray", NAPI_AUTO_LENGTH, make_sparse_array, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeSparseArray", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "arrayHasElement", NAPI_AUTO_LENGTH, array_has_element, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "arrayHasElement", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "deleteArrayElement", NAPI_AUTO_LENGTH, delete_array_element, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "deleteArrayElement", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "mutateAfterCallback", NAPI_AUTO_LENGTH, mutate_after_callback, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "mutateAfterCallback", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "assignAndReturn", NAPI_AUTO_LENGTH, assign_and_return, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "assignAndReturn", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeSharedArray", NAPI_AUTO_LENGTH, make_shared_array, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeSharedArray", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeCycle", NAPI_AUTO_LENGTH, make_cycle, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeCycle", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeCyclicArray", NAPI_AUTO_LENGTH, make_cyclic_array, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeCyclicArray", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeBuffer", NAPI_AUTO_LENGTH, make_buffer, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeBuffer", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "isBuffer", NAPI_AUTO_LENGTH, is_buffer, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "isBuffer", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "typedArrayLength", NAPI_AUTO_LENGTH, typed_array_length, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "typedArrayLength", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeTypedArray", NAPI_AUTO_LENGTH, make_typed_array, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeTypedArray", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeArrayBuffer", NAPI_AUTO_LENGTH, make_array_buffer, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeArrayBuffer", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "arrayBufferLength", NAPI_AUTO_LENGTH, array_buffer_length, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "arrayBufferLength", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "makeDataView", NAPI_AUTO_LENGTH, make_data_view, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "makeDataView", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "dataViewByte", NAPI_AUTO_LENGTH, data_view_byte, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "dataViewByte", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "fail", NAPI_AUTO_LENGTH, fail, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "fail", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "mutateThenThrow", NAPI_AUTO_LENGTH, mutate_then_throw, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "mutateThenThrow", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseResult", NAPI_AUTO_LENGTH, promise_result, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseResult", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseReject", NAPI_AUTO_LENGTH, promise_reject, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseReject", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promiseRejectError", NAPI_AUTO_LENGTH, promise_reject_error, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promiseRejectError", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "promisePending", NAPI_AUTO_LENGTH, promise_pending, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "promisePending", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "onLater", NAPI_AUTO_LENGTH, on_later, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "onLater", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "onThreadsafe", NAPI_AUTO_LENGTH, on_threadsafe, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "onThreadsafe", fn) != napi_ok) return NULL;
  if (napi_create_function(env, "onSync", NAPI_AUTO_LENGTH, on_sync, NULL, &fn) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "onSync", fn) != napi_ok) return NULL;
  napi_property_descriptor counter_methods[] = {
    {"increment", NULL, counter_increment, NULL, NULL, NULL, napi_default, NULL},
    {"self", NULL, counter_self, NULL, NULL, NULL, napi_default, NULL},
  };
  napi_value counter;
  if (napi_define_class(env, "Counter", NAPI_AUTO_LENGTH, counter_constructor, NULL,
                        2, counter_methods, &counter) != napi_ok) return NULL;
  if (napi_set_named_property(env, exports, "Counter", counter) != napi_ok) return NULL;
  return exports;
}

NAPI_MODULE(NODE_GYP_MODULE_NAME, init)
"#,
        )
        .unwrap();
        let compile = ProcessCommand::new("cc")
            .arg("-shared")
            .arg("-fPIC")
            .arg("-pthread")
            .arg("-DNODE_GYP_MODULE_NAME=fixture")
            .arg(format!("-I{}", include.display()))
            .arg(&source)
            .arg("-o")
            .arg(&addon)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "could not compile Node-API fixture: {}",
            String::from_utf8_lossy(&compile.stderr)
        );

        let sparse_fixture = "const addon = require('./fixture.node'); const sparse = addon.makeSparseArray(); const initial = {hole: Object.hasOwn(sparse, 0), value: Object.hasOwn(sparse, 1), explicitUndefined: Object.hasOwn(sparse, 3), nativeHole: addon.arrayHasElement(sparse, 0), nativeValue: addon.arrayHasElement(sparse, 1)}; sparse[0] = 5; delete sparse[1]; const guestDeleted = !Object.hasOwn(sparse, 1); const guestDeleteReachedNative = !addon.arrayHasElement(sparse, 1); const hostDeleteResult = addon.deleteArrayElement(sparse, 3); const hostDeleteVisible = !Object.hasOwn(sparse, 3); addon.mutateArray(sparse); const mapped = sparse.map(value => value); ({length: sparse.length, initialHole: initial.hole, initialValue: initial.value, initialExplicitUndefined: initial.explicitUndefined, initialNativeHole: initial.nativeHole, initialNativeValue: initial.nativeValue, written: Object.hasOwn(sparse, 0), guestDeleted, guestDeleteReachedNative, hostDeleteResult, hostDeleteVisible, nativeDeleted: !addon.arrayHasElement(sparse, 3), restored: Object.hasOwn(sparse, 1), nativeRestored: addon.arrayHasElement(sparse, 1), keys: Object.keys(sparse).join(','), mappedLength: mapped.length, mappedIndex0: Object.hasOwn(mapped, 0), mappedIndex1: Object.hasOwn(mapped, 1), mappedIndex2: Object.hasOwn(mapped, 2), mappedIndex3: Object.hasOwn(mapped, 3)});";
        let sparse_reference = ProcessCommand::new("node")
            .arg("-e")
            .arg(format!(
                "process.stdout.write(JSON.stringify(eval({})))",
                serde_json::to_string(sparse_fixture).unwrap()
            ))
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(
            sparse_reference.status.success(),
            "Node sparse array reference failed: {}",
            String::from_utf8_lossy(&sparse_reference.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&sparse_reference.stdout),
            r#"{"length":4,"initialHole":false,"initialValue":true,"initialExplicitUndefined":true,"initialNativeHole":false,"initialNativeValue":true,"written":true,"guestDeleted":true,"guestDeleteReachedNative":true,"hostDeleteResult":true,"hostDeleteVisible":true,"nativeDeleted":true,"restored":true,"nativeRestored":true,"keys":"0,1,2,tag","mappedLength":4,"mappedIndex0":true,"mappedIndex1":true,"mappedIndex2":true,"mappedIndex3":false}"#
        );

        let node_reference = ProcessCommand::new("node")
            .arg("-e")
            .arg("const {createRequire}=require('node:module');const req=createRequire(process.argv[1]);const a=req('fixture');const b=req('#native');const c=req('./fixture.node');const d=req('fixture-wrapper');const target={value:40};let ownKeysCalls=0;const proxy=new Proxy(target,{get:(t,k)=>k==='value'?t.value+2:Reflect.get(t,k),set:(t,k,v)=>{t[k]=v;return true},ownKeys:()=>{ownKeysCalls++;return ['value']}});const proxyBefore=a.readProperty(proxy);a.writeProperty(proxy,9);const simpleTarget={value:1};const setOnlyProxy=new Proxy(simpleTarget,{set:(t,k,v)=>{t[k]=v;return true}});const proxyWriteRead=a.writeThenRead(setOnlyProxy,17);const inheritedValue=a.readProperty(Object.create({value:29}));const inheritedObject=Object.create({read(){return this.value+3}});inheritedObject.value=40;const inheritedCall=a.callInherited(inheritedObject);const prototypeTarget={value:40};const customPrototype={read(){return this.value+3}};const initialDefault=a.prototypeIsDefault(prototypeTarget);a.setPrototype(prototypeTarget,customPrototype);const customPrototypeMatch=a.prototypeMatches(prototypeTarget,customPrototype);const inheritedPrototypeValue=a.callInherited(prototypeTarget);a.setPrototype(prototypeTarget,null);const nullPrototype=a.prototypeIsNull(prototypeTarget);a.setDefaultPrototype(prototypeTarget);const restoredDefault=a.prototypeIsDefault(prototypeTarget);const guestErrorText=a.callToString(new TypeError('bridge'));const nested=a.onSync(value=>a.onSync(inner=>a.add(19,23)));process.stdout.write(JSON.stringify({sum:a.add(19,23),same:a===c,importSame:a===b,wrapperSame:a===d,proxyBefore,proxyAfter:a.readProperty(proxy),targetValue:target.value,ownKeysCalls,proxyWriteRead,inheritedValue,inheritedCall,initialDefault,customPrototypeMatch,inheritedPrototypeValue,nullPrototype,restoredDefault,guestErrorText,nested}));")
            .arg(root.join("main.cjs"))
            .output()
            .unwrap();
        assert!(
            node_reference.status.success(),
            "Node reference could not load the native addon through package exports and imports: {}",
            String::from_utf8_lossy(&node_reference.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&node_reference.stdout),
            r#"{"sum":42,"same":true,"importSame":true,"wrapperSame":true,"proxyBefore":42,"proxyAfter":11,"targetValue":9,"ownKeysCalls":0,"proxyWriteRead":17,"inheritedValue":29,"inheritedCall":43,"initialDefault":true,"customPrototypeMatch":true,"inheritedPrototypeValue":43,"nullPrototype":true,"restoredDefault":true,"guestErrorText":"TypeError: bridge","nested":42}"#
        );

        let class_reference = ProcessCommand::new("node")
            .arg("-e")
            .arg("const {createRequire}=require('node:module');const req=createRequire(process.argv[1]);const addon=req('./fixture.node');class Box{static value=2;constructor(value){this.value=value}read(){return this.value+Box.value}}const direct=addon.constructAndRead(Box,40);const proxiedBox=new Proxy(Box,{construct:(target,args)=>Reflect.construct(target,args)});const proxied=addon.constructAndRead(proxiedBox,40);const classStatic=addon.readProperty(Box);const classWriteRead=addon.writeThenRead(Box,23);process.stdout.write(JSON.stringify({direct,proxied,classStatic,classWriteRead,classValue:Box.value}));")
            .arg(root.join("main.cjs"))
            .output()
            .unwrap();
        assert!(
            class_reference.status.success(),
            "Node reference could not construct the guest class fixture: {}",
            String::from_utf8_lossy(&class_reference.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&class_reference.stdout),
            r#"{"direct":42,"proxied":42,"classStatic":2,"classWriteRead":23,"classValue":23}"#
        );

        let mut interpreter = Interpreter::with_builtins();
        let expected_sha256: [u8; 32] = Sha256::digest(fs::read(&addon).unwrap()).into();
        let _bridge = interpreter
            .enable_node_addons(
                NodeAddonOptions::new("node", [root.clone()])
                    .allow_native_addon_with_sha256(addon.clone(), expected_sha256)
                    .minimum_napi_version(1)
                    .entry(root.join("main.cjs")),
            )
            .unwrap();
        assert!(!_bridge.runtime_info().node_version.is_empty());
        assert!(_bridge.runtime_info().napi_version >= 1);
        let mut incompatible_interpreter = Interpreter::with_builtins();
        let version_error = incompatible_interpreter
            .enable_node_addons(
                NodeAddonOptions::new("node", [root.clone()])
                    .allow_native_addon_with_sha256(addon.clone(), expected_sha256)
                    .minimum_napi_version(u32::MAX),
            )
            .unwrap_err();
        assert!(version_error.to_string().contains("Node-API v4294967295"));
        let package_result = interpreter
            .eval_source(
                "const packageAddon = require('fixture'); const importAddon = require('#native'); const wrapperAddon = require('fixture-wrapper'); const target = {value:40}; let ownKeysCalls=0; const proxy = new Proxy(target, {get:(t,k)=>k==='value'?t.value+2:Reflect.get(t,k),set:(t,k,v)=>{t[k]=v;return true},ownKeys:()=>{ownKeysCalls++;return ['value']}}); const proxyBefore = packageAddon.readProperty(proxy); packageAddon.writeProperty(proxy,9); const simpleTarget={value:1}; const setOnlyProxy=new Proxy(simpleTarget,{set:(t,k,v)=>{t[k]=v;return true}}); const proxyWriteRead=packageAddon.writeThenRead(setOnlyProxy,17); const inheritedValue=packageAddon.readProperty(Object.create({value:29})); const inheritedObject=Object.create({read:function(){return this.value+3}}); inheritedObject.value=40; const inheritedCall=packageAddon.callInherited(inheritedObject); const prototypeTarget={value:40}; const customPrototype={read:function(){return this.value+3}}; const initialDefault=packageAddon.prototypeIsDefault(prototypeTarget); packageAddon.setPrototype(prototypeTarget,customPrototype); const customPrototypeMatch=packageAddon.prototypeMatches(prototypeTarget,customPrototype); const inheritedPrototypeValue=packageAddon.callInherited(prototypeTarget); packageAddon.setPrototype(prototypeTarget,null); const nullPrototype=packageAddon.prototypeIsNull(prototypeTarget); packageAddon.setDefaultPrototype(prototypeTarget); const restoredDefault=packageAddon.prototypeIsDefault(prototypeTarget); const guestErrorText=packageAddon.callToString(new TypeError('bridge')); ({sum: packageAddon.add(19, 23), same: packageAddon === require('./fixture.node'), importSame: packageAddon === importAddon, wrapperSame: packageAddon === wrapperAddon, proxyBefore, proxyAfter: packageAddon.readProperty(proxy), targetValue: target.value, ownKeysCalls, proxyWriteRead, inheritedValue, inheritedCall, initialDefault, customPrototypeMatch, inheritedPrototypeValue, nullPrototype, restoredDefault, guestErrorText});",
            )
            .unwrap();
        assert!(matches!(
            package_result.get_prop("sum"),
            Some(Value::Number(value)) if value == 42.0
        ));
        assert!(matches!(
            package_result.get_prop("same"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            package_result.get_prop("importSame"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            package_result.get_prop("wrapperSame"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            package_result.get_prop("proxyBefore"),
            Some(Value::Number(value)) if value == 42.0
        ));
        assert!(matches!(
            package_result.get_prop("proxyAfter"),
            Some(Value::Number(value)) if value == 11.0
        ));
        assert!(matches!(
            package_result.get_prop("targetValue"),
            Some(Value::Number(value)) if value == 9.0
        ));
        assert!(matches!(
            package_result.get_prop("ownKeysCalls"),
            Some(Value::Number(value)) if value == 0.0
        ));
        assert!(matches!(
            package_result.get_prop("proxyWriteRead"),
            Some(Value::Number(value)) if value == 17.0
        ));
        assert!(matches!(
            package_result.get_prop("inheritedValue"),
            Some(Value::Number(value)) if value == 29.0
        ));
        assert!(matches!(
            package_result.get_prop("inheritedCall"),
            Some(Value::Number(value)) if value == 43.0
        ));
        assert!(matches!(
            package_result.get_prop("initialDefault"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            package_result.get_prop("customPrototypeMatch"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            package_result.get_prop("inheritedPrototypeValue"),
            Some(Value::Number(value)) if value == 43.0
        ));
        assert!(matches!(
            package_result.get_prop("nullPrototype"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            package_result.get_prop("restoredDefault"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            package_result.get_prop("guestErrorText"),
            Some(Value::String(ref value)) if value == "TypeError: bridge"
        ));
        let class_result = interpreter
            .eval_source(
                "class Box { static value = 2; constructor(value) { this.value = value; } read() { return this.value + Box.value; } } const direct = packageAddon.constructAndRead(Box, 40); const proxiedBox = new Proxy(Box, {construct:(target,args) => Reflect.construct(target,args)}); const proxied = packageAddon.constructAndRead(proxiedBox, 40); const classStatic = packageAddon.readProperty(Box); const classWriteRead = packageAddon.writeThenRead(Box, 23); ({direct, proxied, classStatic, classWriteRead, classValue: Box.value});",
            )
            .unwrap();
        assert!(matches!(
            class_result.get_prop("direct"),
            Some(Value::Number(value)) if value == 42.0
        ));
        assert!(matches!(
            class_result.get_prop("proxied"),
            Some(Value::Number(value)) if value == 42.0
        ));
        assert!(matches!(
            class_result.get_prop("classStatic"),
            Some(Value::Number(value)) if value == 2.0
        ));
        assert!(matches!(
            class_result.get_prop("classWriteRead"),
            Some(Value::Number(value)) if value == 23.0
        ));
        assert!(matches!(
            class_result.get_prop("classValue"),
            Some(Value::Number(value)) if value == 23.0
        ));
        let result = interpreter
            .eval_source("require('./fixture.node').add(19, 23);")
            .unwrap();
        assert!(matches!(result, Value::Number(value) if value == 42.0));
        let bigint = interpreter
            .eval_source("String(require('./fixture.node').big());")
            .unwrap();
        assert!(matches!(bigint, Value::String(ref value) if value == "9007199254740993"));
        let builtins = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const nativeDate = addon.makeDate(); const nativeRegex = addon.makeRegex(); const nativeSymbol = addon.makeSymbol(); const guestSymbol = Symbol('guest'); const nativeRegexMatched = nativeRegex.test('AAA'); ({inputDate:addon.dateValue(new Date(1700000000123)), nativeDate:nativeDate.getTime(), inputRegex:addon.regexSource(/a+/gi) + '/' + addon.regexFlags(/a+/gi), nativeRegex:nativeRegex.source + '/' + nativeRegex.flags, nativeRegexMatched, nativeRegexLastIndex:nativeRegex.lastIndex, nativeSymbolType:typeof nativeSymbol, nativeSymbolDescription:nativeSymbol.description, nativeSymbolNapiType:addon.isSymbol(nativeSymbol), guestSymbolNapiType:addon.isSymbol(guestSymbol), iteratorSymbolNapiType:addon.isNodeIterator(Symbol.iterator), nativeSymbolIdentity:addon.identity(nativeSymbol) === nativeSymbol, guestSymbolIdentity:addon.identity(guestSymbol) === guestSymbol});",
            )
            .unwrap();
        assert!(matches!(
            builtins.get_prop("inputDate"),
            Some(Value::Number(value)) if value == 1_700_000_000_123.0
        ));
        assert!(matches!(
            builtins.get_prop("nativeDate"),
            Some(Value::Number(value)) if value == 123_456.0
        ));
        assert!(matches!(
            builtins.get_prop("inputRegex"),
            Some(Value::String(ref value)) if value == "a+/gi"
        ));
        assert!(matches!(
            builtins.get_prop("nativeRegex"),
            Some(Value::String(ref value)) if value == "a+/gi"
        ));
        assert!(matches!(
            builtins.get_prop("nativeRegexMatched"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            builtins.get_prop("nativeRegexLastIndex"),
            Some(Value::Number(value)) if value == 3.0
        ));
        assert!(matches!(
            builtins.get_prop("nativeSymbolType"),
            Some(Value::String(ref value)) if value == "symbol"
        ));
        assert!(matches!(
            builtins.get_prop("nativeSymbolDescription"),
            Some(Value::String(ref value)) if value == "native"
        ));
        assert!(matches!(
            builtins.get_prop("nativeSymbolNapiType"),
            Some(Value::Number(value)) if value == 1.0
        ));
        assert!(matches!(
            builtins.get_prop("guestSymbolNapiType"),
            Some(Value::Number(value)) if value == 1.0
        ));
        assert!(matches!(
            builtins.get_prop("iteratorSymbolNapiType"),
            Some(Value::Number(value)) if value == 1.0
        ));
        assert!(matches!(
            builtins.get_prop("nativeSymbolIdentity"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            builtins.get_prop("guestSymbolIdentity"),
            Some(Value::Bool(true))
        ));
        let identities = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const child = {value:1}; const parent = {left:child, right:child}; const nativeArray = addon.makeSharedArray(); ({nested:addon.sameObject(parent.left, parent.right), topLevel:addon.sameObject(child, child), distinct:addon.sameObject({}, {}), nativeArray:nativeArray[0] === nativeArray[1]});",
            )
            .unwrap();
        assert!(matches!(
            identities.get_prop("nested"),
            Some(Value::Number(value)) if value == 1.0
        ));
        assert!(matches!(
            identities.get_prop("topLevel"),
            Some(Value::Number(value)) if value == 1.0
        ));
        assert!(matches!(
            identities.get_prop("distinct"),
            Some(Value::Number(value)) if value == 0.0
        ));
        assert!(matches!(
            identities.get_prop("nativeArray"),
            Some(Value::Bool(true))
        ));
        let cycles = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const guestObject = {value:1}; guestObject.self = guestObject; const guestArray = []; guestArray.push(guestArray); const nativeObject = addon.makeCycle(); const nativeArray = addon.makeCyclicArray(); ({guestObject:guestObject.self === addon.echo(guestObject), guestArray:guestArray[0] === addon.echo(guestArray), nativeObject:nativeObject.self === nativeObject, nativeArray:nativeArray[0] === nativeArray});",
            )
            .unwrap();
        for key in ["guestObject", "guestArray", "nativeObject", "nativeArray"] {
            assert!(
                matches!(cycles.get_prop(key), Some(Value::Bool(true))),
                "{key}"
            );
        }
        let cyclic_writeback = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const object = addon.makeCycle(); const result = addon.mutateObject(object); ({same:result === object, self:object.self === object, changed:object.changed, child:object.child.value});",
            )
            .unwrap();
        assert!(matches!(
            cyclic_writeback.get_prop("same"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            cyclic_writeback.get_prop("self"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            cyclic_writeback.get_prop("changed"),
            Some(Value::Number(value)) if value == 73.0
        ));
        assert!(matches!(
            cyclic_writeback.get_prop("child"),
            Some(Value::Number(value)) if value == 91.0
        ));
        let mutations = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const child = {value:1}; const object = {child, removeMe:5}; const array = [1,2]; const objectResult = addon.mutateObject(object); const arrayResult = addon.mutateArray(array); ({objectIdentity:objectResult === object, direct:object.changed, nested:child.value, removed:'removeMe' in object, arrayIdentity:arrayResult === array, item:array[1], appended:array[2], length:array.length, named:array.tag});",
            )
            .unwrap();
        assert!(matches!(
            mutations.get_prop("objectIdentity"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            mutations.get_prop("direct"),
            Some(Value::Number(value)) if value == 73.0
        ));
        assert!(matches!(
            mutations.get_prop("nested"),
            Some(Value::Number(value)) if value == 91.0
        ));
        assert!(matches!(
            mutations.get_prop("removed"),
            Some(Value::Bool(false))
        ));
        assert!(matches!(
            mutations.get_prop("arrayIdentity"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            mutations.get_prop("item"),
            Some(Value::Number(value)) if value == 42.0
        ));
        assert!(matches!(
            mutations.get_prop("appended"),
            Some(Value::Number(value)) if value == 84.0
        ));
        assert!(matches!(
            mutations.get_prop("length"),
            Some(Value::Number(value)) if value == 3.0
        ));
        assert!(matches!(
            mutations.get_prop("named"),
            Some(Value::String(ref value)) if value == "native"
        ));
        let accessor_bridge = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); let captured = 14; const object = {}; Object.defineProperty(object, 'value', {get: () => captured, set: (value) => { captured = value; }, enumerable: true, configurable: true}); const before = addon.readProperty(object); addon.writeProperty(object, 37); ({before, captured, after: addon.readProperty(object)});",
            )
            .unwrap();
        assert!(matches!(
            accessor_bridge.get_prop("before"),
            Some(Value::Number(value)) if value == 14.0
        ));
        assert!(matches!(
            accessor_bridge.get_prop("captured"),
            Some(Value::Number(value)) if value == 37.0
        ));
        assert!(matches!(
            accessor_bridge.get_prop("after"),
            Some(Value::Number(value)) if value == 37.0
        ));
        let symbol_bridge = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const key = Symbol('addon-key'); const defined = Symbol('defined-key'); const accessKey = Symbol('accessor-key'); const object = {[key]:18}; Object.defineProperty(object, defined, {value:29, writable:true, enumerable:true, configurable:true}); let captured = 6; Object.defineProperty(object, accessKey, {get: () => captured, set: (value) => { captured = value; }, enumerable:true, configurable:true}); const accessorBefore = addon.readSymbolProperty(object, accessKey); addon.writeSymbolProperty(object, accessKey, 61); const accessorAfter = addon.readSymbolProperty(object, accessKey); const fromKey = Symbol('from-entries'); const fromObject = Object.fromEntries([[fromKey, 71]]); const fromEntries = addon.readSymbolProperty(fromObject, fromKey); const before = addon.readSymbolProperty(object, key); addon.writeSymbolProperty(object, key, 53); const after = object[key]; const deleted = addon.deleteSymbolProperty(object, key); ({before, after, deleted, final: object[key], defined: object[defined], fromEntries, accessorBefore, accessorAfter, captured});",
            )
            .unwrap();
        assert!(matches!(
            symbol_bridge.get_prop("before"),
            Some(Value::Number(value)) if value == 18.0
        ));
        assert!(matches!(
            symbol_bridge.get_prop("after"),
            Some(Value::Number(value)) if value == 53.0
        ));
        assert!(matches!(
            symbol_bridge.get_prop("deleted"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            symbol_bridge.get_prop("final"),
            Some(Value::Undefined)
        ));
        assert!(matches!(
            symbol_bridge.get_prop("defined"),
            Some(Value::Number(value)) if value == 29.0
        ));
        assert!(matches!(
            symbol_bridge.get_prop("fromEntries"),
            Some(Value::Number(value)) if value == 71.0
        ));
        assert!(matches!(
            symbol_bridge.get_prop("accessorBefore"),
            Some(Value::Number(value)) if value == 6.0
        ));
        assert!(matches!(
            symbol_bridge.get_prop("accessorAfter"),
            Some(Value::Number(value)) if value == 61.0
        ));
        assert!(matches!(
            symbol_bridge.get_prop("captured"),
            Some(Value::Number(value)) if value == 61.0
        ));
        let returned_symbol_bridge = interpreter
            .eval_source(
                "const native = require('./fixture.node').makeSymbolObject(); ({description: native.key.description, value: native[native.key]});",
            )
            .unwrap();
        assert!(matches!(
            returned_symbol_bridge.get_prop("description"),
            Some(Value::String(ref value)) if value == "native-key"
        ));
        assert!(matches!(
            returned_symbol_bridge.get_prop("value"),
            Some(Value::Number(value)) if value == 89.0
        ));
        let callback_mutations = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const object = {value:1}; const returned = addon.mutateAfterCallback(object, () => { object.guest = 19; }); ({same:returned === object, guest:object.guest, native:object.native});",
            )
            .unwrap();
        assert!(matches!(
            callback_mutations.get_prop("same"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            callback_mutations.get_prop("guest"),
            Some(Value::Number(value)) if value == 19.0
        ));
        assert!(matches!(
            callback_mutations.get_prop("native"),
            Some(Value::Number(value)) if value == 27.0
        ));
        let returned_alias = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const holder = {}; const result = addon.assignAndReturn(holder); ({same:result === holder.created, value:holder.created.value});",
            )
            .unwrap();
        assert!(matches!(
            returned_alias.get_prop("same"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            returned_alias.get_prop("value"),
            Some(Value::Number(value)) if value == 15.0
        ));
        let buffers = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const buffer = addon.makeBuffer(); const view = addon.makeDataView(); ({text:buffer.toString('utf8'), first:buffer[0], roundTrip:addon.isBuffer(buffer), inputLength:addon.typedArrayLength(new Uint16Array([300, 400])), typed:addon.makeTypedArray(), arrayBufferLength:addon.arrayBufferLength(new Uint8Array([1, 2, 3]).buffer), arrayBufferByte:new Uint8Array(addon.makeArrayBuffer())[1], dataViewLength:view.byteLength, dataViewByte:view.getUint8(1), dataViewRoundTrip:addon.dataViewByte(new DataView(new Uint8Array([4, 5]).buffer))});",
            )
            .unwrap();
        assert!(
            matches!(buffers.get_prop("text"), Some(Value::String(ref value)) if value == "abc")
        );
        assert!(matches!(buffers.get_prop("first"), Some(Value::Number(value)) if value == 97.0));
        assert!(
            matches!(buffers.get_prop("roundTrip"), Some(Value::Number(value)) if value == 1.0)
        );
        assert!(
            matches!(buffers.get_prop("inputLength"), Some(Value::Number(value)) if value == 2.0)
        );
        assert!(
            matches!(buffers.get_prop("arrayBufferLength"), Some(Value::Number(value)) if value == 3.0)
        );
        assert!(
            matches!(buffers.get_prop("arrayBufferByte"), Some(Value::Number(value)) if value == 121.0)
        );
        assert!(
            matches!(buffers.get_prop("dataViewLength"), Some(Value::Number(value)) if value == 2.0)
        );
        assert!(
            matches!(buffers.get_prop("dataViewByte"), Some(Value::Number(value)) if value == 29.0)
        );
        assert!(
            matches!(buffers.get_prop("dataViewRoundTrip"), Some(Value::Number(value)) if value == 5.0)
        );
        let typed_value = buffers.get_prop("typed").unwrap_or(Value::Undefined);
        let Value::TypedArray(typed) = &typed_value else {
            panic!("native Uint16Array was not preserved as a typed array");
        };
        assert_eq!(typed.kind, TypedKind::Uint16);
        assert_eq!(typed.length, 2);
        assert!(matches!(
            crate::builtins::read_element(typed, 0),
            Some(Value::Number(value)) if value == 300.0
        ));
        let error = interpreter
            .eval_source(
                "try { require('./fixture.node').fail(); } catch (error) { ({name:error.name, message:error.message, code:error.code}); }",
            )
            .unwrap();
        assert!(
            matches!(error.get_prop("name"), Some(Value::String(ref name)) if name == "TypeError")
        );
        assert!(
            matches!(error.get_prop("message"), Some(Value::String(ref message)) if message == "fixture failure")
        );
        assert!(
            matches!(error.get_prop("code"), Some(Value::String(ref code)) if code == "E_FIXTURE")
        );
        let error_after_mutation = interpreter
            .eval_source(
                "const object = {}; let observed; try { require('./fixture.node').mutateThenThrow(object); } catch (error) { observed = {name:error.name, code:error.code, message:error.message, value:object.afterThrow}; } observed;",
            )
            .unwrap();
        assert!(matches!(
            error_after_mutation.get_prop("name"),
            Some(Value::String(ref name)) if name == "TypeError"
        ));
        assert!(matches!(
            error_after_mutation.get_prop("code"),
            Some(Value::String(ref code)) if code == "E_AFTER_MUTATION"
        ));
        assert!(matches!(
            error_after_mutation.get_prop("message"),
            Some(Value::String(ref message)) if message == "mutation happened"
        ));
        assert!(matches!(
            error_after_mutation.get_prop("value"),
            Some(Value::Number(value)) if value == 88.0
        ));
        let async_result = interpreter
            .eval_source(
                "await require('./fixture.node').promiseResult().then(value => value + '-chained');",
            )
            .unwrap();
        assert!(
            matches!(async_result, Value::String(ref value) if value == "native-promise-value-chained")
        );
        let async_function_result = interpreter
            .eval_source(
                "async function readNativePromise() { return await require('./fixture.node').promiseResult(); } await readNativePromise();",
            )
            .unwrap();
        assert!(
            matches!(async_function_result, Value::String(ref value) if value == "native-promise-value")
        );
        let unrelated_await = interpreter
            .eval_source(
                "globalThis.pendingNativePromise = require('./fixture.node').promisePending(); await Promise.resolve(); 'unrelated-await-completed';",
            )
            .unwrap();
        assert!(
            matches!(unrelated_await, Value::String(ref value) if value == "unrelated-await-completed")
        );
        let async_rejection = interpreter
            .eval_source(
                "try { await require('./fixture.node').promiseReject(); } catch (reason) { reason; }",
            )
            .unwrap();
        assert!(
            matches!(async_rejection, Value::String(ref reason) if reason == "native-rejection")
        );
        let async_error_rejection = interpreter
            .eval_source(
                "try { await require('./fixture.node').promiseRejectError(); } catch (error) { ({name:error.name, message:error.message, code:error.code}); }",
            )
            .unwrap();
        assert!(matches!(
            async_error_rejection.get_prop("name"),
            Some(Value::String(ref name)) if name == "Error"
        ));
        assert!(matches!(
            async_error_rejection.get_prop("message"),
            Some(Value::String(ref message)) if message == "native error rejection"
        ));
        assert!(matches!(
            async_error_rejection.get_prop("code"),
            Some(Value::String(ref code)) if code == "E_NATIVE_REJECTION"
        ));
        let sync_result = interpreter
            .eval_source(
                "globalThis.syncCallbackThisType = ''; const syncCallbackResult = require('./fixture.node').onSync(function(value) { syncCallbackThisType = typeof this; return value + '-reply'; }); ({value:syncCallbackResult, thisType:syncCallbackThisType});",
            )
            .unwrap();
        assert!(
            matches!(sync_result.get_prop("value"), Some(Value::String(ref value)) if value == "sync-value-reply")
        );
        assert!(
            matches!(sync_result.get_prop("thisType"), Some(Value::String(ref value)) if value == "object")
        );
        let sync_throw = interpreter
            .eval_source(
                "try { require('./fixture.node').onSync(() => { throw new TypeError('guest callback failure'); }); } catch (error) { ({name:error.name, message:error.message}); }",
            )
            .unwrap();
        assert!(
            matches!(sync_throw.get_prop("name"), Some(Value::String(ref name)) if name == "TypeError"),
            "unexpected sync callback throw: {sync_throw:?}"
        );
        assert!(
            matches!(sync_throw.get_prop("message"), Some(Value::String(ref message)) if message == "guest callback failure"),
            "unexpected sync callback throw: {sync_throw:?}"
        );
        let nested_addon_call = interpreter
            .eval_source(
                "require('./fixture.node').onSync(value => require('./fixture.node').onSync(inner => require('./fixture.node').add(19, 23)));",
            )
            .unwrap();
        assert!(matches!(
            nested_addon_call,
            Value::Number(value) if value == 42.0
        ));
        interpreter
            .eval_source(
                "globalThis.callbackValues = []; globalThis.callbackThisType = ''; require('./fixture.node').onLater(function(value) { callbackValues.push(value); callbackThisType = typeof this; });",
            )
            .unwrap();
        assert!(
            interpreter
                .run_event_loop_once(Duration::from_secs(2))
                .unwrap()
        );
        let callback_value = interpreter.eval_source("callbackValues[0];").unwrap();
        assert!(matches!(callback_value, Value::String(ref value) if value == "async-value"));
        let callback_this = interpreter.eval_source("callbackThisType;").unwrap();
        assert!(matches!(callback_this, Value::String(ref value) if value == "object"));
        interpreter
            .eval_source(
                "globalThis.asyncReferenceTarget = {value:1}; globalThis.asyncCallbackIdentity = false; require('./fixture.node').onLater(value => { value.changed = 23; asyncCallbackIdentity = value === asyncReferenceTarget; }, asyncReferenceTarget);",
            )
            .unwrap();
        assert!(
            interpreter
                .run_event_loop_once(Duration::from_secs(2))
                .unwrap()
        );
        assert!(matches!(
            interpreter
                .eval_source("asyncReferenceTarget.changed;")
                .unwrap(),
            Value::Number(23.0)
        ));
        assert!(matches!(
            interpreter.eval_source("asyncCallbackIdentity;").unwrap(),
            Value::Bool(true)
        ));
        interpreter
            .eval_source(
                "globalThis.threadsafeValues = []; require('./fixture.node').onThreadsafe(value => { threadsafeValues.push(value); queueMicrotask(() => threadsafeValues.push('microtask')); });",
            )
            .unwrap();
        assert!(matches!(
            interpreter.eval_source("threadsafeValues.length;").unwrap(),
            Value::Number(0.0)
        ));
        let mut threadsafe_callback_received = false;
        for _ in 0..10 {
            if interpreter
                .run_event_loop_once(Duration::from_millis(250))
                .unwrap()
            {
                let values = interpreter
                    .eval_source("threadsafeValues.join(',');")
                    .unwrap();
                if matches!(values, Value::String(ref values) if values == "threadsafe-value,microtask")
                {
                    threadsafe_callback_received = true;
                    break;
                }
            }
        }
        assert!(
            threadsafe_callback_received,
            "Node-API threadsafe callback did not reach the guest event loop"
        );
        let counter = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const counter = new addon.Counter(19); counter.increment(); counter.count = 41; counter.increment(); const other = new addon.Counter(5); counter.increment.call(other); const spread = {...counter}; const assigned = Object.assign({}, counter); let iterated = ''; for (const key in counter) iterated += key; ({count:counter.count, receiver:other.count, same:counter === counter.self(), keys:Object.keys(counter).join(','), values:Object.values(counter).join(','), entries:Object.entries(counter)[0][0] + ':' + Object.entries(counter)[0][1], spread:spread.count, assigned:assigned.count, iterated, has:'count' in counter});",
            )
            .unwrap();
        assert!(matches!(counter.get_prop("count"), Some(Value::Number(count)) if count == 42.0));
        assert!(matches!(counter.get_prop("receiver"), Some(Value::Number(value)) if value == 6.0));
        assert!(matches!(counter.get_prop("same"), Some(Value::Bool(true))));
        assert!(
            matches!(counter.get_prop("keys"), Some(Value::String(ref keys)) if keys == "count")
        );
        assert!(
            matches!(counter.get_prop("values"), Some(Value::String(ref values)) if values == "42")
        );
        assert!(
            matches!(counter.get_prop("entries"), Some(Value::String(ref entries)) if entries == "count:42")
        );
        assert!(matches!(counter.get_prop("spread"), Some(Value::Number(value)) if value == 42.0));
        assert!(
            matches!(counter.get_prop("assigned"), Some(Value::Number(value)) if value == 42.0)
        );
        assert!(
            matches!(counter.get_prop("iterated"), Some(Value::String(ref iterated)) if iterated == "count")
        );
        assert!(matches!(counter.get_prop("has"), Some(Value::Bool(true))));
        let roundtrip = interpreter
            .eval_source(
                "const addon = require('./fixture.node'); const counter = new addon.Counter(5); addon.echo(counter) === counter;",
            )
            .unwrap();
        assert!(matches!(roundtrip, Value::Bool(true)));

        drop(interpreter);
        drop(_bridge);
        let mut sparse_interpreter = Interpreter::with_builtins();
        let _sparse_bridge = sparse_interpreter
            .enable_node_addons(
                NodeAddonOptions::new("node", [root.clone()])
                    .allow_native_addon_with_sha256(addon.clone(), expected_sha256)
                    .entry(root.join("main.cjs")),
            )
            .unwrap();
        let sparse_result = sparse_interpreter.eval_source(sparse_fixture).unwrap();
        for (key, expected) in [
            ("initialHole", false),
            ("initialValue", true),
            ("initialExplicitUndefined", true),
            ("initialNativeHole", false),
            ("initialNativeValue", true),
            ("written", true),
            ("guestDeleted", true),
            ("guestDeleteReachedNative", true),
            ("hostDeleteResult", true),
            ("hostDeleteVisible", true),
            ("nativeDeleted", true),
            ("restored", true),
            ("nativeRestored", true),
            ("mappedIndex0", true),
            ("mappedIndex1", true),
            ("mappedIndex2", true),
            ("mappedIndex3", false),
        ] {
            assert!(
                matches!(sparse_result.get_prop(key), Some(Value::Bool(value)) if value == expected),
                "sparse array result mismatch for {key}: {:?}",
                sparse_result.get_prop(key)
            );
        }
        for key in ["length", "mappedLength"] {
            assert!(
                matches!(sparse_result.get_prop(key), Some(Value::Number(value)) if value == 4.0),
                "sparse array length mismatch for {key}"
            );
        }
        assert!(matches!(
            sparse_result.get_prop("keys"),
            Some(Value::String(ref value)) if value == "0,1,2,tag"
        ));
        drop(sparse_interpreter);
        drop(_sparse_bridge);
        fs::remove_dir_all(root).unwrap();
    }
}
