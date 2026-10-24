use napi_vm::Value;

#[test]
fn dense_iterator_drain_preserves_cursor_overrides_and_live_getters() {
    let mut vm = napi_vm::Interpreter::with_builtins();
    let result = vm
        .eval_source(
            r#"
        var it = [1,2,3].values();
        it.next();
        var a = new Uint8Array({[Symbol.iterator]: function() { return it; }});
        var done = it.next().done;
        var source = [1,2,3];
        Object.defineProperty(source, '1', {get: function() {source[2] = 9; return 7; }});
        var b = new Uint8Array(source);
        var custom = [4,5].values();
        var next = custom.next;
        var calls = 0;
        custom.next = function() { calls++; return next.call(this); };
        var c = new Uint8Array({[Symbol.iterator]: function() { return custom; }});
        JSON.stringify([Array.from(a), done, Array.from(b), Array.from(c), calls]);
    "#,
        )
        .unwrap();
    assert!(matches!(result, Value::String(ref text) if text == "[[2,3],true,[1,7,9],[4,5],3]"));
}

#[test]
fn dense_iterator_drain_keeps_budget_and_partial_cursor() {
    let mut vm = napi_vm::Interpreter::with_builtins();
    vm.eval_source("var it = [1,2,3,4].values();").unwrap();
    vm.set_loop_budget(2);
    let error = vm
        .eval_source("new Uint8Array({[Symbol.iterator]: function() { return it; }});")
        .unwrap_err();
    assert!(error.to_string().contains("Maximum loop iterations"));
    vm.set_loop_budget(100);
    assert!(matches!(
        vm.eval_source("it.next().value;").unwrap(),
        Value::Number(3.0)
    ));
}

#[test]
fn fill_checks_descriptors_after_coercion_and_preserves_partial_writes() {
    let mut vm = napi_vm::Interpreter::with_builtins();
    let result = vm
        .eval_source(
            r#"
        var a = new Array(4);
        var seen = 0;
        var proto = Object.create(Array.prototype);
        a.fill(7, {valueOf: function() {
            Object.defineProperty(proto, '1', {set: function(v) { seen = v; }});
            Object.setPrototypeOf(a, proto);
            return 0;
        }});
        var b = [0, 0, 0];
        Object.defineProperty(b, '1', {writable: false});
        var threw = false;
        try { b.fill(9); } catch (e) { threw = e instanceof TypeError; }
        var c = new Array(2);
        Object.preventExtensions(c);
        var rejected = false;
        try { c.fill(1); } catch(e) { rejected = e instanceof TypeError; }
        JSON.stringify([seen, Object.hasOwn(a, 1), a[0], a[3], b, threw, rejected]);
    "#,
        )
        .unwrap();
    assert!(matches!(result, Value::String(ref text) if text == "[7,false,7,7,[9,0,0],true,true]"));
}

#[test]
fn filling_holes_preserves_presence_through_later_mutations() {
    let Value::Array(ref array) =
        Value::array_with_presence(vec![Value::Undefined; 20_000], vec![false; 20_000])
    else {
        unreachable!()
    };
    for index in 0..20_000 {
        array.store_index(index, Value::Number(index as f64));
    }
    assert!(array.presence_snapshot().iter().all(|present| *present));
    array.set_index_presence(10_000, false);
    array.store_index(20_000, Value::Undefined);
    assert!(!array.has_index(10_000));
    assert!(array.has_index(20_000));
    array.store_index(10_000, Value::Undefined);
    array.set_length(10);
    array.set_length(12);
    assert!(array.has_index(9));
    assert!(!array.has_index(10));
    assert!(!array.has_index(11));
}

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
