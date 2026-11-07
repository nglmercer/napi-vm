use napi_vm::{Interpreter, Lexer, Parser, Value};
#[test]
fn member_target_precedes_rhs() {
    assert_tiers(
        "var log='';var a={};var b={};var target=a;function rhs(){log+='r';target=b;return 5}function object(){log+='o';return target}var key={toString:function(){log+='k';return 'x'}};object()[key]=rhs();JSON.stringify([log,a.x,b.x]);",
        "[\"ork\",5,null]",
    );
}
#[test]
fn plain_assignment_does_not_get() {
    assert_tiers(
        "var log='';var o={get x(){log+='g';throw 0},set x(v){log+='s'+v}};o.x=3;JSON.stringify(log);",
        "\"s3\"",
    );
}
#[test]
fn names_of_anonymous_rhs() {
    assert_tiers(
        "var a;var b;var c;a=function(){};b=()=>1;c=class {};JSON.stringify([a.name,b.name,c.name]);",
        "[\"a\",\"b\",\"c\"]",
    );
}
#[test]
fn rhs_runs_before_const_failure() {
    assert_tiers(
        "const x=1;var n=0;try{x=(n++,3)}catch(e){}JSON.stringify([n,x]);",
        "[1,1]",
    );
}
#[test]
fn null_target_runs_rhs_before_write_error() {
    assert_tiers(
        "var log='';var key={toString:function(){log+='k';return 'x'}};try{null[key]=(log+='r',1)}catch(e){}JSON.stringify(log);",
        "\"rk\"",
    );
}
#[test]
fn private_field_assignment() {
    assert_tiers(
        "class C{#x=1;set(v){this.#x=v;return this.#x}}JSON.stringify(new C().set(7));",
        "7",
    );
}
#[test]
fn parenthesized_target_keeps_anonymous_name() {
    assert_tiers("var fn;(fn)=function(){};JSON.stringify(fn.name);", "\"\"");
}
#[test]
fn super_key_conversion_follows_rhs() {
    assert_tiers(
        "var log='';var key={toString:function(){log+='k';return 'x'}};class C extends Object{set(){super[key]=(log+='r',7);return this.x}}JSON.stringify([new C().set(),log]);",
        "[7,\"rk\"]",
    );
}

#[test]
fn existing_data_writes_preserve_descriptors_and_reject_read_only_properties() {
    assert_tiers(
        r#"var o={};Object.defineProperty(o,'x',{value:1,writable:true,enumerable:false,configurable:false});Object.preventExtensions(o);o.x=2;var d=Object.getOwnPropertyDescriptor(o,'x');Object.freeze(o);o.x=3;var error='';try{(function(){'use strict';o.x=4;})()}catch(e){error=e.name;}JSON.stringify([o.x,d.writable,d.enumerable,d.configurable,error]);"#,
        "[2,true,false,false,\"TypeError\"]",
    );
}

#[test]
fn existing_data_shadows_inherited_setters_and_proxy_prototypes() {
    assert_tiers(
        r#"var calls=0;var p=new Proxy({set x(v){calls++;}},{getOwnPropertyDescriptor(){calls++;throw 0;},set(){calls++;throw 0;}});var own={x:1};Object.setPrototypeOf(own,p);own.x=2;var inherited=Object.create({set x(v){calls+=v;}});inherited.x=3;JSON.stringify([own.x,calls,Object.hasOwn(inherited,'x')]);"#,
        "[2,3,false]",
    );
}

#[test]
fn own_accessors_and_distinct_receivers_keep_ordinary_set_behavior() {
    assert_tiers(
        r#"var calls=0;var o={get x(){return calls;},set x(v){calls+=v;}};o.x=2;var target={x:1};var receiver={x:3};var accepted=Reflect.set(target,'x',4,receiver);var blocked=Reflect.set(target,'x',5,{get x(){return 9;}});JSON.stringify([o.x,target.x,receiver.x,accepted,blocked]);"#,
        "[2,1,4,true,false]",
    );
}

#[test]
fn proxy_writes_and_boxed_string_exotics_stay_observable() {
    assert_tiers(
        r#"var calls=0;var target={x:1};var proxy=new Proxy(target,{set(t,k,v,r){calls++;return Reflect.set(t,k,v,r);},defineProperty(t,k,d){calls++;return Reflect.defineProperty(t,k,d);}});proxy.x=2;var boxed=new String('xy');boxed.extra=1;boxed.extra=2;boxed[0]='z';boxed.length=9;JSON.stringify([target.x,calls,boxed.extra,boxed[0],boxed.length]);"#,
        "[2,2,2,\"x\",2]",
    );
}

#[test]
fn data_values_named_like_accessors_and_public_hash_keys_remain_data() {
    assert_tiers(
        r#"var getter=Object.getOwnPropertyDescriptor({get x(){throw 0;}},'x').get;var o={};Object.defineProperty(o,'x',{value:getter,writable:true});o.x=7;o['#x']=1;o['#x']++;o.__proto__=null;JSON.stringify([o.x,o['#x'],Object.getPrototypeOf(o)]);"#,
        "[7,2,null]",
    );
}

#[test]
fn existing_symbol_data_writes_keep_symbol_identity_and_attributes() {
    assert_tiers(
        r#"var s=Symbol('x');var o={};Object.defineProperty(o,s,{value:1,writable:true,enumerable:false,configurable:false});o[s]=2;var d=Object.getOwnPropertyDescriptor(o,s);JSON.stringify([o[s],Reflect.ownKeys(o)[0]===s,Object.keys(o).length,d.enumerable,d.configurable]);"#,
        "[2,true,0,false,false]",
    );
}

#[test]
fn warmed_property_accesses_survive_descriptor_transitions_and_reinsertion() {
    assert_tiers(
        r#"var o={x:0,y:0};function read(){return o.x;}function write(v){o.x=v;return o.x;}for(var i=0;i<40;i++){write(i);read();}var log=[read()];var stored=50,hits=0;Object.defineProperty(o,'x',{get:function(){hits++;return stored;},set:function(v){hits++;stored=v;},configurable:true,enumerable:false});log.push(write(60));log.push(read());Object.defineProperty(o,'x',{value:70,writable:true,configurable:true,enumerable:true});log.push(write(80));delete o.x;o.x=90;log.push(read());log.push(Object.keys(o).join(','));log.push(hits);JSON.stringify(log);"#,
        "[39,60,60,80,90,\"y,x\",3]",
    );
}

#[test]
fn existing_proto_data_property_shadows_the_legacy_prototype_setter() {
    assert_tiers(
        r#"var p={marker:1};var o={};Object.defineProperty(o,'__proto__',{value:1,writable:true,enumerable:true,configurable:true});Object.setPrototypeOf(o,p);o.__proto__={marker:2};var accepted=Reflect.set(o,'__proto__',7);var d=Object.getOwnPropertyDescriptor(o,'__proto__');JSON.stringify([o.__proto__,Object.getPrototypeOf(o)===p,accepted,d.writable,d.enumerable,d.configurable,Object.keys(o).join(',')]);"#,
        "[7,true,true,true,true,true,\"__proto__\"]",
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
