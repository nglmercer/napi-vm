use napi_vm_core::{
    CompositeLoader, Interpreter, ModuleBytes, ModuleLoader, ModuleSource, Value, VirtualLoader,
    VmErr,
};
use std::rc::Rc;

fn truth(vm: &mut Interpreter, source: &str) {
    assert!(
        matches!(vm.eval_source(source), Ok(Value::Bool(true))),
        "{source}"
    );
}

#[test]
fn byte_modules_preserve_binary_data_and_use_immutable_intrinsic_storage() {
    let loader = Rc::new(VirtualLoader::new());
    loader.insert_bytes("blob", vec![0, 255, 128, 13, 10]);
    loader.insert(
        "main",
        "export {default as bytes} from 'blob' with {type:'bytes'};",
    );
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(Rc::new(CompositeLoader::new(loader)));
    vm.eval_source("var OriginalUint8Array=Uint8Array,OriginalArrayBuffer=ArrayBuffer;Uint8Array=function(){throw 99;};ArrayBuffer=function(){throw 98;};").unwrap();
    truth(
        &mut vm,
        "import {bytes} from 'main';bytes instanceof OriginalUint8Array && bytes.buffer instanceof OriginalArrayBuffer && bytes.buffer.immutable && bytes.length===5 && bytes[0]===0 && bytes[1]===255 && bytes[2]===128 && bytes[3]===13 && bytes[4]===10",
    );
    truth(
        &mut vm,
        "var rejected=0;for(var op of [()=>{ 'use strict';bytes[0]=7;},()=>bytes.set([1]),()=>new DataView(bytes.buffer).setUint8(0,7),()=>bytes.buffer.transfer()]){try{op();}catch(e){if(e instanceof TypeError)rejected++;}}rejected===4 && bytes[0]===0 && bytes.buffer.byteLength===5 && Reflect.set(bytes,'0',7)===false",
    );
}

#[test]
fn byte_modules_share_static_dynamic_and_reexported_identity() {
    let loader = Rc::new(VirtualLoader::new());
    loader.insert_bytes("blob", vec![1, 2, 3]);
    loader.insert(
        "barrel",
        "export {default as bytes} from 'blob' with {type:'bytes',flavor:'x'};",
    );
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(loader);
    vm.eval_source("import data from 'blob' with {type:'bytes',flavor:'x'};import {bytes} from 'barrel';var imported,other,names;import('blob',{with:{flavor:'x',type:'bytes'}}).then(ns=>{imported=ns.default;names=Object.keys(ns).join(',');});import('blob',{with:{type:'bytes'}}).then(ns=>other=ns.default);").unwrap();
    truth(
        &mut vm,
        "data===bytes && data===imported && data!==other && other[2]===3 && names==='default'",
    );
}

#[test]
fn importing_javascript_as_bytes_does_not_execute_it_or_expose_named_exports() {
    let loader = Rc::new(VirtualLoader::new());
    loader.insert("blob", "effects++;export const named=1;export default 9;");
    loader.insert(
        "bad",
        "effects++;import {named} from 'blob' with {type:'bytes'};",
    );
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(loader);
    vm.eval_source("var effects=0;").unwrap();
    assert!(
        vm.link_module("bad")
            .unwrap_err()
            .to_string()
            .contains("SyntaxError")
    );
    truth(
        &mut vm,
        "import bytes from 'blob' with {type:'bytes'};effects===0 && bytes[0]===101",
    );
    truth(&mut vm, "import value from 'blob';effects===1 && value===9");
}

struct Host {
    mismatch: bool,
}
impl ModuleLoader for Host {
    fn resolve(&self, specifier: &str, _: Option<&str>) -> Result<String, VmErr> {
        Ok(specifier.into())
    }
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
        Ok(ModuleSource {
            id: id.into(),
            source: "export default 1;".into(),
        })
    }
    fn load_bytes(
        &self,
        id: &str,
        _: &[(String, napi_vm_core::JsString)],
    ) -> Result<ModuleBytes, VmErr> {
        if self.mismatch {
            Ok(ModuleBytes {
                id: format!("{id}-changed"),
                bytes: vec![1],
            })
        } else {
            Err(VmErr::Msg("TypeError: binary access denied".into()))
        }
    }
}
#[test]
fn binary_loading_checks_canonical_identity_and_propagates_host_denial() {
    for mismatch in [true, false] {
        let mut vm = Interpreter::with_builtins();
        vm.set_module_loader(Rc::new(Host { mismatch }));
        vm.eval_source("var reason;import('blob',{with:{type:'bytes'}}).catch(e=>reason=e);")
            .unwrap();
        if mismatch {
            truth(&mut vm, "String(reason).includes('canonical identity')");
        } else {
            truth(
                &mut vm,
                "reason instanceof TypeError && reason.message==='binary access denied'",
            );
        }
    }
}

#[test]
fn static_deferred_imports_preserve_binary_and_host_attributes() {
    let loader = Rc::new(VirtualLoader::new());
    loader.insert_bytes("blob", vec![255, 128]);
    loader.insert("main","import defer * as ns from 'blob' with {type:'bytes'};export function read(){return ns.default[0];}");
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(loader);
    truth(&mut vm, "import {read} from 'main';read()===255");
}

#[test]
fn deferred_host_attributes_select_source_without_executing_the_body() {
    struct Attributed;
    impl ModuleLoader for Attributed {
        fn resolve(&self, id: &str, _: Option<&str>) -> Result<String, VmErr> {
            Ok(id.into())
        }
        fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
            Ok(ModuleSource {
                id: id.into(),
                source: "effects++;export const value=1;".into(),
            })
        }
        fn load_with_attributes(
            &self,
            id: &str,
            attributes: &[(String, napi_vm_core::JsString)],
        ) -> Result<ModuleSource, VmErr> {
            assert_eq!(attributes.len(), 1);
            assert_eq!(attributes[0].0, "flavor");
            assert_eq!(attributes[0].1, "selected");
            Ok(ModuleSource {
                id: id.into(),
                source: "effects++;export const value=42;".into(),
            })
        }
    }
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(Rc::new(Attributed));
    vm.eval_source("var effects=0;").unwrap();
    vm.define_module("main","import defer * as ns from 'dep' with {flavor:'selected'};export function read(){return ns.value;}".into());
    vm.load_module("main").unwrap();
    truth(&mut vm, "effects===0");
    truth(
        &mut vm,
        "import {read} from 'main';read()===42 && effects===1",
    );
}

#[test]
fn data_url_byte_modules_decode_binary_without_javascript_media_or_utf8() {
    use napi_vm_core::DataUrlLoader;
    for url in [
        "data:application/octet-stream;base64,AP+A",
        "data:image/png,%00%ff%80",
        "data:text/plain;charset=iso-8859-1,%00%ff%80",
        "data:,%00%FF%80#ignored",
    ] {
        let mut vm = Interpreter::with_builtins();
        vm.set_module_loader(Rc::new(DataUrlLoader::new(16)));
        let code = format!(
            "import bytes from {} with {{type:'bytes'}};bytes.buffer.immutable && bytes.length===3 && bytes[0]===0 && bytes[1]===255 && bytes[2]===128",
            serde_json::to_string(url).unwrap()
        );
        truth(&mut vm, &code);
    }
}
#[test]
fn data_url_byte_loading_preserves_validation_and_size_limits() {
    use napi_vm_core::DataUrlLoader;
    let loader = DataUrlLoader::new(2);
    for url in [
        "data:,%00%ff%80",
        "data:;base64,AP+A",
        "data:,%zz",
        "data:;base64,?",
        "data:;base64;base64,AA==",
    ] {
        assert!(loader.load_bytes(url, &[]).is_err(), "{url}");
    }
    assert!(loader.load("data:text/plain,1").is_err());
    assert!(loader.load("data:application/javascript,%ff").is_err());
    assert_eq!(loader.load("data:text/javascript,1").unwrap().source, "1");
}
