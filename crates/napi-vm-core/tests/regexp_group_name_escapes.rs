use napi_vm_core::{Interpreter, Value};
#[test]
fn group_names_use_unicode_escape_rules_without_unicode_flag() {
    let mut vm = Interpreter::with_builtins();
    let value=vm.eval_source(r#"var ok=true;for(var flags of ['', 'u','v']){var a=new RegExp('(?<\\u{61}>x)\\k<\\u{61}>',flags).exec('xx');var b=new RegExp('(?<\\u{1d4d1}>brown)',flags).exec('brown');var c=new RegExp('(?<\\ud835\\udcd1>brown)',flags).exec('brown');ok=ok && a.groups.a==='x' && b.groups['𝓑']==='brown' && c.groups['𝓑']==='brown';}ok"#).unwrap();
    assert!(matches!(value, Value::Bool(true)));
}
#[test]
fn capture_name_escapes_do_not_accept_legacy_identity_fallback() {
    let mut vm = Interpreter::with_builtins();
    let value=vm.eval_source(r#"var ok=true;for(var name of ['\\u','\\u12G4','\\u{}','\\u{110000}']){var caught=false;try{new RegExp('(?<'+name+'>x)');}catch(e){caught=e instanceof SyntaxError;}ok=ok && caught;}ok && new RegExp('\\u{2}').test('uu')"#).unwrap();
    assert!(matches!(value, Value::Bool(true)));
}
