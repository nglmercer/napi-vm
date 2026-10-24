use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    let compiled = Interpreter::with_builtins().execute(&prepared).unwrap();
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let ast = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    for value in [compiled, ast] {
        assert!(
            matches!(value, Value::String(ref s) if s == expected),
            "{value:?}"
        );
    }
}

#[test]
fn globals_keep_lexical_shadowing_accessors_deletion_and_local_bindings() {
    both(
        r#"
        globalThis.shadowed = 1; let shadowed = 7;
        var data = 9; var initial = data;
        function read() {return data;}
        var local = (function(data) {return data;})(23);
        var seen = 0;
        Object.defineProperty(globalThis, 'accessed', {configurable:true,
          get:function() {seen++; return data;}});
        var a = accessed; data = 11; var b = accessed; var c = read();
        delete globalThis.data;
        var missing = typeof data === 'undefined';
        JSON.stringify([shadowed,globalThis.shadowed,initial,local,a,b,c,seen,missing]);
    "#,
        "[7,1,9,23,9,11,11,2,true]",
    );
}

#[test]
fn with_receivers_unscopables_and_tdz_use_reference_resolution() {
    both(
        r#"
        var value = 3; var object = {value:9, method:function(){return this.value;}};
        var first, second;
        with (object) {first = method(); value = 10;}
        object[Symbol.unscopables] = {value:true};
        with (object) {second = value;}
        var dead = false;
        try {{var unused = lexical; let lexical = 1;}} catch(e) {dead = e instanceof ReferenceError;}
        JSON.stringify([first,second,value,object.value,dead]);
    "#,
        "[9,3,3,10,true]",
    );
}

#[test]
fn date_data_reads_observe_shadowing_getters_and_prototype_replacement() {
    both(
        r#"
        var date = new Date(7), other = new Date(11);
        var initial = date.getTime();
        var original = Date.prototype.getTime;
        Date.prototype.getTime = function(){return 9;};
        var replaced = date.getTime();
        date.getTime = function(){return 13;}; var own = date.getTime();
        delete date.getTime;
        var seen;
        Object.defineProperty(Date.prototype, 'getTime', {configurable:true,
          get:function(){seen = this; return function(){return 17;};}});
        var getter = date.getTime(); var receiver = seen === date;
        var method = Reflect.get(date, 'getTime', other); var reflect = seen === other && method() === 17;
        Object.defineProperty(Date.prototype, 'getTime', {value:original,writable:true,configurable:true});
        var hits = 0;
        Object.setPrototypeOf(date, new Proxy({}, {get:function(t,k,r){hits++; return 21;}}));
        var proxy = date.marker;
        Object.setPrototypeOf(date, {marker:23}); var direct = date.marker;
        Object.setPrototypeOf(date, null); var missing = date.marker === undefined;
        JSON.stringify([initial,replaced,own,getter,receiver,reflect,proxy,hits,direct,missing]);
    "#,
        "[7,9,13,17,true,true,21,1,23,true]",
    );
}
