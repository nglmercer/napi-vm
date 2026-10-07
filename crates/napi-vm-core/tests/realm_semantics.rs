use napi_vm_core::{Interpreter, Value};

fn truth(vm: &mut Interpreter, source: &str) {
    let result = vm.eval_source(source);
    assert!(
        matches!(result, Ok(Value::Bool(true))),
        "{source}: {result:?}"
    );
}

#[test]
fn fresh_realms_isolate_globals_intrinsics_and_function_receivers() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    let global = child.realm_global_object();
    vm.set_global_checked("otherGlobal", global.clone())
        .unwrap();
    vm.eval_in_realm(
        &global,
        "var realmSecret=17; function realmFunction(){return this;}",
    )
    .unwrap();
    truth(
        &mut vm,
        "otherGlobal!==globalThis && otherGlobal.globalThis===otherGlobal && otherGlobal.realmSecret===17 && typeof realmSecret==='undefined';",
    );
    truth(
        &mut vm,
        "otherGlobal.Object!==Object && otherGlobal.Array!==Array && otherGlobal.Object.prototype!==Object.prototype;",
    );
    truth(
        &mut vm,
        "var f=otherGlobal.realmFunction;f()===otherGlobal && f.call(null)===otherGlobal;",
    );
    truth(
        &mut vm,
        "Object.getOwnPropertyDescriptor(otherGlobal,'realmSecret').value===17 && Object.prototype.hasOwnProperty.call(otherGlobal,'realmSecret');",
    );
    truth(
        &mut vm,
        "otherGlobal.realmSecret=19;otherGlobal.realmSecret===19;",
    );
    truth(&mut child, "realmSecret===19;");
    truth(
        &mut vm,
        "Object.getPrototypeOf(new otherGlobal.Array())===otherGlobal.Array.prototype;",
    );
    truth(
        &mut vm,
        "Object.getPrototypeOf(otherGlobal)===otherGlobal.Object.prototype;",
    );
    truth(
        &mut vm,
        "Object.getPrototypeOf(otherGlobal.Array.prototype)===otherGlobal.Object.prototype;",
    );
}

#[test]
fn escaped_realm_values_keep_their_intrinsics_after_interpreter_drop() {
    let mut vm = Interpreter::with_builtins();
    let global = {
        let mut child = vm.create_realm();
        child
            .eval_source("var object={x:1};var array=[];function f(){return [this,{}];}")
            .unwrap();
        child.realm_global_object()
    };
    vm.set_global_checked("otherGlobal", global).unwrap();
    vm.collect_cycles();
    truth(
        &mut vm,
        "Object.getPrototypeOf(otherGlobal.object)===otherGlobal.Object.prototype && Object.getPrototypeOf(otherGlobal.array)===otherGlobal.Array.prototype;",
    );
    truth(
        &mut vm,
        "var f=otherGlobal.f;var result=f();result[0]===otherGlobal && Object.getPrototypeOf(result)===otherGlobal.Array.prototype && Object.getPrototypeOf(result[1])===otherGlobal.Object.prototype;",
    );
    let result = vm.eval_in_realm(
        &vm.global_value("otherGlobal").unwrap(),
        "throw new Error('realm error');",
    );
    assert!(result.is_err());
    truth(
        &mut vm,
        "this===globalThis && Object.getPrototypeOf({})===Object.prototype;",
    );
}

#[test]
fn foreign_eval_keeps_its_realm_and_cannot_become_direct_eval() {
    let mut vm = Interpreter::with_builtins();
    let child = vm.create_realm();
    vm.set_global_checked("otherGlobal", child.realm_global_object())
        .unwrap();
    truth(
        &mut vm,
        "var eval=otherGlobal.eval;function f(){var local=17;return eval('typeof local');}f()==='undefined';",
    );
    truth(
        &mut vm,
        "eval('var otherSecret=42');otherGlobal.otherSecret===42 && typeof otherSecret==='undefined';",
    );
    truth(&mut vm, "eval('this')===otherGlobal;");
}

#[test]
fn constructor_default_prototypes_come_from_the_new_target_realm() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    child
        .eval_source("function F(){}F.prototype=undefined;")
        .unwrap();
    vm.set_global_checked("otherGlobal", child.realm_global_object())
        .unwrap();
    for kind in ["Map", "Set", "WeakMap", "WeakSet"] {
        truth(
            &mut vm,
            &format!(
                "Object.getPrototypeOf(Reflect.construct({kind},[],otherGlobal.F))===otherGlobal.{kind}.prototype;"
            ),
        );
    }
    truth(
        &mut vm,
        "function F(){}Object.getPrototypeOf(Reflect.construct(F,[],otherGlobal.F))===otherGlobal.Object.prototype;",
    );
    truth(
        &mut vm,
        "var bound=otherGlobal.F.bind(null);bound.prototype=undefined;Object.getPrototypeOf(Reflect.construct(F,[],bound))===otherGlobal.Object.prototype;",
    );
    truth(
        &mut vm,
        "function G(){}Object.defineProperty(G,'prototype',{value:1});Object.getPrototypeOf(Reflect.construct(WeakMap,[],G))===WeakMap.prototype;",
    );
}

#[test]
fn foreign_generators_and_async_resumes_allocate_in_the_defining_realm() {
    let mut vm = Interpreter::with_builtins();
    let mut child = vm.create_realm();
    child
        .eval_source("function* g(){yield {};yield [];}async function f(){await 0;return [];}")
        .unwrap();
    vm.set_global_checked("otherGlobal", child.realm_global_object())
        .unwrap();
    truth(
        &mut vm,
        "var g=otherGlobal.g();var a=g.next().value;var b=g.next().value;Object.getPrototypeOf(a)===otherGlobal.Object.prototype && Object.getPrototypeOf(b)===otherGlobal.Array.prototype;",
    );
    truth(
        &mut vm,
        "var result=await otherGlobal.f();Object.getPrototypeOf(result)===otherGlobal.Array.prototype;",
    );
}
