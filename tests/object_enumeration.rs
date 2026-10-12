use napi_vm::{Interpreter, Lexer, Parser, Value};

#[test]
fn math_namespace_metadata_does_not_enter_descriptor_maps() {
    assert_tiers(
        "var descriptor = Object.getOwnPropertyDescriptor(Math,'PI'); var method = Object.getOwnPropertyDescriptor(Math,'abs'); Math.prop = {value:7}; var target = Object.create(null,Math); delete Math.prop; JSON.stringify([target.prop, Object.keys(Math), descriptor.writable, descriptor.enumerable, descriptor.configurable, method.writable, method.enumerable, method.configurable, Math.abs.length, Math.pow.length, Math.random.length, Object.getPrototypeOf(Math.abs) === Function.prototype]);",
        "[7,[],false,false,false,true,false,true,1,2,0,true]",
    );
}

#[test]
fn object_methods_have_function_metadata_and_are_nonconstructors() {
    assert_tiers(
        "var method = Object.defineProperties; var length = Object.getOwnPropertyDescriptor(method,'length'); var owner = Object.getOwnPropertyDescriptor(Object,'defineProperties'); var threw = false; try {new method();} catch(e){threw = e instanceof TypeError;} JSON.stringify([method.name, method.length, Object.getPrototypeOf(method) === Function.prototype, Object.hasOwn(method,'prototype'), length.writable, length.enumerable, length.configurable, owner.writable, owner.enumerable, owner.configurable, threw, Object.keys(Object), Object.prototype.hasOwnProperty.length, Object.prototype.toString.length]);",
        "[\"defineProperties\",2,true,false,false,false,true,true,false,true,true,[],1,0]",
    );
    assert_tiers(
        "var method = Object.keys; method.extra = 1; Object.defineProperty(method,'length',{value:9}); var before = [method.extra, method.length, typeof method.call, typeof method.bind]; delete method.name; JSON.stringify([before, Object.hasOwn(method,'name')]);",
        "[[1,9,\"function\",\"function\"],false]",
    );
}

#[test]
fn define_properties_converts_all_descriptors_before_mutating() {
    assert_tiers(
        "var target = {}; var seen = ''; var descriptors = {get a(){seen += String(Object.hasOwn(target,'a')); return {value:1};}, get b(){seen += String(Object.hasOwn(target,'a')); return {get:1};}}; var threw = false; try {Object.defineProperties(target, descriptors);} catch(e){threw = e instanceof TypeError;} JSON.stringify([seen, threw, Object.keys(target), Object.hasOwn(target,'a')]);",
        "[\"falsefalse\",true,[],false]",
    );
}

#[test]
fn define_properties_observes_proxy_operations_and_symbol_keys() {
    assert_tiers(
        "var key = Symbol('x'); var log = ''; var descriptors = new Proxy({a:{value:1}, [key]:{value:2}}, {ownKeys:function(t){log += 'keys;'; return [key,'a'];}, getOwnPropertyDescriptor:function(t,k){log += 'desc;'; return Reflect.getOwnPropertyDescriptor(t,k);}, get:function(t,k){log += 'get;'; return Reflect.get(t,k);}}); var target = new Proxy({}, {defineProperty:function(t,k,d){log += 'define;'; return Reflect.defineProperty(t,k,d);}}); var result = Object.defineProperties(target,descriptors); JSON.stringify([result === target, target.a, target[key], log]);",
        "[true,1,2,\"keys;desc;get;desc;get;define;define;\"]",
    );
}

#[test]
fn descriptor_map_boxes_inputs_and_rejects_nullish() {
    assert_tiers(
        "var count = 0; try {Object.defineProperties({},null);} catch(e){if(e instanceof TypeError) count++;} try {Object.defineProperties({});} catch(e){if(e instanceof TypeError) count++;} try {Object.create(null,null);} catch(e){if(e instanceof TypeError) count++;} try {Object.defineProperties({},'x');} catch(e){if(e instanceof TypeError) count++;} var key = Symbol('x'); var value = Object.create(null,{[key]:{value:7}}); JSON.stringify([count,Object.keys(Object.defineProperties({},42)),value[key],Object.getPrototypeOf(value)]);",
        "[4,[],7,null]",
    );
}

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
fn enumeration_orders_array_indices_before_other_strings() {
    assert_tiers(
        "var source = {b: 'b', 2: 'two', 1: 'one', '01': 'zero-one', '4294967295': 'max', a: 'a'}; Object.defineProperty(source, '3', {value:'hidden'}); JSON.stringify([Object.keys(source), Object.values(source), Object.entries(source), Object.getOwnPropertyNames(source)]);",
        "[[\"1\",\"2\",\"b\",\"01\",\"4294967295\",\"a\"],[\"one\",\"two\",\"b\",\"zero-one\",\"max\",\"a\"],[[\"1\",\"one\"],[\"2\",\"two\"],[\"b\",\"b\"],[\"01\",\"zero-one\"],[\"4294967295\",\"max\"],[\"a\",\"a\"]],[\"1\",\"2\",\"3\",\"b\",\"01\",\"4294967295\",\"a\"]]",
    );
}

#[test]
fn values_and_entries_observe_mutations_before_later_descriptors() {
    assert_tiers(
        "function make(){ return {get a(){ delete this.b; Object.defineProperty(this, 'c', {enumerable:false}); this.d = 4; return 1; }, b:2, c:3}; } JSON.stringify([Object.values(make()), Object.entries(make()), Object.keys(make())]);",
        "[[1],[[\"a\",1]],[\"a\",\"b\",\"c\"]]",
    );
}

#[test]
fn enumeration_preserves_proxy_order_and_skips_symbol_descriptors() {
    assert_tiers(
        "var log = ''; var key = Symbol('key'); var target = {a:1, b:2, [key]:3}; var source = new Proxy(target, {ownKeys:function(t){ log += 'keys;'; return [key,'b','a']; }, getOwnPropertyDescriptor:function(t,k){ log += 'd'+String(k)+';'; return Object.getOwnPropertyDescriptor(t,k); }, get:function(t,k){ log += 'g'+String(k)+';'; return Reflect.get(t,k); }}); var keys = Object.keys(source); var keyLog = log; log = ''; var values = Object.values(source); JSON.stringify([keys, keyLog, values, log]);",
        "[[\"b\",\"a\"],\"keys;db;da;\",[2,1],\"keys;db;gb;da;ga;\"]",
    );
    assert_tiers(
        "var source = new Proxy({1:'one', 2:'two'}, {ownKeys:function(t){return ['2','1'];}}); JSON.stringify(Object.entries(source));",
        "[[\"2\",\"two\"],[\"1\",\"one\"]]",
    );
}

#[test]
fn enumeration_boxes_strings_and_preserves_utf16_elements() {
    assert_tiers(
        "var values = Object.values('A\\ud800'); JSON.stringify([Object.keys('ab'), values[0], values[1].charCodeAt(0), Object.entries('ab'), Object.getOwnPropertyNames('ab'), Object.keys(1), Object.values(true), Object.entries(Symbol('x'))]);",
        "[[\"0\",\"1\"],\"A\",55296,[[\"0\",\"a\"],[\"1\",\"b\"]],[\"0\",\"1\",\"length\"],[],[],[]]",
    );
}

#[test]
fn all_enumeration_methods_reject_nullish_inputs() {
    assert_tiers(
        "var count = 0; var methods = [Object.keys, Object.values, Object.entries]; for (var index = 0; index < methods.length; index++) { var method = methods[index]; try {method();} catch(e){if(e instanceof TypeError) count++;} try {method(null);} catch(e){if(e instanceof TypeError) count++;} try {method(undefined);} catch(e){if(e instanceof TypeError) count++;} } JSON.stringify([count]);",
        "[9]",
    );
}

#[test]
fn value_enumeration_stops_at_an_abrupt_getter() {
    assert_tiers(
        "var reads = 0; var source = {get a(){throw new Error('stop');}, get b(){reads++; return 2;}}; var caught = false; try {Object.entries(source);} catch(e){caught = e.message === 'stop';} JSON.stringify([caught, reads]);",
        "[true,0]",
    );
}

#[test]
fn descriptor_collection_preserves_symbols_and_data_attributes() {
    assert_tiers(
        "var key = Symbol('key'); var reads = 0; var source = {2:'two', ['__proto__']:7, get x(){reads++; return 1;}, [key]:3}; Object.defineProperty(source, 'hidden', {value:4}); var descriptors = Object.getOwnPropertyDescriptors(source); JSON.stringify([Reflect.ownKeys(descriptors).length, Reflect.ownKeys(descriptors)[4] === key, descriptors[key].value, descriptors.hidden.enumerable, descriptors.hidden.writable, descriptors.__proto__.value, Object.getPrototypeOf(descriptors) === Object.prototype, typeof descriptors.x.get, reads]);",
        "[5,true,3,false,false,7,true,\"function\",0]",
    );
}

#[test]
fn descriptor_collection_omits_deleted_properties_and_late_additions() {
    assert_tiers(
        "var target = {a:1, b:2, c:3}; var source = new Proxy(target, {getOwnPropertyDescriptor:function(t,k){ if(k === 'a'){delete t.b; t.d = 4;} return Object.getOwnPropertyDescriptor(t,k); }}); JSON.stringify(Object.keys(Object.getOwnPropertyDescriptors(source)));",
        "[\"a\",\"c\"]",
    );
    assert_tiers(
        "var count = 0; try{Object.getOwnPropertyDescriptors();}catch(e){if(e instanceof TypeError)count++;} try{Object.getOwnPropertyDescriptors(null);}catch(e){if(e instanceof TypeError)count++;} var desc = Object.getOwnPropertyDescriptors('ab'); JSON.stringify([count, desc[0].value, desc[0].writable, desc.length.enumerable]);",
        "[2,\"a\",false,false]",
    );
}
