use napi_vm::{Interpreter, Value};

#[test]
fn empty_eval_preserves_completion_directive_boundaries_and_errors() {
    let source = r#"
      var indirect = eval;
      var good = eval('{};{{;}}/*comment*/') === undefined && indirect(';;{}') === undefined;
      good = good && eval('1;{};') === 1;
      good = good && eval('{};"use strict"; undeclaredEmptyEval = 9') === 9;
      var invalid = ['{', '}', '{} }', '/*', '{ // }'];
      for (var i = 0; i < invalid.length; i++) {
        try {eval(invalid[i]); good = false;} catch(e) {good = good && e instanceof SyntaxError;}
      }
      function local() {var x = 7; eval('{{;}}'); return x;}
      good && local() === 7;
    "#;
    let prepared = Interpreter::compile(source).unwrap();
    assert!(matches!(
        Interpreter::with_builtins().execute(&prepared).unwrap(),
        Value::Bool(true)
    ));
}
