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
fn canonicalization_handles_aliases_reserved_languages_and_unicode_extensions() {
    check(
        r#"var a=new Intl.Locale('posix-US-u-ca-islamicc-kn-yes');var result=[a.language,a.calendar,a.numeric,a.toString(),Intl.getCanonicalLocales(['iw-il','he-IL','en-u-ms-imperial','und-t-m0-names'])];"#,
        r#"["posix","islamic-civil",true,"posix-US-u-ca-islamic-civil-kn",["he-IL","en-u-ms-uksystem","und-t-m0-prprname"]]"#,
    );
}
#[test]
fn constructor_reads_options_in_order_and_locale_slots_do_not_depend_on_properties() {
    check(
        r#"var log=[],options=new Proxy({language:'fr',script:'Latn',region:'CA'},{get(t,k){log.push(k);return t[k]}});var a=new Intl.Locale('en',options);a.toString=function(){throw 1};var b=new Intl.Locale(a);var result=[log,b.toString(),b.maximize().toString(),new Intl.Locale('und').minimize().toString()];"#,
        r#"[["language","script","region","variants","calendar","collation","firstDayOfWeek","hourCycle","caseFirst","numeric","numberingSystem"],"fr-Latn-CA","fr-Latn-CA","en"]"#,
    );
}
#[test]
fn locale_info_uses_region_overrides_and_returns_fresh_data() {
    check(
        r#"var a=new Intl.Locale('en-US-u-rg-thzzzz-fw-mon'),first=a.getCalendars();first.push('changed');var b=new Intl.Locale('en-US');var result=[a.getCalendars(),a.getWeekInfo().firstDay,b.getHourCycles(),new Intl.Locale('ar').getTextInfo(),Object.keys(a),Object.getPrototypeOf(a)===Intl.Locale.prototype];"#,
        r#"[["buddhist","gregory"],1,["h12"],{"direction":"rtl"},[],true]"#,
    );
}
#[test]
fn locale_branding_and_invalid_tags_are_checked() {
    check(
        r#"var errors=[];for(var tag of ['en_fonipa','en-abcde-abcde','abcd','x-private']){try{new Intl.Locale(tag)}catch(e){errors.push(e.name)}}try{Intl.Locale.prototype.toString.call({})}catch(e){errors.push(e.name)}var result=errors;"#,
        r#"["RangeError","RangeError","RangeError","RangeError","TypeError"]"#,
    );
}
