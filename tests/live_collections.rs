use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let program = Interpreter::compile(source).unwrap();
    assert_eq!(
        program.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode,
        "{}",
        program.fallback_reason().unwrap_or("bytecode")
    );
    let result = Interpreter::with_builtins().execute(&program).unwrap();
    assert!(
        matches!(&result, Value::String(value) if value == expected),
        "prepared: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&result, Value::String(value) if value == expected),
        "AST: {result:?}"
    );
}

#[test]
fn map_cursors_observe_updates_deletions_and_appends() {
    both(
        "var m = new Map([['a',1],['b',2],['c',3]]); var it = m.entries(); var log = [it.next().value.join(':')]; m.delete('b'); m.set('c',30); m.set('d',4); log.push(it.next().value.join(':'), it.next().value.join(':'), String(it.next().done)); log.join(',')",
        "a:1,c:30,d:4,true",
    );
}

#[test]
fn clearing_preserves_the_list_for_suspended_cursors() {
    both(
        "var m = new Map([[1,1],[2,2]]); var a = m.keys(), b = m.values(); a.next(); m.clear(); m.set(3,30); [a.next().value,b.next().value,a.next().done,b.next().done].join(',')",
        "3,30,true,true",
    );
    both(
        "var s = new Set([1,2]); var it = s.values(); it.next(); s.clear(); s.add(3); [it.next().value,it.next().done].join(',')",
        "3,true",
    );
}

#[test]
fn reinsertion_is_visited_and_done_iterators_never_revive() {
    both(
        "var s = new Set([1,2]); var it = s.values(); var log = [it.next().value]; s.delete(1); s.add(1); log.push(it.next().value,it.next().value,it.next().done); s.add(3); log.push(it.next().done); log.join(',')",
        "1,2,1,true,true",
    );
    both(
        "var m = new Map(); var it = m.entries(); var first = it.next().done; m.set('x',1); [first,it.next().done].join(',')",
        "true,true",
    );
}

#[test]
fn entry_results_do_not_expose_internal_storage() {
    both(
        "var m = new Map([['x',1]]); var pair = m.entries().next().value; pair[0] = 'changed'; pair[1] = 99; [m.get('x'),m.has('changed'),m.entries().next().value.join(':')].join(',')",
        "1,false,x:1",
    );
    both(
        "var s = new Set([1]); var pair = s.entries().next().value; pair[0] = 9; pair[1] = 9; [s.has(1),s.has(9),s.entries().next().value.join(':')].join(',')",
        "true,false,1:1",
    );
}

#[test]
fn iterator_brands_are_distinct_and_internal_state_is_not_a_property() {
    both(
        "var m = new Map([[1,2]]); var it = m.keys(); var next = it.next; var setNext = new Set().values().next; var arrayNext = [].values().next; var rejected = 0; for (var bad of [Object.create(it), new Proxy(it,{}), {'__items__':[9],'__kind__':'values','__cursor__':0}]) { try { next.call(bad); } catch(e) { if(e instanceof TypeError) rejected++; } } for(var f of [setNext,arrayNext]) { try { f.call(it); } catch(e) { if(e instanceof TypeError) rejected++; } } it.__cursor__ = 99; Object.freeze(it); [rejected,next.call(it).value,Object.getOwnPropertyNames(new Map().entries()).length].join(',')",
        "5,1,0",
    );
}

#[test]
fn collection_data_cannot_be_replaced_through_old_slot_names() {
    both(
        "var m = new Map([['x',1]]); m.__symbol_entries__ = [['x',99]]; m.__symbol_collection__ = 'Set'; [m.get('x'),m.size,m.entries().next().value.join(':')].join(',')",
        "1,1,x:1",
    );
}

#[test]
fn map_keys_canonicalize_negative_zero() {
    both(
        "var m = new Map(); m.set(-0, 'v'); [1/m.keys().next().value,m.get(0),m.size].join(',')",
        "Infinity,v,1",
    );
}

#[test]
fn backing_entries_survive_gc_and_are_released_on_exhaustion() {
    let mut vm = Interpreter::with_builtins();
    let record = vm
        .eval_source("var record = {}; record.self = record; record")
        .unwrap();
    let weak = match &record {
        Value::Object { props } => std::rc::Rc::downgrade(props),
        _ => unreachable!(),
    };
    vm.eval_source(
        "var m = new Map([['key',record]]); var iterator = m.values(); record = null; m = null;",
    )
    .unwrap();
    drop(record);
    assert!(vm.collect_cycles().skipped.is_none());
    assert!(
        weak.upgrade().is_some(),
        "live iterator must retain its backing entries"
    );
    vm.eval_source("iterator.next(); iterator.next();").unwrap();
    assert!(vm.collect_cycles().skipped.is_none());
    assert!(
        weak.upgrade().is_none(),
        "exhaustion must release the backing entries"
    );
}

#[test]
fn collection_and_iterator_cycles_are_collectable() {
    let mut vm = Interpreter::with_builtins();
    let collection = vm.eval_source("var m = new Map(); var iterator = m.values(); m.set('self',m); m.set('iterator',iterator); m").unwrap();
    let weak = match &collection {
        Value::Object { props } => std::rc::Rc::downgrade(props),
        _ => unreachable!(),
    };
    vm.eval_source("m = null; iterator = null;").unwrap();
    drop(collection);
    assert!(vm.collect_cycles().skipped.is_none());
    assert!(weak.upgrade().is_none());
}

#[test]
fn borrowed_iterator_methods_use_their_defining_realms() {
    let mut vm = Interpreter::with_builtins();
    let mut other = vm.create_realm();
    other
        .eval_source("var iterator = new Map([[1,2]]).entries(); var mapNext = iterator.next;")
        .unwrap();
    vm.set_global_checked("other", other.realm_global_object())
        .unwrap();
    drop(other);
    let result = vm.eval_source("var local = new Map([[3,4]]); var foreign = other.Map.prototype.entries.call(local); var localIterator = local.entries(); var result = other.mapNext.call(localIterator); Object.getPrototypeOf(foreign) === Object.getPrototypeOf(other.iterator) && Object.getPrototypeOf(result) === other.Object.prototype && Object.getPrototypeOf(result.value) === other.Array.prototype && result.value.join(':') === '3:4';").unwrap();
    assert!(matches!(result, Value::Bool(true)));
    assert!(vm.collect_cycles().skipped.is_none());
    let result = vm.eval_source("Object.getPrototypeOf(other.mapNext.call(localIterator)) === other.Object.prototype && foreign.next().value.join(':') === '3:4'").unwrap();
    assert!(matches!(result, Value::Bool(true)));
}

#[test]
fn individual_next_calls_do_not_consume_guest_loop_iterations() {
    let mut vm = Interpreter::with_builtins();
    vm.set_loop_budget(0);
    let result = vm
        .eval_source("var m = new Map(); m.set(1,2); m.entries().next().value[1]")
        .unwrap();
    assert!(matches!(result, Value::Number(2.0)));
}
