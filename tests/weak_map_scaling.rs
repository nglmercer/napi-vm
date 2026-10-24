use napi_vm::{Interpreter, Value};

#[test]
fn deep_ephemeron_chain_survives_collection_and_then_releases() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source("var map = new WeakMap(); var head = {}; head.self=head; var key = head; for (var i=0; i<10000; i++) { var next = {}; next.self=next; map.set(key,next); key=next; } key=null; next=null;").unwrap();
    assert!(vm.collect_cycles().skipped.is_none());
    assert!(matches!(
        vm.eval_source("var count=0; for(var p=head; p!==undefined; p=map.get(p)) count++; count;")
            .unwrap(),
        Value::Number(10001.)
    ));
    vm.eval_source("head=null;").unwrap();
    let stats = vm.collect_cycles();
    assert!(stats.skipped.is_none());
    assert!(stats.collected > 0);
}

#[test]
fn indexed_weak_keys_preserve_identity_and_deletion() {
    let mut vm = Interpreter::with_builtins();
    assert!(matches!(
        vm.eval_source(
            r#"
      var m=new WeakMap(), s=new WeakSet();
      var a={}, b={}, symbol=Symbol('key');
      m.set(a,1); m.set(b,2); m.set(a,3); m.set(symbol,4);
      s.add(a); s.add(b);
      m.get(a)===3 && m.get(b)===2 && m.get(symbol)===4 &&
      m.has(a) && !m.has({}) && !m.has(1) && !m.delete(1) &&
      m.delete(a) && !m.delete(a) && m.get(a)===undefined &&
      s.delete(a) && !s.has(a) && s.has(b);
    "#
        )
        .unwrap(),
        Value::Bool(true)
    ));
}

#[test]
fn ephemeron_value_cannot_keep_its_own_key_alive() {
    let mut vm = Interpreter::with_builtins();
    let key = vm
        .eval_source("var m=new WeakMap(); var key={}; m.set(key,{owner:key}); key;")
        .unwrap();
    let Value::Object { props } = &key else {
        panic!("object key")
    };
    let weak = std::rc::Rc::downgrade(props);
    drop(key);
    vm.eval_source("key=null;").unwrap();
    assert!(vm.collect_cycles().skipped.is_none());
    assert!(weak.upgrade().is_none());
}
