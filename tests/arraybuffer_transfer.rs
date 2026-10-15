use napi_vm::{Interpreter, Value};
fn check(source: &str, expected: &str) {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(source).unwrap();
    let result = vm.eval_source("JSON.stringify(result)").unwrap();
    assert!(
        matches!(&result,Value::String(s) if s==expected),
        "{result:?}"
    );
}
#[test]
fn transfers_preserve_resizability_or_make_a_fixed_buffer_and_detach_all_views() {
    check(
        r#"var a=new ArrayBuffer(3,{maxByteLength:6}),view=new Uint8Array(a);view.set([1,2,3]);var b=a.transfer(5),after=[a.detached,view.length,b.resizable,b.maxByteLength,Array.from(new Uint8Array(b))];var c=b.transferToFixedLength(7);var result=[after,b.detached,c.resizable,c.maxByteLength,Array.from(new Uint8Array(c)),Object.getPrototypeOf(c)===ArrayBuffer.prototype];"#,
        "[[true,0,true,6,[1,2,3,0,0]],true,false,7,[1,2,3,0,0,0,0],true]",
    );
}
#[test]
fn invalid_transfer_lengths_leave_the_source_attached_and_ignore_species() {
    check(
        r#"var log=[],a=new ArrayBuffer(2,{maxByteLength:3});Object.defineProperty(a,'constructor',{get(){throw 'species'}});try{a.transfer(4)}catch(e){log.push(e.name)}log.push(a.detached);try{a.transfer(-1)}catch(e){log.push(e.name)}var b=a.transfer({valueOf(){log.push('length');a.resize(1);return 3}});var result=[log,a.detached,b.byteLength];"#,
        "[[\"RangeError\",false,\"RangeError\",\"length\"],true,3]",
    );
}
#[test]
fn a_transfer_detects_detachment_during_length_conversion() {
    check(
        r#"var log=[],a=new ArrayBuffer(2);try{a.transfer({valueOf(){a.transfer();return 1}})}catch(e){log.push(e.name)}var result=[log,a.detached];"#,
        "[[\"TypeError\"],true]",
    );
}
