use napi_vm_core::{Interpreter, Value};

fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(matches!(vm.eval_source(source).unwrap(), Value::Bool(true)));
}

#[test]
fn primitive_left_operand_skips_ordinary_prototype_access() {
    yes(
        "var gets=0;Object.defineProperty(Function.prototype,'prototype',{get:function(){gets++;throw 7;}});var values=[undefined,null,true,0,'text',Symbol(),1n];var ok=true;for(var value of values){ok=ok && !(value instanceof Function.prototype);}ok && gets===0",
    );
    yes("Function.prototype.prototype=true;!(0 instanceof Function.prototype)");
}

#[test]
fn bound_instanceof_still_observes_target_has_instance_for_primitives() {
    yes(
        "function A(){}var B=A.bind(null).bind(null);var seen=0;Object.defineProperty(A,Symbol.hasInstance,{value:function(x){seen++;return x===42;}});(42 instanceof B) && seen===1",
    );
}

#[test]
fn object_left_operand_still_reads_and_validates_prototype() {
    yes(
        "var gets=0;Object.defineProperty(Function.prototype,'prototype',{get:function(){gets++;return 1;}});var threw=false;try{({}) instanceof Function.prototype;}catch(e){threw=e instanceof TypeError;}threw && gets===1",
    );
}
