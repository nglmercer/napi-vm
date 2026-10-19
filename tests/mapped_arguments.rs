use napi_vm::{Interpreter, Lexer, Parser, Value};

fn both(source: &str, expected: &str) {
    let prepared = Interpreter::compile(source).unwrap();
    let result = Interpreter::with_builtins().execute(&prepared).unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "prepared {source}: {result:?}"
    );
    let statements = Parser::new(Lexer::new(source).tokenize())
        .parse_program()
        .unwrap();
    let result = Interpreter::with_builtins()
        .run_program_body(&statements)
        .unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "AST {source}: {result:?}"
    );
}

#[test]
fn parameter_and_index_assignments_share_a_cell() {
    both(
        "function f(a){a=2;var x=arguments[0];arguments[0]=3;return x+','+a;}f(1)",
        "2,3",
    );
}

#[test]
fn descriptors_observe_the_current_parameter_value() {
    both(
        "function f(a){a=2;Object.defineProperty(arguments,'0',{value:3});var d=Object.getOwnPropertyDescriptor(arguments,'0');return a+','+d.value;}f(1)",
        "3,3",
    );
}

#[test]
fn deleting_an_index_permanently_breaks_its_mapping() {
    both(
        "function f(a){delete arguments[0];a=2;arguments[0]=3;return a+','+arguments[0];}f(1)",
        "2,3",
    );
}

#[test]
fn readonly_descriptors_snapshot_and_disconnect() {
    both(
        "function f(a){a=2;Object.defineProperty(arguments,'0',{writable:false});a=3;arguments[0]=4;return a+','+arguments[0];}function g(a){Object.defineProperty(arguments,'0',{value:5,writable:false});a=6;return a+','+arguments[0];}f(1)+';'+g(1)",
        "3,2;6,5",
    );
}

#[test]
fn accessors_disconnect_the_parameter() {
    both(
        "function f(a){Object.defineProperty(arguments,'0',{get:function(){return 9;},set:function(v){}});a=2;arguments[0]=3;return a+','+arguments[0];}f(1)",
        "2,9",
    );
}

#[test]
fn seal_preserves_mapping_and_freeze_disconnects() {
    both(
        "function f(a){Object.seal(arguments);a=2;var x=arguments[0];Object.freeze(arguments);a=3;return x+','+arguments[0]+','+a;}f(1)",
        "2,2,3",
    );
}

#[test]
fn only_the_last_duplicate_parameter_maps_and_only_supplied_indexes_exist() {
    both(
        "function f(a,a){a=3;arguments[0]=4;return a+','+arguments[0]+','+arguments[1];}f(1,2)+';'+f(1)",
        "3,4,3;3,4,undefined",
    );
}

#[test]
fn strict_and_nonsimple_parameters_are_unmapped() {
    both(
        "function f(a){'use strict';a=2;return arguments[0];}function g(a=1){a=2;return arguments[0];}function h(a,...rest){a=2;return arguments[0];}function j({a}){a=2;return arguments[0].a;}[f(1),g(1),h(1),j({a:1})].join(',')",
        "1,1,1,1",
    );
}

#[test]
fn escaped_arguments_and_closures_keep_the_shared_cell() {
    both(
        "function f(a){return [arguments,function(v){a=v;return a;}];}var pair=f(1);pair[1](2);var x=pair[0][0];pair[0][0]=3;[x,pair[1](4),pair[0][0]].join(',')",
        "2,4,4",
    );
}

#[test]
fn distinct_receivers_do_not_write_the_parameter() {
    both(
        "function f(a){var child={};Reflect.set(arguments,'0',2,child);return a+','+arguments[0]+','+child[0];}f(1)",
        "1,1,2",
    );
}

#[test]
fn generator_parameters_remain_mapped_across_suspension() {
    both(
        "function* f(a){yield arguments;a=2;yield arguments[0];arguments[0]=3;return a;}var g=f(1),args=g.next().value;args[0]=4;[g.next().value,g.next().value,args[0]].join(',')",
        "2,3,3",
    );
}

#[test]
fn arrow_arguments_refer_to_the_enclosing_parameter_map() {
    both(
        "function f(a){var get=()=>arguments;a=2;get()[0]=3;return a+','+get()[0];}f(1)",
        "3,3",
    );
}

#[test]
fn ordinary_parameters_and_hot_index_writes_use_verified_bytecode() {
    let source =
        "function f(a){for(var i=0;i<100;i++){arguments[0]=i;}a++;return a+','+arguments[0];}f(1)";
    let prepared = Interpreter::compile(source).unwrap();
    assert_eq!(
        prepared.tier(),
        napi_vm::interpreter::ExecutionTier::Bytecode
    );
    let mut interp = Interpreter::with_builtins();
    interp.execute(&prepared).unwrap();
    let function = interp.global.borrow().get("f").unwrap();
    assert!(matches!(&function, Value::Function(data) if data.bytecode.is_some()));
    both(source, "100,100");
}

#[test]
fn mapped_cells_trace_parameter_objects_and_release_replacements() {
    let mut vm = Interpreter::with_builtins();
    let record = vm
        .eval_source("var record={};record.self=record;record")
        .unwrap();
    let weak = match &record {
        Value::Object { props } => std::rc::Rc::downgrade(props),
        _ => unreachable!(),
    };
    vm.eval_source("function f(a){return arguments;}var keep=f(record);record=null;")
        .unwrap();
    drop(record);
    assert!(vm.collect_cycles().skipped.is_none());
    assert!(weak.upgrade().is_some());
    vm.eval_source("keep[0]=null;").unwrap();
    assert!(vm.collect_cycles().skipped.is_none());
    assert!(weak.upgrade().is_none());
}

#[test]
fn unreachable_arguments_parameter_cycles_are_collected() {
    let mut vm = Interpreter::with_builtins();
    let object = vm
        .eval_source("function f(a){a=arguments;return arguments;}var keep=f(null);keep")
        .unwrap();
    let weak = match &object {
        Value::Object { props } => std::rc::Rc::downgrade(props),
        _ => unreachable!(),
    };
    drop(object);
    vm.eval_source("keep=null;").unwrap();
    assert!(vm.collect_cycles().skipped.is_none());
    assert!(weak.upgrade().is_none());
}

#[test]
fn failed_deletion_and_descriptor_changes_preserve_the_mapping() {
    both(
        "function f(a){Object.defineProperty(arguments,'0',{configurable:false});var deleted=delete arguments[0];var changed=Reflect.defineProperty(arguments,'0',{get:function(){return 9;}});a=2;var x=arguments[0];arguments[0]=3;return deleted+','+changed+','+x+','+a;}f(1)",
        "false,false,2,3",
    );
}

#[test]
fn proxies_forward_writes_to_mapped_indexes() {
    both(
        "function f(a){var p=new Proxy(arguments,{});p[0]=2;a=3;return p[0]+','+a;}f(1)",
        "3,3",
    );
}

#[test]
fn direct_eval_sees_the_same_arguments_and_parameter_cells() {
    both(
        "function f(a){return eval('arguments[0]=2; a=3; String(arguments[0])');}f(1)",
        "3",
    );
}

#[test]
fn reassigning_arguments_does_not_disconnect_escaped_indexes() {
    both(
        "function f(a){var old=arguments;arguments=null;a=2;return old[0]+','+JSON.stringify(old);}f(1)",
        "2,{\"0\":2}",
    );
}

#[test]
fn var_arguments_preserves_the_object_and_other_bindings_shadow_it() {
    both(
        "function f(a){var arguments;a=2;return arguments[0];}function g(arguments){return arguments;}function h(){let arguments=4;return arguments;}function j(){function arguments(){return 5;}return arguments();}[f(1),g(3),h(),j()].join(',')",
        "2,3,4,5",
    );
}

#[test]
fn duplicate_parameters_keep_the_last_initial_value_before_any_write() {
    both(
        "function f(a,a,a){return [a,arguments[0],arguments[1],arguments[2]].join(',');}function g(a,a){return [a,arguments[0],arguments[1]].join(',');}f(1,2,3)+';'+g(1)",
        "3,1,2,3;,1,",
    );
    both(
        "function f(a,a,a){arguments[Symbol.isConcatSpreadable]=true;return [].concat(arguments,arguments).join(',');}f(1,2,3)",
        "1,2,3,1,2,3",
    );
}
