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
fn collator_unsupported_option_falls_back_to_supported_extension() {
    check(
        "var r=new Intl.Collator('de-u-co-phonebk', {collation: 'pinyin'}).resolvedOptions();var result=[r.locale,r.collation];",
        r#"["de-u-co-phonebk","phonebk"]"#,
    );
    check(
        "var r=new Intl.Collator('en-u-co-phonebk', {collation: 'pinyin'}).resolvedOptions();var result=[r.locale,r.collation];",
        r#"["en","default"]"#,
    );
}
#[test]
fn collator_supported_option_replaces_extension() {
    check(
        "var r=new Intl.Collator('de-u-co-phonebk', {collation: 'eor'}).resolvedOptions();var result=[r.locale,r.collation];",
        r#"["de","eor"]"#,
    );
    check(
        "var r=new Intl.Collator('de-u-co-phonebk', {collation: 'phonebk'}).resolvedOptions();var result=[r.locale,r.collation];",
        r#"["de-u-co-phonebk","phonebk"]"#,
    );
}
#[test]
fn collator_german_search_sorts_like_phonebook() {
    check(
        "var result=['AE','Ä'].sort(new Intl.Collator('de', {usage: 'search'}).compare);",
        r#"["AE","Ä"]"#,
    );
    check(
        "var r=new Intl.Collator('de', {usage: 'search'}).resolvedOptions();var result=[r.collation,r.usage];",
        r#"["default","search"]"#,
    );
}
#[test]
fn plurals_manx_cardinal_categories() {
    check(
        "var result=new Intl.PluralRules('gv').resolvedOptions().pluralCategories;",
        r#"["one","two","few","many","other"]"#,
    );
}
