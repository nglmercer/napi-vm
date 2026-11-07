use napi_vm::{Interpreter, Value};

fn yes(vm: &mut Interpreter, source: &str) {
    let result = vm.eval_source(source);
    assert!(
        matches!(result, Ok(Value::Bool(true))),
        "{source}: {result:?}"
    );
}

#[test]
fn indexing_preserves_updates_deletions_reinsertion_order_and_clear() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        r#"
        var m=new Map();var s=new Set();
        for(var i=0;i<100;i++){m.set(i,i);s.add(i);}
        for(var i=0;i<100;i+=2){m.delete(i);s.delete(i);}
        for(var i=1;i<100;i+=2)m.set(i,2*i);
        for(var i=0;i<100;i+=2){m.set(i,-i);s.add(i);}
        var expected=[];for(var i=1;i<100;i+=2)expected.push(i);for(var i=0;i<100;i+=2)expected.push(i);
        var ok=m.size===100&&s.size===100&&[...m.keys()].join()===expected.join()&&[...s].join()===expected.join();
        for(var i=0;i<100;i++)ok=ok&&m.get(i)===(i%2?2*i:-i)&&s.has(i);
        m.clear();s.clear();ok=ok&&m.size===0&&s.size===0&&!m.has(99)&&!s.has(99);
        m.set(7,42);s.add(7);ok&&m.size===1&&s.size===1&&m.get(7)===42&&[...m.keys()].join()==='7'&&[...s].join()==='7';
    "#,
    );
}

#[test]
fn indexed_keys_use_same_value_zero_and_reference_identity_without_coercion() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        r#"
        var traps=0;var revoked=Proxy.revocable({},{});var proxy=revoked.proxy;revoked.revoke();
        var object={toString(){traps++;throw 1;},valueOf(){traps++;throw 2;}};
        var buffer=new ArrayBuffer(8);var symbol=Symbol('x');var otherSymbol=Symbol('x');
        var keys=[undefined,null,true,false,NaN,-0,1,1n,123456789012345678901234567890n,
            'key','\ud800',symbol,otherSymbol,object,{},[],function(){},class C{},
            new Date(0),/x/,buffer,new Uint8Array(buffer),new DataView(buffer),new Error('x'),
            Promise.resolve(1),(function*(){yield 1;})(),globalThis,proxy,Math.abs];
        var m=new Map();for(var i=0;i<64;i++)m.set('filler'+i,i);
        var s=new Set();for(var i=0;i<64;i++)s.add('filler'+i);
        for(var i=0;i<keys.length;i++){m.set(keys[i],i);s.add(keys[i]);}
        var ok=true;for(var i=0;i<keys.length;i++)ok=ok&&m.get(keys[i])===i&&s.has(keys[i]);
        ok=ok&&m.get(0)===5&&m.get(Number('x'))===4&&m.get(BigInt('123456789012345678901234567890'))===8;
        m.set(0,99);s.add(0);ok=ok&&m.get(-0)===99&&m.size===64+keys.length&&s.size===64+keys.length;
        ok&&!m.has({})&&!m.has(Symbol('x'))&&!m.has(function(){})&&traps===0;
    "#,
    );
}

#[test]
fn live_iterators_survive_index_activation_clear_and_reinsertion() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        r#"
        var m=new Map([[0,0]]);var iterator=m.keys();var ok=iterator.next().value===0;
        for(var i=1;i<80;i++)m.set(i,i);
        m.delete(1);m.set(1,100);ok=ok&&iterator.next().value===2;
        m.clear();m.set(100,42);var step=iterator.next();ok=ok&&!step.done&&step.value===100&&iterator.next().done;
        m.set(101,43);ok=ok&&iterator.next().done;
        var s=new Set();for(var i=0;i<64;i++)s.add(i);var seen=[];
        s.forEach(function(value){seen.push(value);if(value===0){s.delete(1);s.add(100);}if(value===2){s.clear();s.add(200);}});
        ok&&seen.join()==='0,2,200'&&s.size===1&&s.has(200);
    "#,
    );
}

#[test]
fn set_composition_and_group_by_populate_indexes_and_live_counts() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        r#"
        var a=new Set();var b=new Set();for(var i=0;i<100;i++){a.add(i);b.add(i+50);}
        var union=a.union(b),intersection=a.intersection(b),difference=a.difference(b),symmetric=a.symmetricDifference(b);
        var ok=union.size===150&&intersection.size===50&&difference.size===50&&symmetric.size===100;
        for(var i=0;i<150;i++)ok=ok&&union.has(i)&&intersection.has(i)===(i>=50&&i<100)&&difference.has(i)===(i<50)&&symmetric.has(i)===(i<50||i>=100);
        var input=Array.from({length:100},(_,i)=>i);var groups=Map.groupBy(input,x=>x%40);
        ok=ok&&groups.size===40&&groups.get(0).join()==='0,40,80'&&groups.get(39).join()==='39,79';
        groups.delete(0);groups.set(0,['new']);ok&&groups.size===40&&[...groups.keys()][39]===0;
    "#,
    );
}

#[test]
fn computed_upsert_rechecks_mutations_on_indexed_maps() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        r#"
        var m=new Map();for(var i=0;i<64;i++)m.set(i,i);
        var answer=m.getOrInsertComputed('new',function(key){m.clear();m.set(key,1);return 42;});
        var calls=0;var existing=m.getOrInsertComputed('new',function(){calls++;return 0;});
        answer===42&&existing===42&&calls===0&&m.size===1&&m.get('new')===42&&[...m.keys()].join()==='new';
    "#,
    );
}

#[test]
fn index_does_not_retain_deleted_keys_and_collection_entries_remain_gc_roots() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(r#"
        var m=new Map();var s=new Set();for(var i=0;i<64;i++){m.set(i,i);s.add(i);}
        var key={answer:42};key.self=key;m.set(key,{key:key});s.add(key);var ref=new WeakRef(key);key=undefined;
    "#).unwrap();
    assert!(vm.collect_cycles().skipped.is_none());
    yes(
        &mut vm,
        "ref.deref().answer===42&&m.get(ref.deref()).key===ref.deref()&&s.has(ref.deref());",
    );
    vm.eval_source("m.delete(ref.deref());s.delete(ref.deref());")
        .unwrap();
    assert!(vm.collect_cycles().collected > 0);
    yes(
        &mut vm,
        "ref.deref()===undefined&&m.size===64&&s.size===64;",
    );
}
