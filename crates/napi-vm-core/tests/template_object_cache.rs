use napi_vm_core::{Interpreter, Lexer, Parser, Value};
fn run(source: &str, require_bytecode: bool) {
    for ast in [true, false] {
        let mut vm = Interpreter::with_builtins();
        let result = if ast {
            let mut parser = Parser::new(Lexer::new(source).tokenize());
            vm.run_program_body(&parser.parse())
        } else {
            let p = Interpreter::compile(source).unwrap();
            if require_bytecode {
                assert_eq!(p.tier(), napi_vm_core::interpreter::ExecutionTier::Bytecode);
            }
            vm.execute(&p)
        };
        assert!(
            matches!(result, Ok(Value::Bool(true))),
            "AST={ast}: {result:?}"
        );
    }
}
fn both(source: &str) {
    run(source, true);
}
#[test]
fn sites_reuse_across_calls_and_factory_functions_but_distinct_sites_do_not() {
    both(
        "var seen=[];function tag(p){seen.push(p);}function factory(){return function(x){tag`a${x}b`;};}factory()(1);factory()(2);for(var i=0;i<2;i++)tag`a${i}b`;seen[0]===seen[1] && seen[2]===seen[3] && seen[0]!==seen[2] && Object.isFrozen(seen[0]) && Object.isFrozen(seen[0].raw)",
    );
}
#[test]
fn each_eval_has_distinct_sites_but_reuses_sites_within_one_eval() {
    run(
        "var seen=[];function tag(p){seen.push(p);}for(var i=0;i<2;i++)eval('for(var j=0;j<2;j++)tag`x${j}`');seen[0]===seen[1] && seen[2]===seen[3] && seen[0]!==seen[2]",
        false,
    );
}
#[test]
fn function_constructor_parse_sites_are_distinct() {
    both(
        "function tag(p){return p;}var a=Function('tag','return tag`x`');var b=Function('tag','return tag`x`');a(tag)===a(tag) && a(tag)!==b(tag)",
    );
}
#[test]
fn prepared_site_objects_are_separate_in_each_realm() {
    let p = Interpreter::compile("function tag(p){return p;}var value=tag`realm`;value").unwrap();
    let mut a = Interpreter::with_builtins();
    let mut b = Interpreter::with_builtins();
    let left = a.execute(&p).unwrap();
    let right = b.execute(&p).unwrap();
    let (Value::Array(left), Value::Array(right)) = (&left, &right) else {
        panic!("arrays expected")
    };
    assert!(!std::rc::Rc::ptr_eq(left, right));
}
#[test]
fn cache_retains_template_identity_across_cycle_collection() {
    let p = Interpreter::compile(
        "function tag(p){return p;}function site(){return tag`retained`;}site()",
    )
    .unwrap();
    let mut vm = Interpreter::with_builtins();
    let first = vm.execute(&p).unwrap();
    let weak = if let Value::Array(array) = &first {
        std::rc::Rc::downgrade(array)
    } else {
        panic!("array expected")
    };
    drop(first);
    vm.collect_cycles();
    assert!(weak.upgrade().is_some());
    let second = vm.eval_source("site()").unwrap();
    let Value::Array(array) = &second else {
        panic!("array expected")
    };
    assert!(std::rc::Rc::ptr_eq(array, &weak.upgrade().unwrap()));
}
