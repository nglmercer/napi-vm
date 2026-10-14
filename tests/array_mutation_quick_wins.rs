use napi_vm::{Interpreter, Lexer, Parser, Value};

fn assert_tiers(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(
        prepared.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let result = Interpreter::with_builtins().execute(&prepared).unwrap();
    assert!(
        matches!(result, Value::String(ref s) if s == expected),
        "bytecode: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(result, Value::String(ref s) if s == expected),
        "AST: {result:?}"
    );
}

#[test]
fn push_and_pop_use_logical_lengths_and_throwing_writes() {
    assert_tiers(
        "var o={length:9007199254740990};var n=Array.prototype.push.call(o,7);var v=Array.prototype.pop.call(o);var bad=0;try{Array.prototype.push.call({length:Infinity},1)}catch(e){if(e instanceof TypeError)bad++}var p={length:0};Object.defineProperty(p,'length',{writable:false});for(var i=0;i<2;i++){try{Array.prototype[i===0?'push':'pop'].call(p)}catch(e){if(e instanceof TypeError)bad++}}JSON.stringify([n,v,o.length,Object.hasOwn(o,'9007199254740990'),bad,Array.prototype.push.call(7)]);",
        "[9007199254740991,7,9007199254740990,false,3,0]",
    );
    assert_tiers(
        "var p={length:1};Object.defineProperty(p,'0',{value:8,configurable:false});var bad=false;try{Array.prototype.pop.call(p)}catch(e){bad=e instanceof TypeError}var q={length:0};Object.defineProperty(q,'1',{value:0,writable:false});var rejected=false;try{Array.prototype.push.call(q,1,2)}catch(e){rejected=e instanceof TypeError}JSON.stringify([bad,p.length,p[0],rejected,q[0],q[1],q.length]);",
        "[true,1,8,true,1,0,0]",
    );
}
#[test]
fn shift_and_unshift_preserve_holes_and_inherited_elements() {
    assert_tiers(
        "var proto={1:'b'};var o=Object.create(proto);o.length=3;o[0]='a';o[2]='c';var v=Array.prototype.shift.call(o);var a=[1,,3];var n=a.unshift(7,8);var b=[,2];var first=b.shift();JSON.stringify([v,o[0],o[1],o.length,Object.hasOwn(o,2),a,n,Object.hasOwn(a,3),first===undefined,b]);",
        "[\"a\",\"b\",\"c\",2,false,[7,8,1,null,3],5,false,true,[2]]",
    );
    assert_tiers(
        "var o={length:2,0:'a',1:'b'};Object.defineProperty(o,'0',{writable:false});var bad=false;try{Array.prototype.shift.call(o)}catch(e){bad=e instanceof TypeError}var p={length:2,0:'a',1:'b'};Object.defineProperty(p,'0',{writable:false});var rejected=false;try{Array.prototype.unshift.call(p,'x')}catch(e){rejected=e instanceof TypeError}JSON.stringify([bad,o[0],o[1],o.length,rejected,p[0],p[1],p[2],p.length]);",
        "[true,\"a\",\"b\",2,true,\"a\",\"a\",\"b\",2]",
    );
}
#[test]
fn mutations_observe_proxy_operation_order() {
    assert_tiers(
        "var log='';var p=new Proxy({0:'a',length:2},{get:function(t,k){log+='g'+k+';';return t[k]},has:function(t,k){log+='h'+k+';';return k in t},set:function(t,k,v){log+='s'+k+';';t[k]=v;return true},deleteProperty:function(t,k){log+='d'+k+';';return delete t[k]}});var v=Array.prototype.shift.call(p);JSON.stringify([v,log]);",
        "[\"a\",\"glength;g0;h1;d0;d1;slength;\"]",
    );
    assert_tiers(
        "var log='';var p=new Proxy({0:'a',length:2},{get:function(t,k){log+='g'+k+';';return t[k]},has:function(t,k){log+='h'+k+';';return k in t},set:function(t,k,v){log+='s'+k+';';t[k]=v;return true},deleteProperty:function(t,k){log+='d'+k+';';return delete t[k]}});var n=Array.prototype.unshift.call(p,'x');JSON.stringify([n,log]);",
        "[3,\"glength;h1;d2;h0;g0;s1;s0;slength;\"]",
    );
}
#[test]
fn searches_use_logical_indexes_and_skip_holes() {
    assert_tiers(
        "var o={length:Infinity,9007199254740990:7,0:8};var calls=0;var arg={valueOf:function(){calls++;return 0}};var a=[];var p={length:0};var x=Array.prototype.indexOf.call(p,7,arg);var y=Array.prototype.lastIndexOf.call(p,7,arg);JSON.stringify([Array.prototype.indexOf.call(o,7,-1),Array.prototype.lastIndexOf.call(o,7),Array.prototype.indexOf.call(o,8),Array.prototype.lastIndexOf.call(o,8,undefined),[,].indexOf(undefined),[NaN].lastIndexOf(NaN),Object.is([1].indexOf(1,-0),0),Object.is([1].lastIndexOf(1,-0),0),x,y,calls]);",
        "[9007199254740990,9007199254740990,0,0,-1,-1,true,true,-1,-1,0]",
    );
}
#[test]
fn map_and_filter_preserve_species_and_callback_order() {
    assert_tiers(
        "var log='';var result={};var a=[1,,3];a.constructor={[Symbol.species]:function(n){log+='s'+n+';';return result}};var same=a.map(function(v,i,o){log+='c'+i+';';return v*2})===result;var sparse=[1,,3].map(function(v){return v+1});var filtered=Array.prototype.filter.call('abc',function(v,i,o){return typeof o==='object'&&i!==1});JSON.stringify([same,result,log,sparse,Object.hasOwn(sparse,1),filtered]);",
        "[true,{\"0\":2,\"2\":6},\"s3;c0;c2;\",[2,null,4],false,[\"a\",\"c\"]]",
    );
    assert_tiers(
        "var a=new Proxy([],{get:function(t,k){if(k==='length')return Infinity;if(k==='constructor')return {[Symbol.species]:function(n){throw n}};return t[k]}});var captured=0;try{Array.prototype.map.call(a,function(){})}catch(e){captured=e}var calls=0;var bad=false;try{Array.prototype.map.call({get length(){calls++;return Infinity}},null)}catch(e){bad=e instanceof TypeError}JSON.stringify([captured,calls,bad]);",
        "[9007199254740991,1,true]",
    );
}
#[test]
fn new_generic_loops_obey_execution_budgets() {
    for source in [
        "Array.prototype.indexOf.call({length:Infinity},1);",
        "Array.prototype.lastIndexOf.call({length:Infinity},1);",
        "Array.prototype.shift.call({length:Infinity});",
        "Array.prototype.unshift.call({length:100},1);",
        "Array.prototype.filter.call({length:Infinity},function(){return true});",
        "var a=new Proxy([],{get:function(t,k){if(k==='length')return Infinity;if(k==='constructor')return {[Symbol.species]:function(){return {}}};return t[k]}});a.map(function(){return 1});",
    ] {
        let prepared = Interpreter::compile(source).unwrap();
        assert_eq!(
            prepared.tier(),
            napi_vm::interpreter::ExecutionTier::Bytecode
        );
        let statements = Parser::new(Lexer::new(source).tokenize())
            .parse_program()
            .unwrap();
        for bytecode in [true, false] {
            let mut vm = Interpreter::with_builtins();
            vm.set_loop_budget(3);
            let result = if bytecode {
                vm.execute(&prepared)
            } else {
                vm.run_program_body(&statements)
            };
            assert!(result.unwrap_err().to_string().contains("loop"), "{source}");
        }
    }
}

#[test]
fn native_arrays_observe_descriptors_and_partial_mutation() {
    assert_tiers(
        "var a=[1,2];Object.defineProperty(a,'length',{writable:false});var bad=false;try{a.pop()}catch(e){bad=e instanceof TypeError}var b=[1,2];Object.defineProperty(b,'0',{writable:false});var rejected=false;try{b.shift()}catch(e){rejected=e instanceof TypeError}var c=[];var calls=0;Object.defineProperty(Array.prototype,'0',{set:function(v){calls+=v},configurable:true});var n=c.push(4);delete Array.prototype[0];JSON.stringify([bad,a.length,Object.hasOwn(a,1),a[0],rejected,b,n,calls,Object.hasOwn(c,0)]);",
        "[true,2,false,1,true,[1,2],1,4,false]",
    );
}

#[test]
fn bounded_searches_preserve_the_existing_execution_policy() {
    let source = "JSON.stringify([[0,1,2,3].indexOf(3),[0,1,2,3].lastIndexOf(0)]);";
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(
        prepared.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    for bytecode in [true, false] {
        let mut vm = Interpreter::with_builtins();
        vm.set_loop_budget(3);
        let result = if bytecode {
            vm.execute(&prepared)
        } else {
            vm.run_program_body(&statements)
        }
        .unwrap();
        assert!(matches!(result,Value::String(ref s) if s=="[3,0]"));
    }
}
