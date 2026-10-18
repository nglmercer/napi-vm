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
fn check_throws(source: &str, class: &str) {
    let mut vm = Interpreter::with_builtins();
    let probe =
        format!("var result=\"no-throw\";try{{{source}}}catch(e){{result=e.constructor.name;}}");
    vm.eval_source(&probe).unwrap();
    vm.collect_cycles();
    let result = vm.eval_source("JSON.stringify(result)").unwrap();
    assert!(
        matches!(&result, Value::String(s) if *s == format!("\"{class}\"")),
        "{result:?}"
    );
}
#[test]
fn display_names_requires_type_option() {
    check_throws("new Intl.DisplayNames('en', undefined)", "TypeError");
    check_throws("new Intl.DisplayNames('en', {})", "TypeError");
    check_throws("new Intl.DisplayNames('en', null)", "TypeError");
    check_throws(
        "new Intl.DisplayNames('en', {type: 'dayPeriod'})",
        "RangeError",
    );
    check_throws("Intl.DisplayNames('en', {type: 'language'})", "TypeError");
}
#[test]
fn display_names_resolves_default_options() {
    check(
        "var result=new Intl.DisplayNames('en-US', {type: 'language'}).resolvedOptions();",
        r#"{"locale":"en-US","style":"long","type":"language","fallback":"code","languageDisplay":"dialect"}"#,
    );
    check(
        "var result=new Intl.DisplayNames('en-US', {type: 'region', style: 'narrow', fallback: 'none', languageDisplay: 'standard'}).resolvedOptions();",
        r#"{"locale":"en-US","style":"narrow","type":"region","fallback":"none","languageDisplay":"standard"}"#,
    );
}
#[test]
fn display_names_of_accepts_structural_codes() {
    check(
        "var d=new Intl.DisplayNames(undefined, {type: 'language'});var result=[typeof d.of('en'),typeof d.of('en-Latn-US-posix-1abc'),typeof d.of('zzzzzzzz')];",
        r#"["string","string","string"]"#,
    );
    check(
        "var d=new Intl.DisplayNames(undefined, {type: 'region'});var result=[typeof d.of('US'),typeof d.of('419')];",
        r#"["string","string"]"#,
    );
    check(
        "var d=new Intl.DisplayNames(undefined, {type: 'calendar'});var result=typeof d.of('gregory');",
        r#""string""#,
    );
    check(
        "var d=new Intl.DisplayNames(undefined, {type: 'dateTimeField'});var result=typeof d.of('timeZoneName');",
        r#""string""#,
    );
}
#[test]
fn display_names_of_rejects_malformed_codes() {
    check_throws(
        "new Intl.DisplayNames(undefined, {type: 'language'}).of('en-u-hebrew')",
        "RangeError",
    );
    check_throws(
        "new Intl.DisplayNames(undefined, {type: 'language'}).of('aa-aaaaa-aaaaa')",
        "RangeError",
    );
    check_throws(
        "new Intl.DisplayNames(undefined, {type: 'language'}).of('abcd')",
        "RangeError",
    );
    check_throws(
        "new Intl.DisplayNames(undefined, {type: 'language'}).of('root')",
        "RangeError",
    );
    check_throws(
        "new Intl.DisplayNames(undefined, {type: 'region'}).of('aaa')",
        "RangeError",
    );
    check_throws(
        "new Intl.DisplayNames(undefined, {type: 'calendar'}).of('00')",
        "RangeError",
    );
    check_throws(
        "new Intl.DisplayNames(undefined, {type: 'dateTimeField'}).of('timezoneName')",
        "RangeError",
    );
}
#[test]
fn display_names_currency_fallback_none_returns_undefined_for_unknown() {
    check(
        "var d=new Intl.DisplayNames('en', {type: 'currency', fallback: 'none'});var result=[typeof d.of('USD'),d.of('XXX'),typeof d.of('EUR')];",
        r#"["string",null,"string"]"#,
    );
    check(
        "var d=new Intl.DisplayNames('en', {type: 'currency'});var result=typeof d.of('XXX');",
        r#""string""#,
    );
}
#[test]
fn display_names_rejects_bad_options() {
    check_throws(
        "new Intl.DisplayNames('en', {type: 'language', style: 'wide'})",
        "RangeError",
    );
    check_throws(
        "new Intl.DisplayNames('en', {type: 'language', fallback: 'code-only'})",
        "RangeError",
    );
    check_throws(
        "new Intl.DisplayNames('en', {type: 'language', localeMatcher: 'best'})",
        "RangeError",
    );
    check_throws(
        "new Intl.DisplayNames('en', {type: 'language', languageDisplay: 'formal'})",
        "RangeError",
    );
}
