use napi_vm_core::{Interpreter, Value};

fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(matches!(vm.eval_source(source).unwrap(), Value::Bool(true)));
}

#[test]
fn nested_bound_new_target_is_replaced_at_each_immediate_target() {
    yes(
        "var seen;function A(){seen=new.target;}var B=A.bind(null);var C=B.bind(null);var x=Reflect.construct(C,[],B);seen===A && Object.getPrototypeOf(x)===A.prototype",
    );
    yes(
        "var seen;function A(){seen=new.target;}function Other(){}var B=A.bind(null);var C=B.bind(null);var x=Reflect.construct(C,[],Other);seen===Other && Object.getPrototypeOf(x)===Other.prototype",
    );
}

#[test]
fn bound_chains_preserve_receiver_arguments_and_metadata_order() {
    yes(
        "function f(a,b,c){return this.id+':'+a+':'+b+':'+c;}var b=f.bind({id:'first'},1).bind({id:'second'},2);var names=Object.getOwnPropertyNames(b);b(3)==='first:1:2:3' && b.length===1 && b.name==='bound bound f' && names[0]==='length' && names[1]==='name'",
    );
    yes(
        "function A(a,b,c){this.values=[a,b,c];}var B=A.bind(null,1).bind(null,2);var x=new B(3);x instanceof A && x.values.join(',')==='1,2,3'",
    );
}

#[test]
fn deep_bound_chains_call_and_construct_without_host_recursion() {
    yes(
        "function A(){if(new.target){this.ok=true;}else{return this.ok;}}var B=A;for(var i=0;i<2000;i++){B=B.bind({ok:true});Object.defineProperty(B,'name',{value:''});}B()===true && (new B()).ok===true && (new B()) instanceof B",
    );
}

#[test]
fn intermediate_bound_has_instance_hooks_remain_observable() {
    yes(
        "function A(){}var B=A.bind(null);var C=B.bind(null);var seen=0;Object.defineProperty(B,Symbol.hasInstance,{value:function(x){seen++;return x===42;}});(42 instanceof C) && seen===1",
    );
}
