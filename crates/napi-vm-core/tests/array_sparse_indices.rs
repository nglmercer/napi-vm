use napi_vm_core::{Interpreter, Value};

fn yes(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(
        matches!(vm.eval_source(source).unwrap(), Value::Bool(true)),
        "{source}"
    );
}

#[test]
fn sparse_indices_read_reflect_delete_and_preserve_length() {
    yes(
        "var a=[];a[999999]=42;a[4294967294]=7;var d=Object.getOwnPropertyDescriptor(a,'999999');var keys=Reflect.ownKeys(a);var ok=a.length===4294967295 && a[999999]===42 && a['4294967294']===7 && 999999 in a && d.value===42 && d.writable && d.enumerable && d.configurable && keys.join(',')==='999999,4294967294,length';delete a[999999];ok && !(999999 in a) && a.length===4294967295",
    );
}

#[test]
fn sparse_descriptors_and_length_truncation_follow_array_rules() {
    yes(
        "var a=[];Object.defineProperty(a,'999999',{value:1,writable:true,configurable:false});a[1000001]=2;var ok=!Reflect.defineProperty(a,'length',{value:3,writable:false});ok && a.length===1000000 && a[999999]===1 && !(1000001 in a) && !Object.getOwnPropertyDescriptor(a,'length').writable && !Reflect.set(a,'1000000',3) && Reflect.set(a,'999999',4) && a[999999]===4",
    );
    yes(
        "var a=[];a[999999]=1;a.length=2;!(999999 in a) && a.length===2 && Reflect.ownKeys(a).join(',')==='length'",
    );
}

#[test]
fn sparse_accessors_integrity_and_callback_methods() {
    yes(
        "var a=[];var receiver;Object.defineProperty(a,'999999',{get(){receiver=this;return 4;},enumerable:true,configurable:true});var d=Object.getOwnPropertyDescriptor(a,'999999');var ok=a[999999]===4 && receiver===a && typeof d.get==='function';Object.freeze(a);ok && Object.isFrozen(a) && !Reflect.deleteProperty(a,'999999')",
    );
    yes(
        "var a=[1];a[999999]=2;var seen=[];var ok=a.every(function(v,i,o){seen.push(i);return arguments.length===3 && o===a;});ok && seen.join(',')==='0,999999'",
    );
}

#[test]
fn sparse_species_result_keeps_holes_and_large_indices() {
    yes(
        "var a=[];a[999999]=3;var b=a.map(v=>v+1);b.length===1000000 && !(0 in b) && b[999999]===4 && Object.keys(b).join(',')==='999999'",
    );
}

#[test]
fn sparse_length_never_allocates_intervening_elements() {
    let mut vm = Interpreter::with_builtins();
    let Value::Array(ref array) = vm.eval_source("var a=[];a[4294967294]=1;a").unwrap() else {
        panic!("array expected")
    };
    assert_eq!(array.array_length(), u32::MAX as usize);
    assert!(array.borrow().is_empty());
    assert_eq!(array.named.borrow().len(), 1);
}

#[test]
fn uint32_max_is_named_and_frozen_length_allows_existing_holes() {
    yes(
        "var a=new Array(1000000);Object.defineProperty(a,'length',{writable:false});var ok=Reflect.set(a,'999999',5) && a[999999]===5 && a.length===1000000;ok && !Reflect.set(a,'1000000',6) && Reflect.set(a,'4294967295',7) && a.length===1000000 && a[4294967295]===7",
    );
}

#[test]
fn host_property_writes_share_index_storage() {
    let value = Value::array(vec![]);
    value.set_prop("2".into(), Value::Number(5.0)).unwrap();
    value.set_prop("999999".into(), Value::Number(7.0)).unwrap();
    assert!(matches!(value.get_prop("2"), Some(Value::Number(5.0))));
    assert!(matches!(value.get_prop("999999"), Some(Value::Number(7.0))));
    assert!(matches!(
        value.get_prop("length"),
        Some(Value::Number(1000000.0))
    ));
}
