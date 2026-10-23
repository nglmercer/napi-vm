use napi_vm::Value;

#[test]
fn indexed_append_preserves_dense_and_holey_arrays() {
    let Value::Array(ref array) = Value::array(vec![]) else {
        unreachable!()
    };
    for index in 0..20_000 {
        array.store_index(index, Value::Number(index as f64));
    }
    assert_eq!(array.array_length(), 20_000);
    for index in [0, 10_000, 19_999] {
        assert!(matches!(array.index_value(index), Some(Value::Number(n)) if n == index as f64));
    }
    array.set_index_presence(1, false);
    array.store_index(20_000, Value::Number(42.0));
    assert!(!array.has_index(1));
    assert!(array.has_index(20_000));
    array.store_index(20_002, Value::Number(7.0));
    assert!(!array.has_index(20_001));
    assert!(array.has_index(20_002));
}

#[test]
fn object_assign_rejects_a_new_key_at_the_distinct_property_limit() {
    use napi_vm::{Interpreter, value::MAX_OBJECT_PROPS};
    let properties = (0..MAX_OBJECT_PROPS)
        .map(|index| (format!("key{index}"), Value::Number(0.0)))
        .collect();
    let target = Value::checked_object(properties).unwrap();
    let mut vm = Interpreter::with_builtins();
    vm.global.borrow_mut().set("quotaTarget", target);
    let error = vm
        .eval_source("Object.assign(quotaTarget, {overflow: 1});")
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Maximum object property count exceeded")
    );
}
