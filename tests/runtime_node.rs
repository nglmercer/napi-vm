#![cfg(feature = "runtime-node")]
use napi_vm::Value;
use napi_vm::runtime::RuntimeBuilder;
#[test]
fn explicit_node_modules_work_in_esm_and_commonjs() {
    let mut runtime = RuntimeBuilder::new().node_compat().build().unwrap();
    for source in [
        "import path from 'node:path'; path.join('/data','sub','..','file.js')",
        "require('node:path').join('/data','sub','..','file.js')",
    ] {
        assert!(matches!(runtime.eval(source).unwrap(),Value::String(ref s) if s=="/data/file.js"));
    }
    assert!(matches!(runtime.eval("const assert=require('node:assert');assert.strictEqual(42,42);assert.deepStrictEqual({a:[1,2]}, {a:[1,2]});true").unwrap(),Value::Bool(true)));
    assert!(
        runtime
            .eval("require('node:assert').strictEqual(1,2)")
            .is_err()
    );
    assert!(matches!(runtime.eval("const events=require('node:events');const e=new events.EventEmitter();let total=0;e.once('value',n=>total+=n);e.emit('value',3);e.emit('value',3);total").unwrap(),Value::Number(3.0)));
    assert!(
        matches!(runtime.eval("require('node:util').format('%s = %d','answer',42)").unwrap(),Value::String(ref s) if s=="answer = 42")
    );
    assert!(
        matches!(runtime.eval("require('node:buffer').Buffer.from('hello').toString()").unwrap(),Value::String(ref s) if s=="hello")
    );
    assert!(runtime.eval("require('node:child_process')").is_err());
}
#[test]
fn compiling_node_features_does_not_install_compatibility() {
    let mut runtime = RuntimeBuilder::new().build().unwrap();
    assert!(
        matches!(runtime.eval("typeof Buffer + ',' + typeof require").unwrap(),Value::String(ref s) if s=="undefined,undefined")
    );
}

#[test]
fn deep_assertions_compare_collection_and_binary_contents() {
    let mut runtime = RuntimeBuilder::new().node_compat().build().unwrap();
    runtime.eval("const assert=require('node:assert');assert.deepStrictEqual(new Map([[1,'a'],[2,'b']]),new Map([[2,'b'],[1,'a']]));assert.deepStrictEqual(new Set([1,2]),new Set([2,1]));").unwrap();
    for source in [
        "assert.deepStrictEqual(new Map([[1,'a']]),new Map([[1,'b']]))",
        "assert.deepStrictEqual(new Set([1]),new Set([2]))",
        "assert.deepStrictEqual(new Uint8Array([1]),new Uint8Array([2]))",
        "assert.deepStrictEqual(new Number(1),new Number(2))",
    ] {
        assert!(runtime.eval(source).is_err(), "{source}");
    }
}
