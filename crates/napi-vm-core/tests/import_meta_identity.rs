use napi_vm_core::{Interpreter, Value};
#[test]
fn module_meta_is_stable_distinct_mutable_and_retained_by_gc() {
    let mut vm = Interpreter::with_builtins();
    vm.define_module("meta-a","import.meta.marker=17;export function read(){return import.meta;}export const initial=import.meta===read();".into());
    vm.define_module(
        "meta-b",
        "export function read(){return import.meta;}".into(),
    );
    let value=vm.eval_source("import {read as a,initial} from 'meta-a';import {read as b} from 'meta-b';var m=a();initial && m===a() && m!==b() && m.marker===17 && Object.getPrototypeOf(m)===null && (m.extra=42,a().extra===42) && (Object.freeze(m),Object.isFrozen(a()))").unwrap();
    assert!(matches!(value, Value::Bool(true)));
    vm.collect_cycles();
    assert!(matches!(
        vm.eval_source("a()===m && a().marker===17 && Object.isFrozen(a())")
            .unwrap(),
        Value::Bool(true)
    ));
}
#[test]
fn module_record_keeps_unexported_meta_alive_across_collection() {
    let mut vm = Interpreter::with_builtins();
    vm.define_module(
        "meta",
        "import.meta.marker=23;export function get(){return import.meta;}".into(),
    );
    let value = vm.eval_source("import {get} from 'meta';get()").unwrap();
    let weak = if let Value::Object { props } = &value {
        std::rc::Rc::downgrade(props)
    } else {
        panic!("object expected")
    };
    drop(value);
    vm.collect_cycles();
    assert!(weak.upgrade().is_some());
    let value = vm.eval_source("get()").unwrap();
    let Value::Object { props } = &value else {
        panic!("object expected")
    };
    assert!(std::rc::Rc::ptr_eq(props, &weak.upgrade().unwrap()));
}
#[test]
fn dynamic_function_constructors_reject_import_meta_in_bodies_and_parameters() {
    let mut vm = Interpreter::with_builtins();
    let value=vm.eval_source("var constructors=[Function,(async function(){}).constructor,(function*(){}).constructor,(async function*(){}).constructor];var count=0;for(var C of constructors){for(var args of [['return import.meta'],['x=import.meta','return x']]){try{C(...args);}catch(e){if(e instanceof SyntaxError)count++;}}}count===8 && Function('return new.target')()===undefined").unwrap();
    assert!(matches!(value, Value::Bool(true)));
}
