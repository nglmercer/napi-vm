use napi_vm::{Interpreter, Value};

#[test]
fn host_main_updates_preserve_cached_meta_identity() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source("var savedMeta = import.meta; savedMeta.custom = 42;")
        .unwrap();
    for main in [true, false, true] {
        vm.set_import_meta_main(main).unwrap();
        let result = vm
            .eval_source(&format!(
                "savedMeta === import.meta && savedMeta.custom === 42 && savedMeta.main === {main};"
            ))
            .unwrap();
        assert!(matches!(result, Value::Bool(true)));
    }
}
