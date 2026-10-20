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
