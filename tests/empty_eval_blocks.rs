use napi_vm::{Interpreter, Value};
fn check(source: &str) {
    let mut vm = Interpreter::with_builtins();
    assert!(matches!(vm.eval_source(source).unwrap(), Value::Bool(true)));
}
#[test]
fn empty_blocks_have_undefined_completion_and_keep_other_completions() {
    check(
        r#"
eval('{}{}')===undefined && eval('{ \t\r\n } { }')===undefined && eval('9;{}')===9;
"#,
    );
}
#[test]
fn meaningful_nested_and_commented_sources_use_normal_semantics() {
    check(
        r#"
var x=1;eval('{}; x=7; {}');eval('{{x++;}}');
x===8 && eval('{/*comment*/}')===undefined && eval('\ufeff{}')===undefined;
"#,
    );
}
#[test]
fn malformed_suffixes_are_syntax_errors_before_execution() {
    check(
        r#"
var x=0,errors=0;
for(var s of ['{}{','}{',"{} 'unterminated",'x=1;{}(']){
 try{eval(s);}catch(e){if(e instanceof SyntaxError)errors++;}
}
x===0 && errors===4;
"#,
    );
}
#[test]
fn strict_and_indirect_eval_do_not_change_caller_bindings() {
    check(
        r#"
var x=2;function f(){'use strict';let x=7;return eval('{}{}')===undefined && x===7 && (0,eval)('{}{}')===undefined;}
f() && x===2;
"#,
    );
}
#[test]
fn large_generated_block_sequences_evaluate_normally() {
    check("eval('{}'.repeat(262144))===undefined;");
}
