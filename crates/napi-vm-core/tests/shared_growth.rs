use napi_vm_core::{Interpreter, Value};

fn truth(vm: &mut Interpreter, source: &str) {
    let result = vm.eval_source(source);
    assert!(
        matches!(result, Ok(Value::Bool(true))),
        "{source}: {result:?}"
    );
}

#[test]
fn buffer_construction_observes_new_target_before_backing_allocation_limits() {
    let mut vm = Interpreter::with_builtins();
    for constructor in ["ArrayBuffer", "SharedArrayBuffer"] {
        for arguments in ["[8388608]", "[0,{maxByteLength:8388608}]"] {
            truth(
                &mut vm,
                &format!(
                    "var marker={{}};var reads=0;var target=function(){{}}.bind(null);Object.defineProperty(target,'prototype',{{get(){{reads++;throw marker;}}}});var caught;try{{Reflect.construct({constructor},{arguments},target);}}catch(e){{caught=e;}}caught===marker&&reads===1;"
                ),
            );
        }
        truth(
            &mut vm,
            &format!(
                "var order='';var prototype={{}};var target=function(){{}}.bind(null);Object.defineProperty(target,'prototype',{{get(){{order+='p';return prototype;}}}});var buffer=Reflect.construct({constructor},[{{valueOf(){{order+='l';return 4;}}}},{{get maxByteLength(){{order+='m';return {{valueOf(){{order+='n';return 8;}}}};}}}}],target);order==='lmnp'&&Object.getPrototypeOf(buffer)===prototype&&Object.getOwnPropertyDescriptor({constructor}.prototype,'byteLength').get.call(buffer)===4;"
            ),
        );
        truth(
            &mut vm,
            &format!(
                "reads=0;target=function(){{}}.bind(null);Object.defineProperty(target,'prototype',{{get(){{reads++;return {{}};}}}});try{{Reflect.construct({constructor},[-1],target);}}catch(e){{caught=e;}}caught instanceof RangeError&&reads===0;"
            ),
        );
    }
}

#[test]
fn typed_iterators_use_internal_lengths_even_when_length_is_shadowed() {
    let mut vm = Interpreter::with_builtins();
    for source in [
        "var array=new Uint8Array([1,2]);Object.defineProperty(array,'length',{value:0});var iterator=array.values();iterator.next().value===1&&iterator.next().value===2&&iterator.next().done;",
        "var empty=new Uint8Array();Object.defineProperty(empty,'length',{get(){throw 'read';}});empty.values().next().done;",
    ] {
        truth(&mut vm, source);
    }
}

#[test]
fn growth_updates_tracking_views_preserves_fixed_views_and_zeroes_new_bytes() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "var shared=new SharedArrayBuffer(4,{maxByteLength:16});var tracking=new Int32Array(shared);var fixed=new Int32Array(shared,0,1);var view=new DataView(shared);var fixedView=new DataView(shared,0,4);tracking[0]=23;shared.grow(16);shared.growable&&shared.maxByteLength===16&&shared.byteLength===16&&tracking.length===4&&fixed.length===1&&view.byteLength===16&&fixedView.byteLength===4&&tracking[0]===23&&tracking[3]===0;",
    );
    truth(
        &mut vm,
        "Atomics.store(tracking,3,42);Atomics.load(tracking,3)===42&&view.getInt32(12,true)===42;",
    );
    truth(
        &mut vm,
        "var range=0;for(var length of [3,17,-1,Infinity]){try{shared.grow(length);}catch(e){if(e.constructor===RangeError)range++;}}range===4&&shared.byteLength===16;",
    );
    truth(
        &mut vm,
        "var ordinary=new SharedArrayBuffer(4);var failed=false;try{ordinary.grow({valueOf(){throw 'coerced';}});}catch(e){failed=e.constructor===TypeError;}failed&&!ordinary.growable&&ordinary.maxByteLength===4;",
    );
    truth(
        &mut vm,
        "var odd=new SharedArrayBuffer(3,{maxByteLength:9});var words=new Uint16Array(odd);odd.grow(9);words.length===4;",
    );
}

#[test]
fn native_growth_crosses_only_the_shared_data_block_and_preserves_wrapper_realms() {
    let mut vm = Interpreter::with_builtins();
    let Value::SharedArrayBuffer(ref shared) = vm
        .eval_source("new SharedArrayBuffer(4,{maxByteLength:16})")
        .unwrap()
    else {
        panic!("shared buffer")
    };
    let memory = shared.shared_memory().unwrap();
    let child = vm.create_realm();
    vm.set_global_checked("other", child.realm_global_object())
        .unwrap();
    vm.set_global_checked(
        "imported",
        child.shared_array_buffer_from_memory(memory.clone()),
    )
    .unwrap();
    vm.set_global_checked("shared", Value::SharedArrayBuffer(shared.clone()))
        .unwrap();
    truth(
        &mut vm,
        "var view=new Int32Array(shared);var otherView=new Int32Array(imported);view[0]=7;imported.growable&&imported.maxByteLength===16&&Object.getPrototypeOf(imported)===other.SharedArrayBuffer.prototype;",
    );
    std::thread::spawn(move || memory.grow(16).unwrap())
        .join()
        .unwrap();
    truth(
        &mut vm,
        "view.length===4&&otherView.length===4&&imported.byteLength===16&&Atomics.load(otherView,0)===7&&Atomics.load(otherView,3)===0;",
    );
    drop(child);
    assert!(vm.collect_cycles().skipped.is_none());
    truth(
        &mut vm,
        "Object.getPrototypeOf(imported)===other.SharedArrayBuffer.prototype;",
    );
}

#[test]
fn growth_coercions_and_accessor_receivers_follow_shared_buffer_semantics() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "var log='';var shared=new SharedArrayBuffer({valueOf(){log+='l';return 4;}},{get maxByteLength(){log+='m';return {valueOf(){log+='v';return 16;}};}});shared.grow({valueOf(){log+='g';return 8;}});log==='lmvg'&&shared.byteLength===8;",
    );
    truth(
        &mut vm,
        "var checked=0;for(var name of ['byteLength','maxByteLength','growable']){var descriptor=Object.getOwnPropertyDescriptor(SharedArrayBuffer.prototype,name);try{descriptor.get.call({});}catch(e){if(e.constructor===TypeError)checked++;}}checked===3;",
    );
    truth(
        &mut vm,
        "var errors=0;try{SharedArrayBuffer(1);}catch(e){if(e.constructor===TypeError)errors++;}try{new SharedArrayBuffer(4,{maxByteLength:3});}catch(e){if(e.constructor===RangeError)errors++;}try{new Uint32Array(shared,0,Number.MAX_SAFE_INTEGER);}catch(e){if(e.constructor===RangeError)errors++;}errors===3;",
    );
}

#[test]
fn tracking_subarrays_and_iterators_observe_growth_without_restarting_exhausted_iterators() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "var shared=new SharedArrayBuffer(4,{maxByteLength:16});var view=new Int32Array(shared);view[0]=7;var values=view.values();var keys=view.keys();var entries=view.entries();var sub=view.subarray(0);var fixedSub=view.subarray(0,1);var first=values.next().value;keys.next();entries.next();shared.grow(16);Atomics.store(view,3,42);first===7&&sub.length===4&&fixedSub.length===1&&keys.next().value===1&&values.next().value===0&&entries.next().value[0]===1;",
    );
    truth(
        &mut vm,
        "values.next();values.next().value===42&&values.next().done&&view.values===view[Symbol.iterator];",
    );
    truth(
        &mut vm,
        "var emptyShared=new SharedArrayBuffer(0,{maxByteLength:8});var emptyView=new Int32Array(emptyShared);var exhausted=emptyView.values();var done=exhausted.next().done;emptyShared.grow(8);done&&exhausted.next().done&&emptyView.values().next().value===0;",
    );
}

#[test]
fn resizable_buffers_preserve_view_identity_and_restore_bounds_after_regrowth() {
    let mut vm = Interpreter::with_builtins();
    truth(
        &mut vm,
        "var buffer=new ArrayBuffer(8,{maxByteLength:16});var tracking=new Uint8Array(buffer,4);var fixed=new Uint8Array(buffer,4,4);var data=new DataView(buffer,4);fixed[0]=9;buffer.resize(6);tracking.length===2&&fixed.length===0&&fixed.byteOffset===0&&fixed[0]===undefined&&data.byteLength===2&&buffer.resizable&&buffer.maxByteLength===16;",
    );
    truth(
        &mut vm,
        "buffer.resize(2);var failed=false;try{data.byteLength;}catch(e){failed=e.constructor===TypeError;}failed&&tracking.length===0&&tracking.byteOffset===0;",
    );
    truth(
        &mut vm,
        "buffer.resize(16);tracking.length===12&&fixed.length===4&&fixed.byteOffset===4&&fixed[0]===0&&data.byteLength===12;",
    );
    let buffer = vm.global_value("buffer").unwrap();
    let Value::ArrayBuffer(ref buffer) = buffer else {
        panic!("array buffer")
    };
    buffer.detach();
    truth(
        &mut vm,
        "buffer.detached&&buffer.resizable&&buffer.maxByteLength===0&&tracking.length===0&&fixed.length===0;",
    );
}
