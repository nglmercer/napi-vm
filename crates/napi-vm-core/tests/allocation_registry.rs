use napi_vm_core::{Interpreter, Value, heap};

#[test]
fn transient_guest_allocations_do_not_accumulate_dead_registry_entries() {
    let mut vm = Interpreter::with_builtins();
    vm.set_collection_threshold(0);
    let before = heap::counters();
    let result = vm
        .eval_source(
            "var kept={answer:42}; kept.self=kept; for(var i=0;i<20000;i++){[i];} kept.self.answer === 42",
        )
        .unwrap();
    let after = heap::counters();
    assert!(matches!(result, Value::Bool(true)));
    assert_eq!(before.total_collected, after.total_collected);
    assert!(
        after.tracked < before.tracked + 8192,
        "dead allocation registrations accumulated: {before:?} -> {after:?}"
    );
}
