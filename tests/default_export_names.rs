use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str, observed: Option<&str>) {
    let program = Interpreter::compile(source).unwrap();
    assert_eq!(
        program.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    for ast in [false, true] {
        let mut vm = Interpreter::with_builtins();
        vm.begin_module("test");
        if ast {
            vm.run_program_body(&statements).unwrap();
        } else {
            vm.execute(&program).unwrap();
        }
        let value = vm.module("test").unwrap().default.unwrap();
        let name = value.get_prop("name");
        assert!(
            matches!(name,Some(Value::String(ref name)) if name==expected),
            "AST={ast}, name={name:?}, {source}"
        );
        if let Some(expected) = observed {
            let observed = value.get_prop("observed");
            assert!(
                matches!(observed,Some(Value::String(ref name)) if name==expected),
                "AST={ast}, observed={observed:?}"
            );
        }
    }
}

#[test]
fn anonymous_default_declarations_have_the_public_default_name() {
    for source in [
        "export default function(){}",
        "export default async function(){}",
        "export default function*(){}",
        "export default async function*(){}",
        "export default class {}",
    ] {
        both(source, "default", None);
    }
}

#[test]
fn anonymous_default_class_static_initializers_observe_default() {
    both(
        "export default class {static observed=this.name;}",
        "default",
        Some("default"),
    );
    both(
        "export default (class {static observed=this.name;})",
        "default",
        Some("default"),
    );
    both(
        "export default class {static observed=this.name;static name='chosen';}",
        "chosen",
        Some("default"),
    );
}

#[test]
fn default_expression_name_inference_preserves_explicit_and_existing_names() {
    for source in [
        "export default (()=>{})",
        "export default (function(){})",
        "export default (async function(){})",
        "export default (function*(){})",
    ] {
        both(source, "default", None);
    }
    both("export default function Named(){}", "Named", None);
    both(
        "export default class Named {static observed=Named.name;}",
        "Named",
        Some("Named"),
    );
    both(
        "const existing=()=>{};export default existing;",
        "existing",
        None,
    );
    both(
        "const obj={'*default*':class{}};export default obj['*default*'];",
        "*default*",
        None,
    );
    both(
        "const obj={'*default*':function(){}};export default obj['*default*'];",
        "*default*",
        None,
    );
}

#[test]
fn linked_module_default_names_are_visible_through_imports() {
    let mut vm = Interpreter::with_builtins();
    vm.define_module("fn", "export default function(){}".into());
    vm.define_module(
        "cls",
        "export default class {static observed=this.name;}".into(),
    );
    let value = vm
        .eval_source("import f from 'fn';import C from 'cls';[f.name,C.name,C.observed].join('|')")
        .unwrap();
    assert!(matches!(value,Value::String(ref value) if value=="default|default|default"));
}
