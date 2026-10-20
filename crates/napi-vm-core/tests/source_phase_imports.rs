use napi_vm_core::parser::{ImportPhase, ParseGoal, Statement, parse_cached_with_goal};
use napi_vm_core::{Interpreter, Value};
#[test]
fn static_source_import_grammar_distinguishes_contextual_binding_names() {
    for binding in ["value", "source", "from"] {
        for separator in [" ", "\n\r\u{2028}\u{2029}"] {
            let code = format!(
                "import{separator}source{separator}{binding} from 'dep' with {{flavor:'x'}};"
            );
            let ast = parse_cached_with_goal(&code, ParseGoal::Module).unwrap();
            assert!(
                matches!(&ast[0],Statement::Import{phase:ImportPhase::Source,default:Some(name),attributes,..} if name==binding && attributes.len()==1)
            );
        }
    }
    for binding in ["source", "from"] {
        let ast =
            parse_cached_with_goal(&format!("import {binding} from 'dep';"), ParseGoal::Module)
                .unwrap();
        assert!(
            matches!(&ast[0],Statement::Import{phase:ImportPhase::Evaluation,default:Some(name),..} if name==binding)
        );
    }
    for code in [
        "import source {x} from 'dep';",
        "import source * as ns from 'dep';",
        "import source x,y from 'dep';",
    ] {
        assert!(
            parse_cached_with_goal(code, ParseGoal::Module).is_err(),
            "{code}"
        );
    }
}
#[test]
fn unavailable_static_source_import_never_executes_module_bodies() {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source("var effects=0;").unwrap();
    vm.define_module("dep", "effects++;export default 42;".into());
    vm.define_module("main", "effects++;import source value from 'dep';".into());
    let error = vm.link_module("main").unwrap_err();
    assert!(error.to_string().contains("source-phase"), "{error}");
    assert!(matches!(
        vm.eval_source("effects").unwrap(),
        Value::Number(0.)
    ));
}

#[test]
fn host_module_source_brand_is_internal_realm_owned_and_not_inherited() {
    let mut vm = Interpreter::with_builtins();
    let source = vm.create_module_source_object("HostModule");
    vm.global.borrow_mut().set("source", source);
    let constructor = vm.abstract_module_source_constructor();
    vm.global.borrow_mut().set("Abstract", constructor);
    let result=vm.eval_source("var getter=Object.getOwnPropertyDescriptor(Abstract.prototype,Symbol.toStringTag).get;source instanceof Abstract && getter.call(source)==='HostModule' && Object.prototype.toString.call(source)==='[object HostModule]' && getter.call(Object.create(source))===undefined && getter.call(new Proxy(source,{}))===undefined && getter.call({ModuleSourceClassName:'Fake'})===undefined").unwrap();
    assert!(matches!(result, Value::Bool(true)));
    vm.eval_source("Object.freeze(source);").unwrap();
    assert!(
        matches!(vm.eval_source("getter.call(source)").unwrap(),Value::String(ref name) if name=="HostModule")
    );
    let other = vm.create_realm();
    vm.global
        .borrow_mut()
        .set("other", other.create_module_source_object("OtherModule"));
    assert!(matches!(
        vm.eval_source("!(other instanceof Abstract) && getter.call(other)==='OtherModule'")
            .unwrap(),
        Value::Bool(true)
    ));
}

struct SourceHost {
    calls: std::cell::Cell<usize>,
}
impl napi_vm_core::ModuleLoader for SourceHost {
    fn resolve(&self, id: &str, _: Option<&str>) -> Result<String, napi_vm_core::VmErr> {
        Ok(id.into())
    }
    fn load(&self, id: &str) -> Result<napi_vm_core::ModuleSource, napi_vm_core::VmErr> {
        Ok(napi_vm_core::ModuleSource {
            id: id.into(),
            source: "effects++;export default 42;".into(),
        })
    }
    fn load_source_object(
        &self,
        vm: &mut Interpreter,
        id: &str,
        _: &[(String, napi_vm_core::JsString)],
    ) -> Result<(String, Value), napi_vm_core::VmErr> {
        self.calls.set(self.calls.get() + 1);
        Ok((id.into(), vm.create_module_source_object("HostModule")))
    }
}
#[test]
fn static_dynamic_and_reexported_sources_share_identity_without_evaluation() {
    use std::rc::Rc;
    let host = Rc::new(SourceHost {
        calls: std::cell::Cell::new(0),
    });
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(host.clone());
    vm.eval_source("var effects=0;var dynamic;").unwrap();
    vm.define_module(
        "left",
        "import source value from 'dep';export {value};".into(),
    );
    vm.define_module(
        "right",
        "import source value from 'dep';export {value};".into(),
    );
    vm.define_module(
        "barrel",
        "export * from 'left';export * from 'right';".into(),
    );
    vm.eval_source("import {value} from 'barrel';import * as sourceNamespace from 'barrel';import source direct from 'dep';import.source('dep').then(v=>dynamic=v);").unwrap();
    assert!(matches!(vm.eval_source("value===direct && sourceNamespace.value===value && direct===dynamic && effects===0 && Object.prototype.toString.call(value)==='[object HostModule]'").unwrap(),Value::Bool(true)));
    assert_eq!(host.calls.get(), 1);
    vm.eval_source("import normal from 'dep';").unwrap();
    assert!(matches!(
        vm.eval_source("normal===42 && effects===1").unwrap(),
        Value::Bool(true)
    ));
    vm.eval_source("import.source('dep').then(v=>dynamic=v);")
        .unwrap();
    assert!(matches!(
        vm.eval_source("dynamic===direct && effects===1").unwrap(),
        Value::Bool(true)
    ));
    assert_eq!(host.calls.get(), 1);
}
#[test]
fn dynamic_source_loading_does_not_link_the_evaluation_graph() {
    use std::rc::Rc;
    struct Host;
    impl napi_vm_core::ModuleLoader for Host {
        fn resolve(&self, id: &str, _: Option<&str>) -> Result<String, napi_vm_core::VmErr> {
            Ok(id.into())
        }
        fn load(&self, _: &str) -> Result<napi_vm_core::ModuleSource, napi_vm_core::VmErr> {
            panic!("source import must not load evaluation text")
        }
        fn load_source_object(
            &self,
            vm: &mut Interpreter,
            id: &str,
            _: &[(String, napi_vm_core::JsString)],
        ) -> Result<(String, Value), napi_vm_core::VmErr> {
            Ok((id.into(), vm.create_module_source_object("OnlySource")))
        }
    }
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(Rc::new(Host));
    vm.eval_source("var value;import.source('dep').then(v=>value=v);")
        .unwrap();
    assert!(matches!(
        vm.eval_source("Object.prototype.toString.call(value)==='[object OnlySource]'")
            .unwrap(),
        Value::Bool(true)
    ));
}

#[test]
fn source_loader_rejects_invalid_identity_and_brand_without_caching_failures() {
    use std::{cell::Cell, rc::Rc};
    struct Host {
        attempt: Cell<usize>,
    }
    impl napi_vm_core::ModuleLoader for Host {
        fn resolve(&self, id: &str, _: Option<&str>) -> Result<String, napi_vm_core::VmErr> {
            Ok(id.into())
        }
        fn load(&self, _: &str) -> Result<napi_vm_core::ModuleSource, napi_vm_core::VmErr> {
            panic!("evaluation source requested")
        }
        fn load_source_object(
            &self,
            vm: &mut Interpreter,
            id: &str,
            _: &[(String, napi_vm_core::JsString)],
        ) -> Result<(String, Value), napi_vm_core::VmErr> {
            let n = self.attempt.get();
            self.attempt.set(n + 1);
            Ok(match n {
                0 => ("wrong".into(), vm.create_module_source_object("Host")),
                1 => (id.into(), Value::object(vec![])),
                _ => (id.into(), vm.create_module_source_object("Host")),
            })
        }
    }
    let host = Rc::new(Host {
        attempt: Cell::new(0),
    });
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(host.clone());
    vm.eval_source("var errors=0;var value;").unwrap();
    for _ in 0..2 {
        vm.eval_source("import.source('dep').catch(()=>errors++);")
            .unwrap();
    }
    vm.eval_source("import.source('dep').then(v=>value=v);")
        .unwrap();
    assert_eq!(host.attempt.get(), 3);
    assert!(matches!(
        vm.eval_source("errors===2 && Object.prototype.toString.call(value)==='[object Host]'")
            .unwrap(),
        Value::Bool(true)
    ));
    vm.eval_source("import source immutable from 'dep';var rejected=false;try{immutable=1;}catch(e){rejected=e instanceof TypeError;}").unwrap();
    assert!(matches!(
        vm.eval_source("immutable===value && rejected").unwrap(),
        Value::Bool(true)
    ));
}

#[test]
fn javascript_without_a_source_representation_rejects_with_syntax_error() {
    use std::rc::Rc;
    let loader = Rc::new(napi_vm_core::VirtualLoader::new());
    loader.insert("dep", "throw new Error('must not execute');");
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(loader);
    vm.eval_source("var reason;import.source('dep').catch(e=>reason=e);")
        .unwrap();
    assert!(matches!(
        vm.eval_source("reason instanceof SyntaxError").unwrap(),
        Value::Bool(true)
    ));
}

#[test]
fn deferred_readiness_ignores_source_edges_to_an_evaluating_module() {
    use std::rc::Rc;
    let host = Rc::new(SourceHost {
        calls: std::cell::Cell::new(0),
    });
    let mut vm = Interpreter::with_builtins();
    vm.set_module_loader(host);
    vm.eval_source("var release;var gate=new Promise(r=>release=r);var reason;var result;")
        .unwrap();
    vm.define_module("dep", "await gate;export const value=1;".into());
    vm.define_module(
        "root",
        "import source value from 'dep';export const tag=Object.prototype.toString.call(value);"
            .into(),
    );
    vm.eval_source("import('dep');import.defer('root').then(ns=>result=ns.tag,e=>reason=e);")
        .unwrap();
    assert!(matches!(
        vm.eval_source("result==='[object HostModule]' && reason===undefined")
            .unwrap(),
        Value::Bool(true)
    ));
    vm.eval_source("release();").unwrap();
}
