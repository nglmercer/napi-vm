use napi_vm_core::{Interpreter, Value, parser::parse_cached};

fn yes(source: &str) {
    for ast in [false, true] {
        let mut vm = Interpreter::with_builtins();
        let dda = vm.create_html_dda();
        vm.global.borrow_mut().set("dda", dda);
        let result = if ast {
            vm.run_program_body(&parse_cached(source).unwrap())
        } else {
            vm.eval_source(source)
        }
        .unwrap();
        assert!(matches!(result, Value::Bool(true)), "{source}: {result:?}");
    }
}

#[test]
fn html_dda_only_changes_boolean_typeof_and_loose_null_comparisons() {
    yes(
        "!dda && Boolean(dda)===false && typeof dda==='undefined' && dda==null && null==dda && dda==undefined && undefined==dda && dda!==undefined && dda!==null && dda===dda && (dda ?? 42)===dda && dda?.()===null",
    );
}

#[test]
fn html_dda_remains_an_object_for_properties_and_iteration_protocols() {
    yes(
        "Object.defineProperty(dda,'answer',{value:42});var received;function f(value=99){received=value;}f(dda);var caught=false;try{Array.from({[Symbol.iterator]:dda});}catch(e){caught=e instanceof TypeError;}dda.answer===42 && Object(dda)===dda && received===dda && dda()===null && dda('')===null && caught",
    );
}

#[test]
fn html_dda_brand_is_neither_inherited_nor_forwarded_by_a_proxy() {
    yes(
        "var child=Object.create(dda);var proxy=new Proxy(dda,{});Boolean(child) && Boolean(proxy) && typeof child==='object' && typeof proxy==='function' && child!=null && proxy!=null && proxy()===null",
    );
}
