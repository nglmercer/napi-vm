use napi_vm_core::{Interpreter, Value};

fn check(source: &str, expected: &str) {
    let mut vm = Interpreter::with_builtins();
    let result = vm.eval_source(source).unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "{result:?}"
    );
}

#[test]
fn wide_object_writes_preserve_order_deletion_and_reinsertion() {
    check(
        r#"
        var o = {};
        for (var i = 0; i < 2000; ++i) o['p' + i] = i;
        for (var i = 0; i < 2000; ++i) o['p' + i] += 1;
        delete o.p5;
        o.p5 = 99;
        var keys = Object.keys(o);
        JSON.stringify([keys.length, keys[0], keys[5], keys[1999], o.p5, o.p1999]);
    "#,
        r#"[2000,"p0","p6","p5",99,2000]"#,
    );
}

#[test]
fn wide_object_writes_keep_descriptors_inherited_setters_and_coercion_order() {
    check(
        r#"
        var o = {}, seen = [];
        for (var i = 0; i < 200; ++i) o['p' + i] = i;
        var proto = {set inherited(v){seen.push(this === o, v)}};
        Object.setPrototypeOf(o, proto);
        o.inherited = 7;
        Object.defineProperty(o, 'p5', {get(){return 8}, configurable: true});
        var getterRejected = !Reflect.set(o, 'p5', 9);
        Object.defineProperty(o, 'p6', {writable: false});
        var readonlyRejected = !Reflect.set(o, 'p6', 9);
        o[{toString(){delete o.p7; return 'p7'}}] = 42;
        var last = Object.keys(o).pop();
        Object.preventExtensions(o);
        var extensionRejected = !Reflect.set(o, 'newKey', 1);
        o.p8 = 88;
        JSON.stringify([seen, Object.hasOwn(o, 'inherited'), getterRejected,
            o.p5, readonlyRejected, o.p6, o.p7, last, extensionRejected, o.p8]);
    "#,
        r#"[[true,7],false,true,8,true,6,42,"p7",true,88]"#,
    );
}

#[test]
fn host_storage_aliases_cannot_leave_guest_inline_caches_stale() {
    let mut vm = Interpreter::with_builtins();
    let value = vm.eval_source("var o={};for(var i=0;i<200;i++)o['p'+i]=i;function read(){return o.p199}read();read();o;").unwrap();
    let Value::Object { ref props } = value else {
        panic!("ordinary object")
    };
    let raw: &std::cell::RefCell<Vec<(String, Value)>> = props;
    vm.eval_source("read();o.extra;").unwrap();
    raw.borrow_mut()
        .insert(0, ("p199".into(), Value::Number(99.)));
    raw.borrow_mut().push(("extra".into(), Value::Number(42.)));
    let result = vm.eval_source("JSON.stringify([read(),o.extra]);").unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == "[99,42]"),
        "{result:?}"
    );
}

#[test]
fn prototype_traps_distinct_receivers_and_non_extensible_setters_stay_observable() {
    check(
        r#"
        var log = [];
        var proto = new Proxy({}, {set(t,k,v,r){log.push(k,v,r===o);return true}});
        var o = Object.create(proto);
        o.intercepted = 7;
        var parent = {}, child = Object.create(parent);
        for(var i=0;i<200;i++)parent['p'+i]=i;
        Reflect.set(parent, 'p5', 99, child);
        var setterProto = {set value(v){log.push(v)}};
        var sealed = Object.preventExtensions(Object.create(setterProto));
        sealed.value = 42;
        JSON.stringify([log,Object.hasOwn(o,'intercepted'),parent.p5,child.p5,
            Object.hasOwn(child,'p5'),Object.hasOwn(sealed,'value')]);
    "#,
        r#"[["intercepted",7,true,42],false,5,99,true,false]"#,
    );
}

#[test]
fn data_functions_and_symbol_properties_keep_ordinary_assignment_semantics() {
    check(
        r#"
        var o = {}, symbol = Symbol('key');
        for(var i=0;i<200;i++)o['p'+i]=i;
        function f(){throw new Error('data function called')}
        Object.defineProperty(f, 'name', {value:'get p5'});
        o.p5 = f;
        o.p5 = 9;
        o[symbol] = 42;
        o.p200 = 200;
        var d = Object.getOwnPropertyDescriptor(o, symbol);
        JSON.stringify([o.p5,o[symbol],Object.getOwnPropertySymbols(o)[0]===symbol,
            d.writable,d.enumerable,d.configurable,o.p200]);
    "#,
        "[9,42,true,true,true,true,200]",
    );
}
