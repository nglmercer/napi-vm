use napi_vm_core::{Interpreter, Value};
fn yes(vm: &mut Interpreter, source: &str) {
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}
const SETUP: &str = "var proto=Object.getPrototypeOf(Object.getPrototypeOf((async function*(){}).prototype));var dispose=proto[Symbol.asyncDispose];";
#[test]
fn async_iterator_disposal_awaits_return_and_discards_its_value() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(SETUP).unwrap();
    vm.eval_source("var release;var gate=new Promise(r=>release=r);var calls=0;var argc;var receiver;var done=false;var result=99;var iterator={return(){calls++;argc=arguments.length;receiver=this;return gate;}};var pending=dispose.call(iterator);pending.then(v=>{done=true;result=v;});").unwrap();
    yes(
        &mut vm,
        "calls===1 && argc===0 && receiver===iterator && !done && pending instanceof Promise",
    );
    vm.eval_source("release(42);").unwrap();
    yes(&mut vm, "done && result===undefined");
}
#[test]
fn async_iterator_disposal_rejects_getter_call_and_promise_constructor_errors() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(SETUP).unwrap();
    vm.eval_source("var marker={};var caught=0;var sync=0;var badPromise=Promise.resolve();Object.defineProperty(badPromise,'constructor',{get(){throw marker;}});for(var receiver of [{get return(){throw marker;}},{return(){throw marker;}},{return(){return Promise.reject(marker);}},{return(){return badPromise;}}]){try{dispose.call(receiver).catch(e=>{if(e===marker)caught++;});}catch(e){sync++;}}").unwrap();
    yes(&mut vm, "caught===4 && sync===0");
}
#[test]
fn async_iterator_disposal_uses_internal_reactions_and_closes_real_generators() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(SETUP).unwrap();
    vm.eval_source("var oldThen=Promise.prototype.then;var calls=0;var result;var trace='';Promise.prototype.then=function(){calls++;throw 99;};oldThen.call(dispose.call({return(){return Promise.resolve(42);}}),v=>result=v);Promise.prototype.then=oldThen;").unwrap();
    yes(&mut vm, "calls===0 && result===undefined");
    vm.eval_source("var generator=(async function*(){try{yield 1;}finally{await 0;trace+='closed';}})();generator.next();").unwrap();
    vm.eval_source("generator[Symbol.asyncDispose]();").unwrap();
    yes(&mut vm, "trace==='closed'");
}

#[test]
fn missing_return_fulfills_and_invalid_methods_reject_asynchronously() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(SETUP).unwrap();
    vm.eval_source("var fulfilled=0;var rejected=0;var sync=0;for(var receiver of [{},{return:null},{return:undefined},1]){dispose.call(receiver).then(v=>{if(v===undefined)fulfilled++;});}for(var receiver of [null,undefined,{return:1}]){try{dispose.call(receiver).catch(e=>{if(e instanceof TypeError)rejected++;});}catch(e){sync++;}}").unwrap();
    yes(&mut vm, "fulfilled===4 && rejected===3 && sync===0");
}

#[test]
fn await_using_closes_async_generators_before_leaving_the_scope() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source("var trace='';var completed=false;async function run(){var iterator=(async function*(){try{yield 1;}finally{await 0;trace+='cleanup,';}})();await iterator.next();{await using resource=iterator;trace+='body,';}trace+='after';}run().then(()=>completed=true);").unwrap();
    yes(&mut vm, "completed && trace==='body,cleanup,after'");
}
