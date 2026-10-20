use napi_vm_core::{Interpreter, Value};
fn yes(vm: &mut Interpreter, source: &str) {
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}
#[test]
fn array_string_uses_join_getters_boxed_receivers_and_all_callables() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        "'use strict';var reads=0,receiver,args;Object.defineProperty(Number.prototype,'join',{configurable:true,get:function(){reads++;receiver=this;return new Proxy(function(){args=arguments.length;return this===receiver && this.valueOf()===7?'joined':'wrong';},{});}});Array.prototype.toString.call(7)==='joined' && reads===1 && typeof receiver==='object' && args===0",
    );
    yes(
        &mut vm,
        "var object={join:new Proxy(function(){return this===object?'proxy':'wrong';},{})};Array.prototype.toString.call(object)==='proxy'",
    );
    yes(
        &mut vm,
        "var threw=false;try{Array.prototype.toString.call({join:class{}});}catch(e){threw=e instanceof TypeError;}var receiverObject={join:Object};var joinedObject=Array.prototype.toString.call(receiverObject);threw && typeof joinedObject==='object' && joinedObject!==receiverObject && Object.getPrototypeOf(joinedObject)===Object.prototype",
    );
}
#[test]
fn array_string_fallback_uses_the_intrinsic_even_when_globals_are_replaced() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        "var stringify=Array.prototype.toString;var original=Object;Object.prototype.toString=function(){throw 99;};Object={get prototype(){throw 98;}};stringify.call({join:null,[Symbol.toStringTag]:'Tagged'})==='[object Tagged]'",
    );
}
#[test]
fn fallback_proxy_brands_only_forward_array_and_callable_internal_slots() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        "var stringify=Array.prototype.toString;var types=[new Date(),new RegExp('x'),new Error(),Object(true),Object(1),Object('x')];types.every(value=>stringify.call(new Proxy(value,{}))==='[object Object]') && stringify.call(new Proxy([], {get(t,k,r){return k==='join'?null:Reflect.get(t,k,r);}}))==='[object Array]' && stringify.call(new Proxy(function(){},{}))==='[object Function]'",
    );
    yes(
        &mut vm,
        "var pair=Proxy.revocable([],{get(t,k){if(k==='join'){pair.revoke();return null;}return Reflect.get(t,k);}});var caught=false;try{stringify.call(pair.proxy);}catch(e){caught=e instanceof TypeError;}caught",
    );
}
#[test]
fn array_string_propagates_getters_and_rejects_null_before_property_access() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        "var stringify=Array.prototype.toString;var marker={};var count=0;try{stringify.call({get join(){throw marker;}});}catch(e){if(e===marker)count++;}for(var value of [null,undefined]){try{stringify.call(value);}catch(e){if(e instanceof TypeError)count++;}}count===3",
    );
}
#[test]
fn removed_modern_to_string_tags_fall_back_to_object() {
    let mut vm = Interpreter::with_builtins();
    yes(
        &mut vm,
        "var stringify=Object.prototype.toString;var buffer=new ArrayBuffer(4);var view=new Uint8Array(buffer);var data=new DataView(buffer);delete ArrayBuffer.prototype[Symbol.toStringTag];delete DataView.prototype[Symbol.toStringTag];delete Object.getPrototypeOf(Uint8Array.prototype)[Symbol.toStringTag];stringify.call(buffer)==='[object Object]' && stringify.call(view)==='[object Object]' && stringify.call(data)==='[object Object]'",
    );
}
