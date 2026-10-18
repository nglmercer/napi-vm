use napi_vm::interpreter::ExecutionTier;
use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(prepared.tier(), ExecutionTier::Bytecode);
    let result = Interpreter::with_builtins().execute(&prepared).unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "bytecode: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "AST: {result:?}"
    );
}

#[test]
fn deleted_properties_are_skipped_without_reading_values() {
    both(
        "var o={a:1,b:2,c:3},keys='';for(var k in o){keys+=k;delete o.b;}keys",
        "ac",
    );
}

#[test]
fn descriptors_are_checked_between_iterations_without_has_traps() {
    both(
        "var log=[];var p=new Proxy({a:1,b:2},{ownKeys:function(t){log.push('keys');return ['a','b'];},getOwnPropertyDescriptor:function(t,k){log.push('desc:'+k);return Object.getOwnPropertyDescriptor(t,k);},has:function(){throw 'unexpected has';},getPrototypeOf:function(){log.push('proto');return null;}});for(var k in p){log.push('body:'+k);delete p.b;}log.join(',')",
        "keys,desc:a,body:a,desc:b,proto",
    );
}

#[test]
fn deleted_own_keys_do_not_shadow_inherited_keys() {
    both(
        "var proto={b:2,c:3};var o=Object.create(proto);o.a=1;o.b=4;var keys='';for(var k in o){keys+=k;delete o.b;}keys",
        "abc",
    );
}

#[test]
fn nonenumerable_own_keys_shadow_inherited_keys() {
    both(
        "var o=Object.create({b:2});o.a=1;Object.defineProperty(o,'b',{value:3,enumerable:false});var keys='';for(var k in o){keys+=k;}keys",
        "a",
    );
}

#[test]
fn proxies_enumerate_inherited_properties_lazily() {
    both(
        "var log=[];var proto={z:1};var p=new Proxy({a:1},{getPrototypeOf:function(){log.push('proto');return proto;}});for(var k in p){log.push(k);}log.join(',')",
        "a,proto,z",
    );
}

#[test]
fn nested_enumerators_unwind_across_catches_finally_and_labels() {
    both(
        "var log='';outer:for(var a in {a:1,b:2}){try{for(var b in {x:1}){continue outer;}}finally{log+=a;}}for(var c in {c:1}){try{for(var d in {d:1}){throw 't';}}catch(e){log+=c;}}for(var e in {e:1}){break;}log",
        "abc",
    );
}

#[test]
fn nullish_and_primitive_sources_share_enumeration_rules() {
    both(
        "var keys='';for(var k in null){keys+='bad';}for(var k in undefined){keys+='bad';}for(var k in 'ab'){keys+=k;}keys",
        "01",
    );
}

#[test]
fn changed_enumerability_and_getters_are_observed_without_value_reads() {
    both(
        "var o={a:1,b:2};Object.defineProperty(o,'c',{enumerable:true,get:function(){throw 'value read';}});var keys='';for(var k in o){keys+=k;Object.defineProperty(o,'b',{enumerable:false});}keys",
        "ac",
    );
}

#[test]
fn breaking_does_not_query_the_prototype() {
    both(
        "var p=new Proxy({a:1},{getPrototypeOf:function(){throw 'eager prototype';}});var key;for(var k in p){key=k;break;}key",
        "a",
    );
}

#[test]
fn descriptor_errors_release_the_failed_enumerator() {
    both(
        "var p=new Proxy({a:1},{getOwnPropertyDescriptor:function(){throw 'descriptor';}});var log='';try{for(var k in p){}}catch(e){log=e;}for(var k in {b:1}){log+=k;}log",
        "descriptorb",
    );
}

#[test]
fn symbols_are_skipped_without_descriptor_queries() {
    both(
        "var s=Symbol('s'),log=[];var p=new Proxy({},{ownKeys:function(){return [s,'a'];},getOwnPropertyDescriptor:function(t,k){if(typeof k==='symbol')throw 'symbol descriptor';log.push(k);return {configurable:true,enumerable:true};},getPrototypeOf:function(){return null;}});for(var k in p){log.push(k);}log.join(',')",
        "a,a",
    );
}
