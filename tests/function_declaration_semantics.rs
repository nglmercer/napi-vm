use napi_vm::{Interpreter, Value};
fn check(source: &str, expected: &str) {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(source).unwrap();
    let result = vm.eval_source("JSON.stringify(result)").unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "{result:?}"
    );
}
#[test]
fn declarations_preserve_hoisted_identity_and_prior_assignments() {
    check(
        r#"f.prototype=[1,2];var before=f;function f(){}var instance=new f();var result=[before===f,instance.length,instance.map(x=>x+1).join(',')];"#,
        r#"[true,2,"2,3"]"#,
    );
}
#[test]
fn block_functions_update_the_variable_only_when_the_statement_executes() {
    check(
        r#"var result=(function(){var before=f;{var blockBefore=f;function f(){return 7}}return [before,blockBefore===f,f()];})();"#,
        r#"[null,true,7]"#,
    );
}
#[test]
fn parameters_and_enclosing_lexicals_suppress_annex_b_bindings() {
    check(
        r#"var result=[(function(f){{function f(){}}return f})(3),(function(){{let f;{function f(){}}}return typeof f})(),(function(){'use strict';{function f(){}}return typeof f})()];"#,
        r#"[3,"undefined","undefined"]"#,
    );
}
