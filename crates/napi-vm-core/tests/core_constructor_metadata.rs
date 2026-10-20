use napi_vm_core::{Interpreter, Value};

fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}

#[test]
fn core_constructors_own_ordered_function_metadata() {
    yes(
        "var ok=true;for(var name of ['Object','Array','String','Number','Boolean']){var C=globalThis[name];var keys=Object.getOwnPropertyNames(C);var l=Object.getOwnPropertyDescriptor(C,'length');var n=Object.getOwnPropertyDescriptor(C,'name');ok=ok && typeof C==='function' && Object.getPrototypeOf(C)===Function.prototype && keys[0]==='length' && keys[1]==='name' && !!l && l.value===1 && !l.writable && !l.enumerable && l.configurable && !!n && n.value===name && !n.writable && !n.enumerable && n.configurable;}ok",
    );
}

#[test]
fn metadata_is_configurable_and_does_not_control_calling() {
    yes(
        "var ok=true;for(var name of ['Array','String','Number','Boolean']){var C=globalThis[name];C.length=99;ok=ok && C.length===1;Object.defineProperty(C,'length',{value:7});ok=ok && C.length===7;delete C.length;ok=ok && !Object.hasOwn(C,'length') && C.length===0;delete C.name;ok=ok && !Object.hasOwn(C,'name') && C.name==='';}ok && Array(3).length===3 && String(5)==='5' && Number('6')===6 && Boolean(1)===true",
    );
}

#[test]
fn array_unscopables_excludes_with_but_keeps_copy_methods() {
    yes(
        "var d=Object.getOwnPropertyDescriptor(Array.prototype,Symbol.unscopables);var u=d.value;var ok=!d.writable && !d.enumerable && d.configurable && Object.getPrototypeOf(u)===null && !Object.hasOwn(u,'with');for(var k of ['toReversed','toSorted','toSpliced']){var p=Object.getOwnPropertyDescriptor(u,k);ok=ok && p.value===true && p.writable && p.enumerable && p.configurable;}ok && [1].with(0,2)[0]===2",
    );
}

#[test]
fn constructors_keep_call_construct_and_subclass_behavior() {
    yes(
        "class A extends Array{};var a=new A(3);var b=new String('value');var n=new Number('7');var t=new Boolean(1);a.length===3 && !(0 in a) && a instanceof A && a.map(v=>v) instanceof A && b.valueOf()==='value' && n.valueOf()===7 && t.valueOf()===true && Array.prototype.constructor===Array && String.prototype.constructor===String && Number.prototype.constructor===Number && Boolean.prototype.constructor===Boolean",
    );
}
