use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    let value = Interpreter::with_builtins().execute(&prepared).unwrap();
    assert!(
        matches!(value, Value::String(ref s) if s == expected),
        "compiled: {value:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let value = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(value, Value::String(ref s) if s == expected),
        "AST: {value:?}"
    );
}

#[test]
fn growth_and_overwrites_preserve_keys_descriptors_and_symbol_identity() {
    both(
        r#"
        var o = {length: 2048};
        for (var i = 0; i < o.length; i++) o[i] = i;
        for (var i = 0; i < o.length; i++) o[i] += 1;
        var s = Symbol('key'); o[s] = 8; o[s] = 9;
        var d = Object.getOwnPropertyDescriptor(o, '2047');
        Object.defineProperty(o, '2047', {writable: false});
        o[2047] = 0;
        Object.preventExtensions(o); o.extra = 1; o[0] = 7;
        JSON.stringify([o[0], o[2047], o[s], Object.getOwnPropertySymbols(o)[0] === s,
          d.writable, d.enumerable, d.configurable, Object.hasOwn(o, 'extra'),
          Object.keys(o).length]);
    "#,
        "[7,2048,9,true,true,true,true,false,2049]",
    );
}

#[test]
fn inherited_setters_frozen_data_and_proxy_receivers_keep_ordinary_set_order() {
    both(
        r#"
        var seen = 0; var p = {};
        Object.defineProperty(p, 'x', {set: function(v) { seen = v; }});
        Object.defineProperty(p, 'y', {value: 1, writable: false});
        var o = Object.create(p); o.x = 4; o.y = 9;
        var events = [];
        var proxy = new Proxy({}, {set: function(t,k,v,r) {events.push(k); return true;}});
        var q = Object.create(proxy); q.z = 8;
        var target = {a: 1}; var receiver = {};
        Reflect.set(target, 'a', 3, receiver);
        JSON.stringify([seen, Object.hasOwn(o,'x'),o.y,Object.hasOwn(o,'y'),events,
          Object.hasOwn(q,'z'),target.a,receiver.a]);
    "#,
        "[4,false,1,false,[\"z\"],false,1,3]",
    );
}

#[test]
fn accessors_mapped_arguments_and_callable_data_remain_distinct() {
    both(
        r#"
        var count = 0;
        var o = {get x() {return count;}, set x(v) {count = v;}}; o.x = 5;
        var f = {x: function named() {return 7;}}; f.x = 6;
        function mapped(a) {arguments[0] = 9; a = 10; return [a,arguments[0]];}
        var strict = false; var frozen = {};
        Object.defineProperty(frozen, 'x', {value:1, writable:false});
        try {(function(){'use strict'; frozen.x=2;})();}catch(e){strict=e instanceof TypeError;}
        JSON.stringify([o.x,f.x,mapped(1),strict,frozen.x]);
    "#,
        "[5,6,[10,10],true,1]",
    );
}

#[test]
fn numeric_typed_array_keys_preserve_negative_zero_and_invalid_write_coercion() {
    both(
        r#"
        var a = new Uint8Array([1,2]); a[-0] = 7; a['-0'] = 9;
        var conversions = 0;
        var value = {valueOf: function() { conversions++; return 5; }};
        a[-1] = value; a[1.5] = value; a[NaN] = value; a[Infinity] = value;
        var keyCalls = 0;
        a[{toString: function() {keyCalls++; return '1';}}] = 8;
        var bigintThrows = false;
        try {new BigInt64Array(1)[-1] = 1;} catch(e) {bigintThrows = e instanceof TypeError;}
        var inherited = Object.create(a);
        Reflect.set(a, '0', 6, inherited);
        JSON.stringify([a[0],a[1],a['-0'] === undefined,a[-1] === undefined,
          a[NaN] === undefined,a[Infinity] === undefined,conversions,keyCalls,
          bigintThrows,inherited[0],a[0]]);
    "#,
        "[7,8,true,true,true,true,4,1,true,6,7]",
    );
}
