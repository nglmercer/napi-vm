(function(){
    function normalize(path){
        if(typeof path!=='string')throw new TypeError('Path must be a string');
        const absolute=path.startsWith('/'),trailing=path.endsWith('/'),parts=[];
        for(const part of path.split('/')){if(part===''||part==='.')continue;if(part==='..'){if(parts.length&&parts[parts.length-1]!=='..')parts.pop();else if(!absolute)parts.push('..');}else parts.push(part);}
        let result=(absolute?'/':'')+parts.join('/');if(!result)result='.';if(trailing&&result!=='.'&&result!=='/')result+='/';return result;
    }
    function dirname(path){if(typeof path!=='string')throw new TypeError('Path must be a string');if(!path)return '.';let end=path.length;while(end>1&&path[end-1]==='/')end--;const i=path.slice(0,end).lastIndexOf('/');if(i<0)return '.';if(i===0)return '/';return path.slice(0,i);}
    function basename(path,suffix){if(typeof path!=='string')throw new TypeError('Path must be a string');let end=path.length;while(end>0&&path[end-1]==='/')end--;let base=path.slice(0,end).split('/').pop()||'';if(suffix&&base.endsWith(suffix))base=base.slice(0,base.length-suffix.length);return base;}
    const path={normalize:normalize,dirname:dirname,basename:basename,sep:'/',delimiter:':',
        join:function(...parts){for(const p of parts)if(typeof p!=='string')throw new TypeError('Path must be a string');return normalize(parts.filter(p=>p.length).join('/'));},
        isAbsolute:function(p){if(typeof p!=='string')throw new TypeError('Path must be a string');return p.startsWith('/');},
        extname:function(p){const base=basename(p),i=base.lastIndexOf('.');return i<=0||base==='..'?'':base.slice(i);},
        resolve:function(...parts){let result='';for(let i=parts.length-1;i>=0;i--){if(typeof parts[i]!=='string')throw new TypeError('Path must be a string');result=parts[i]+'/'+result;if(parts[i].startsWith('/'))break;}if(!result.startsWith('/'))throw new Error('Relative resolve requires an explicitly supplied absolute base');const normalized=normalize(result);return normalized.length>1&&normalized.endsWith('/')?normalized.slice(0,-1):normalized;},
        relative:function(from,to){from=path.resolve(from).split('/').filter(Boolean);to=path.resolve(to).split('/').filter(Boolean);let common=0;while(common<from.length&&common<to.length&&from[common]===to[common])common++;return from.slice(common).map(()=> '..').concat(to.slice(common)).join('/');},
        parse:function(p){const base=basename(p),ext=path.extname(p);return {root:p.startsWith('/')?'/':'',dir:p.includes('/')?dirname(p):'',base:base,ext:ext,name:base.slice(0,base.length-ext.length)};},
        format:function(p){const dir=p.dir||p.root||'',base=p.base||((p.name||'')+(p.ext||''));return dir?(dir.endsWith('/')?dir:dir+'/')+base:base;}
    };path.posix=path;globalThis.__napiVmNodePath=path;
    class EventEmitter{
        constructor(){this._events=new Map();this._maxListeners=10;}
        on(name,listener){if(typeof listener!=='function')throw new TypeError('Listener must be a function');if(!this._events.has(name))this._events.set(name,[]);this._events.get(name).push(listener);return this;}
        addListener(name,listener){return this.on(name,listener);}
        once(name,listener){const self=this;function wrapper(...args){self.removeListener(name,wrapper);listener.apply(self,args);}wrapper.listener=listener;return this.on(name,wrapper);}
        prependListener(name,listener){this.on(name,listener);const list=this._events.get(name);list.unshift(list.pop());return this;}
        prependOnceListener(name,listener){this.once(name,listener);const list=this._events.get(name);list.unshift(list.pop());return this;}
        emit(name,...args){const listeners=(this._events.get(name)||[]).slice();if(name==='error'&&!listeners.length)throw args[0]||new Error('Unhandled error event');for(const listener of listeners)listener.apply(this,args);return listeners.length>0;}
        removeListener(name,listener){const list=this._events.get(name)||[];for(let i=list.length-1;i>=0;i--)if(list[i]===listener||list[i].listener===listener){list.splice(i,1);break;}if(!list.length)this._events.delete(name);return this;}
        off(name,listener){return this.removeListener(name,listener);}
        removeAllListeners(name){if(name===undefined)this._events.clear();else this._events.delete(name);return this;}
        listeners(name){return (this._events.get(name)||[]).map(fn=>fn.listener||fn);}
        rawListeners(name){return (this._events.get(name)||[]).slice();}
        listenerCount(name){return (this._events.get(name)||[]).length;}
        eventNames(){return Array.from(this._events.keys());}
        setMaxListeners(n){if(n<0||Number.isNaN(n))throw new RangeError('Invalid listener limit');this._maxListeners=n;return this;}
        getMaxListeners(){return this._maxListeners;}
    }
    EventEmitter.EventEmitter=EventEmitter;globalThis.__napiVmNodeEvents={EventEmitter:EventEmitter,once:function(emitter,event){return new Promise((resolve,reject)=>{const done=(...args)=>{emitter.removeListener('error',failed);resolve(args);};const failed=error=>{emitter.removeListener(event,done);reject(error);};emitter.once(event,done);if(event!=='error')emitter.once('error',failed);});}};
    class AssertionError extends Error{constructor(message,actual,expected,operator){super(message||'Assertion failed');this.name='AssertionError';this.code='ERR_ASSERTION';this.actual=actual;this.expected=expected;this.operator=operator;}}
    function fail(actual,expected,operator,message){throw new AssertionError(message,actual,expected,operator);}
    function assert(value,message){if(!value)fail(value,true,'==',message);}
    function deep(a,b,seen){if(Object.is(a,b))return true;if(a===null||b===null||typeof a!=='object'||typeof b!=='object')return false;if(Object.getPrototypeOf(a)!==Object.getPrototypeOf(b))return false;seen=seen||new Map();if(seen.has(a))return seen.get(a)===b;seen.set(a,b);if(a instanceof Date)return Object.is(a.getTime(),b.getTime());if(a instanceof RegExp)return a.source===b.source&&a.flags===b.flags;
        if(a instanceof Number||a instanceof Boolean||a instanceof String)return Object.is(a.valueOf(),b.valueOf());
        if(a instanceof Error&&(!deep(a.name,b.name,seen)||!deep(a.message,b.message,seen)))return false;
        if(a instanceof Map||a instanceof Set){
            if(a.size!==b.size)return false;const right=Array.from(b.entries()),used=[];
            for(const entry of a.entries()){let found=false;for(let i=0;i<right.length;i++){if(!used[i]&&deep(entry[0],right[i][0],new Map(seen))&&deep(entry[1],right[i][1],new Map(seen))){used[i]=true;found=true;break;}}if(!found)return false;}return true;
        }
        if(a instanceof ArrayBuffer||a instanceof SharedArrayBuffer){if(a.byteLength!==b.byteLength)return false;const av=new Uint8Array(a),bv=new Uint8Array(b);for(let i=0;i<av.length;i++)if(av[i]!==bv[i])return false;return true;}
        if(ArrayBuffer.isView(a)){if(a.byteLength!==b.byteLength)return false;const av=new Uint8Array(a.buffer,a.byteOffset,a.byteLength),bv=new Uint8Array(b.buffer,b.byteOffset,b.byteLength);for(let i=0;i<av.length;i++)if(av[i]!==bv[i])return false;return true;}const keys=Reflect.ownKeys(a).filter(k=>Object.prototype.propertyIsEnumerable.call(a,k));if(keys.length!==Reflect.ownKeys(b).filter(k=>Object.prototype.propertyIsEnumerable.call(b,k)).length)return false;for(const key of keys)if(!Object.prototype.hasOwnProperty.call(b,key)||!deep(a[key],b[key],seen))return false;return true;}
    function matches(error,expected){if(expected===undefined)return true;if(expected instanceof RegExp)return expected.test(String(error));if(typeof expected==='function'){if(expected.prototype instanceof Error||expected===Error)return error instanceof expected;return expected(error)===true;}if(expected&&typeof expected==='object'){for(const key of Object.keys(expected))if(expected[key] instanceof RegExp?!expected[key].test(String(error[key])):!deep(error[key],expected[key]))return false;return true;}throw new TypeError('Invalid expected error');}
    assert.ok=assert;assert.AssertionError=AssertionError;assert.fail=message=>fail(undefined,undefined,'fail',message);
    assert.equal=(actual,expected,message)=>{if(actual!=expected)fail(actual,expected,'==',message);};
    assert.strictEqual=(actual,expected,message)=>{if(!Object.is(actual,expected))fail(actual,expected,'strictEqual',message);};
    assert.notStrictEqual=(actual,expected,message)=>{if(Object.is(actual,expected))fail(actual,expected,'notStrictEqual',message);};
    assert.deepStrictEqual=(actual,expected,message)=>{if(!deep(actual,expected))fail(actual,expected,'deepStrictEqual',message);};
    assert.throws=(fn,expected,message)=>{let caught=false,error;try{fn();}catch(e){caught=true;error=e;}if(!caught||!matches(error,expected))fail(error,expected,'throws',message);};
    assert.rejects=async(fn,expected,message)=>{let caught=false,error;try{await(typeof fn==='function'?fn():fn);}catch(e){caught=true;error=e;}if(!caught||!matches(error,expected))fail(error,expected,'rejects',message);};
    assert.strict=assert;globalThis.__napiVmNodeAssert=assert;
    function inspect(value){try{return typeof value==='string'?value:JSON.stringify(value);}catch(error){return '[Circular]';}}
    const util={inspect:inspect,
        format:function(format,...args){if(typeof format!=='string')return [format].concat(args).map(inspect).join(' ');let index=0;const result=format.replace(/%[sdifjoO%]/g,token=>{if(token==='%%')return '%';if(index>=args.length)return token;const value=args[index++];if(token==='%s')return String(value);if(token==='%d'||token==='%f')return String(Number(value));if(token==='%i')return String(parseInt(value,10));return inspect(value);});return result+(index<args.length?' '+args.slice(index).map(inspect).join(' '):'');},
        promisify:function(fn){return function(...args){const self=this;return new Promise((resolve,reject)=>{fn.apply(self,args.concat([(error,result)=>{if(error)reject(error);else resolve(result);} ]));});};},
        callbackify:function(fn){return function(...args){const callback=args.pop();if(typeof callback!=='function')throw new TypeError('Callback required');Promise.resolve(fn.apply(this,args)).then(value=>callback(null,value),error=>callback(error));};},
        inherits:function(ctor,parent){Object.setPrototypeOf(ctor.prototype,parent.prototype);ctor.super_=parent;}
    };globalThis.__napiVmNodeUtil=util;globalThis.__napiVmNodeBuffer={Buffer:Buffer};
})();
