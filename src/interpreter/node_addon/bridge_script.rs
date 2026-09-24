//! JavaScript worker script embedded in the Node sidecar process.

pub(super) const NODE_BRIDGE: &str = r#"
'use strict';
const net = require('node:net');
const { Worker } = require('node:worker_threads');
const socket = net.connect({host:'127.0.0.1',port:Number(process.env.NAPI_VM_BRIDGE_PORT)});
let input = Buffer.alloc(0);
let connected = false;
let workerReady = false;
let shuttingDown = false;
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
    if(message.shutdown===true){
      if(!shuttingDown){shuttingDown=true;worker.postMessage({kind:'shutdown'});}
      return;
    }
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
      case 'typedArray':{const bytes=Uint8Array.from(value.bytes);if(value.isBuffer)return Buffer.from(bytes);const Constructor=globalThis[value.kind];if(typeof Constructor!=='function')throw new TypeError('unsupported guest typed array kind');return new Constructor(bytes.buffer,0,value.length);}
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
    if(message.kind==='shutdown'){
      parentPort.close();
      process.exit(0);
    }else if(message.kind==='request'){
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
worker.on('exit',code=>{if(shuttingDown){socket.end();return;}if(code!==0)socket.destroy(new Error('Node addon worker exited with code '+code));});
socket.on('connect',()=>{connected=true;maybeSendHello();});
socket.on('data',c=>{input=Buffer.concat([input,c]);consume();});
socket.on('error',e=>process.stderr.write('napi-vm sidecar: '+e.message+'\n'));
"#;
