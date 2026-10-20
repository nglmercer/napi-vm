use napi_vm_core::{Interpreter, Value};
fn yes(vm: &mut Interpreter, source: &str) {
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}
#[test]
fn locale_string_invokes_getters_with_the_original_primitive_receiver() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        "'use strict';var log=[];for(var pair of [[Boolean.prototype,true,'boolean'],[Number.prototype,7,'number'],[String.prototype,'x','string'],[Symbol.prototype,Symbol(),'symbol'],[BigInt.prototype,1n,'bigint']]){Object.defineProperty(pair[0],'toString',{configurable:true,get:function(){var receiver=this;var kind=typeof this;log.push(kind);return function(){log.push(this===receiver);return kind;};}});if(Object.prototype.toLocaleString.call(pair[1])!==pair[2])throw new Error('result');}log.join(',')==='boolean,true,number,true,string,true,symbol,true,bigint,true'",
    );
}
#[test]
fn locale_string_preserves_accessor_and_proxy_abrupt_completions() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        "var marker={};var caught=0;var args;var gets=0;var object={get toString(){gets++;return function(){args=arguments.length;return this===object?42:0;};}};var result=object.toLocaleString('ignored');for(var receiver of [{get toString(){throw marker;}},new Proxy({},{get(t,k){if(k==='toString')throw marker;return Reflect.get(t,k);}})]){try{Object.prototype.toLocaleString.call(receiver);}catch(e){if(e===marker)caught++;}}result===42 && args===0 && gets===1 && caught===2",
    );
}
