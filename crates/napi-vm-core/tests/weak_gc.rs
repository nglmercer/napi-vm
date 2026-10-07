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
