(function () {
    const hostFetch = typeof __napiVmFetch === 'function' ? __napiVmFetch : null;
    const copyBuffer = __napiVmCopyBuffer;
    const hostAbort = typeof __napiVmAbort === 'function' ? __napiVmAbort : null;
    class Headers {
        constructor(init) {
            this._pairs = [];
            if (init instanceof Headers) init = init._pairs;
            if (Array.isArray(init)) {
                for (const pair of init) {
                    if (pair.length !== 2) throw new TypeError('Header pair requires two values');
                    this.append(pair[0], pair[1]);
                }
            } else if (init) {
                for (const key of Object.keys(init)) this.append(key, init[key]);
            }
        }
        _name(name) {
            name = String(name).toLowerCase();
            if (!/^[!#$%&'*+.^_`|~0-9a-z-]+$/.test(name)) throw new TypeError('Invalid header name');
            return name;
        }
        append(name, value) {
            name = this._name(name); value = String(value).trim();
            if (value.includes('\r') || value.includes('\n') || value.includes('\0')) throw new TypeError('Invalid header value');
            const old = this._pairs.findIndex(p => p[0] === name);
            if (old < 0) this._pairs.push([name, value]);
            else this._pairs[old][1] += ', ' + value;
        }
        set(name, value) { this.delete(name); this.append(name, value); }
        get(name) { name = this._name(name); const p = this._pairs.find(p => p[0] === name); return p ? p[1] : null; }
        has(name) { return this.get(name) !== null; }
        delete(name) { name = this._name(name); this._pairs = this._pairs.filter(p => p[0] !== name); }
        entries() { return this._pairs.slice().sort((a,b) => a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)[Symbol.iterator](); }
        keys() { return this._pairs.map(p => p[0]).sort()[Symbol.iterator](); }
        values() { return Array.from(this.entries()).map(p => p[1])[Symbol.iterator](); }
        [Symbol.iterator]() { return this.entries(); }
        forEach(callback, thisArg) { for (const p of this.entries()) callback.call(thisArg, p[1], p[0], this); }
    }
    class AbortSignal {
        constructor() { this.aborted = false; this.reason = undefined; this.onabort = null; this._listeners = []; }
        addEventListener(name, fn, options) { if (name === 'abort' && typeof fn === 'function') this._listeners.push([fn, options && options.once]); }
        removeEventListener(name, fn) { if (name === 'abort') this._listeners = this._listeners.filter(p => p[0] !== fn); }
        throwIfAborted() { if (this.aborted) throw this.reason; }
        static abort(reason) { const c = new AbortController(); c.abort(reason); return c.signal; }
    }
    class AbortController {
        constructor() { this.signal = new AbortSignal(); }
        abort(reason) {
            const s = this.signal; if (s.aborted) return;
            if (reason === undefined) { reason = new Error('The operation was aborted'); reason.name = 'AbortError'; }
            s.aborted = true; s.reason = reason;
            const event = {type:'abort', target:s};
            const listeners = s._listeners.slice();
            s._listeners = s._listeners.filter(p => !p[1]);
            for (const p of listeners) p[0].call(s, event);
            if (typeof s.onabort === 'function') s.onabort.call(s, event);
        }
    }
    function bytes(body) {
        if (body === undefined || body === null) return new Uint8Array(0);
        if (body instanceof ArrayBuffer) return new Uint8Array(copyBuffer(body));
        if (ArrayBuffer.isView(body)) return new Uint8Array(copyBuffer(body));
        return new TextEncoder().encode(String(body));
    }
    class Body {
        _init(body) { this._bytes = bytes(body); this.bodyUsed = false; }
        _consume() {
            if (this.bodyUsed) return Promise.reject(new TypeError('Body already consumed'));
            this.bodyUsed = true; return Promise.resolve(this._bytes);
        }
        text() { return this._consume().then(b => new TextDecoder().decode(new Uint8Array(b))); }
        json() { return this.text().then(t => JSON.parse(t)); }
        arrayBuffer() { return this._consume().then(b => b.buffer); }
    }
    class Request extends Body {
        constructor(input, init) {
            super(); init = init || {};
            const original = input instanceof Request ? input : null;
            this.url = String(original ? original.url : input);
            this.method = String(init.method || (original && original.method) || 'GET').toUpperCase();
            if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(this.method) || ['CONNECT','TRACE','TRACK'].includes(this.method)) throw new TypeError('Invalid HTTP method');
            this.headers = new Headers(init.headers !== undefined ? init.headers : original && original.headers);
            this.signal = init.signal || (original && original.signal) || new AbortSignal();
            this.redirect = init.redirect || (original && original.redirect) || 'follow';
            if (!['follow','error','manual'].includes(this.redirect)) throw new TypeError('Invalid redirect mode');
            const body = init.body !== undefined ? init.body : original ? new Uint8Array(original._bytes) : null;
            if (body !== null && (this.method === 'GET' || this.method === 'HEAD')) throw new TypeError('GET/HEAD cannot have a body');
            this._init(body);
        }
        clone() { if (this.bodyUsed) throw new TypeError('Body already consumed'); return new Request(this); }
    }
    class Response extends Body {
        constructor(body, init) {
            super(); init = init || {};
            this.status = init.status === undefined ? 200 : Number(init.status);
            if (this.status < 200 || this.status > 599 || this.status !== Math.floor(this.status)) throw new RangeError('Invalid response status');
            if (body != null && [204,205,304].includes(this.status)) throw new TypeError('Null-body status');
            this.statusText = String(init.statusText || '');
            this.headers = new Headers(init.headers); this.url = ''; this.redirected = false; this.type = 'default';
            this.ok = this.status >= 200 && this.status < 300; this._init(body);
        }
        clone() {
            if (this.bodyUsed) throw new TypeError('Body already consumed');
            const r = new Response(null, {status:this.status || 200, statusText:this.statusText, headers:this.headers});
            r._bytes = new Uint8Array(copyBuffer(this._bytes)); r.url=this.url; r.redirected=this.redirected; r.type=this.type; r.status=this.status; r.ok=this.ok; return r;
        }
        static json(data, init) { const r = new Response(JSON.stringify(data), init); if (!r.headers.has('content-type')) r.headers.set('content-type','application/json'); return r; }
        static redirect(url, status) {
            status = status === undefined ? 302 : status;
            if (![301,302,303,307,308].includes(status)) throw new RangeError('Invalid redirect status');
            return new Response(null, {status:status, headers:{location:String(url)}});
        }
        static error() { const r=new Response(); r.status=0; r.ok=false; r.type='error'; return r; }
    }
    globalThis.Headers = Headers; globalThis.Request = Request; globalThis.Response = Response;
    globalThis.AbortSignal = AbortSignal; globalThis.AbortController = AbortController;
    if (hostFetch) globalThis.fetch = function(input, init) {
        let request, operation;
        try {
            request = new Request(input, init); request.signal.throwIfAborted();
            operation = hostFetch(JSON.stringify({url:request.url, method:request.method, headers:request.headers._pairs,
                redirect:request.redirect}), request._bytes.length?request._bytes:undefined);
        } catch (error) { return Promise.reject(error); }
        return new Promise((resolve,reject) => {
            const abort = () => { hostAbort(operation.id); reject(request.signal.reason); };
            request.signal.addEventListener('abort', abort, {once:true});
            operation.promise.then(result => {
                request.signal.removeEventListener('abort', abort);
                if (request.signal.aborted) return;
                const response = new Response(null, {status:result.status, statusText:result.status_text, headers:result.headers});
                response._bytes = new Uint8Array(result.bytes); response.url=result.url; response.redirected=result.redirected; response.type='basic'; resolve(response);
            }, error => { request.signal.removeEventListener('abort', abort); reject(new TypeError(String(error))); });
        });
    };
})();
(function () {
    if (typeof __napiVmSocketOpen !== 'function') return;
    const open=__napiVmSocketOpen, next=__napiVmSocketNext, send=__napiVmSocketSend, close=__napiVmSocketClose, release=__napiVmSocketRelease;
    function encode(data) {
        if (data instanceof ArrayBuffer) return new Uint8Array(data);
        if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer,data.byteOffset,data.byteLength);
        return String(data);
    }
    class WebSocket {
        constructor(url, protocols) {
            this.url=String(url); this.readyState=0; this.protocol=''; this.extensions=''; this.binaryType='arraybuffer'; this.bufferedAmount=0;
            this.onopen=null;this.onmessage=null;this.onerror=null;this.onclose=null;this._listeners={};this._handle=null;
            protocols=typeof protocols==='string'?[protocols]:(protocols||[]);
            if (!Array.isArray(protocols)) throw new TypeError('WebSocket protocols must be a sequence');
            for (let i=0;i<protocols.length;i++) {
                protocols[i]=String(protocols[i]);
                if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(protocols[i]) || protocols.indexOf(protocols[i])!==i) throw new TypeError('Invalid or duplicate WebSocket protocol');
            }
            const operation=open(JSON.stringify({kind:'websocket',url:this.url,protocols:protocols}));
            this._handle=operation.id;
            operation.promise.then(info=>{
                this.protocol=info.protocol;
                if(this.readyState===2){ close(this._handle,1000,''); this._read(); return; }
                this.readyState=1; this._emit('open',{}); this._read();
            },error=>{this._emit('error',{message:String(error)});this._finish({code:1006,reason:'',wasClean:false});});
        }
        addEventListener(type,fn,options){if(typeof fn==='function'){if(!this._listeners[type])this._listeners[type]=[];this._listeners[type].push([fn,options&&options.once]);}}
        removeEventListener(type,fn){if(this._listeners[type])this._listeners[type]=this._listeners[type].filter(p=>p[0]!==fn);}
        _emit(type,event){
            event.type=type;event.target=this;
            const list=(this._listeners[type]||[]).slice();this._listeners[type]=(this._listeners[type]||[]).filter(p=>!p[1]);
            for(const p of list)p[0].call(this,event);
            const handler=this['on'+type];if(typeof handler==='function')handler.call(this,event);
        }
        _finish(event){if(this.readyState===3)return;this.readyState=3;release(this._handle);this._emit('close',event);}
        _read(){
            if(this.readyState===3)return;
            let promise;
            try {promise=next(this._handle);}catch(error){this._emit('error',{message:String(error)});this._finish({code:1006,reason:'',wasClean:false});return;}
            promise.then(event=>{
                if(event.type==='close'){this._finish(event);return;}
                if(this.readyState===1){this._emit('message',{data:event.binary?new Uint8Array(event.bytes).buffer:event.data});}
                this._read();
            },error=>{this._emit('error',{message:String(error)});this._finish({code:1006,reason:'',wasClean:false});});
        }
        send(data){
            if(this.readyState===0)throw new Error('InvalidStateError: socket is connecting');
            if(this.readyState!==1)return;
            const encoded=encode(data);const size=typeof encoded==='string'?new TextEncoder().encode(encoded).length:encoded.length;
            this.bufferedAmount+=size;
            let promise;
            try{promise=typeof encoded==='string'?send(this._handle,JSON.stringify(encoded)):send(this._handle,undefined,encoded);}catch(error){this.bufferedAmount-=size;throw error;}
            promise.then(()=>{this.bufferedAmount-=size;},error=>{this.bufferedAmount-=size;this._emit('error',{message:String(error)});this._finish({code:1006,reason:'',wasClean:false});});
        }
        close(code,reason){
            code=code===undefined?1000:Number(code);reason=reason===undefined?'':String(reason);
            if(code!==1000 && (code<3000 || code>4999 || code!==Math.floor(code)))throw new RangeError('Invalid close code');
            if(new TextEncoder().encode(reason).length>123)throw new RangeError('WebSocket close reason exceeds 123 bytes');
            if(this.readyState>=2)return;this.readyState=2;
            close(this._handle,code,reason);
        }
    }
    for (const entry of [['CONNECTING',0],['OPEN',1],['CLOSING',2],['CLOSED',3]]){
        Object.defineProperty(WebSocket,entry[0],{value:entry[1]});Object.defineProperty(WebSocket.prototype,entry[0],{value:entry[1]});
    }
    globalThis.WebSocket=WebSocket;
    if(typeof napiVm==='undefined')globalThis.napiVm={};
    napiVm.connect=function(options){
        let operation;
        try{operation=open(JSON.stringify({kind:'tcp',host:String(options.hostname),port:Number(options.port)}));}catch(error){return Promise.reject(error);}
        return operation.promise.then(info=>{
            let closed=false;
            return {
                read:function(){if(closed)return Promise.resolve(null);return next(info.id).then(event=>{if(event.type==='close'){closed=true;release(info.id);return null;}return new Uint8Array(event.bytes);});},
                write:function(data){if(closed)return Promise.reject(new Error('Socket is closed'));const encoded=encode(data);return typeof encoded==='string'?send(info.id,JSON.stringify(encoded)):send(info.id,undefined,encoded);},
                close:function(){if(closed)return;closed=true;close(info.id,1000,'');release(info.id);}
            };
        });
    };
})();
(function () {
    const random=__napiVmRandom,digest=__napiVmDigest;
    globalThis.crypto={
        getRandomValues:function(view){
            if(!ArrayBuffer.isView(view) || view instanceof DataView || view instanceof Float32Array || view instanceof Float64Array)throw new TypeError('Expected an integer TypedArray');
            if(view.byteLength>65536)throw new Error('QuotaExceededError: random byte count exceeds 65536');
            const target=new Uint8Array(view.buffer,view.byteOffset,view.byteLength);target.set(random(view.byteLength));return view;
        },
        randomUUID:function(){
            const bytes=this.getRandomValues(new Uint8Array(16));bytes[6]=(bytes[6]&15)|64;bytes[8]=(bytes[8]&63)|128;
            let result='';for(let i=0;i<16;i++){if([4,6,8,10].includes(i))result+='-';result+=bytes[i].toString(16).padStart(2,'0');}return result;
        },
        subtle:{digest:function(algorithm,data){
            try{
                const name=String(typeof algorithm==='string'?algorithm:algorithm.name).toUpperCase();
                let bytes;if(data instanceof ArrayBuffer)bytes=new Uint8Array(data);else if(ArrayBuffer.isView(data))bytes=new Uint8Array(data.buffer,data.byteOffset,data.byteLength);else throw new TypeError('Expected BufferSource');
                return Promise.resolve(new Uint8Array(digest(name,bytes)).buffer);
            }catch(error){return Promise.reject(error);}
        }}
    };
})();
(function(){
    if(typeof __napiVmSocketOpen!=='function')return;
    const open=__napiVmSocketOpen,next=__napiVmSocketNext,send=__napiVmSocketSend,close=__napiVmSocketClose,release=__napiVmSocketRelease;
    napiVm.serve=function(options,handler){
        if(typeof handler!=='function')return Promise.reject(new TypeError('HTTP handler must be a function'));
        let operation;try{operation=open(JSON.stringify({kind:'http-server',host:String(options.hostname||'127.0.0.1'),port:Number(options.port)}));}catch(error){return Promise.reject(error);}
        return operation.promise.then(info=>{
            let stopped=false,finish,fail;
            const finished=new Promise((resolve,reject)=>{finish=resolve;fail=reject;});
            function read(){
                if(stopped)return;
                let pending;try{pending=next(info.id);}catch(error){release(info.id);fail(error);return;}
                pending.then(event=>{
                    if(event.type==='close'){stopped=true;release(info.id);finish();return;}
                    read();
                    const request=new Request(event.url,{method:event.method,headers:event.headers,body:event.bytes.byteLength?new Uint8Array(event.bytes):null});
                    Promise.resolve().then(()=>handler(request)).catch(()=>new Response('Internal Server Error',{status:500})).then(response=>{
                        if(!(response instanceof Response))response=new Response('Invalid HTTP response',{status:500});
                        const body=request.method==='HEAD'||[204,205,304].includes(response.status)?new Uint8Array(0):response._bytes;
                        return send(info.id,JSON.stringify({status:response.status,headers:response.headers._pairs}),body); 
                    }).catch(error=>{stopped=true;release(info.id);fail(error);});
                },error=>{stopped=true;release(info.id);fail(error);});
            }
            read();
            return {addr:{hostname:String(options.hostname||'127.0.0.1'),port:info.port},finished:finished,
                shutdown:function(){if(stopped)return;close(info.id,1000,'');}};
        });
    };
})();
