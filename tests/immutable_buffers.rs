use napi_vm::{Interpreter, Value};
fn check(source: &str, expected: &str) {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(source).unwrap();
    vm.collect_cycles();
    let result = vm.eval_source("JSON.stringify(result)").unwrap();
    assert!(
        matches!(&result,Value::String(s) if s==expected),
        "{result:?}"
    );
}
#[test]
fn immutable_creation_copies_bytes_and_slice_bounds_survive_source_resize() {
    check(
        r#"var source=new ArrayBuffer(3,{maxByteLength:6}),a=new Uint8Array(source);a.set([1,2,3]);Object.defineProperty(source,'constructor',{get(){throw 'species'}});var error;try{source.sliceToImmutable(0,{valueOf(){source.resize(1);return 3}})}catch(e){error=e.name}source.resize(3);a.set([1,2,3]);var frozen=source.sliceToImmutable(0,{valueOf(){source.resize(1);return 1}});a[0]=9;var moved=source.transferToImmutable(3);var result=[frozen.immutable,frozen.resizable,Array.from(new Uint8Array(frozen)),source.detached,moved.immutable,Array.from(new Uint8Array(moved)),frozen.slice().immutable,error];"#,
        "[true,false,[1],true,true,[9,0,0],false,\"RangeError\"]",
    );
}
#[test]
fn immutable_integer_indices_reject_writes_without_coercion_and_have_frozen_descriptors() {
    check(
        r#"var log=[],buffer=new Uint8Array([1,2]).buffer.transferToImmutable(),view=new Uint8Array(buffer),v={valueOf(){log.push('coerced');return 7}},receiver={};var writes=[Reflect.set(view,'0',v),Reflect.set(view,'-1',v),Reflect.set(view,'NaN',v),Reflect.set(view,'0',v,receiver),Reflect.set(view,'1.0',7)];var descriptors=[Object.getOwnPropertyDescriptor(view,'0'),Reflect.defineProperty(view,'0',{value:1,writable:false,configurable:false}),Reflect.defineProperty(view,'0',{value:257})];view[0]=v;try{(function(){'use strict';view[0]=v})()}catch(e){log.push(e.name)}var result=[writes,descriptors,Array.from(view),view['1.0'],Object.keys(receiver),log];"#,
        r#"[[false,false,false,false,true],[{"value":1,"writable":false,"enumerable":true,"configurable":false},true,false],[1,2],7,[],["TypeError"]]"#,
    );
}
#[test]
fn write_methods_reject_before_argument_conversion_but_reading_and_copying_work() {
    check(
        r#"var log=[],buffer=new ArrayBuffer(8).transferToImmutable(),view=new Uint8Array(buffer),data=new DataView(buffer),arg={valueOf(){log.push('coerced');return 0}};for(var f of [function(){view.fill(arg)},function(){view.copyWithin(arg,arg)},function(){view.set([],arg)},function(){view.reverse()},function(){data.setInt8(arg,arg)},function(){Atomics.add(new Int32Array(buffer),arg,arg)}]){try{f()}catch(e){log.push(e.name)}}var mapped=view.map(function(x){return x+1});var sub=view.subarray();var result=[log,mapped.buffer.immutable,mapped[0],sub.buffer===buffer,Atomics.load(new Int32Array(buffer),0)];"#,
        r#"[["TypeError","TypeError","TypeError","TypeError","TypeError","TypeError"],false,1,true,0]"#,
    );
}
#[test]
fn write_species_is_rejected_even_for_an_empty_result() {
    check(
        r#"var log=[],frozen=new Uint8Array(new ArrayBuffer(0).transferToImmutable()),source=new Uint8Array(0);source.constructor={[Symbol.species]:function(){log.push('species');return frozen}};try{source.map(function(){log.push('callback')})}catch(e){log.push(e.name)}try{Uint8Array.of.call(function(){return frozen})}catch(e){log.push(e.name)}var result=log;"#,
        r#"["species","TypeError","TypeError"]"#,
    );
}
#[test]
fn immutable_backing_cannot_be_detached_or_exposed_as_a_mutable_backing_pointer() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source("var immutable=new ArrayBuffer(2).transferToImmutable();")
        .unwrap();
    let value = vm.global_value("immutable").unwrap();
    let Value::ArrayBuffer(buffer) = &value else {
        panic!("expected buffer")
    };
    assert!(buffer.try_detach().is_err());
    buffer.detach();
    assert!(!buffer.is_detached());
    assert_eq!(buffer.borrow().len(), 2);
    let backing = napi_vm::value::BufferBacking::Array(buffer.clone());
    assert!(!backing.write(0, &[7]));
    assert!(backing.data_ptr().is_null());
}
