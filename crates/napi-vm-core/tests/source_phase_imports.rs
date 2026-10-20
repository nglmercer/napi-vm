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
