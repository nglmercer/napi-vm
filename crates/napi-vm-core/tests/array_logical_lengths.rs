use napi_vm_core::{Interpreter, Value};

fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}

#[test]
fn slice_reads_logical_tail_and_preserves_holes() {
    yes(
        "var max=9007199254740991;var o={length:Infinity,[max-2]:'x'};var r=Array.prototype.slice.call(o,-2);r.length===2 && r[0]==='x' && !(1 in r)",
    );
}

#[test]
fn splice_shrinks_and_grows_at_the_safe_integer_boundary() {
    yes(
        "var max=9007199254740991;var o={length:Infinity,[max-4]:'a',[max-3]:'b',[max-1]:'c'};var r=Array.prototype.splice.call(o,max-4,1);r.length===1 && r[0]==='a' && o.length===max-1 && o[max-4]==='b' && !(max-3 in o) && o[max-2]==='c' && !(max-1 in o)",
    );
    yes(
        "var max=9007199254740991;var o={length:max-1,[max-2]:'tail'};var r=Array.prototype.splice.call(o,max-2,0,'new');r.length===0 && o.length===max && o[max-2]==='new' && o[max-1]==='tail'",
    );
}

#[test]
fn splice_noop_clamps_length_and_overflow_precedes_species() {
    yes(
        "var max=9007199254740991;var o={length:Infinity};var r=Array.prototype.splice.call(o);o.length===max && r.length===0",
    );
    yes(
        "var seen=0;var a=new Proxy([],{get(t,k,r){if(k==='length')return Infinity;if(k==='constructor')seen++;return Reflect.get(t,k,r);}});var caught=false;try{a.splice(9007199254740991,0,'x');}catch(e){caught=e instanceof TypeError;}caught && seen===0",
    );
}

#[test]
fn custom_species_receives_large_counts_before_index_access() {
    yes(
        "var max=9007199254740991;var stop={};var counts=[];var gets=[];var a=new Proxy([],{get(t,k,r){if(k==='length')return Infinity;return Reflect.get(t,k,r);},has(t,k){gets.push(k);throw stop;}});a.constructor={[Symbol.species]:function(n){counts.push(n);return {};}};var ok=true;for(var method of ['slice','splice']){try{Array.prototype[method].call(a,0,max);}catch(e){ok=ok && e===stop;}}ok && counts.join(',')===max+','+max && gets.join(',')==='0,0'",
    );
}

#[test]
fn push_preserves_writes_before_invalid_array_length() {
    yes(
        "var a=[];a.length=4294967295;var n=a.push();var caught=false;try{a.push('written');}catch(e){caught=e instanceof RangeError;}n===4294967295 && caught && a[4294967295]==='written' && a.length===4294967295 && a.unshift()===4294967295",
    );
    yes(
        "var a=[];a[999999]='tail';a.push('new')===1000001 && a[1000000]==='new' && a.length===1000001",
    );
}

#[test]
fn concat_checks_safe_integer_overflow_before_spread_access() {
    yes(
        "var reads=0;var spread=new Proxy({length:Infinity,[Symbol.isConcatSpreadable]:true},{has(){reads++;throw 7;}});var caught=false;try{[1].concat(spread);}catch(e){caught=e instanceof TypeError;}caught && reads===0",
    );
}
