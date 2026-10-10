use napi_vm_core::{Interpreter, Value};
fn run(vm: &mut Interpreter, text: &str) {
    vm.eval_source(text).unwrap();
}
fn yes(vm: &mut Interpreter, text: &str) {
    let value = vm
        .eval_source(text)
        .unwrap_or_else(|error| panic!("{text}: {error}"));
    assert!(matches!(value, Value::Bool(true)), "{text}: {value:?}");
}
#[test]
fn private_field_chains_drop_without_native_recursion() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "class Node{#next;constructor(next){this.#next=next;}}var head;for(var i=0;i<10000;i++){head=new Node(head);}head=undefined;",
    );
    assert!(vm.collect_cycles().skipped.is_none());
}

#[test]
fn private_fields_and_exotic_properties_trace_their_children() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "class Holder{#value={answer:42};read(){return this.#value;}}var holder=new Holder();var child=holder.read();child.self=child;var privateRef=new WeakRef(child);child=undefined;var buffer=new ArrayBuffer(4);buffer.child={answer:7};buffer.child.self=buffer.child;var view=new Uint8Array(buffer);var bufferRef=new WeakRef(buffer.child);buffer=undefined;",
    );
    assert!(vm.collect_cycles().skipped.is_none());
    yes(
        &mut vm,
        "holder.read()===privateRef.deref()&&view.buffer.child===bufferRef.deref()",
    );
    run(&mut vm, "holder=undefined;view=undefined;");
    assert!(vm.collect_cycles().collected > 0);
    yes(
        &mut vm,
        "privateRef.deref()===undefined&&bufferRef.deref()===undefined",
    );
}

#[test]
fn escaped_constructor_arrows_trace_and_release_the_bound_receiver() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "class Base{}class Child extends Base{constructor(){super();this.answer=42;this.self=this;return ()=>this;}}var get=new Child();var ref=new WeakRef(get());",
    );
    assert!(vm.collect_cycles().skipped.is_none());
    yes(&mut vm, "get()===ref.deref()&&get().answer===42");
    run(&mut vm, "get=undefined;");
    assert!(vm.collect_cycles().collected > 0);
    yes(&mut vm, "ref.deref()===undefined");
}

#[test]
fn weak_map_value_backreferences_do_not_keep_dead_keys_alive() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "var map=new WeakMap();var key={};key.self=key;map.set(key,{key});var ref=new WeakRef(key);key=undefined;",
    );
    let stats = vm.collect_cycles();
    assert!(stats.skipped.is_none(), "{stats:?}");
    assert!(stats.collected > 0, "{stats:?}");
    yes(&mut vm, "ref.deref()===undefined");
}
#[test]
fn ephemeron_values_stay_live_and_reach_a_fixed_point() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "var first=new WeakMap();var second=new WeakMap();var key={};var intermediate={};var value={answer:42};value.self=value;first.set(key,intermediate);second.set(intermediate,value);var ref=new WeakRef(value);intermediate=undefined;value=undefined;",
    );
    assert!(vm.collect_cycles().skipped.is_none());
    yes(
        &mut vm,
        "ref.deref().answer===42 && second.get(first.get(key)).answer===42",
    );
    run(&mut vm, "key=undefined;");
    assert!(vm.collect_cycles().collected > 0);
    yes(&mut vm, "ref.deref()===undefined");
}
#[test]
fn weak_set_does_not_retain_keys_and_invalid_keys_fail() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "var set=new WeakSet();var key={};key.self=key;set.add(key);var ref=new WeakRef(key);key=undefined;",
    );
    vm.collect_cycles();
    yes(
        &mut vm,
        "ref.deref()===undefined && set.has(42)===false && set.delete(42)===false",
    );
    yes(
        &mut vm,
        "var caught=false;try{set.add(42);}catch(e){caught=e.name==='TypeError';}caught",
    );
}
#[test]
fn finalization_is_queued_once_and_holdings_survive_collection() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "var count=0;var answer;var registry=new FinalizationRegistry(held=>{count++;answer=held.answer;});var target={};target.self=target;var held={answer:42};held.self=held;registry.register(target,held);target=undefined;held=undefined;",
    );
    assert!(vm.collect_cycles().skipped.is_none());
    assert!(matches!(
        vm.global.borrow().get("count"),
        Some(Value::Number(0.))
    ));
    vm.collect_cycles();
    vm.drain_jobs().unwrap();
    yes(&mut vm, "count===1 && answer===42");
    vm.collect_cycles();
    vm.drain_jobs().unwrap();
    yes(&mut vm, "count===1");
}
#[test]
fn unregister_removes_all_matching_records_and_dead_registries_do_not_run() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "var count=0;var registry=new FinalizationRegistry(()=>{count++;});var target={};target.self=target;var token={};registry.register(target,1,token);registry.register(target,2,token);",
    );
    yes(
        &mut vm,
        "registry.unregister(token)===true && registry.unregister(token)===false",
    );
    run(&mut vm, "target=undefined;token=undefined;");
    vm.collect_cycles();
    vm.drain_jobs().unwrap();
    yes(&mut vm, "count===0");
    run(
        &mut vm,
        "target={};target.self=target;registry.register(target,3);registry=undefined;target=undefined;",
    );
    vm.collect_cycles();
    vm.drain_jobs().unwrap();
    yes(&mut vm, "count===0");
}
#[test]
fn nonregistered_symbols_and_buffer_views_are_traced_as_weak_keys() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "var map=new WeakMap();var symbol=Symbol('local');var object={[symbol]:1};var symbolRef=new WeakRef(symbol);map.set(symbol,{answer:42});var buffer=new ArrayBuffer(4);var view=new Uint8Array(buffer);map.set(buffer,{answer:7});var ref=new WeakRef(buffer);symbol=undefined;buffer=undefined;",
    );
    vm.collect_cycles();
    yes(
        &mut vm,
        "map.get(symbolRef.deref()).answer===42 && map.get(ref.deref()).answer===7",
    );
    yes(
        &mut vm,
        "var caught=false;try{map.set(Symbol.for('registered'),1);}catch(e){caught=e.name==='TypeError';}caught",
    );
    run(&mut vm, "object=undefined;view=undefined;");
    vm.collect_cycles();
    yes(&mut vm, "ref.deref()===undefined");
}
#[test]
fn a_mutably_borrowed_root_aborts_collection_without_losing_children() {
    let mut vm = Interpreter::with_builtins();
    let value = vm
        .eval_source("var root={child:{answer:42}};root.child.self=root.child;root;")
        .unwrap();
    let Value::Object { props } = &value else {
        panic!("object");
    };
    let borrow = props.borrow_mut();
    assert_eq!(
        vm.collect_cycles().skipped,
        Some(napi_vm_core::heap::SkipReason::BorrowedCells)
    );
    drop(borrow);
    vm.collect_cycles();
    yes(&mut vm, "root.child.answer===42");
}
#[test]
fn deref_keeps_the_target_until_the_checkpoint_finishes() {
    use napi_vm_core::interpreter::{DrainPolicy, EvaluationOptions};
    let mut vm = Interpreter::with_builtins();
    vm.eval_source_with_options(
        "var target={};target.self=target;var ref=new WeakRef(target);target=undefined;",
        EvaluationOptions {
            drain: DrainPolicy::None,
            ..Default::default()
        },
    )
    .unwrap();
    vm.collect_cycles();
    vm.eval_source_with_options(
        "ref.deref()",
        EvaluationOptions {
            drain: DrainPolicy::None,
            ..Default::default()
        },
    )
    .unwrap();
    vm.collect_cycles();
    vm.drain_jobs().unwrap();
    vm.collect_cycles();
    yes(&mut vm, "ref.deref()===undefined");
}
#[test]
fn live_module_maps_are_roots_across_interpreters_without_snapshot_updates() {
    let mut owner = Interpreter::with_builtins();
    let mut collector = Interpreter::with_builtins();
    let value = Value::object(vec![("answer".into(), Value::Number(42.))]);
    value.set_prop("self".into(), value.clone()).unwrap();
    let record = napi_vm_core::interpreter::Module {
        namespace: Default::default(),
        exports: std::collections::HashMap::from([("value".into(), value.clone())]),
        default: None,
        scope: None,
    };
    owner.modules.borrow_mut().insert("host".into(), record);
    collector.collect_cycles();
    assert!(matches!(value.get_prop("answer"), Some(Value::Number(42.))));
    let borrow = owner.modules.borrow_mut();
    assert_eq!(
        collector.collect_cycles().skipped,
        Some(napi_vm_core::heap::SkipReason::BorrowedCells)
    );
    drop(borrow);
    owner.remove_module("host");
    drop(value);
    assert!(collector.collect_cycles().collected > 0);
}

#[test]
fn cached_module_rejections_are_traced_without_permanent_pins() {
    let mut vm = Interpreter::with_builtins();
    vm.define_module(
        "bad",
        "var reason={answer:42};reason.self=reason;throw reason;".into(),
    );
    vm.define_module(
        "bridge",
        "export function load(){return import('bad');}".into(),
    );
    let mut child = vm.create_realm();
    child.load_module("bridge").unwrap();
    vm.set_global_checked(
        "loadChild",
        child.module("bridge").unwrap().exports["load"].deref_binding(),
    )
    .unwrap();
    drop(child);
    run(
        &mut vm,
        "var failure;loadChild().catch(reason=>{failure=reason;});",
    );
    run(
        &mut vm,
        "var reference=new WeakRef(failure);failure=undefined;",
    );
    assert!(vm.collect_cycles().skipped.is_none());
    yes(&mut vm, "reference.deref().answer===42;");
    run(
        &mut vm,
        "var again;loadChild().catch(reason=>{again=reason;});",
    );
    yes(&mut vm, "again===reference.deref();");
    run(&mut vm, "loadChild=undefined;again=undefined;");
    assert!(vm.collect_cycles().skipped.is_none());
    yes(&mut vm, "reference.deref()===undefined;");
}

#[test]
fn with_object_environment_keeps_captured_receivers_alive() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "var read;var receiver={answer:42};receiver.self=receiver;var weak=new WeakRef(receiver);with(receiver){read=function(){return answer;};}receiver=undefined;",
    );
    assert!(vm.collect_cycles().skipped.is_none());
    yes(&mut vm, "read()===42&&weak.deref().answer===42;");
    run(&mut vm, "read=undefined;");
    assert!(vm.collect_cycles().collected > 0);
    yes(&mut vm, "weak.deref()===undefined;");
}

#[test]
fn proxy_revokers_trace_live_slots_and_release_targets_and_handlers() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "var target={answer:42};target.self=target;var handler={};handler.self=handler;var targetRef=new WeakRef(target);var handlerRef=new WeakRef(handler);var pair=Proxy.revocable(target,handler);var revoke=pair.revoke;target=undefined;handler=undefined;pair=undefined;",
    );
    assert!(vm.collect_cycles().skipped.is_none());
    yes(
        &mut vm,
        "targetRef.deref().answer===42&&handlerRef.deref()!==undefined;",
    );
    run(&mut vm, "revoke();");
    assert!(vm.collect_cycles().skipped.is_none());
    yes(
        &mut vm,
        "targetRef.deref()===undefined&&handlerRef.deref()===undefined;",
    );
}

#[test]
fn proxy_private_fields_preserve_branding_and_roots_after_revocation() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "class Identity{constructor(object){return object;}}class Stamp extends Identity{#value;constructor(object,value){super(object);this.#value=value;}static read(object){return object.#value;}static write(object,value){object.#value=value;}}var traps=0;var target={};var revocable=Proxy.revocable(target,{get(){traps++;throw 'get';},set(){traps++;throw 'set';},defineProperty(){traps++;throw 'define';}});var proxy=revocable.proxy;var child={answer:42};child.self=proxy;var childRef=new WeakRef(child);new Stamp(proxy,child);revocable.revoke();child=undefined;revocable=undefined;",
    );
    assert!(vm.collect_cycles().skipped.is_none());
    yes(
        &mut vm,
        "Stamp.read(proxy)===childRef.deref()&&Stamp.read(proxy).answer===42&&traps===0;",
    );
    yes(
        &mut vm,
        "var missing;try{Stamp.read(target);}catch(e){missing=e;}missing instanceof TypeError&&traps===0;",
    );
    yes(
        &mut vm,
        "var duplicate;try{new Stamp(proxy,0);}catch(e){duplicate=e;}duplicate instanceof TypeError&&Stamp.read(proxy)===childRef.deref()&&traps===0;",
    );
    run(&mut vm, "proxy=undefined;target=undefined;");
    assert!(vm.collect_cycles().collected > 0);
    yes(&mut vm, "childRef.deref()===undefined;");
}

#[test]
fn proxy_private_field_chains_drop_without_native_recursion() {
    let mut vm = Interpreter::with_builtins();
    run(
        &mut vm,
        "class Identity{constructor(object){return object;}}class Node extends Identity{#next;constructor(next){super(new Proxy({},{}));this.#next=next;}}var head;for(var i=0;i<10000;i++){head=new Node(head);}head=undefined;",
    );
    assert!(vm.collect_cycles().skipped.is_none());
}

#[test]
fn foreign_private_field_initializers_keep_their_realm_on_proxy_receivers() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    child.eval_source("class Identity{constructor(object){return object;}}class Stamp extends Identity{#value={answer:42};static read(object){return object.#value;}}").unwrap();
    vm.set_global_checked("ForeignStamp", child.eval_source("Stamp").unwrap())
        .unwrap();
    vm.set_global_checked(
        "foreignObjectPrototype",
        child.eval_source("Object.prototype").unwrap(),
    )
    .unwrap();
    drop(child);
    run(
        &mut vm,
        "var pair=Proxy.revocable({},{});var proxy=pair.proxy;pair.revoke();pair=undefined;new ForeignStamp(proxy);ForeignStamp.read(proxy).self=proxy;var ref=new WeakRef(ForeignStamp.read(proxy));",
    );
    assert!(vm.collect_cycles().skipped.is_none());
    yes(
        &mut vm,
        "ForeignStamp.read(proxy)===ref.deref()&&ForeignStamp.read(proxy).answer===42&&Object.getPrototypeOf(ForeignStamp.read(proxy))===foreignObjectPrototype;",
    );
    run(&mut vm, "proxy=undefined;");
    assert!(vm.collect_cycles().collected > 0);
    yes(&mut vm, "ref.deref()===undefined;");
}
