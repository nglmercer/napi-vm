use napi_vm_core::{Interpreter, Value, parser::parse_cached};

fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    let result = vm.eval_source(source);
    assert!(
        matches!(result, Ok(Value::Bool(true))),
        "prepared {source}: {result:?}"
    );
    let mut vm = Interpreter::with_builtins();
    let result = vm.run_program_body(&parse_cached(source).unwrap());
    assert!(
        matches!(result, Ok(Value::Bool(true))),
        "AST {source}: {result:?}"
    );
}

#[test]
fn date_to_temporal_instant_preserves_exact_slots_and_intrinsics() {
    yes(r#"
        var good=true;
        for(var ms of [-8640000000000000,-1,0,1,8640000000000000]) {
            var date=new Date(ms);
            date.valueOf=()=>{throw new Error('must not coerce');};
            var instant=date.toTemporalInstant();
            good=good && instant.epochNanoseconds===BigInt(ms)*1000000n;
        }
        var desc=Object.getOwnPropertyDescriptor(Date.prototype,'toTemporalInstant');
        var method=desc.value;
        Temporal.Instant=function(){throw new Error('replaced');};
        good && new Date(1).toTemporalInstant().epochNanoseconds===1000000n &&
        method.name==='toTemporalInstant' && method.length===0 &&
        desc.writable && desc.configurable && !desc.enumerable && !Object.hasOwn(method,'prototype')
    "#);
}

#[test]
fn date_to_temporal_instant_rejects_invalid_dates_and_foreign_brands() {
    yes(r#"
        var method=Date.prototype.toTemporalInstant, count=0;
        for(var receiver of [undefined,null,true,0,'',{},Object.create(Date.prototype),new Proxy(new Date(0),{})]) {
            try {method.call(receiver);} catch(e){if(e instanceof TypeError)count++;}
        }
        try {new Date(NaN).toTemporalInstant();} catch(e){if(e instanceof RangeError)count++;}
        count===9
    "#);
}

#[test]
fn proxy_descriptor_output_order_is_independent_of_observable_input_reads() {
    yes(r#"
        var log=[], keys;
        var descriptor=new Proxy({configurable:true,set:undefined},{
            has(t,k){log.push('has '+k);return Reflect.has(t,k);},
            get(t,k){log.push('get '+k);return Reflect.get(t,k);}
        });
        Object.defineProperty(new Proxy({}, {defineProperty(t,k,d){keys=Object.keys(d).join(','); return true;}}),'x',descriptor);
        keys==='set,configurable' && log.join(',')===
        'has enumerable,has configurable,get configurable,has value,has writable,has get,has set,get set'
    "#);
    yes(r#"
        var results=[];
        for(var descriptor of [{configurable:true,value:1,writable:true,enumerable:false},{configurable:true,get:undefined,set:undefined,enumerable:true}]) {
            Reflect.defineProperty(new Proxy({}, {defineProperty(t,k,d){results.push(Object.keys(d).join(','));return true;}}),'x',descriptor);
        }
        results.join('|')==='value,writable,enumerable,configurable|get,set,enumerable,configurable'
    "#);
}
