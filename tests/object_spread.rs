use napi_vm::{Interpreter, Lexer, Parser, Value};

fn assert_tiers(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(
        prepared.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let mut bytecode = Interpreter::with_builtins();
    let result = bytecode.execute(&prepared).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == expected),
        "bytecode: {result:?}"
    );

    let mut parser = Parser::new(Lexer::new(source).tokenize());
    let statements = parser.parse_program().unwrap();
    let mut ast = Interpreter::with_builtins();
    let result = ast.run_program_body(&statements).unwrap();
    assert!(
        matches!(result, Value::String(ref text) if text == expected),
        "AST: {result:?}"
    );
}

#[test]
fn spread_preserves_symbol_identity_and_integer_key_order() {
    assert_tiers(
        "var key = Symbol('key'); var source = {z: 1, a: 2, 1: 3, [key]: 4}; var out = {...source}; var keys = Reflect.ownKeys(out); JSON.stringify([keys[0], keys[1], keys[2], keys[3] === key, out[key], Object.hasOwn(out, key)]);",
        "[\"1\",\"z\",\"a\",true,4,true]",
    );
}

#[test]
fn spread_boxes_primitives_and_copies_array_and_function_properties() {
    assert_tiers(
        "var array = [, 2]; array.extra = 3; function f(){} f.extra = 4; var out = {...array, ...f}; var text = {...'A\\ud800'}; JSON.stringify([Object.keys(out), out[1], out.extra, text[0], text[1].charCodeAt(0), Object.keys({...null, ...undefined, ...true, ...4, ...1n, ...Symbol('x')})]);",
        "[[\"1\",\"extra\"],2,4,\"A\",55296,[]]",
    );
}

#[test]
fn spread_checks_each_descriptor_after_the_previous_value_read() {
    assert_tiers(
        "var source = {get a(){ delete source.b; source.c = 3; return 1; }, b: 2}; var out = {...source}; JSON.stringify([Object.keys(out), out.a, Object.hasOwn(out, 'b'), Object.hasOwn(out, 'c')]);",
        "[[\"a\"],1,false,false]",
    );
}

#[test]
fn spread_observes_proxy_descriptor_and_get_traps_in_key_order() {
    assert_tiers(
        "var log = ''; var target = {a: 1, b: 2}; var source = new Proxy(target, {ownKeys: function(t){ log += 'keys;'; return ['a','b']; }, getOwnPropertyDescriptor: function(t,k){ log += 'd'+k+';'; return Object.getOwnPropertyDescriptor(t,k); }, get: function(t,k){ log += 'g'+k+';'; Object.defineProperty(t,'b',{enumerable:false}); return Reflect.get(t,k); }}); var out = {...source}; JSON.stringify([out, log]);",
        "[{\"a\":1},\"keys;da;ga;db;\"]",
    );
}

#[test]
fn spread_copy_precedes_later_property_expressions() {
    assert_tiers(
        "var log = ''; var source = {get value(){ log += 'get;'; return 1; }}; var out = {...source, later: (log += 'later;', source.other = 2)}; JSON.stringify([out.value, out.later, log, Object.keys(out)]);",
        "[1,2,\"get;later;\",[\"value\",\"later\"]]",
    );
}

#[test]
fn abrupt_spread_copy_stops_later_property_evaluation() {
    assert_tiers(
        "var steps = 0; var source = {get value(){ throw new Error('copy'); }}; try { var out = {...source, later: ++steps}; } catch (error) {} JSON.stringify([steps]);",
        "[0]",
    );
}

#[test]
fn spread_creates_data_properties_without_invoking_inherited_setters() {
    assert_tiers(
        "var calls = 0; Object.defineProperty(Object.prototype, 'x', {set: function(v){ calls++; }, configurable:true}); var source = {}; Object.defineProperty(source, 'x', {value:7, enumerable:true}); var out = {...source}; var desc = Object.getOwnPropertyDescriptor(out, 'x'); JSON.stringify([calls, out.x, desc.value, desc.writable, desc.enumerable, desc.configurable]);",
        "[0,7,7,true,true,true]",
    );
}

#[test]
fn rest_enumerates_after_named_bindings_and_excludes_symbol_keys() {
    assert_tiers(
        "var source = {get a(){ source.c = 3; return 1; }, b: 2}; var {a, ...rest} = source; JSON.stringify([a, rest, Object.keys(rest)]);",
        "[1,{\"b\":2,\"c\":3},[\"b\",\"c\"]]",
    );
    assert_tiers(
        "var key = Symbol('key'); var converted = 0; var selection = {toString: function(){ converted++; return key; }}; var source = {x: 2, [key]: 1}; var {[selection]: selected, ...rest} = source; JSON.stringify([selected, rest.x, converted, Object.hasOwn(rest, key)]);",
        "[1,2,1,false]",
    );
    assert_tiers(
        "var key = Symbol('key'); var source = {x: 1, [key]: 2}; var x; var rest; ({x, ...rest} = source); JSON.stringify([x, rest[key], Object.keys(rest), Reflect.ownKeys(rest)[0] === key]);",
        "[1,2,[],true]",
    );
}

#[test]
fn rest_skips_excluded_proxy_descriptors_and_boxes_string_sources() {
    assert_tiers(
        "var log = ''; var source = new Proxy({a: 1, b: 2}, {ownKeys: function(t){ log += 'keys;'; return ['a','b']; }, getOwnPropertyDescriptor: function(t,k){ log += 'd'+k+';'; return Object.getOwnPropertyDescriptor(t,k); }, get: function(t,k){ log += 'g'+k+';'; return Reflect.get(t,k); }}); var {a, ...rest} = source; JSON.stringify([a, rest, log]);",
        "[1,{\"b\":2},\"ga;keys;db;gb;\"]",
    );
    assert_tiers(
        "var {0: first, ...rest} = 'ab'; JSON.stringify([first, rest, Object.keys(rest)]);",
        "[\"a\",{\"1\":\"b\"},[\"1\"]]",
    );
}

#[test]
fn assign_copies_symbols_boxes_sources_and_interleaves_reads_and_writes() {
    assert_tiers(
        "var key = Symbol('key'); var source = {a: 1, b: 2, [key]: 3}; var log = ''; var target = {set a(v){ log += 'set;'; Object.defineProperty(source,'b',{enumerable:false}); }}; var result = Object.assign(target, source, 'x', null, undefined); JSON.stringify([result === target, log, Object.hasOwn(target,'b'), target[key], target[0]]);",
        "[true,\"set;\",false,3,\"x\"]",
    );
}

#[test]
fn assign_throws_on_nullish_targets_and_rejected_writes_in_sloppy_code() {
    assert_tiers(
        "var failures = 0; try { Object.assign(); } catch(e) { if (e instanceof TypeError) failures++; } try { Object.assign(null, {}); } catch(e) { if (e instanceof TypeError) failures++; } var target = {}; Object.defineProperty(target,'x',{value:1}); try { Object.assign(target,{x:2}); } catch(e) { if (e instanceof TypeError) failures++; } var boxed = Object.assign(1,{x:3}); JSON.stringify([failures, target.x, typeof boxed, boxed.x]);",
        "[3,1,\"object\",3]",
    );
}
