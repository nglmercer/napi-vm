use napi_vm::{Interpreter, Value};
fn check(source: &str, expected: &str) {
    let mut vm = Interpreter::with_builtins();
    vm.eval_source(source).unwrap();
    vm.collect_cycles();
    let result = vm.eval_source("JSON.stringify(result)").unwrap();
    assert!(
        matches!(&result, Value::String(s) if s == expected),
        "{result:?}"
    );
}
#[test]
fn intl_legacy_constructors_ignore_this_and_call() {
    check(
        "var result=[typeof Intl.Collator(),typeof Intl.NumberFormat(),typeof Intl.DateTimeFormat()];",
        r#"["object","object","object"]"#,
    );
    check(
        "var o=new Intl.Collator();var n=Intl.DateTimeFormat.call(o);var result=[o!==n,n instanceof Intl.DateTimeFormat];",
        r#"[true,true]"#,
    );
}
#[test]
fn intl_modern_constructors_require_new() {
    check(
        "var result=['PluralRules','RelativeTimeFormat','ListFormat','DisplayNames','Segmenter','Locale','DurationFormat'].map(function(n){try{Intl[n]();return 'callable';}catch(e){return e.constructor.name;}});",
        r#"["TypeError","TypeError","TypeError","TypeError","TypeError","TypeError","TypeError"]"#,
    );
}
#[test]
fn intl_sparse_array_length_without_allocation() {
    check(
        "var r=['en-US','fr'];r.length=Math.pow(2,32)-1;var huge=r.length;r.length=2;var result=[huge,r.length,r.join('|')];",
        r#"[4294967295,2,"en-US|fr"]"#,
    );
    check("var result=new Array(4294967295).length;", r#"4294967295"#);
}
